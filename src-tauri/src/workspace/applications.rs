//! Resolve the Harness macOS directory openers against the local installation.

use super::canonical_root;
use serde::Serialize;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

enum Locator {
    Finder,
    Bundles(&'static [&'static str]),
    Xcode,
    Terminal,
}

const FINDER: &str = "/System/Library/CoreServices/Finder.app";
const TERMINAL: &str = "/System/Applications/Utilities/Terminal.app";

const CATALOG: &[(&str, &str, Locator)] = &[
    ("finder", "访达", Locator::Finder),
    ("cursor", "Cursor", Locator::Bundles(&["Cursor.app"])),
    (
        "vscode",
        "VS Code",
        Locator::Bundles(&["Visual Studio Code.app"]),
    ),
    (
        "vscodeinsiders",
        "VS Code Insiders",
        Locator::Bundles(&["Visual Studio Code - Insiders.app"]),
    ),
    ("windsurf", "Windsurf", Locator::Bundles(&["Windsurf.app"])),
    (
        "zed",
        "Zed",
        Locator::Bundles(&["Zed.app", "Zed Preview.app"]),
    ),
    (
        "sublimetext",
        "Sublime Text",
        Locator::Bundles(&["Sublime Text.app"]),
    ),
    ("xcode", "Xcode", Locator::Xcode),
    (
        "androidstudio",
        "Android Studio",
        Locator::Bundles(&["Android Studio.app"]),
    ),
    (
        "intellij",
        "IntelliJ IDEA",
        Locator::Bundles(&[
            "IntelliJ IDEA.app",
            "IntelliJ IDEA Ultimate.app",
            "IntelliJ IDEA CE.app",
        ]),
    ),
    (
        "pycharm",
        "PyCharm",
        Locator::Bundles(&[
            "PyCharm.app",
            "PyCharm Professional.app",
            "PyCharm CE.app",
            "PyCharm Community.app",
        ]),
    ),
    ("webstorm", "WebStorm", Locator::Bundles(&["WebStorm.app"])),
    ("phpstorm", "PhpStorm", Locator::Bundles(&["PhpStorm.app"])),
    ("goland", "GoLand", Locator::Bundles(&["GoLand.app"])),
    (
        "rider",
        "Rider",
        Locator::Bundles(&["Rider.app", "JetBrains Rider.app"]),
    ),
    (
        "rustrover",
        "RustRover",
        Locator::Bundles(&["RustRover.app"]),
    ),
    ("fork", "Fork", Locator::Bundles(&["Fork.app"])),
    (
        "sourcetree",
        "Sourcetree",
        Locator::Bundles(&["Sourcetree.app"]),
    ),
    (
        "github",
        "GitHub Desktop",
        Locator::Bundles(&["GitHub Desktop.app"]),
    ),
    ("tower", "Tower", Locator::Bundles(&["Tower.app"])),
    (
        "gitkraken",
        "GitKraken",
        Locator::Bundles(&["GitKraken.app"]),
    ),
    ("smartgit", "SmartGit", Locator::Bundles(&["SmartGit.app"])),
    (
        "sublimemerge",
        "Sublime Merge",
        Locator::Bundles(&["Sublime Merge.app"]),
    ),
    ("ghostty", "Ghostty", Locator::Bundles(&["Ghostty.app"])),
    ("warp", "Warp", Locator::Bundles(&["Warp.app"])),
    ("iterm", "iTerm2", Locator::Bundles(&["iTerm.app"])),
    ("kitty", "kitty", Locator::Bundles(&["kitty.app"])),
    ("terminal", "终端", Locator::Terminal),
];

impl Locator {
    fn bundle(&self, roots: &[PathBuf], developer: Option<&Path>) -> Option<PathBuf> {
        match self {
            Self::Finder => Some(FINDER.into()),
            Self::Terminal => Some(TERMINAL.into()),
            Self::Xcode => {
                let bundle = developer?.parent()?.parent()?;
                (bundle.extension().is_some_and(|ext| ext == "app") && bundle.is_dir())
                    .then(|| bundle.to_path_buf())
            }
            Self::Bundles(names) => roots
                .iter()
                .flat_map(|root| names.iter().map(|name| root.join(name)))
                .find(|bundle| bundle.is_dir()),
        }
    }
}

fn application_roots() -> Vec<PathBuf> {
    let mut roots = vec!["/Applications".into()];
    roots.extend(std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Applications")));
    roots
}

fn selected_developer_directory() -> Option<PathBuf> {
    let output = Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .output()
        .ok()?;
    let path = String::from_utf8(output.stdout).ok()?;
    output.status.success().then(|| PathBuf::from(path.trim()))
}

fn declared_icon(bundle: &Path) -> Option<PathBuf> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleIconFile", "raw", "-o", "-"])
        .arg(bundle.join("Contents/Info.plist"))
        .output()
        .ok()?;
    let name = std::str::from_utf8(&output.stdout).ok()?.trim();
    if !output.status.success() || name.is_empty() || Path::new(name).file_name()? != name {
        return None;
    }
    Some(if name.ends_with(".icns") {
        name.into()
    } else {
        format!("{name}.icns").into()
    })
}

fn extract_icon(bundle: &Path, target: &Path) -> Option<Vec<u8>> {
    let resources = bundle.join("Contents/Resources");
    let icon = declared_icon(bundle)
        .map(|name| resources.join(name))
        .or_else(|| {
            let mut icons: Vec<_> = fs::read_dir(&resources)
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "icns"))
                .collect();
            icons.sort_unstable();
            icons.into_iter().next()
        })?;
    let status = Command::new("/usr/bin/sips")
        .args(["-s", "format", "png", "-Z", "128"])
        .arg(icon)
        .arg("--out")
        .arg(target)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    status.success().then_some(())?;
    fs::read(target).ok()
}

#[derive(Serialize)]
pub(crate) struct WorkspaceApplication {
    id: &'static str,
    name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<Vec<u8>>,
}

#[tauri::command]
pub(crate) async fn workspace_applications(
    webview: tauri::Webview,
) -> Result<Vec<WorkspaceApplication>, String> {
    if webview.label() != "main" {
        return Err("只能从主界面读取工作区应用。".into());
    }
    tokio::task::spawn_blocking(|| {
        let roots = application_roots();
        let developer = selected_developer_directory();
        let work = std::env::temp_dir().join(format!("dsh-app-icons-{}", uuid::Uuid::new_v4()));
        let can_extract = fs::create_dir(&work).is_ok();
        let applications = CATALOG
            .iter()
            .filter_map(|&(id, name, ref locator)| {
                let bundle = locator.bundle(&roots, developer.as_deref())?;
                let icon = can_extract
                    .then(|| extract_icon(&bundle, &work.join(format!("{id}.png"))))
                    .flatten();
                Some(WorkspaceApplication { id, name, icon })
            })
            .collect();
        if can_extract {
            let _ = fs::remove_dir_all(work);
        }
        applications
    })
    .await
    .map_err(|error| error.to_string())
}

fn launch(mut command: Command) -> bool {
    for key in crate::private_child_env_keys() {
        command.env_remove(key);
    }
    command.process_group(0);
    let Ok(mut child) = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return true;
            }
            Err(_) => return false,
        }
    }
}

#[tauri::command]
pub(crate) async fn open_workspace_application(
    webview: tauri::Webview,
    workspace: String,
    application: String,
    remember: bool,
) -> Result<Option<String>, String> {
    if webview.label() != "main" {
        return Err("只能从主界面打开工作区。".into());
    }
    tokio::task::spawn_blocking(move || {
        let workspace = canonical_root(Path::new(&workspace))?;
        let (_, _, locator) = CATALOG
            .iter()
            .find(|entry| entry.0 == application)
            .ok_or_else(|| "未知的工作区应用。".to_owned())?;
        let developer = matches!(locator, Locator::Xcode)
            .then(selected_developer_directory)
            .flatten();
        let bundle = locator
            .bundle(&application_roots(), developer.as_deref())
            .ok_or_else(|| "应用已不可用。".to_owned())?;

        if matches!(locator, Locator::Xcode) {
            let mut xed = Command::new("/usr/bin/xed");
            xed.arg(&workspace);
            if launch(xed) {
                return Ok(remember.then_some(application));
            }
        }

        let mut open = Command::new("/usr/bin/open");
        match locator {
            Locator::Finder => {}
            Locator::Terminal => {
                open.args(["-a", "Terminal"]);
            }
            _ => {
                open.arg("-a").arg(bundle);
            }
        }
        open.arg(workspace);
        launch(open)
            .then(|| remember.then_some(application))
            .ok_or_else(|| "打开失败，请重试".to_owned())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_in_harness_order_with_system_roots_first_and_selected_xcode() {
        let root = std::env::temp_dir().join(format!("dsh-app-test-{}", uuid::Uuid::new_v4()));
        let system = root.join("System");
        let user = root.join("User");
        let zed_preview = system.join("Zed Preview.app");
        let xcode = root.join("Xcode-beta.app");
        let developer = xcode.join("Contents/Developer");
        let command_line_tools = root.join("Developer/CommandLineTools");
        for path in [
            &zed_preview,
            &user.join("Zed.app"),
            &user.join("Cursor.app"),
            &developer,
            &command_line_tools,
        ] {
            fs::create_dir_all(path).unwrap();
        }
        let roots = [system, user];
        let resolve = |developer: &Path| -> Vec<_> {
            CATALOG
                .iter()
                .filter_map(|(id, _, locator)| {
                    locator
                        .bundle(&roots, Some(developer))
                        .map(|bundle| (*id, bundle))
                })
                .collect()
        };

        let with_xcode = resolve(&developer);
        let without_xcode = resolve(&command_line_tools);
        fs::remove_dir_all(root).unwrap();
        assert_eq!(
            with_xcode.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            ["finder", "cursor", "zed", "xcode", "terminal"]
        );
        assert_eq!(with_xcode[2].1, zed_preview);
        assert_eq!(with_xcode[3].1, xcode);
        assert_eq!(
            without_xcode.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            ["finder", "cursor", "zed", "terminal"]
        );
    }

    #[test]
    fn reads_system_bundle_icon_declarations() {
        for bundle in [FINDER, TERMINAL] {
            let path = Path::new(bundle);
            let icon = declared_icon(path).unwrap();
            assert!(path.join("Contents/Resources").join(icon).is_file());
        }
    }
}
