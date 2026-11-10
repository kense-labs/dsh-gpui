//! Native Session ZIP writer. The archive is published only after every object verifies.

#[cfg(target_os = "macos")]
mod macos;

use super::{SessionLog, current_log_in, dsh_home, find_session_log, read_session_log};
use crate::acp::{AcpService, SessionFlush};
use indexmap::IndexMap;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, Write};
use std::path::{Path, PathBuf};
use tauri::{Emitter, Manager};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const LOG_NAME: &str = "session.v4.jsonl";
const ZIP_ERROR: &str = "无法生成或保存 Session ZIP 文件。";
const ROOT_ERROR: &str = "Export failed: HTTP 500 session log export failed to read the stored log";
const NOT_FOUND: &str = "Export failed: HTTP 404 session not found";

#[tauri::command]
pub(crate) async fn download_session_log(
    webview: tauri::Webview,
    request_id: u64,
    session_id: String,
    cwd: String,
    service: tauri::State<'_, AcpService>,
) -> Result<(), String> {
    if webview.label() != "main" || session_id.is_empty() {
        return Err(ROOT_ERROR.into());
    }
    let flush = SessionFlush::new(&service).await;
    flush.flush(&session_id).await.map_err(|_| ROOT_ERROR)?;
    let home = dsh_home();
    let sessions = home.join("sessions");
    let (id, root) = tauri::async_runtime::spawn_blocking(move || {
        let path = find_session_log(&sessions, &session_id, &cwd)
            .map_err(|_| ROOT_ERROR)?
            .ok_or(NOT_FOUND)?;
        let log = read_session_log(&path, &session_id).map_err(|_| ROOT_ERROR)?;
        Ok::<_, &'static str>((session_id, log))
    })
    .await
    .map_err(|_| ROOT_ERROR)??;
    let filename = format!("dsh-session-{}.zip", safe_segment(&id));
    let app = webview.app_handle().clone();
    let _ = app.emit_to(
        "main",
        "acp-event",
        json!({
            "kind": "session_log_download_started",
            "requestId": request_id,
            "sessionId": id,
        }),
    );
    #[cfg(target_os = "macos")]
    let destination = macos::choose_destination(webview, &filename).await?;
    #[cfg(not(target_os = "macos"))]
    let destination = Some(
        app.path()
            .download_dir()
            .map_err(|_| ZIP_ERROR)?
            .join(&filename),
    );
    let Some(destination) = destination else {
        return Ok(());
    };
    let runtime = tokio::runtime::Handle::current();
    tauri::async_runtime::spawn_blocking(move || {
        write_archive(
            &home,
            &root,
            &destination,
            cfg!(target_os = "macos"),
            |id| runtime.block_on(flush.flush(id)),
        )
        .map_err(|_| ZIP_ERROR.to_owned())
    })
    .await
    .map_err(|_| ZIP_ERROR)?
}

#[derive(Clone)]
struct SessionRecord {
    id: String,
    parent: Option<String>,
    created: i64,
    path: PathBuf,
}

fn records(root: &Path) -> Result<HashMap<String, SessionRecord>, String> {
    let boundary = root.canonicalize().map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    for project in fs::read_dir(root)
        .map_err(|error| error.to_string())?
        .flatten()
    {
        let Ok(entries) = fs::read_dir(project.path()) else {
            continue;
        };
        files.extend(
            entries
                .flatten()
                .filter_map(|entry| current_log_in(&entry.path())),
        );
    }
    files.sort_unstable();
    let mut records = HashMap::new();
    for path in files {
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        if !canonical.starts_with(&boundary) {
            continue;
        }
        if let Some(record) = read_record(canonical) {
            records.entry(record.id.clone()).or_insert(record);
        }
    }
    Ok(records)
}

fn read_record(path: PathBuf) -> Option<SessionRecord> {
    let file = File::open(&path).ok()?;
    let input: Box<dyn BufRead> = if path.extension().is_some_and(|ext| ext == "zstd") {
        Box::new(BufReader::new(
            zstd::Decoder::new(file).ok()?.single_frame(),
        ))
    } else {
        Box::new(BufReader::new(file))
    };
    let mut line = String::new();
    // Session headers contain metadata only. Bound discovery reads even if a
    // damaged log omits the header's newline.
    let mut input = BufReader::new(input.take(1024 * 1024));
    input.read_line(&mut line).ok()?;
    if !line.ends_with('\n') {
        return None;
    }
    let header: Value = serde_json::from_str(&line).ok()?;
    if header["type"] != "session" || header["version"] != 4 {
        return None;
    }
    Some(SessionRecord {
        id: header["id"]
            .as_str()
            .filter(|id| !id.is_empty())?
            .to_owned(),
        parent: header["parentSession"].as_str().map(str::to_owned),
        created: header["createdAt"].as_i64()?,
        path,
    })
}

fn descendants<'a>(
    root: &Value,
    records: &'a HashMap<String, SessionRecord>,
) -> Result<Vec<&'a SessionRecord>, String> {
    let id = root["id"].as_str().ok_or(ROOT_ERROR)?;
    if !records.contains_key(id) {
        return Err(NOT_FOUND.into());
    }
    let mut ancestors = HashSet::from([id]);
    let mut parent = root["parentSession"].as_str();
    while let Some(id) = parent {
        if !ancestors.insert(id) {
            return Err("Session lineage contains a cycle".into());
        }
        parent = records.get(id).and_then(|record| record.parent.as_deref());
    }

    let mut children: HashMap<&str, Vec<_>> = HashMap::new();
    for record in records.values() {
        if let Some(parent) = &record.parent {
            children.entry(parent).or_default().push(record);
        }
    }
    for nodes in children.values_mut() {
        nodes.sort_unstable_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    }
    let mut order = Vec::new();
    let mut seen = HashSet::from([id]);
    let mut stack: Vec<_> = children
        .get(id)
        .into_iter()
        .flatten()
        .rev()
        .copied()
        .collect();
    while let Some(node) = stack.pop() {
        if !seen.insert(&node.id) {
            return Err("Session lineage contains a cycle".into());
        }
        order.push(node);
        stack.extend(
            children
                .get(node.id.as_str())
                .into_iter()
                .flatten()
                .rev()
                .copied(),
        );
    }
    Ok(order)
}

fn safe_segment(id: &str) -> String {
    id.encode_utf16()
        .map(|unit| {
            let ch = u8::try_from(unit).map(char::from).unwrap_or('_');
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn write_archive(
    home: &Path,
    root: &SessionLog,
    destination: &Path,
    replace: bool,
    mut flush: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    let records = records(&home.join("sessions"))?;
    let children = descendants(&root.header, &records)?;
    let directory = destination.parent().ok_or(ZIP_ERROR)?;
    let mut pending =
        tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    let mut writer = ZipWriter::new(pending.as_file_mut());
    let mut refs = Attachments::default();
    write_log(&mut writer, LOG_NAME, root, &mut refs)?;
    for node in children {
        flush(&node.id)?;
        let log = read_session_log(&node.path, &node.id)?;
        let name = format!("subagents/{}/{}", safe_segment(&node.id), LOG_NAME);
        write_log(&mut writer, &name, &log, &mut refs)?;
    }
    let attachment_root = home.join("attachments/v1");
    for (id, image) in refs.images {
        write_attachment(&mut writer, &attachment_root, &id, &image, true)?;
    }
    for ((id, _), file) in refs.files {
        write_attachment(&mut writer, &attachment_root, &id, &file, false)?;
    }
    writer.finish().map_err(|error| error.to_string())?;
    pending
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    if replace {
        pending
            .persist(destination)
            .map_err(|error| error.error.to_string())?;
        return Ok(());
    }
    let stem = destination
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or(ZIP_ERROR)?;
    for index in 0.. {
        let target = if index == 0 {
            destination.to_path_buf()
        } else {
            directory.join(format!("{stem} ({index}).zip"))
        };
        match pending.persist_noclobber(target) {
            Ok(_) => return Ok(()),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                pending = error.file;
            }
            Err(error) => return Err(error.error.to_string()),
        }
    }
    unreachable!()
}

fn options() -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
}

fn write_log(
    writer: &mut ZipWriter<&mut File>,
    name: &str,
    log: &SessionLog,
    refs: &mut Attachments,
) -> Result<(), String> {
    writer
        .start_file(name, options())
        .map_err(|error| error.to_string())?;
    let mut header = Map::new();
    for field in [
        "type",
        "version",
        "id",
        "createdAt",
        "cwd",
        "parentSession",
        "isSeeded",
        "origin",
        "delegationDepth",
        "agentPreset",
    ] {
        if let Some(value) = log.header.get(field) {
            header.insert(field.into(), value.clone());
        } else if field == "delegationDepth" {
            header.insert(field.into(), 0.into());
        }
    }
    json_line(writer, &header)?;
    for event in &log.events {
        refs.event(event)?;
        if let Some(ranges) = event.get("sourceEventSeqs") {
            let mut logical = event.clone();
            logical["sourceEventSeqs"] =
                expand_sources(ranges, event["seq"].as_u64().ok_or(ZIP_ERROR)?)?;
            json_line(writer, &logical)?;
        } else {
            json_line(writer, event)?;
        }
    }
    Ok(())
}

fn json_line(
    writer: &mut ZipWriter<&mut File>,
    value: &impl serde::Serialize,
) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, value).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())
}

fn expand_sources(value: &Value, seq: u64) -> Result<Value, String> {
    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    let mut has_range = false;
    for entry in value.as_array().ok_or(ZIP_ERROR)? {
        let (start, end) = if let Some(source) = entry.as_u64() {
            (source, source)
        } else {
            let pair = entry
                .as_array()
                .filter(|pair| pair.len() == 2)
                .ok_or(ZIP_ERROR)?;
            has_range = true;
            (
                pair[0].as_u64().ok_or(ZIP_ERROR)?,
                pair[1].as_u64().ok_or(ZIP_ERROR)?,
            )
        };
        if end >= seq || start > end {
            return Err(ZIP_ERROR.into());
        }
        for source in start..=end {
            if !seen.insert(source) {
                return Err(ZIP_ERROR.into());
            }
            sources.push(source);
        }
    }
    if has_range && !sources.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(ZIP_ERROR.into());
    }
    Ok(json!(sources))
}

#[derive(Default)]
struct Attachments {
    images: IndexMap<String, Value>,
    files: IndexMap<(String, String), Value>,
}

impl Attachments {
    fn content(&mut self, content: &Value) -> Result<(), String> {
        for block in content.as_array().into_iter().flatten() {
            self.block(block)?;
        }
        Ok(())
    }

    fn block(&mut self, block: &Value) -> Result<(), String> {
        let reference = &block["attachment"];
        match block["type"].as_str() {
            Some("image") => {
                self.images.insert(
                    reference["attachmentId"].as_str().ok_or(ZIP_ERROR)?.into(),
                    reference.clone(),
                );
            }
            Some("file") => {
                let id = reference["attachmentId"].as_str().ok_or(ZIP_ERROR)?;
                let name = reference["name"].as_str().ok_or(ZIP_ERROR)?;
                self.files
                    .insert((id.into(), name.into()), reference.clone());
            }
            _ => {}
        }
        Ok(())
    }

    fn event(&mut self, event: &Value) -> Result<(), String> {
        let data = &event["data"];
        match event["type"].as_str() {
            Some("user/message" | "tool/ptc-dispatch") => self.content(&data["content"]),
            Some(
                "system/message" | "developer/message" | "tool/result" | "team/message/queued",
            ) => self.content(&data["message"]["content"]),
            Some("agent/inbox/spliced") => {
                for message in data["inserted"].as_array().into_iter().flatten() {
                    self.content(&message["content"])?;
                }
                Ok(())
            }
            Some("compaction/summary") => {
                self.content(&data["summary"])?;
                self.content(&data["rawOutput"])
            }
            Some("assistant/message" | "assistant/attempt") => {
                if event["type"] == "assistant/message" {
                    self.content(&data["message"]["content"])?;
                }
                for record in data["stream"].as_array().into_iter().flatten() {
                    if record["type"] == "chunk" && record["chunk"]["type"] == "block-end" {
                        self.block(&record["chunk"]["block"])?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

fn write_attachment(
    writer: &mut ZipWriter<&mut File>,
    root: &Path,
    id: &str,
    reference: &Value,
    image: bool,
) -> Result<(), String> {
    let digest = id
        .strip_prefix("sha256:")
        .filter(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or(ZIP_ERROR)?;
    let expected_size = reference["bytes"].as_u64().ok_or(ZIP_ERROR)?;
    let prefix = &digest[..2];
    let (source, entry, format) = if image {
        let (extension, format) = match reference["mediaType"].as_str() {
            Some("image/png") => ("png", image::ImageFormat::Png),
            Some("image/jpeg") => ("jpg", image::ImageFormat::Jpeg),
            Some("image/webp") => ("webp", image::ImageFormat::WebP),
            Some("image/gif") => ("gif", image::ImageFormat::Gif),
            _ => return Err(ZIP_ERROR.into()),
        };
        (
            root.join("objects").join(prefix).join(digest),
            format!("media/{id}.{extension}"),
            Some(format),
        )
    } else {
        let name = reference["name"]
            .as_str()
            .filter(|name| {
                !name.is_empty()
                    && !matches!(*name, "." | "..")
                    && !name
                        .chars()
                        .any(|ch| matches!(ch, '/' | '\\' | '\0'..='\u{1f}' | '\u{7f}'))
            })
            .ok_or(ZIP_ERROR)?;
        let entry = format!("files/{prefix}/{digest}/{name}");
        (root.join(&entry), entry, None)
    };
    let boundary = root.canonicalize().map_err(|error| error.to_string())?;
    let source = source.canonicalize().map_err(|error| error.to_string())?;
    if !source.starts_with(boundary) {
        return Err(ZIP_ERROR.into());
    }
    let mut file = File::open(source).map_err(|error| error.to_string())?;
    if let Some(format) = format {
        let reader = image::ImageReader::new(BufReader::new(&mut file))
            .with_guessed_format()
            .map_err(|error| error.to_string())?;
        if reader.format() != Some(format) {
            return Err(ZIP_ERROR.into());
        }
        let (width, height) = reader
            .into_dimensions()
            .map_err(|error| error.to_string())?;
        if reference["width"].as_u64() != Some(width.into())
            || reference["height"].as_u64() != Some(height.into())
        {
            return Err(ZIP_ERROR.into());
        }
        file.rewind().map_err(|error| error.to_string())?;
    }
    writer
        .start_file(
            entry,
            options().large_file(expected_size >= u32::MAX.into()),
        )
        .map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut size = 0_u64;
    let mut chunk = [0; 64 * 1024];
    loop {
        let count = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        size += count as u64;
        if size > expected_size {
            return Err(ZIP_ERROR.into());
        }
        hash.update(&chunk[..count]);
        writer
            .write_all(&chunk[..count])
            .map_err(|error| error.to_string())?;
    }
    if size != expected_size || format!("{:x}", hash.finalize()) != digest {
        return Err(ZIP_ERROR.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn session(
        home: &Path,
        id: &str,
        parent: Option<&str>,
        created: u64,
        events: &[Value],
    ) -> SessionLog {
        let mut header = json!({"type":"session","version":4,"id":id,"createdAt":created,
            "cwd":"/fixture","isSeeded":false,"delegationDepth":0});
        if let Some(parent) = parent {
            header["parentSession"] = parent.into();
        }
        let folder = home.join("sessions/project").join(safe_segment(id));
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(LOG_NAME);
        let mut log = File::create(&path).unwrap();
        writeln!(log, "{header}").unwrap();
        for (seq, event) in events.iter().enumerate() {
            let mut row = json!({"seq":seq,"time":seq});
            row.as_object_mut()
                .unwrap()
                .extend(event.as_object().unwrap().clone());
            writeln!(log, "{row}").unwrap();
        }
        read_session_log(&path, id).unwrap()
    }

    fn attach_file(home: &Path, name: &str, bytes: &[u8]) -> (Value, String) {
        let digest = format!("{:x}", Sha256::digest(bytes));
        let entry = format!("files/{}/{digest}/{name}", &digest[..2]);
        let path = home.join("attachments/v1").join(&entry);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        (
            json!({"type":"file","attachment":{"attachmentId":format!("sha256:{digest}"),"bytes":bytes.len(),"name":name}}),
            entry,
        )
    }

    fn attach_image(home: &Path) -> (Value, String, Vec<u8>) {
        let mut buffer = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 1)
            .write_to(&mut buffer, image::ImageFormat::Png)
            .unwrap();
        let bytes = buffer.into_inner();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let path = home
            .join("attachments/v1/objects")
            .join(&digest[..2])
            .join(&digest);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, &bytes).unwrap();
        let id = format!("sha256:{digest}");
        (
            json!({"type":"image","attachment":{"attachmentId":id,"bytes":bytes.len(),"width":2,"height":1,"mediaType":"image/png"}}),
            format!("media/{id}.png"),
            bytes,
        )
    }

    #[test]
    fn writes_lineage_order_and_verified_attachment_paths() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path();
        let (file, file_path) = attach_file(home, "测试.txt", b"shared content");
        let (alias, alias_path) = attach_file(home, "alias.txt", b"shared content");
        let (image, image_path, image_bytes) = attach_image(home);
        let root = session(
            home,
            "root",
            None,
            1,
            &[
                json!({"type":"user/message","data":{"content":[file.clone(),image.clone()]}}),
                json!({"type":"extension/opaque","data":{"content":[{"type":"file","attachment":{"attachmentId":"missing"}}]}}),
                json!({"type":"assistant/message","data":{"message":{"content":[]}},"sourceEventSeqs":[[0,1]]}),
            ],
        );
        session(
            home,
            "later",
            Some("root"),
            5,
            &[json!({"type":"tool/result","data":{"message":{"content":[image.clone(),file]}}})],
        );
        session(
            home,
            "first",
            Some("root"),
            3,
            &[
                json!({"type":"assistant/attempt","data":{"stream":[{"type":"chunk","chunk":{"type":"block-end","block":alias}}]}}),
            ],
        );
        session(home, "child/🦊", Some("first"), 4, &[]);
        session(home, "outside", None, 2, &[]);

        let path = home.join("archive.zip");
        let mut flushed = Vec::new();
        write_archive(home, &root, &path, false, |id| {
            flushed.push(id.to_owned());
            Ok(())
        })
        .unwrap();
        assert_eq!(flushed, ["first", "child/🦊", "later"]);
        let mut zip = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
        assert_eq!(
            zip.file_names().collect::<Vec<_>>(),
            [
                LOG_NAME,
                "subagents/first/session.v4.jsonl",
                "subagents/child___/session.v4.jsonl",
                "subagents/later/session.v4.jsonl",
                &image_path,
                &file_path,
                &alias_path
            ],
        );
        let mut body = String::new();
        zip.by_name(LOG_NAME)
            .unwrap()
            .read_to_string(&mut body)
            .unwrap();
        let rows: Vec<Value> = body
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows[0], root.header);
        assert_eq!(rows[3]["sourceEventSeqs"], json!([0, 1]));
        assert!(body.ends_with('\n'));
        assert_eq!(
            zip.by_name(LOG_NAME).unwrap().compression(),
            CompressionMethod::Deflated
        );
        for (path, expected) in [
            (&image_path, image_bytes.as_slice()),
            (&file_path, b"shared content"),
            (&alias_path, b"shared content"),
        ] {
            let mut data = Vec::new();
            zip.by_name(path).unwrap().read_to_end(&mut data).unwrap();
            assert_eq!(data, expected);
        }
    }

    #[test]
    fn corrupt_or_missing_objects_never_replace_a_previous_download() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path();
        let (file, relative) = attach_file(home, "one.txt", b"good");
        let root = session(
            home,
            "root",
            None,
            1,
            &[json!({"type":"user/message","data":{"content":[file]}})],
        );
        let original = home.join("existing.zip");
        fs::write(&original, b"previous").unwrap();
        let object = home.join("attachments/v1").join(relative);
        fs::write(&object, b"evil").unwrap();
        assert!(write_archive(home, &root, &original, true, |_| Ok(())).is_err());
        fs::remove_file(object).unwrap();
        assert!(write_archive(home, &root, &original, true, |_| Ok(())).is_err());
        assert_eq!(fs::read(original).unwrap(), b"previous");
        assert_eq!(fs::read_dir(home).unwrap().count(), 3);
    }

    #[test]
    fn preserves_an_existing_default_name_and_chooses_the_next_number() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path();
        let root = session(home, "root", None, 1, &[]);
        let path = home.join("dsh-session-root.zip");
        fs::write(&path, b"previous").unwrap();
        write_archive(home, &root, &path, false, |_| Ok(())).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"previous");
        let new = File::open(home.join("dsh-session-root (1).zip")).unwrap();
        assert_eq!(zip::ZipArchive::new(new).unwrap().len(), 1);
    }
}
