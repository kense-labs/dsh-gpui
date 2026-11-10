//! Shared committed Session log reader and detached Markdown export.

pub(crate) mod archive;

use super::{candidate_dirs, dsh_home, looks_like_zstd};
use rquickjs::{CatchResultExt, Context, Function, Module, Runtime};
use serde_json::Value;
use std::fs::File;
use std::io::{self, Read, Seek};
use std::path::Path;

const SESSION_FILES: [&str; 2] = ["session.v4.jsonl.zstd", "session.v4.jsonl"];
const INVALID_LOG: &str = "会话日志损坏或不受支持。";

#[tauri::command]
pub(crate) async fn session_markdown(
    webview: tauri::Webview,
    session_id: String,
    cwd: String,
) -> Result<String, String> {
    if webview.label() != "main" || session_id.is_empty() {
        return Err("无法读取 Session。".into());
    }
    tokio::task::spawn_blocking(move || {
        markdown_for_session(&dsh_home().join("sessions"), &session_id, &cwd)
            .map_err(|_| "无法读取 Session。".into())
    })
    .await
    .map_err(|_| "无法读取 Session。".to_owned())?
}

pub(super) struct SessionLog {
    header: Value,
    pub(super) events: Vec<Value>,
}

fn markdown_for_session(root: &Path, session_id: &str, cwd: &str) -> Result<String, String> {
    let path = find_session_log(root, session_id, cwd)?.ok_or("未找到会话日志。")?;
    let log = read_session_log(&path, session_id)?;
    format_markdown(&log.events, &path.to_string_lossy())
}

fn current_log_in(directory: &Path) -> Option<std::path::PathBuf> {
    SESSION_FILES
        .iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
}

pub(super) fn find_session_log(
    root: &Path,
    session_id: &str,
    cwd: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    if !root.exists() {
        return Ok(None);
    }
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    for directory in candidate_dirs(root, cwd, session_id) {
        if let Some(path) = current_log_in(&directory) {
            let canonical = path.canonicalize().map_err(|error| error.to_string())?;
            if canonical.starts_with(&canonical_root) {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}

pub(super) fn read_session_log(path: &Path, session_id: &str) -> Result<SessionLog, String> {
    let bytes = read_snapshot(path).map_err(|error| error.to_string())?;
    let compressed = path.extension().is_some_and(|ext| ext == "zstd") || looks_like_zstd(&bytes);
    committed_log(&bytes, compressed, session_id)
}

/// Limit each attempt to its pre-read size, so a concurrent append cannot
/// supply half of a newly published batch. Retry once to observe a fresher log.
fn read_snapshot(path: &Path) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    for _ in 0..2 {
        let size = file.metadata()?.len();
        bytes.clear();
        (&mut file).take(size).read_to_end(&mut bytes)?;
        if file.metadata()?.len() == size {
            break;
        }
        file.rewind()?;
    }
    Ok(bytes)
}

fn committed_log(bytes: &[u8], compressed: bool, session_id: &str) -> Result<SessionLog, String> {
    let mut decoded;
    let plain = if compressed {
        let first_frame = zstd::zstd_safe::find_frame_compressed_size(bytes)
            .map_err(|_| INVALID_LOG.to_owned())?;
        let header =
            zstd::stream::decode_all(&bytes[..first_frame]).map_err(|error| error.to_string())?;
        if !header.ends_with(b"\n") || header.iter().filter(|&&byte| byte == b'\n').count() != 1 {
            return Err(INVALID_LOG.into());
        }
        decoded = header;
        let mut decoder = zstd::stream::read::Decoder::new(&bytes[first_frame..])
            .map_err(|error| error.to_string())?;
        if first_frame < bytes.len() {
            match decoder.read_to_end(&mut decoded) {
                Ok(_) if decoded.ends_with(b"\n") => {}
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {}
                _ => return Err(INVALID_LOG.into()),
            }
        }
        decoded.as_slice()
    } else {
        bytes
    };
    let committed_end = plain
        .iter()
        .rposition(|&byte| byte == b'\n')
        .ok_or(INVALID_LOG)?;
    let mut rows = plain[..committed_end].split(|&byte| byte == b'\n');
    let header = parse_row(rows.next().ok_or(INVALID_LOG)?).ok_or(INVALID_LOG)?;
    let seeded = header["isSeeded"].as_bool().ok_or(INVALID_LOG)?;
    if header["type"] != "session" || header["version"] != 4 || header["id"] != session_id {
        return Err(INVALID_LOG.into());
    }

    let mut events = Vec::new();
    let mut invalid_tail = false;
    let mut inherited = false;
    for row in rows {
        let event = parse_row(row);
        let kind = event.as_ref().and_then(|event| event["type"].as_str());
        let contiguous = event
            .as_ref()
            .is_some_and(|event| event["seq"].as_u64() == Some(events.len() as u64));
        if invalid_tail || !contiguous || kind.is_none() {
            // A turn/end after malformed rows certifies the damage is inside a
            // committed turn. An interrupted malformed suffix can be dropped.
            if kind == Some("turn/end") {
                return Err(INVALID_LOG.into());
            }
            invalid_tail = true;
            continue;
        }
        let event = event.expect("valid event envelope");
        inherited |= event["type"] == "session/end-seed" && event["data"]["inherited"] == true;
        events.push(event);
    }
    if inherited != seeded {
        return Err(INVALID_LOG.into());
    }
    Ok(SessionLog { header, events })
}

fn parse_row(bytes: &[u8]) -> Option<Value> {
    serde_json::from_str(&String::from_utf8_lossy(bytes)).ok()
}

fn format_markdown(events: &[Value], path: &str) -> Result<String, String> {
    let runtime = Runtime::new().map_err(|error| error.to_string())?;
    let context = Context::full(&runtime).map_err(|error| error.to_string())?;
    let serialized = serde_json::to_string(events).map_err(|error| error.to_string())?;
    context.with(|ctx| {
        let render = || {
            let (module, ready) = Module::declare(
                ctx.clone(),
                "session-markdown",
                include_str!("session-markdown.mjs"),
            )?
            .eval()?;
            ready.finish::<()>()?;
            let format: Function = module.get("formatSessionMarkdown")?;
            format.call((ctx.json_parse(serialized)?, path))
        };
        render().catch(&ctx).map_err(|error| error.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{fs, io::Write};

    const HEADER: &str = r#"{"type":"session","version":4,"id":"fixture","createdAt":1,"isSeeded":true,"delegationDepth":0}"#;

    fn seeded_rows() -> Vec<Value> {
        let rows = [
            json!({"type":"session/title","data":{"title":"  Fork\r\nTranscript  "}}),
            json!({"type":"turn/start","data":{"turn":1}}),
            json!({"type":"user/message","surfaceOp":"append","data":{"source":{"kind":"user"},"content":[{"type":"text","text":"Parent prompt"},{"type":"file","attachment":{"name":"notes.txt","bytes":42}}]}}),
            json!({"type":"assistant/message","surfaceOp":"append","data":{"message":{"content":[{"type":"reasoning","text":"private reasoning"},{"type":"text","text":"Look </details>"},{"type":"tool-call"}]}}}),
            json!({"type":"tool/call","data":{"callId":"call","name":"read","arguments":"{\"b\":2,\"a\":1}"}}),
            json!({"type":"tool/result","surfaceOp":"append","data":{"message":{"toolCallId":"call","content":[{"type":"text","text":"read output"}]}}}),
            json!({"type":"assistant/message","surfaceOp":"append","data":{"message":{"content":[{"type":"text","text":"```rust\nlet n = 1;"}]}}}),
            json!({"type":"turn/end","data":{"turn":1}}),
            json!({"type":"session/end-seed","data":{"inherited":true}}),
            json!({"type":"user/message","surfaceOp":"append","data":{"source":{"kind":"model"},"content":[{"type":"text","text":"injected text"}]}}),
            json!({"type":"user/message","surfaceOp":"append","data":{"source":{"kind":"user"},"content":[{"type":"text","text":"Fork prompt"}]}}),
        ];
        rows.into_iter()
            .enumerate()
            .map(|(seq, mut row)| {
                row["seq"] = seq.into();
                row["time"] = seq.into();
                row
            })
            .collect()
    }

    fn frame(text: &str) -> Vec<u8> {
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 0).unwrap();
        encoder.include_checksum(true).unwrap();
        encoder.write_all(text.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    fn lines(rows: &[Value]) -> String {
        rows.iter().map(|row| format!("{row}\n")).collect()
    }

    #[test]
    fn exports_the_selected_fork_prefix_and_visible_blocks_from_disk() {
        let root = std::env::temp_dir().join(format!("dsh-export-{}", uuid::Uuid::new_v4()));
        let directory = root.join("project").join("fixture");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(SESSION_FILES[0]);
        let rows = seeded_rows();
        let mut log = frame(&format!("{HEADER}\n"));
        log.extend(frame(&lines(&rows[..9])));
        log.extend(frame(&lines(&rows[9..])));
        fs::write(&path, log).unwrap();
        let result = markdown_for_session(&root, "fixture", "");
        fs::remove_dir_all(root).unwrap();
        let markdown = result.unwrap();
        let expected = format!(
            "# Fork Transcript\n\nSession file:\n\n```text\n{}\n```\n\n{}",
            path.display(),
            concat!(
                "> Parent prompt\n>\n> [File: notes.txt (42 bytes)]\n\n",
                "<details><summary>2 previous messages</summary>\n\n",
                "**Assistant**\n\nLook &lt;/details>\n\n",
                "**Tool: `read`**\n\n```json\n{\n  \"b\": 2,\n  \"a\": 1\n}\n```\n\n",
                "**Result**\n\n```\nread output\n```\n\n</details>\n\n",
                "```rust\nlet n = 1;\n```\n\n> Fork prompt\n",
            )
        );
        assert_eq!(markdown, expected);
    }

    #[test]
    fn retains_committed_rows_from_interrupted_plain_and_zstd_logs() {
        let rows = seeded_rows();
        let partial = format!("{}{{\"type\":\"assistant/message\"", lines(&rows[9..]));
        let plain = format!("{HEADER}\n{}{partial}", lines(&rows[..9]));
        assert_eq!(
            committed_log(plain.as_bytes(), false, "fixture")
                .unwrap()
                .events,
            rows
        );
        assert!(committed_log(plain.as_bytes(), false, "unrelated").is_err());

        let mut compressed = frame(&format!("{HEADER}\n"));
        compressed.extend(frame(&lines(&rows[..9])));
        let tail = frame(&partial);
        compressed.extend_from_slice(&tail[..tail.len() - 2]);
        assert_eq!(
            committed_log(&compressed, true, "fixture").unwrap().events,
            rows
        );

        let mut corrupt = frame(&format!("{HEADER}\n"));
        let mut last = frame(&lines(&rows));
        *last.last_mut().unwrap() ^= 0x80;
        corrupt.extend(last);
        assert!(committed_log(&corrupt, true, "fixture").is_err());
    }

    #[test]
    fn refuses_a_malformed_committed_turn_or_seed_cut() {
        let rows = seeded_rows();
        let inside_turn = format!(
            "{HEADER}\n{}broken\n{}",
            lines(&rows[..5]),
            lines(&rows[6..])
        );
        assert!(committed_log(inside_turn.as_bytes(), false, "fixture").is_err());
        let no_seed_marker = format!("{HEADER}\n{}", lines(&rows[..8]));
        assert!(committed_log(no_seed_marker.as_bytes(), false, "fixture").is_err());
    }
}
