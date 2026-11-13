//! Local media for transcript pictures.
//!
//! The Web host cannot fetch `data:` URLs through gpui's HTTP client, so every
//! picture is decoded locally and handed to the image element as an in-memory
//! `ImageSource`. This module owns the decoded-picture cache, the workspace image
//! path cache used by Markdown images, and the tiny header reader that lets the
//! layout reserve a right-sized box before layout.

use gpui::ImageFormat;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

fn image_cache() -> &'static Mutex<HashMap<String, Arc<gpui::Image>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<gpui::Image>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Decode a picture's data URL (once) and build the element source for it.
pub(crate) fn image_source(data_url: &str) -> Option<gpui::ImageSource> {
    if let Ok(cache) = image_cache().lock()
        && let Some(image) = cache.get(data_url)
    {
        return Some(image.clone().into());
    }
    let bytes = crate::data_url::decode_base64_data_url(data_url)?;
    let format = format_for_bytes(&bytes);
    let image = Arc::new(gpui::Image::from_bytes(format, bytes));
    if let Ok(mut cache) = image_cache().lock() {
        cache.insert(data_url.to_owned(), image.clone());
    }
    Some(image.into())
}

/// Display box for one picture: natural size scaled down to the transcript column.
pub(crate) fn display_box(bytes: &[u8], max_width: f32, max_height: f32) -> (f32, f32) {
    let (width, height) = image_dimensions(bytes).unwrap_or((480, 320));
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let scale = (max_width / width).min(max_height / height).min(1.);
    ((width * scale).max(24.), (height * scale).max(24.))
}

fn format_for_bytes(bytes: &[u8]) -> ImageFormat {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        ImageFormat::Png
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        ImageFormat::Jpeg
    } else if bytes.starts_with(b"GIF8") {
        ImageFormat::Gif
    } else if bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        ImageFormat::Webp
    } else {
        ImageFormat::Png
    }
}

/// Read the pixel size from the picture header (PNG / JPEG / GIF / WebP).
pub(crate) fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() >= 24 && bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some((width, height));
    }
    if bytes.len() >= 10 && bytes.starts_with(b"GIF8") {
        let width = u16::from_le_bytes(bytes[6..8].try_into().ok()?) as u32;
        let height = u16::from_le_bytes(bytes[8..10].try_into().ok()?) as u32;
        return Some((width, height));
    }
    if bytes.len() >= 30 && &bytes[8..12] == b"WEBP" && &bytes[12..16] == b"VP8X" {
        let width = 1 + u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]);
        let height = 1 + u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]);
        return Some((width, height));
    }
    if bytes.len() > 4 && bytes.starts_with(&[0xff, 0xd8]) {
        let mut index = 2;
        while index + 9 < bytes.len() {
            if bytes[index] != 0xff {
                index += 1;
                continue;
            }
            let marker = bytes[index + 1];
            let length = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
            if (0xc0..=0xcf).contains(&marker) && marker != 0xc4 && marker != 0xc8 && marker != 0xcc
            {
                let height = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]) as u32;
                let width = u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]) as u32;
                return Some((width, height));
            }
            index += 2 + length;
        }
    }
    None
}

// —— Markdown image destinations (official MarkdownPathImages) ——

/// Absolute POSIX / Windows path of a Markdown image destination, ignoring
/// query and fragment suffixes. Relative destinations are not Host files, so
/// the official client leaves them inert; the same rule applies here.
pub(crate) fn absolute_image_path(destination: &str) -> Option<String> {
    let trimmed = destination.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_suffix = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    let decoded = percent_decode(without_suffix);
    let bytes = decoded.as_bytes();
    let absolute = (decoded.starts_with('/') && !decoded.starts_with("//"))
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/'));
    if absolute { Some(decoded) } else { None }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push(high * 16 + low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    Some(match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => return None,
    })
}

/// One Host-read workspace picture: its data URL plus the box it should occupy.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LocalImage {
    pub(crate) data_url: String,
    pub(crate) media_type: String,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// Loading state of one workspace picture, keyed by its absolute path.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LocalImageState {
    /// Requested from the host, not answered yet.
    Pending,
    /// The host could not read it (missing file, unsupported format, too large).
    Failed,
    /// Decoded and ready to display.
    Ready(LocalImage),
}

fn workspace_images() -> &'static Mutex<HashMap<String, LocalImageState>> {
    static CACHE: OnceLock<Mutex<HashMap<String, LocalImageState>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Look up a workspace picture; `None` means it was never requested.
pub(crate) fn cached_local_image(path: &str) -> Option<LocalImageState> {
    workspace_images()
        .lock()
        .ok()
        .and_then(|cache| cache.get(path).cloned())
}

/// Record a successful workspace image read, computing its display box.
pub(crate) fn store_local_image(path: &str, media_type: &str, data_url: &str) {
    let state = crate::data_url::decode_base64_data_url(data_url)
        .map(|bytes| {
            let (width, height) = display_box(&bytes, 680., 420.);
            LocalImageState::Ready(LocalImage {
                data_url: data_url.to_owned(),
                media_type: media_type.to_owned(),
                width,
                height,
            })
        })
        .unwrap_or(LocalImageState::Failed);
    if let Ok(mut cache) = workspace_images().lock() {
        cache.insert(path.to_owned(), state);
    }
}

/// Mark every picture still waiting on the host as failed (one command error
/// covers whichever reads were in flight).
pub(crate) fn fail_pending_local_images() {
    if let Ok(mut cache) = workspace_images().lock() {
        for state in cache.values_mut() {
            if matches!(state, LocalImageState::Pending) {
                *state = LocalImageState::Failed;
            }
        }
    }
}

/// Mark a workspace image as requested; returns whether this call did so.
pub(crate) fn request_local_image(path: &str) -> bool {
    let mut cache = match workspace_images().lock() {
        Ok(cache) => cache,
        Err(_) => return false,
    };
    if cache.contains_key(path) {
        return false;
    }
    cache.insert(path.to_owned(), LocalImageState::Pending);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_png_gif_and_webp_headers() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
        png.extend_from_slice(&320u32.to_be_bytes());
        png.extend_from_slice(&160u32.to_be_bytes());
        assert_eq!(image_dimensions(&png), Some((320, 160)));

        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&64u16.to_le_bytes());
        gif.extend_from_slice(&32u16.to_le_bytes());
        assert_eq!(image_dimensions(&gif), Some((64, 32)));

        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBPVP8X");
        webp.extend_from_slice(&[0; 8]);
        webp.extend_from_slice(&[(200 - 1) as u8, 0, 0, (100 - 1) as u8, 0, 0]);
        assert_eq!(image_dimensions(&webp), Some((200, 100)));
        assert_eq!(image_dimensions(b"not an image"), None);
    }

    #[test]
    fn display_box_scales_down_inside_the_column() {
        assert_eq!(display_box(&[0; 8], 680., 420.), (480., 320.));
        // A 1000x2000 picture is limited by height.
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
        png.extend_from_slice(&1000u32.to_be_bytes());
        png.extend_from_slice(&2000u32.to_be_bytes());
        let (width, height) = display_box(&png, 680., 420.);
        assert!((height - 420.).abs() < 0.5, "{height}");
        assert!((width - 210.).abs() < 0.5, "{width}");
    }

    #[test]
    fn markdown_image_destinations_must_be_absolute_paths() {
        assert_eq!(
            absolute_image_path("/tmp/graph.png").as_deref(),
            Some("/tmp/graph.png")
        );
        assert_eq!(
            absolute_image_path("/tmp/a%20b.png?raw=1#x").as_deref(),
            Some("/tmp/a b.png")
        );
        assert_eq!(
            absolute_image_path("C:\\tmp\\x.png").as_deref(),
            Some("C:\\tmp\\x.png")
        );
        assert_eq!(absolute_image_path("relative.png"), None);
        assert_eq!(absolute_image_path("https://example.com/a.png"), None);
        assert_eq!(absolute_image_path("//cdn.example.com/a.png"), None);
        assert_eq!(absolute_image_path(""), None);
    }
}
