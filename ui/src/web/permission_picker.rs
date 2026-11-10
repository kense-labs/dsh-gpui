//! Permission catalog and per-session changes through the Harness command writer.

use super::*;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PermissionState {
    pub(super) current_value: String,
    pub(super) options: Vec<PermissionOption>,
}

impl PermissionState {
    pub(super) fn current_label(&self) -> &str {
        let mode = &self.current_value;
        self.options
            .iter()
            .find(|option| option.value == *mode)
            .map_or_else(|| permission_label(mode, mode), PermissionOption::label)
    }
}

#[derive(serde::Deserialize)]
pub(super) struct PermissionOption {
    pub(super) value: String,
    name: String,
    description: Option<String>,
}

impl PermissionOption {
    pub(super) fn label(&self) -> &str {
        permission_label(&self.value, &self.name)
    }

    pub(super) fn detail(&self) -> Option<&str> {
        if self.value == "auto" {
            Some("无沙箱运行；每次原生工具调用和 PTC 内层调用前由同一模型进行实验性审查。")
        } else {
            self.description.as_deref()
        }
    }
}

pub(super) fn permission_label<'a>(id: &str, fallback: &'a str) -> &'a str {
    match id {
        "read-only" => "仅可查看",
        "workspace-write" => "工作区内修改",
        "danger-full-access" => "完全权限",
        "auto" => "自动审查",
        _ => fallback,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PermissionRequestKind {
    Load,
    Change,
}

#[derive(Default)]
pub(super) struct PermissionMode {
    pub(super) state: Option<PermissionState>,
    pending: Option<PermissionRequestKind>,
    pub(super) error: Option<String>,
}

impl PermissionMode {
    pub(super) fn loading(&self) -> bool {
        self.pending == Some(PermissionRequestKind::Load)
    }

    pub(super) fn changing(&self) -> bool {
        self.pending == Some(PermissionRequestKind::Change)
    }
}

pub(super) struct PermissionMenu {
    pub(super) trigger: gpui::FocusHandle,
    pub(super) row: gpui::FocusHandle,
    pub(super) active: Option<usize>,
    pub(super) scroll: gpui::ScrollHandle,
    pub(super) chevron: DockSlide,
}

impl PermissionMenu {
    pub(super) fn new(cx: &mut Context<DshApp>) -> Self {
        Self {
            trigger: cx.focus_handle(),
            row: cx.focus_handle(),
            active: None,
            scroll: gpui::ScrollHandle::new(),
            chevron: DockSlide::with_curve(120., [0.25, 0.1, 0.25, 1.]),
        }
    }
}

impl DshApp {
    pub(super) fn toggle_permission_menu(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.conversation.permission_mode.changing() || self.confirm_permission.is_some() {
            return;
        }
        self.toggle_composer_menu(ComposerMenu::Permission);
        self.permission_menu.active = None;
        self.permission_menu
            .scroll
            .set_offset(gpui::point(px(0.), px(0.)));
        self.permission_menu.trigger.focus(w, cx);
        cx.notify();
    }

    pub(super) fn handle_permission_menu_key(
        &mut self,
        combo: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.composer_menu != Some(ComposerMenu::Permission) {
            return false;
        }
        let focused =
            self.permission_menu.trigger.is_focused(w) || self.permission_menu.row.is_focused(w);
        if combo == "Escape" || (combo == "shift+Tab" && focused) {
            self.composer_menu = None;
            self.permission_menu.trigger.focus(w, cx);
            cx.notify();
            return true;
        }
        if !focused {
            return false;
        }
        let Some(state) = self.conversation.permission_mode.state.as_ref() else {
            return false;
        };
        let count = state.options.len();
        if count == 0 {
            return false;
        }
        let active = self.permission_menu.active.filter(|index| *index < count);
        let next = match combo {
            "ArrowDown" => active.map_or(0, |index| (index + 1) % count),
            "ArrowUp" => active.map_or(count - 1, |index| (index + count - 1) % count),
            "Home" => 0,
            "End" => count - 1,
            "Tab" if active.is_none() => 0,
            "Enter" | "Tab" if active.is_some() => {
                let id = state.options[active.unwrap()].value.clone();
                self.choose_permission(id, w, cx);
                return true;
            }
            _ => return false,
        };
        self.permission_menu.active = Some(next);
        self.permission_menu.scroll.scroll_to_item(next);
        self.permission_menu.row.focus(w, cx);
        cx.notify();
        true
    }

    pub(super) fn request_permission_mode(&mut self) {
        let mode = &mut self.conversation.permission_mode;
        if !self.conversation.acp_open || mode.pending.is_some() {
            return;
        }
        if let Some(session) = self.active_session.as_ref() {
            mode.pending = Some(PermissionRequestKind::Load);
            mode.error = None;
            invoke("permission_mode", json!({"sessionId": session}));
        }
    }

    pub(super) fn finish_permission_mode(
        &mut self,
        owner: Option<&str>,
        result: Result<Value, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.active_session.as_deref() == owner;
        let Some(mode) = owner
            .and_then(|session| self.conversation_mut(session))
            .map(|conversation| &mut conversation.permission_mode)
        else {
            return;
        };
        let pending = mode.pending.take();
        match result.and_then(|reply| {
            serde_json::from_value(reply).map_err(|_| "无法读取当前会话的权限模式。".into())
        }) {
            Ok(state) => {
                mode.state = Some(state);
                if selected && self.composer_menu == Some(ComposerMenu::PermissionChoices) {
                    if pending == Some(PermissionRequestKind::Change) {
                        if self.command_picker.settling {
                            self.finish_command_picker(w, cx);
                        }
                    } else {
                        self.highlight_current_command_option(ComposerMenu::PermissionChoices, cx);
                    }
                }
            }
            Err(error) => {
                mode.error = Some(error.clone());
                if selected
                    && pending == Some(PermissionRequestKind::Change)
                    && self.composer_menu != Some(ComposerMenu::PermissionChoices)
                {
                    self.push_toast(error, cx);
                }
            }
        }
    }

    pub(super) fn choose_permission(&mut self, id: String, w: &mut Window, cx: &mut Context<Self>) {
        let from_picker = self.composer_menu == Some(ComposerMenu::PermissionChoices);
        if !from_picker {
            self.composer_menu = None;
            self.permission_menu.trigger.focus(w, cx);
        }
        let mode = &self.conversation.permission_mode;
        if mode.pending.is_some() {
            return;
        }
        let current = mode
            .state
            .as_ref()
            .is_some_and(|state| state.current_value == id);
        if matches!(id.as_str(), "danger-full-access" | "auto") && (from_picker || !current) {
            self.confirm_permission = Some(id);
            self.full_access_acknowledged = false;
        } else if current {
            if from_picker {
                self.finish_command_picker(w, cx);
            }
        } else {
            self.apply_permission(id, false);
        }
        cx.notify();
    }

    pub(super) fn apply_permission(&mut self, id: String, acknowledged: bool) {
        let mode = &mut self.conversation.permission_mode;
        if mode.pending.is_none()
            && let Some(session) = self.active_session.as_ref()
        {
            mode.pending = Some(PermissionRequestKind::Change);
            mode.error = None;
            self.command_picker.settling =
                self.composer_menu == Some(ComposerMenu::PermissionChoices);
            invoke(
                "permission_mode",
                json!({"sessionId": session, "preset": id, "acknowledged": acknowledged}),
            );
        }
    }
}
