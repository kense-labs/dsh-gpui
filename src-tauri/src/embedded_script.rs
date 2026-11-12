//! Embedded ES modules shipped inside the KWork host binary.
//!
//! The ACP session bridge and the credential helper used to be handed to Node
//! with `--eval`. That put ~90 KB of source onto the command line (a real
//! `ARG_MAX` risk once the child environment is included) and made every stack
//! trace point at a synthetic `[eval1]` location. Both are now written to real
//! `.mjs` files before spawn:
//!
//! * connection-owned modules live in `NamedTempFile`s that are dropped with
//!   the connection, so they disappear as soon as the child is gone;
//! * large immutable bundles are content-addressed in a cache directory and
//!   reused by every connection and every app launch.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::{Builder, NamedTempFile};

/// Materialise an embedded module as a private `.mjs` file owned by the caller.
pub(crate) fn write_module(name: &str, source: &[u8]) -> Result<NamedTempFile, String> {
    let prefix = format!("kwork-{name}-");
    let mut file = Builder::new()
        .prefix(&prefix)
        .suffix(".mjs")
        .tempfile()
        .map_err(|error| format!("无法创建 {name} 临时模块：{error}"))?;
    file.write_all(source)
        .map_err(|error| format!("无法写入 {name} 临时模块：{error}"))?;
    file.flush()
        .map_err(|error| format!("无法刷新 {name} 临时模块：{error}"))?;
    Ok(file)
}

/// Return a stable path for an immutable embedded module.
///
/// The filename is derived from the SHA-256 of the bytes, so a different build
/// can never reuse a stale bundle. The file is only written when it is missing
/// or its content no longer matches; concurrent launches are safe because the
/// staged file is renamed into place atomically.
pub(crate) fn cached_module(name: &str, source: &[u8]) -> Result<PathBuf, String> {
    let digest = format!("{:x}", Sha256::digest(source));
    let dir = std::env::temp_dir().join("kwork-embedded-modules");
    fs::create_dir_all(&dir).map_err(|error| format!("无法创建 {name} 模块缓存目录：{error}"))?;
    let target = dir.join(format!("{name}-{}.mjs", &digest[..16]));
    if same_content(&target, source) {
        return Ok(target);
    }
    let prefix = format!("{name}-");
    let mut staged = Builder::new()
        .prefix(&prefix)
        .suffix(".mjs")
        .tempfile_in(&dir)
        .map_err(|error| format!("无法创建 {name} 模块缓存文件：{error}"))?;
    staged
        .write_all(source)
        .map_err(|error| format!("无法写入 {name} 模块缓存：{error}"))?;
    staged
        .flush()
        .map_err(|error| format!("无法刷新 {name} 模块缓存：{error}"))?;
    match staged.persist(&target) {
        Ok(_) => Ok(target),
        Err(error) => {
            // Another launch may have published the identical bundle first.
            if same_content(&target, source) {
                return Ok(target);
            }
            Err(format!("无法缓存 {name} 模块：{error}"))
        }
    }
}

fn same_content(path: &Path, source: &[u8]) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.len() == source.len() as u64)
        .unwrap_or(false)
        && fs::read(path)
            .map(|bytes| bytes.as_slice() == source)
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_module_has_mjs_suffix_and_exact_content() {
        let source = b"export const value = 1;\n";
        let file = write_module("probe", source).expect("temporary module");
        assert!(file.path().to_string_lossy().ends_with(".mjs"));
        assert_eq!(fs::read(file.path()).unwrap().as_slice(), &source[..]);
    }

    #[test]
    fn cached_module_reuses_identical_content() {
        let source = b"export const value = 42;\n";
        let first = cached_module("kwork-cached-module-test", source).expect("first call");
        let second = cached_module("kwork-cached-module-test", source).expect("second call");
        assert_eq!(first, second);
        assert_eq!(fs::read(&first).unwrap().as_slice(), &source[..]);
        let _ = fs::remove_file(&first);
    }
}
