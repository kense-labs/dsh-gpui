//! Physical bindings shared by the renderer and the desktop keyboard adapter.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CONTROL: u8 = 1;
pub const ALT: u8 = 2;
pub const SHIFT: u8 = 4;
pub const META: u8 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Mac,
    Windows,
    Linux,
}

impl Platform {
    pub fn profile(self) -> &'static str {
        match self {
            Self::Mac => "desktop:macos",
            Self::Windows => "desktop:windows",
            Self::Linux => "desktop:linux",
        }
    }

    fn primary(self) -> u8 {
        if self == Self::Mac { META } else { CONTROL }
    }

    pub fn chords(self) -> bool {
        self != Self::Linux
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Shortcuts,
    NewSession,
    ToggleSidebar,
    SearchSessions,
    RenameSession,
    ForkSession,
    ArchiveSession,
    Settings,
    AddWorkspace,
    OpenWorkspace,
    ToggleDock,
    Files,
    NewBrowser,
    Terminal,
    Split,
    Fullscreen,
    Refresh,
    Close,
}

#[derive(Clone, Copy)]
pub struct Definition {
    pub command: Command,
    pub id: &'static str,
    pub label: &'static str,
    pub aliases: &'static str,
    code: &'static str,
    modifiers: u8,
    primary: bool,
}

macro_rules! command {
    ($name:ident, $id:literal, $label:literal, $aliases:literal, $code:literal, $mods:expr) => {
        Definition {
            command: Command::$name,
            id: $id,
            label: $label,
            aliases: $aliases,
            code: $code,
            modifiers: $mods,
            primary: true,
        }
    };
}

pub const COMMANDS: &[Definition] = &[
    command!(
        Shortcuts,
        "shortcuts.open",
        "快捷键速查",
        "shortcuts keyboard shortcuts",
        "Slash",
        0
    ),
    command!(
        NewSession,
        "session.new",
        "新会话",
        "new session new chat",
        "KeyN",
        0
    ),
    command!(
        ToggleSidebar,
        "sidebar.left.toggle",
        "切换左侧栏",
        "left sidebar toggle left panel",
        "KeyB",
        0
    ),
    command!(
        SearchSessions,
        "session.search",
        "搜索会话",
        "search sessions",
        "KeyK",
        0
    ),
    command!(
        RenameSession,
        "session.rename",
        "重命名会话",
        "rename session",
        "KeyG",
        ALT
    ),
    command!(
        ForkSession,
        "session.fork",
        "分叉会话",
        "fork session",
        "KeyF",
        ALT
    ),
    command!(
        ArchiveSession,
        "session.archive",
        "归档会话",
        "archive session",
        "KeyA",
        SHIFT
    ),
    command!(
        Settings,
        "settings.open",
        "打开设置",
        "settings preferences",
        "Comma",
        0
    ),
    command!(
        AddWorkspace,
        "workspace.add",
        "添加工作区",
        "add workspace open folder",
        "KeyO",
        0
    ),
    command!(
        OpenWorkspace,
        "workspace.openLocal",
        "在应用中打开工作区",
        "open workspace locally open in app",
        "KeyO",
        ALT
    ),
    command!(
        ToggleDock,
        "sidebar.right.toggle",
        "切换右侧栏",
        "right sidebar toggle right panel",
        "KeyB",
        ALT
    ),
    command!(
        Files,
        "workspace.files",
        "工作区文件",
        "workspace files",
        "KeyP",
        0
    ),
    command!(
        NewBrowser,
        "browser.new",
        "新建浏览器",
        "browser new browser tab",
        "KeyT",
        0
    ),
    Definition {
        command: Command::Terminal,
        id: "terminal.new",
        label: "新建终端",
        aliases: "new terminal shell",
        code: "Backquote",
        modifiers: CONTROL,
        primary: false,
    },
    command!(Split, "pane.split", "分栏", "split pane", "Backslash", 0),
    command!(
        Fullscreen,
        "pane.fullscreen.toggle",
        "切换栏内全屏",
        "fullscreen panel",
        "Enter",
        ALT
    ),
    command!(
        Refresh,
        "page.refresh",
        "刷新页面",
        "refresh page",
        "KeyR",
        0
    ),
    command!(Close, "page.close", "关闭页面", "close page", "KeyW", 0),
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Modifier {
    Primary,
    Control,
    Alt,
    Shift,
    Meta,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Binding {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub second_code: Option<String>,
    pub modifiers: Vec<Modifier>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Gesture {
    pub code: String,
    pub second: Option<String>,
    pub modifiers: u8,
}

pub fn physical_key(code: &str) -> Option<&str> {
    Some(match code {
        "Slash" => "/",
        "Comma" => ",",
        "Period" => ".",
        "Backslash" => "\\",
        "Backquote" => "\u{60}",
        "Minus" => "-",
        "Equal" => "=",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Semicolon" => ";",
        "Quote" => "'",
        "Escape" => "Esc",
        "ArrowUp" => "↑",
        "ArrowDown" => "↓",
        "ArrowLeft" => "←",
        "ArrowRight" => "→",
        "Enter" | "Space" | "Tab" | "Backspace" | "Delete" => code,
        _ => match code.as_bytes() {
            [b'K', b'e', b'y', b'A'..=b'Z'] => &code[3..],
            [b'D', b'i', b'g', b'i', b't', b'0'..=b'9'] => &code[5..],
            [b'F', b'1'..=b'9'] | [b'F', b'1', b'0'..=b'9'] | [b'F', b'2', b'0'..=b'4'] => code,
            _ => return None,
        },
    })
}

impl Gesture {
    pub fn new(code: &str, second: Option<&str>, modifiers: u8) -> Option<Self> {
        physical_key(code)?;
        let (code, second) = if let Some(second) = second {
            physical_key(second)?;
            if code == second {
                return None;
            }
            if code < second {
                (code, Some(second))
            } else {
                (second, Some(code))
            }
        } else {
            (code, None)
        };
        Some(Self {
            code: code.into(),
            second: second.map(str::to_owned),
            modifiers,
        })
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.modifiers == other.modifiers
            && match (&self.second, &other.second) {
                (Some(_), Some(_)) => self == other,
                (Some(second), None) => self.code == other.code || *second == other.code,
                (None, Some(second)) => self.code == other.code || self.code == *second,
                (None, None) => self.code == other.code,
            }
    }

    pub fn to_binding(&self) -> Binding {
        Binding {
            code: self.code.clone(),
            second_code: self.second.clone(),
            modifiers: [
                (CONTROL, Modifier::Control),
                (ALT, Modifier::Alt),
                (SHIFT, Modifier::Shift),
                (META, Modifier::Meta),
            ]
            .into_iter()
            .filter_map(|(flag, modifier)| (self.modifiers & flag != 0).then_some(modifier))
            .collect(),
        }
    }

    pub fn keycaps(&self, platform: Platform) -> Vec<&str> {
        let labels = if platform == Platform::Mac {
            ["⌃", "⌥", "⇧", "⌘"]
        } else {
            ["Ctrl", "Alt", "Shift", "Meta"]
        };
        let mut keys: Vec<_> = [CONTROL, ALT, SHIFT, META]
            .into_iter()
            .zip(labels)
            .filter_map(|(flag, label)| (self.modifiers & flag != 0).then_some(label))
            .collect();
        keys.push(physical_key(&self.code).unwrap_or(&self.code));
        if platform == Platform::Windows {
            keys = keys
                .into_iter()
                .enumerate()
                .flat_map(|(index, key)| (index != 0).then_some("+").into_iter().chain([key]))
                .collect();
        }
        if let Some(code) = &self.second {
            keys.push(physical_key(code).unwrap_or(code));
        }
        keys
    }

    pub fn label(&self, platform: Platform) -> String {
        self.keycaps(platform).join(" ")
    }

    pub fn aria(&self) -> Option<String> {
        if self.second.is_some() {
            return None;
        }
        let mut parts: Vec<_> = [CONTROL, ALT, SHIFT, META]
            .into_iter()
            .zip(["Control", "Alt", "Shift", "Meta"])
            .filter_map(|(flag, label)| (self.modifiers & flag != 0).then_some(label))
            .collect();
        let key = if self.code == "Escape" || self.code.starts_with("Arrow") {
            &self.code
        } else {
            physical_key(&self.code).unwrap_or(&self.code)
        };
        parts.push(key);
        Some(parts.join("+"))
    }

    fn issue(&self, platform: Platform) -> Option<&'static str> {
        if platform.chords() {
            return None;
        }
        if self.second.is_some() {
            return Some("暂不支持这个按键。");
        }
        if self.modifiers & !SHIFT == 0 {
            return Some("请同时按下 Ctrl 或 Alt 修饰键。");
        }
        let primary = self.modifiers & CONTROL != 0;
        let alt = self.modifiers & ALT != 0;
        if self.modifiers & META != 0
            || matches!(
                self.code.as_str(),
                "Escape"
                    | "Tab"
                    | "Space"
                    | "Backspace"
                    | "Delete"
                    | "ArrowUp"
                    | "ArrowDown"
                    | "ArrowLeft"
                    | "ArrowRight"
            )
            || self.code == "Enter" && !alt
            || alt && matches!(self.code.as_str(), "F2" | "F4")
            || primary
                && matches!(
                    self.code.as_str(),
                    "KeyC" | "KeyV" | "KeyX" | "KeyZ" | "KeyY" | "KeyQ" | "KeyH"
                )
            || primary && self.code == "KeyA" && self.modifiers & SHIFT == 0
        {
            return Some("此组合由系统或文本编辑操作保留。");
        }
        None
    }
}

impl Binding {
    fn normalize(&self, platform: Platform) -> Option<Gesture> {
        let modifiers = self.modifiers.iter().fold(0, |flags, modifier| {
            flags
                | match modifier {
                    Modifier::Primary => platform.primary(),
                    Modifier::Control => CONTROL,
                    Modifier::Alt => ALT,
                    Modifier::Shift => SHIFT,
                    Modifier::Meta => META,
                }
        });
        Gesture::new(&self.code, self.second_code.as_deref(), modifiers)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Document {
    pub schema_version: u8,
    pub profiles: BTreeMap<String, BTreeMap<String, Option<Binding>>>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: 1,
            profiles: BTreeMap::new(),
        }
    }
}

impl Document {
    pub fn valid(&self) -> bool {
        matches!(self.schema_version, 1 | 2)
            && self.profiles.iter().all(|(profile, bindings)| {
                matches!(
                    profile.as_str(),
                    "desktop:macos"
                        | "desktop:windows"
                        | "desktop:linux"
                        | "web:macos"
                        | "web:windows"
                        | "web:linux"
                ) && bindings.iter().all(|(id, binding)| {
                    let mut parts = id.split('.');
                    let valid_part = |part: &str| {
                        part.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                            && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                    };
                    parts.next().is_some_and(|part| {
                        valid_part(part) && part.as_bytes()[0].is_ascii_lowercase()
                    }) && id.contains('.')
                        && parts.all(valid_part)
                        && binding.as_ref().is_none_or(|binding| {
                            (self.schema_version == 2 || binding.second_code.is_none())
                                && binding.normalize(Platform::Mac).is_some()
                        })
                })
            })
    }

    pub fn modified_count(&self, platform: Platform) -> usize {
        self.profiles
            .get(platform.profile())
            .map_or(0, BTreeMap::len)
    }

    pub fn edit(&self, edit: &Edit, platform: Platform) -> Result<Self, String> {
        let mut next = self.clone();
        if platform.chords() {
            next.schema_version = 2;
        }
        let overrides = next.profiles.entry(platform.profile().into()).or_default();
        let id = match edit {
            Edit::Set { id, binding } => {
                overrides.insert(id.clone(), binding.clone());
                Some(id)
            }
            Edit::Reset { id } => {
                overrides.remove(id);
                Some(id)
            }
            Edit::ResetAll => {
                overrides.clear();
                None
            }
        };
        if id.is_some_and(|id| !COMMANDS.iter().any(|row| row.id == id)) || !next.valid() {
            return Err("暂不支持这个按键。".into());
        }
        let map = Keymap::new(&next, platform);
        if let Some(row) = map.rows.iter().find(|row| {
            id.is_none_or(|id| id == row.definition.id)
                && (row.issue.is_some() || !row.conflicts.is_empty())
        }) {
            if let Some(issue) = row.issue {
                return Err(issue.into());
            }
            return Err(format!("已被「{}」占用", row.conflicts.join("、")));
        }
        if let Edit::Set {
            id,
            binding: Some(binding),
        } = edit
            && let Some(candidate) = binding.normalize(platform)
        {
            let displaced: Vec<_> = map
                .rows
                .iter()
                .filter(|row| {
                    row.definition.id != id
                        && row.issue.is_none()
                        && row
                            .binding
                            .as_ref()
                            .is_some_and(|key| key.overlaps(&candidate))
                })
                .map(|row| row.definition.label)
                .collect();
            if !displaced.is_empty() {
                return Err(format!("已被「{}」占用", displaced.join("、")));
            }
        }
        Ok(next)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Edit {
    Set {
        id: String,
        binding: Option<Binding>,
    },
    Reset {
        id: String,
    },
    ResetAll,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: u64,
    pub document: Document,
    pub error: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            document: Document::default(),
            error: None,
        }
    }
}

pub struct Row {
    pub definition: Definition,
    pub binding: Option<Gesture>,
    pub modified: bool,
    pub issue: Option<&'static str>,
    pub conflicts: Vec<&'static str>,
}

// Fixed editing and menu gestures reserve combinations without making them global.
const FIXED: &[(&str, u8, bool, &str)] = &[
    ("ArrowUp", 0, false, "移动菜单选择"),
    ("ArrowDown", 0, false, "移动菜单选择"),
    ("Enter", 0, false, "选择菜单项"),
    ("Escape", 0, false, "关闭菜单或顶层弹窗"),
    ("Enter", SHIFT, false, "换行"),
    ("Enter", 0, false, "发送消息"),
    ("Enter", CONTROL, false, "组合键发送"),
    ("Enter", META, false, "组合键发送"),
    ("Slash", 0, false, "打开指令菜单"),
    ("Digit2", SHIFT, false, "打开引用菜单"),
    ("Enter", 0, false, "允许一次"),
    ("Escape", 0, false, "拒绝"),
    ("Escape", 0, false, "停止生成"),
    ("Equal", 0, true, "增大正文字号"),
    ("Equal", SHIFT, true, "增大正文字号"),
    ("Minus", 0, true, "减小正文字号"),
];

pub struct Keymap {
    pub rows: Vec<Row>,
}

impl Keymap {
    pub fn new(document: &Document, platform: Platform) -> Self {
        let overrides = document.profiles.get(platform.profile());
        let mut rows: Vec<_> = COMMANDS
            .iter()
            .map(|&definition| {
                let value = overrides.and_then(|overrides| overrides.get(definition.id));
                let binding = match value {
                    Some(binding) => binding.as_ref().and_then(|b| b.normalize(platform)),
                    None => Gesture::new(
                        definition.code,
                        None,
                        definition.modifiers
                            | if definition.primary {
                                platform.primary()
                            } else {
                                0
                            },
                    ),
                };
                let issue = binding.as_ref().and_then(|key| key.issue(platform));
                Row {
                    definition,
                    binding,
                    modified: value.is_some(),
                    issue,
                    conflicts: Vec::new(),
                }
            })
            .collect();
        let conflicts: Vec<_> = rows
            .iter()
            .map(|row| {
                let mut conflicts = Vec::new();
                if let Some(binding) = &row.binding {
                    for other in &rows {
                        if other.definition.id != row.definition.id
                            && (!row.modified || other.modified)
                            && other.issue.is_none()
                            && other
                                .binding
                                .as_ref()
                                .is_some_and(|other| other.overlaps(binding))
                        {
                            conflicts.push(other.definition.label);
                        }
                    }
                    for &(code, modifiers, primary, label) in FIXED {
                        let modifiers = modifiers | if primary { platform.primary() } else { 0 };
                        if Gesture::new(code, None, modifiers)
                            .is_some_and(|key| key.overlaps(binding))
                            && !conflicts.contains(&label)
                        {
                            conflicts.push(label);
                        }
                    }
                }
                conflicts
            })
            .collect();
        for (row, conflicts) in rows.iter_mut().zip(conflicts) {
            row.conflicts = conflicts;
        }
        Self { rows }
    }

    pub fn find(&self, code: &str, second: Option<&str>, modifiers: u8) -> Option<Command> {
        let gesture = Gesture::new(code, second, modifiers)?;
        self.rows.iter().find_map(|row| {
            (row.issue.is_none()
                && row.conflicts.is_empty()
                && row.binding.as_ref() == Some(&gesture))
            .then_some(row.definition.command)
        })
    }
}

#[derive(Default)]
pub struct HeldKeys {
    codes: Vec<String>,
}

impl HeldKeys {
    pub fn clear(&mut self) {
        self.codes.clear();
    }

    pub fn release(&mut self, code: &str) {
        if modifier_key(code) != 0 {
            self.clear();
        } else {
            self.codes.retain(|key| key != code);
        }
    }

    pub fn press(
        &mut self,
        code: &str,
        modifiers: u8,
        keys: &Keymap,
        chords: bool,
    ) -> Option<Command> {
        if modifier_key(code) != 0 {
            self.clear();
            return None;
        }
        physical_key(code)?;
        let chord = if chords && self.codes.len() == 1 && self.codes[0] != code {
            keys.find(code, Some(&self.codes[0]), modifiers)
        } else {
            None
        };
        let command = chord.or_else(|| keys.find(code, None, modifiers));
        if command.is_some() {
            self.clear();
        } else if !self.codes.iter().any(|key| key == code) {
            self.codes.push(code.into());
        }
        command
    }
}

pub fn modifier_key(code: &str) -> u8 {
    match code {
        "ControlLeft" | "ControlRight" => CONTROL,
        "AltLeft" | "AltRight" => ALT,
        "ShiftLeft" | "ShiftRight" => SHIFT,
        "MetaLeft" | "MetaRight" => META,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_add_keeps_the_official_mod_o_default() {
        let add = COMMANDS
            .iter()
            .find(|definition| definition.id == "workspace.add")
            .expect("the official workspace.add command must be registered");
        assert_eq!(add.command, Command::AddWorkspace);
        assert_eq!(add.code, "KeyO");
        assert_eq!(add.modifiers, 0);
        assert!(add.primary);

        // Harness binds "在应用中打开工作区" to Mod+Alt+O. Adding Mod+O must
        // not displace it or produce a conflict in the default keymap.
        let keymap = Keymap::new(&Document::default(), Platform::Mac);
        let row = |command| {
            keymap
                .rows
                .iter()
                .find(|row| row.definition.command == command)
                .unwrap()
        };
        let add_row = row(Command::AddWorkspace);
        let open_row = row(Command::OpenWorkspace);
        assert_eq!(add_row.issue, None);
        assert!(add_row.conflicts.is_empty());
        assert_eq!(open_row.issue, None);
        assert_ne!(add_row.binding, open_row.binding);
        assert_eq!(
            add_row.binding.as_ref().map(|binding| binding.modifiers),
            Some(META)
        );
        assert_eq!(
            open_row.binding.as_ref().map(|binding| binding.modifiers),
            Some(META | ALT)
        );
    }
}
