//! Read installed Simplified Chinese faces with outlines GPUI can shape.

use std::{collections::HashMap, path::PathBuf};

use ttf_parser::{Face, RawFace, Tag, name_id};

const CHINESE_FACES: [&str; 3] = [
    "PingFangSC-Regular",
    "PingFangSC-Medium",
    "PingFangSC-Semibold",
];

pub(super) fn chinese_collection() -> Result<Vec<u8>, String> {
    let active = core_text::font::new_from_name(CHINESE_FACES[0], 13.)
        .ok()
        .filter(|font| font.postscript_name() == CHINESE_FACES[0])
        .and_then(|font| font.url())
        .and_then(|url| url.to_path());
    active
        .into_iter()
        .chain(installed_pingfang_files())
        .filter_map(|path| std::fs::read(path).ok())
        .find_map(|bytes| package_simplified_faces(&bytes))
        .ok_or_else(|| "The installed PingFang SC outlines are unavailable".into())
}

fn installed_pingfang_files() -> impl Iterator<Item = PathBuf> {
    // The reserved UI collection on recent macOS can have Apple-only outline
    // tables. The downloaded system font asset includes standard OpenType outlines.
    let mut assets = std::fs::read_dir("/System/Library/AssetsV2")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("com_apple_MobileAsset_Font"))
        })
        .flat_map(|entry| {
            std::fs::read_dir(entry.path())
                .into_iter()
                .flatten()
                .flatten()
        })
        .map(|entry| entry.path().join("AssetData/PingFang.ttc"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    assets.sort_unstable();
    [
        PathBuf::from("/System/Library/Fonts/PingFang.ttc"),
        PathBuf::from("/System/Library/Fonts/Supplemental/PingFang.ttc"),
    ]
    .into_iter()
    .chain(assets)
}

fn package_simplified_faces(data: &[u8]) -> Option<Vec<u8>> {
    let mut faces = [None; 3];
    for index in 0..ttf_parser::fonts_in_collection(data).unwrap_or(1) {
        let Ok(face) = Face::parse(data, index) else {
            continue;
        };
        let raw = *face.raw_face();
        let cff = raw.table(Tag::from_bytes(b"CFF ")).is_some();
        if face.is_variable() || (!cff && raw.table(Tag::from_bytes(b"glyf")).is_none()) {
            continue;
        }
        for name in face
            .names()
            .into_iter()
            .filter(|name| name.name_id == name_id::POST_SCRIPT_NAME)
        {
            let Some(name) = name.to_string() else {
                continue;
            };
            if let Some(slot) = CHINESE_FACES.iter().position(|face| *face == name) {
                faces[slot] = Some((raw, cff));
            }
        }
    }
    package_collection(&[faces[0]?, faces[1]?, faces[2]?])
}

fn package_collection(faces: &[(RawFace<'_>, bool); 3]) -> Option<Vec<u8>> {
    // TTC directories use absolute offsets. Preserve the source tables and
    // their checksums, including tables shared by more than one weight.
    let header_size = 12 + faces.len() * 4;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"ttcf");
    bytes.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    bytes.extend_from_slice(&(faces.len() as u32).to_be_bytes());
    bytes.resize(header_size, 0);
    let mut table_offsets = HashMap::new();

    for (index, (face, cff)) in faces.iter().enumerate() {
        let directory = u32::try_from(bytes.len()).ok()?;
        bytes[12 + index * 4..16 + index * 4].copy_from_slice(&directory.to_be_bytes());
        let count = face.table_records.len();
        let entry_selector = count.checked_ilog2()? as u16;
        let search_range = (1_u16 << entry_selector).checked_mul(16)?;
        let range_shift = count.checked_mul(16)?.checked_sub(search_range)?;
        bytes.extend_from_slice(if *cff { b"OTTO" } else { b"\0\x01\0\0" });
        for field in [count, search_range, entry_selector, range_shift] {
            bytes.extend_from_slice(&field.to_be_bytes());
        }
        let table_directory = bytes.len();
        bytes.resize(table_directory + usize::from(count) * 16, 0);

        for (slot, table) in face.table_records.into_iter().enumerate() {
            let offset = match table_offsets.entry((table.offset, table.length)) {
                std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let offset = u32::try_from(bytes.len()).ok()?;
                    bytes.extend_from_slice(face.table(table.tag)?);
                    bytes.resize(bytes.len().next_multiple_of(4), 0);
                    *entry.insert(offset)
                }
            };
            let start = table_directory + slot * 16;
            for (field, value) in [table.tag.0, table.check_sum, offset, table.length]
                .into_iter()
                .enumerate()
            {
                bytes[start + field * 4..start + (field + 1) * 4]
                    .copy_from_slice(&value.to_be_bytes());
            }
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_only_the_installed_simplified_chinese_faces() {
        let Ok(collection) = chinese_collection() else {
            // Downloadable system fonts are optional on some macOS installations.
            assert!(!installed_pingfang_files().any(|path| path.is_file()));
            return;
        };
        assert_eq!(ttf_parser::fonts_in_collection(&collection), Some(3));
        for (index, name) in CHINESE_FACES.iter().enumerate() {
            let face = Face::parse(&collection, index as u32).expect("parse exported face");
            assert!(face.names().into_iter().any(|record| {
                record.name_id == name_id::POST_SCRIPT_NAME
                    && record.to_string().as_deref() == Some(*name)
            }));
            assert!(face.glyph_index('权').is_some());
            assert!(face.raw_face().table(Tag::from_bytes(b"head")).is_some());
            assert_eq!(face.weight().to_number(), [400, 500, 600][index]);
        }
    }
}
