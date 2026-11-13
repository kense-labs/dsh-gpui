//! Data-URL decoding for in-transcript pictures.
//!
//! `gpui::img("data:…")` routes the URI through the platform HTTP client, which
//! the web host cannot do for `data:` URLs, so pictures are decoded locally and
//! handed to the image element as `ImageSource::Image` instead.

/// Decode the payload of a `data:<mime>;base64,<payload>` URL.
pub fn decode_base64_data_url(data_url: &str) -> Option<Vec<u8>> {
    let (head, payload) = data_url.split_once(',')?;
    if !head.ends_with(";base64") && !head.contains(";base64;") {
        return None;
    }
    decode_base64(payload)
}

/// Minimal standard base64 decoder accepting padding and embedded whitespace.
pub fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        Some(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for byte in input.bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        if byte == b'=' {
            bits = 0;
            continue;
        }
        let value = value(byte)?;
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_rfc4648_vectors_and_data_urls() {
        assert_eq!(decode_base64(""), Some(Vec::new()));
        assert_eq!(decode_base64("Zg=="), Some(b"f".to_vec()));
        assert_eq!(decode_base64("Zm8="), Some(b"fo".to_vec()));
        assert_eq!(decode_base64("Zm9vYmFy"), Some(b"foobar".to_vec()));
        assert_eq!(
            decode_base64_data_url("data:image/png;base64,Zm9vYmFy"),
            Some(b"foobar".to_vec())
        );
        assert_eq!(decode_base64_data_url("data:image/png,Zm9v"), None);
        assert_eq!(decode_base64("!!!!"), None);
    }
}
