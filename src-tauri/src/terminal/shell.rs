//! Resolve the launch profiles in the local execution environment.

use portable_pty::CommandBuilder;
use serde::Serialize;
use std::{
    collections::HashSet,
    env,
    path::{Path, PathBuf},
};

const SHELL_CANDIDATES: [&str; 6] = ["zsh", "bash", "fish", "pwsh", "powershell", "cmd"];

#[derive(Serialize)]
pub struct ShellProfile {
    pub(super) path: String,
    pub(super) name: String,
    args: &'static [&'static str],
}

impl ShellProfile {
    fn resolve(command: &str, path: &[PathBuf], cwd: &Path) -> Option<(String, Self)> {
        let executable = resolve_executable(command, path, cwd)?;
        let name = Path::new(command).file_name()?.to_str()?.to_owned();
        let args = match shell_kind(&name).as_str() {
            "cmd" => &[][..],
            "pwsh" | "powershell" => &["-NoLogo"][..],
            _ => &["-i"][..],
        };
        let kind = shell_kind(executable.file_name()?.to_str()?);
        Some((
            kind,
            Self {
                path: executable.to_str()?.into(),
                name,
                args,
            },
        ))
    }

    pub(super) fn command(&self, cwd: &Path, session: &str) -> CommandBuilder {
        let mut cmd = CommandBuilder::new(&self.path);
        cmd.args(self.args);
        cmd.cwd(cwd);
        for key in crate::private_child_env_keys() {
            cmd.env_remove(key);
        }
        cmd.env("DSH_SESSION_ID", session);
        cmd.env("TERM", "xterm-256color");
        #[cfg(unix)]
        ensure_utf8_locale(&mut cmd);
        cmd
    }
}

/// zsh renders UTF-8 prompt characters as literal `\M-^X` sequences when its
/// effective locale is C/POSIX. GUI launches (Finder / launchd) inherit no
/// LANG at all, so force a UTF-8 locale unless one is already configured.
#[cfg(unix)]
fn ensure_utf8_locale(cmd: &mut CommandBuilder) {
    let vars: Vec<(&str, Option<String>)> = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .map(|key| (*key, env::var(key).ok()))
        .collect();
    if !needs_utf8_locale(&vars) {
        return;
    }
    // macOS ships en_US.UTF-8; C.UTF-8 is available on modern Linux/glibc.
    let locale = if cfg!(target_os = "macos") {
        "en_US.UTF-8"
    } else {
        "C.UTF-8"
    };
    // LC_ALL takes precedence over LANG, so replace it too when it is C/POSIX.
    cmd.env("LC_ALL", locale);
    cmd.env("LC_CTYPE", locale);
    cmd.env("LANG", locale);
}

fn is_utf8_locale(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    upper.contains("UTF-8") || upper.contains("UTF8")
}

/// The first non-empty variable in precedence order decides whether the shell
/// will treat multi-byte characters as printable.
fn needs_utf8_locale(vars: &[(&str, Option<String>)]) -> bool {
    for (_, value) in vars {
        if let Some(value) = value.as_deref().filter(|value| !value.is_empty()) {
            return !is_utf8_locale(value);
        }
    }
    true
}

#[cfg(test)]
#[test]
fn forces_a_utf8_locale_when_the_effective_locale_is_not_multibyte() {
    let vars = |entries: &[(&'static str, &'static str)]| {
        entries
            .iter()
            .map(|(key, value)| (*key, Some((*value).to_owned())))
            .collect::<Vec<(&str, Option<String>)>>()
    };
    assert!(needs_utf8_locale(&vars(&[])));
    assert!(needs_utf8_locale(&vars(&[("LANG", "C")])));
    assert!(needs_utf8_locale(&vars(&[
        ("LC_ALL", "POSIX"),
        ("LANG", "en_US.UTF-8"),
    ])));
    assert!(!needs_utf8_locale(&vars(&[("LANG", "zh_CN.UTF-8")])));
    assert!(!needs_utf8_locale(&vars(&[
        ("LC_CTYPE", "en_US.UTF-8"),
        ("LANG", "C"),
    ])));
}

fn shell_kind(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    name.strip_suffix(".exe").unwrap_or(&name).into()
}

pub(super) fn discover() -> Result<Vec<ShellProfile>, String> {
    let cwd = env::current_dir().map_err(|error| error.to_string())?;
    let path = env::split_paths(&env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>();
    discover_from(&default_shell(), &path, &cwd)
}

fn discover_from(default: &str, path: &[PathBuf], cwd: &Path) -> Result<Vec<ShellProfile>, String> {
    let (kind, default_shell) = ShellProfile::resolve(default, path, cwd)
        .ok_or_else(|| format!("无法找到可执行的默认 Shell：{default}"))?;
    let mut seen = HashSet::from([kind]);
    let mut profiles = vec![default_shell];
    for candidate in SHELL_CANDIDATES {
        if let Some((kind, profile)) = ShellProfile::resolve(candidate, path, cwd)
            && seen.insert(kind)
        {
            profiles.push(profile);
        }
    }
    Ok(profiles)
}

fn resolve_executable(command: &str, path: &[PathBuf], cwd: &Path) -> Option<PathBuf> {
    let executable = Path::new(command);
    if executable.is_absolute() {
        return is_executable(executable).then(|| executable.to_path_buf());
    }
    if command.is_empty() || executable.file_name()? != command {
        return None;
    }
    #[cfg(windows)]
    let extensions = if executable.extension().is_none() {
        env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
    } else {
        String::new()
    };
    #[cfg(not(windows))]
    let extensions = "";

    path.iter()
        .flat_map(|directory| {
            extensions
                .split(';')
                .map(move |extension| cwd.join(directory).join(format!("{command}{extension}")))
        })
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    if !path.is_file() {
        return false;
    }
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a NUL-terminated C string that outlives the call, which is
    // all `access` requires; the mode is a plain flag. It only reports whether the
    // current process may execute the file and has no other precondition.
    unsafe { libc::access(path.as_ptr(), libc::X_OK) == 0 }
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(windows)]
fn default_shell() -> String {
    env::var("ComSpec")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "cmd.exe".into())
}

#[cfg(unix)]
fn default_shell() -> String {
    env::var("SHELL")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(login_shell)
        .unwrap_or_else(|| "/bin/sh".into())
}

#[cfg(unix)]
fn login_shell() -> Option<String> {
    let mut storage = vec![0; 1024];
    loop {
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // SAFETY: `getuid` takes no arguments, cannot fail, and reads no memory
        // through a pointer; it simply returns the real user ID of this process.
        let uid = unsafe { libc::getuid() };
        // SAFETY: `entry` is a valid, suitably aligned, writable `passwd`, and
        // `storage` is a live buffer of the length passed alongside it, which is
        // the contract `getpwuid_r` expects for its result and scratch space. On
        // success `result` points at `entry`; on ERANGE nothing is assumed and the
        // buffer is retried larger. No aliasing: both buffers are local and no
        // reference to them exists while the call runs.
        let status = unsafe {
            libc::getpwuid_r(
                uid,
                entry.as_mut_ptr(),
                storage.as_mut_ptr(),
                storage.len(),
                &mut result,
            )
        };
        match status {
            libc::ERANGE if storage.len() < 64 * 1024 => storage.resize(storage.len() * 2, 0),
            0 if !result.is_null() => {
                // SAFETY: the call returned 0 and reported a non-null result, so
                // `getpwuid_r` initialized `entry`; `assume_init` is therefore
                // sound. `storage` is still alive, so any pointer it holds remains
                // valid, and the field is only read as a pointer below.
                let shell = unsafe { entry.assume_init() }.pw_shell;
                if shell.is_null() {
                    return None;
                }
                // SAFETY: `pw_shell` is either null (handled above) or a
                // NUL-terminated string owned by the still-live `storage`; the
                // borrow ends before `storage` is dropped or resized.
                return unsafe { std::ffi::CStr::from_ptr(shell) }
                    .to_str()
                    .ok()
                    .filter(|path| !path.is_empty())
                    .map(str::to_owned);
            }
            _ => return None,
        }
    }
}

#[cfg(not(any(windows, unix)))]
fn default_shell() -> String {
    "/bin/sh".into()
}

#[cfg(all(test, unix))]
#[test]
fn lists_the_verified_default_then_distinct_executable_candidates() {
    use std::{fs, os::unix::fs::PermissionsExt};
    let root = env::temp_dir().join(format!("dsh-shell-test-{}", uuid::Uuid::new_v4()));
    let installed = root.join("path");
    fs::create_dir_all(&installed).unwrap();
    let default = root.join("bash");
    fs::write(&default, "").unwrap();
    fs::set_permissions(&default, fs::Permissions::from_mode(0o755)).unwrap();
    for (name, mode) in [
        ("bash", 0o755),
        ("zsh", 0o755),
        ("fish", 0o644),
        ("pwsh", 0o755),
    ] {
        let candidate = installed.join(name);
        fs::write(&candidate, "").unwrap();
        fs::set_permissions(candidate, fs::Permissions::from_mode(mode)).unwrap();
    }
    let paths = [PathBuf::from("path")];
    let found = discover_from(default.to_str().unwrap(), &paths, &root).unwrap();
    let invalid_default = discover_from("missing", &paths, &root);
    fs::remove_dir_all(&root).unwrap();

    assert!(invalid_default.is_err());
    assert_eq!(
        found
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>(),
        ["bash", "zsh", "pwsh"]
    );
    assert_eq!(Path::new(&found[0].path), default);
    assert_eq!(Path::new(&found[1].path), installed.join("zsh"));
    assert_eq!(found[1].args, ["-i"]);
    assert_eq!(found[2].args, ["-NoLogo"]);
}
