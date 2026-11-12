//! Search, recording, and revision-checked editing of desktop shortcuts.

use super::{dock_controls::dock_action_button, *};
use crate::shortcuts::{self, Command, Edit, Gesture, HeldKeys, Keymap, Platform, Snapshot};
use gpui::FocusHandle;

const TITLE: &str = "快捷键";
const SEARCH: &str = "搜索快捷键";
const RESET_ALL: &str = "恢复全部默认";

pub(super) struct ShortcutUi {
    pub(super) open: bool,
    search: gpui::Entity<Editor>,
    recorder: FocusHandle,
    _search_subscription: gpui::Subscription,
    _recorder_focus: gpui::Subscription,
    _recorder_blur: gpui::Subscription,
    scroll: ScrollHandle,
    platform: Platform,
    snapshot: Snapshot,
    map: Keymap,
    held: HeldKeys,
    consumed: std::collections::HashSet<String>,
    draft: Option<Draft>,
    reset: bool,
    busy: bool,
    recording_revision: u32,
    recording_requested: bool,
    restore_focus: Option<FocusHandle>,
}

struct Draft {
    id: &'static str,
    ready: bool,
    held: Vec<String>,
    pending: Option<Gesture>,
    shown: Option<Gesture>,
    blocked: bool,
    error: Option<String>,
    retry: Option<Edit>,
}

impl ShortcutUi {
    pub(super) fn new(w: &mut Window, cx: &mut Context<DshApp>) -> Self {
        let search = editor(SEARCH.into(), InputMode::Single, w, cx);
        let subscription = cx.subscribe(&search, |app, _, event: &EditEvent, cx| {
            if *event == EditEvent::Changed {
                app.shortcuts.scroll.set_offset(gpui::point(px(0.), px(0.)));
                cx.notify();
            }
        });
        let platform = if bridge::is_mac() {
            Platform::Mac
        } else if bridge::is_windows() {
            Platform::Windows
        } else {
            Platform::Linux
        };
        let snapshot = Snapshot::default();
        let map = Keymap::new(&snapshot.document, platform);
        let recorder = cx.focus_handle();
        let recorder_focus = cx.on_focus(&recorder, w, |app, _, cx| {
            if app.shortcuts.draft.is_some() {
                app.set_shortcut_recording(true, cx);
            }
        });
        let recorder_blur = cx.on_focus_out(&recorder, w, |app, _, _, cx| {
            if let Some(draft) = app.shortcuts.draft.as_mut() {
                draft.held.clear();
                draft.pending = None;
                draft.blocked = false;
            }
            app.set_shortcut_recording(false, cx);
        });
        Self {
            open: false,
            search,
            recorder,
            _search_subscription: subscription,
            _recorder_focus: recorder_focus,
            _recorder_blur: recorder_blur,
            scroll: ScrollHandle::new(),
            platform,
            snapshot,
            map,
            held: HeldKeys::default(),
            consumed: Default::default(),
            draft: None,
            reset: false,
            busy: false,
            recording_revision: 0,
            recording_requested: false,
            restore_focus: None,
        }
    }

    pub(super) fn accepted_event(&self, event: &Value) -> bool {
        event["bindingRevision"].as_u64().unwrap_or(0) == self.snapshot.revision
    }

    pub(super) fn revision(&self) -> u64 {
        self.snapshot.revision
    }

    fn binding(&self, command: Command) -> Option<&Gesture> {
        self.map
            .rows
            .iter()
            .find(|row| {
                row.definition.command == command && row.conflicts.is_empty() && row.issue.is_none()
            })?
            .binding
            .as_ref()
    }

    pub(super) fn keycaps(&self, command: Command) -> Vec<String> {
        self.binding(command)
            .map(|binding| {
                binding
                    .keycaps(self.platform)
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn aria(&self, command: Command) -> Option<String> {
        self.binding(command)?.aria()
    }
}

impl DshApp {
    pub(super) fn open_shortcuts(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        super::document_html::restore_shell_focus();
        self.shortcuts.restore_focus = w.focused(cx);
        self.shortcuts.open = true;
        self.shortcuts.held.clear();
        self.shortcuts
            .scroll
            .set_offset(gpui::point(px(0.), px(0.)));
        replace(&self.shortcuts.search, "", w, cx);
        self.shortcuts
            .search
            .update(cx, |editor, cx| editor.focus(w, cx));
        if let Some(error) = self.shortcuts.snapshot.error.clone() {
            self.push_toast(error, cx);
        }
        cx.notify();
    }

    pub(super) fn dismiss_shortcuts(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.shortcuts.busy {
            return;
        }
        if self.shortcuts.reset {
            self.shortcuts.reset = false;
        } else if self.shortcuts.draft.take().is_some() {
            self.set_shortcut_recording(false, cx);
            self.shortcuts
                .search
                .update(cx, |editor, cx| editor.focus(w, cx));
        } else {
            self.shortcuts.open = false;
            if let Some(focus) = self.shortcuts.restore_focus.take() {
                focus.focus(w, cx);
            }
        }
        self.shortcuts.held.clear();
        cx.notify();
    }

    fn set_shortcut_recording(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.shortcuts.platform != Platform::Mac || self.shortcuts.recording_requested == active
        {
            return;
        }
        let Some(shell) = self.browser_shell else {
            if let Some(draft) = self.shortcuts.draft.as_mut() {
                draft.ready = true;
            }
            return;
        };
        self.shortcuts.recording_requested = active;
        if active && let Some(draft) = self.shortcuts.draft.as_mut() {
            draft.ready = false;
        }
        self.shortcuts.recording_revision += 1;
        let revision = self.shortcuts.recording_revision;
        cx.spawn(async move |this, cx| {
            let result = bridge::invoke(
                "set_shortcut_recording",
                json!({"active":active,"revision":revision,"shell":shell}),
            )
            .await;
            let _ = this.update(cx, |app, cx| {
                if !active || app.shortcuts.recording_revision != revision {
                    return;
                }
                if let Some(draft) = app.shortcuts.draft.as_mut() {
                    draft.ready = result.is_ok_and(|value| value.as_bool() == Some(true));
                    if !draft.ready {
                        app.shortcuts.recording_requested = false;
                        draft.error = Some("无法启用桌面录键保护，请关闭后重试。".into());
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn begin_shortcut_recording(
        &mut self,
        id: &'static str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shortcuts.busy
            || self.shortcuts.snapshot.revision == 0
            || self.shortcuts.snapshot.error.is_some()
        {
            return;
        }
        self.shortcuts.draft = Some(Draft {
            id,
            ready: self.shortcuts.platform != Platform::Mac || self.shortcuts.recording_requested,
            held: Vec::new(),
            pending: None,
            shown: None,
            blocked: false,
            error: None,
            retry: None,
        });
        self.shortcuts.held.clear();
        self.shortcuts.recorder.focus(w, cx);
        self.set_shortcut_recording(true, cx);
        cx.notify();
    }

    fn save_shortcut(&mut self, edit: Edit, cx: &mut Context<Self>) {
        if self.shortcuts.busy {
            return;
        }
        if let Err(error) = self
            .shortcuts
            .snapshot
            .document
            .edit(&edit, self.shortcuts.platform)
        {
            if let Some(draft) = self.shortcuts.draft.as_mut() {
                draft.error = Some(error.clone());
            }
            self.push_toast(error, cx);
            return;
        }
        let args = json!({"revision":self.shortcuts.snapshot.revision, "edit":edit});
        if let Some(draft) = self.shortcuts.draft.as_mut() {
            draft.retry = Some(edit);
            draft.error = None;
        }
        self.shortcuts.busy = true;
        invoke("save_shortcuts", args);
        cx.notify();
    }

    pub(super) fn shortcut_reply(
        &mut self,
        save: bool,
        reply: Result<Value, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parsed = reply.and_then(|value| {
            serde_json::from_value::<Snapshot>(value).map_err(|error| error.to_string())
        });
        if save {
            self.shortcuts.busy = false;
        }
        match parsed {
            Ok(snapshot) if snapshot.revision >= self.shortcuts.snapshot.revision => {
                self.shortcuts.map = Keymap::new(&snapshot.document, self.shortcuts.platform);
                self.shortcuts.snapshot = snapshot;
                self.shortcuts.held.clear();
                if save {
                    let reset = self.shortcuts.reset;
                    self.dismiss_shortcuts(w, cx);
                    self.push_toast(
                        if reset {
                            "已恢复默认快捷键"
                        } else {
                            "已修改"
                        },
                        cx,
                    );
                }
            }
            Err(error) if save => {
                if let Some(draft) = self.shortcuts.draft.as_mut() {
                    draft.error = Some(error.clone());
                }
                self.push_toast(error, cx);
            }
            // A failed load leaves the official defaults in place; say so
            // instead of presenting them as a successfully restored profile.
            Err(error) => {
                if let Some(draft) = self.shortcuts.draft.as_mut() {
                    draft.error = Some(error.clone());
                }
                self.push_toast(format!("快捷键加载失败：{error}"), cx);
            }
            _ => {}
        }
    }

    pub(super) fn shortcut_input_event(
        &mut self,
        event: &Value,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.browser_shell.is_none() || self.browser_shell != event["shell"].as_u64() {
            return;
        }
        if event["kind"] == "shortcut_record_input" {
            if event["recordingRevision"].as_u64()
                != Some(u64::from(self.shortcuts.recording_revision))
            {
                return;
            }
            if let (Some(code), Some(modifiers), Some(down)) = (
                event["code"].as_str(),
                event["modifiers"].as_u64(),
                event["down"].as_bool(),
            ) {
                if self.shortcuts.recorder.is_focused(w) {
                    self.record_shortcut_key(code, modifiers as u8, down, w, cx);
                } else if down {
                    match code {
                        "Escape" => self.dismiss_shortcuts(w, cx),
                        "Tab" if modifiers & u64::from(shortcuts::SHIFT) != 0 => w.focus_prev(cx),
                        "Tab" => w.focus_next(cx),
                        _ => {}
                    }
                }
            }
        } else if !super::document_html::owns_focus()
            && self.shortcuts.accepted_event(event)
            && !self.terminal_title_has_focus(w, cx)
            && let Ok(command) = serde_json::from_value(event["command"].clone())
        {
            self.shortcuts.held.clear();
            self.run_application_shortcut(command, None, w, cx);
        }
    }

    pub(super) fn handle_shortcut_input(
        &mut self,
        combo: &str,
        code: &str,
        modifiers: u8,
        phase: u8,
        repeat: bool,
        composing: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if phase == 2 || composing {
            self.shortcuts.held.clear();
            self.shortcuts.consumed.clear();
            if let Some(draft) = self.shortcuts.draft.as_mut() {
                draft.held.clear();
                draft.pending = None;
                draft.blocked = false;
            }
            return false;
        }
        let down = phase == 0;
        if self.shortcuts.draft.is_some() && self.shortcuts.recorder.is_focused(w) {
            if !repeat {
                self.record_shortcut_key(code, modifiers, down, w, cx);
            }
            return true;
        }
        if !down {
            self.shortcuts.held.release(code);
            return self.shortcuts.consumed.remove(code);
        }
        if repeat && combo == "Escape" && self.show_settings {
            return true;
        }
        if self.terminal_title_has_focus(w, cx) {
            return false;
        }
        if repeat {
            if self.shortcuts.consumed.contains(code) {
                return true;
            }
        } else {
            self.shortcuts.consumed.remove(code);
            if let Some(command) = self.shortcuts.held.press(
                code,
                modifiers,
                &self.shortcuts.map,
                self.shortcuts.platform.chords(),
            ) {
                self.run_application_shortcut(command, None, w, cx);
                self.shortcuts.consumed.insert(code.into());
                return true;
            }
            let primary = if self.shortcuts.platform == Platform::Mac {
                shortcuts::META
            } else {
                shortcuts::CONTROL
            };
            if !self.shortcuts.open
                && self.focused_right_pane(w, cx).is_none_or(|pane| {
                    !matches!(
                        self.focused_dock_tab(pane, w, cx),
                        Some(RightPanelTab::Terminal(_))
                    )
                })
            {
                let step = match code {
                    "Equal" if modifiers == primary || modifiers == primary | shortcuts::SHIFT => 1,
                    "Minus" if modifiers == primary => -1,
                    _ => 0,
                };
                if step != 0 {
                    self.set_content_font_delta(self.content_font_delta + step, cx);
                    return true;
                }
            }
        }
        self.handle_fixed_key(combo, w, cx)
            || !self.shortcuts.open && self.handle_file_key(combo, w, cx)
    }

    fn record_shortcut_key(
        &mut self,
        code: &str,
        modifiers: u8,
        down: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shortcuts.busy {
            return;
        }
        let Some(draft) = self.shortcuts.draft.as_mut().filter(|draft| draft.ready) else {
            return;
        };
        let flag = shortcuts::modifier_key(code);
        if down {
            if code == "Escape" && modifiers == 0 {
                self.dismiss_shortcuts(w, cx);
                return;
            }
            if flag != 0 {
                draft.held.clear();
                draft.pending = None;
                return;
            }
            if draft.blocked || draft.held.iter().any(|key| key == code) {
                return;
            }
            draft.held.push(code.into());
            draft.error = None;
            draft.retry = None;
            let chord = if self.shortcuts.platform.chords() {
                draft.held.get(1).map(String::as_str)
            } else {
                None
            };
            draft.pending = (draft.held.len() <= 2)
                .then(|| Gesture::new(&draft.held[0], chord, modifiers))
                .flatten();
            draft.shown = draft.pending.clone();
            if draft.pending.is_none() {
                draft.blocked = true;
                draft.error = Some(
                    if draft.held.len() > 2 {
                        "最多同时按下两个非修饰键。请松开后重试。"
                    } else {
                        "暂不支持这个按键。"
                    }
                    .into(),
                );
            }
        } else {
            draft.held.retain(|key| key != code);
            let release = draft.pending.as_ref().is_some_and(|gesture| {
                gesture.code == code
                    || gesture.second.as_deref() == Some(code)
                    || gesture.modifiers & flag != 0
            });
            if draft.held.is_empty() || flag != 0 {
                draft.held.clear();
                draft.blocked = false;
            }
            if release && let Some(gesture) = draft.pending.take() {
                let edit = Edit::Set {
                    id: draft.id.into(),
                    binding: Some(gesture.to_binding()),
                };
                self.save_shortcut(edit, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn render_shortcut_setting(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        row()
            .w_full()
            .min_h(px(60.))
            .gap(px(8.))
            .py(px(16.))
            .border_b(px(0.5))
            .border_color(p.border_l2)
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .pr(px(48.))
                    .gap(px(4.))
                    .child(div().text_size(px(14.)).line_height(px(22.)).child(TITLE))
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(p.label_tertiary)
                            .child("查看和编辑当前可用的快捷键和输入操作"),
                    ),
            )
            .child(dock_action_button(
                row()
                    .id("open-shortcuts")
                    .role(gpui::Role::Button)
                    .tab_stop(true)
                    .cursor_pointer()
                    .flex_none()
                    .justify_center()
                    .h(px(36.))
                    .min_w(px(90.))
                    .px(px(14.))
                    .rounded(px(12.))
                    .bg(p.bg_module_platform)
                    .hover(|style| style.bg(p.hover))
                    .text_size(px(14.))
                    .child("编辑快捷键"),
                cx.listener(|app, _, w, cx| app.open_shortcuts(w, cx)),
            ))
    }

    pub(super) fn render_shortcut_reference(
        &self,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let viewport: f32 = w.viewport_size().height.into();
        let height = (viewport - 108.).clamp(0., 600.);
        let query = self.shortcuts.search.read(cx).value().trim().to_lowercase();
        let visible = |label: &str, aliases: &str, keys: &str| {
            query.split_whitespace().all(|word| {
                format!("{label} {aliases} {keys}")
                    .to_lowercase()
                    .contains(word)
            })
        };
        let mut list = column()
            .id("shortcut-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.shortcuts.scroll)
            .px(px(16.))
            .pb(px(18.))
            .gap_0();
        let can_edit = self.shortcuts.snapshot.revision != 0
            && self.shortcuts.snapshot.error.is_none()
            && !self.shortcuts.busy;
        let mut count = 0;
        for row in &self.shortcuts.map.rows {
            let keys = row
                .binding
                .as_ref()
                .map(|binding| binding.label(self.shortcuts.platform))
                .unwrap_or_default();
            let compact = keys.replace(' ', "");
            if !visible(
                row.definition.label,
                row.definition.aliases,
                &format!("{keys} {compact}"),
            ) {
                continue;
            }
            count += 1;
            list = list.child(self.render_shortcut_row(row.definition, &keys, can_edit, cx));
        }
        let primary = if self.shortcuts.platform == Platform::Mac {
            "⌘"
        } else {
            "Ctrl +"
        };
        for (heading, rows) in [
            (
                "",
                vec![
                    ("增大正文字号", format!("{primary} =")),
                    ("减小正文字号", format!("{primary} -")),
                ],
            ),
            (
                "消息输入",
                vec![
                    ("发送消息", "Enter".into()),
                    ("换行", "⇧ Enter".into()),
                    ("停止生成", "Esc Esc".into()),
                ],
            ),
            (
                "菜单与弹层",
                vec![
                    ("关闭菜单或顶层弹窗", "Esc".into()),
                    ("移动菜单选择", "↑ ↓".into()),
                    ("选择菜单项", "Enter".into()),
                ],
            ),
            (
                "审批区域",
                vec![("允许一次", "Enter".into()), ("拒绝", "Esc".into())],
            ),
        ] {
            let matches = rows
                .into_iter()
                .filter(|(label, keys)| visible(label, "", keys))
                .collect::<Vec<_>>();
            if !heading.is_empty() && !matches.is_empty() {
                list = list.child(
                    div()
                        .mx(px(8.))
                        .mt(px(18.))
                        .mb(px(4.))
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(p.label_tertiary)
                        .child(heading),
                );
            }
            for (label, keys) in matches {
                count += 1;
                list = list.child(
                    row()
                        .w_full()
                        .h(px(42.))
                        .px(px(8.))
                        .gap(px(8.))
                        .justify_between()
                        .text_size(px(13.))
                        .child(label)
                        .child(div().text_color(p.label_tertiary).child(keys)),
                );
            }
        }
        if count == 0 {
            list = list.child(note("没有匹配的快捷键", cx));
        }
        let modified = self
            .shortcuts
            .snapshot
            .document
            .modified_count(self.shortcuts.platform);
        let panel = column()
            .id("shortcut-reference")
            .role(gpui::Role::Dialog)
            .aria_label(TITLE)
            .relative()
            .w(px(480.))
            .h(px(height))
            .mt(px(60.))
            .rounded(px(24.))
            .overflow_hidden()
            .bg(p.bg_layer_2)
            .shadow(crate::tokens::elevation_prominent())
            .occlude()
            .gap_0()
            .child(
                row()
                    .relative()
                    .h(px(46.))
                    .flex_none()
                    .pt(px(22.))
                    .pl(px(24.))
                    .text_size(px(16.))
                    .line_height(px(24.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(TITLE)
                    .child(
                        div()
                            .absolute()
                            .top(px(20.))
                            .right(px(14.))
                            .child(dock_action_button(
                                row()
                                    .id("close-shortcuts")
                                    .role(gpui::Role::Button)
                                    .aria_label("关闭快捷键")
                                    .tab_stop(true)
                                    .size(px(28.))
                                    .justify_center()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(p.hover))
                                    .child(icon(Glyph::Close, 14., p.label_primary)),
                                cx.listener(|app, _, w, cx| app.dismiss_shortcuts(w, cx)),
                            )),
                    ),
            )
            .child(
                row()
                    .h(px(60.))
                    .px(px(20.))
                    .pt(px(14.))
                    .pb(px(10.))
                    .flex_none()
                    .child(
                        row()
                            .w_full()
                            .h(px(36.))
                            .px(px(14.))
                            .gap(px(8.))
                            .rounded(px(12.))
                            .border(px(0.5))
                            .border_color(p.border_l2)
                            .bg(p.md_code_block)
                            .text_size(px(13.))
                            .line_height(px(18.))
                            .child(icon(Glyph::Search, 14., p.label_caption))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .h(px(28.))
                                    .child(self.shortcuts.search.clone()),
                            )
                            .when(!query.is_empty(), |field| {
                                field.child(dock_action_button(
                                    row()
                                        .id("clear-shortcut-search")
                                        .role(gpui::Role::Button)
                                        .aria_label("清空搜索")
                                        .tab_stop(true)
                                        .size(px(18.))
                                        .justify_center()
                                        .cursor_pointer()
                                        .child(icon(Glyph::Close, 10., p.label_caption)),
                                    cx.listener(|app, _, w, cx| {
                                        replace(&app.shortcuts.search, "", w, cx);
                                        app.shortcuts
                                            .search
                                            .update(cx, |editor, cx| editor.focus(w, cx));
                                        cx.notify();
                                    }),
                                ))
                            }),
                    ),
            )
            .child(list)
            .child(
                row()
                    .h(px(52.))
                    .flex_none()
                    .px(px(24.))
                    .justify_between()
                    .border_t(px(0.5))
                    .border_color(p.border_l2)
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .child(dock_action_button(
                        row()
                            .id("shortcut-reset-all")
                            .role(gpui::Role::Button)
                            .gap(px(8.))
                            .tab_stop(can_edit && modified != 0)
                            .text_color(p.label_secondary)
                            .when(!can_edit || modified == 0, |button| button.opacity(0.45))
                            .when(can_edit && modified != 0, |button| button.cursor_pointer())
                            .child(icon(Glyph::Redo, 12., p.label_secondary))
                            .child(RESET_ALL),
                        cx.listener(move |app, _, _, cx| {
                            if can_edit && modified != 0 {
                                app.shortcuts.draft = None;
                                app.set_shortcut_recording(false, cx);
                                app.shortcuts.reset = true;
                                cx.notify();
                            }
                        }),
                    ))
                    .when(modified > 0, |footer| {
                        footer.child(
                            div()
                                .text_color(p.label_tertiary)
                                .child(format!("{modified} 项已自定义")),
                        )
                    }),
            );
        let mut overlay = div()
            .id("shortcuts-overlay")
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .items_center()
            .occlude()
            .child(
                div()
                    .id("shortcuts-mask")
                    .absolute()
                    .inset_0()
                    .bg(p.bg_mask_1)
                    .on_click(cx.listener(|app, _, w, cx| app.dismiss_shortcuts(w, cx))),
            )
            .child(panel);
        if self.shortcuts.reset {
            overlay = overlay.child(div().id("shortcut-confirm-mask").absolute().inset_0().flex().items_center().justify_center()
                .bg(p.bg_mask_1).occlude()
                .child(column().id("shortcut-confirm").role(gpui::Role::Dialog).w(px(400.)).p(px(24.)).gap(px(12.))
                    .rounded(px(24.)).bg(p.bg_layer_2).shadow(crate::tokens::elevation_prominent())
                    .child(div().text_size(px(16.)).font_weight(FontWeight::MEDIUM).child("恢复全部默认快捷键？"))
                    .child(note("将恢复当前平台的默认快捷键。你修改或移除的快捷键都会恢复，其他平台不受影响。", cx))
                    .child(row().w_full().justify_end().gap(px(8.))
                        .child(Button::new("cancel-shortcut-reset").label("取消").secondary()
                            .on_click(cx.listener(|app,_,w,cx| app.dismiss_shortcuts(w,cx))).into_themed(cx))
                        .child(Button::new("confirm-shortcut-reset").label("恢复默认").loading(self.shortcuts.busy)
                            .on_click(cx.listener(|app,_,_,cx| app.save_shortcut(Edit::ResetAll,cx))).into_themed(cx)))));
        }
        div().absolute().inset_0().child(overlay)
    }

    fn render_shortcut_row(
        &self,
        definition: shortcuts::Definition,
        keys: &str,
        can_edit: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let id = definition.id;
        let line = row()
            .w_full()
            .min_h(px(42.))
            .gap(px(8.))
            .p(px(8.))
            .justify_between()
            .text_size(px(13.))
            .line_height(px(18.))
            .rounded(px(12.))
            .child(div().min_w_0().child(definition.label));
        if let Some(draft) = self.shortcuts.draft.as_ref().filter(|draft| draft.id == id) {
            let label = draft
                .shown
                .as_ref()
                .filter(|_| draft.error.is_none())
                .map(|gesture| gesture.label(self.shortcuts.platform))
                .unwrap_or_else(|| "按下快捷键".into());
            let ready = draft.ready;
            let mut controls = row().gap(px(8.)).justify_end().text_size(px(12.));
            for (label, unbind) in [("恢复默认", false), ("移除", true)] {
                if unbind && keys.is_empty() {
                    continue;
                }
                controls = controls.child(dock_action_button(
                    div()
                        .id(label)
                        .role(gpui::Role::Button)
                        .tab_stop(can_edit)
                        .cursor_pointer()
                        .text_color(p.label_tertiary)
                        .child(label),
                    cx.listener(move |app, _, _, cx| {
                        if can_edit {
                            app.save_shortcut(
                                if unbind {
                                    Edit::Set {
                                        id: id.into(),
                                        binding: None,
                                    }
                                } else {
                                    Edit::Reset { id: id.into() }
                                },
                                cx,
                            );
                        }
                    }),
                ));
            }
            controls = controls.child(dock_action_button(
                row()
                    .id("shortcut-recorder")
                    .role(gpui::Role::Button)
                    .aria_label("按下快捷键")
                    .track_focus(&self.shortcuts.recorder)
                    .tab_stop(ready)
                    .justify_center()
                    .h(px(26.))
                    .min_w(px(96.))
                    .px(px(14.))
                    .rounded(px(10.))
                    .border(px(0.5))
                    .border_color(if draft.error.is_some() {
                        p.state_error_primary
                    } else {
                        p.border_l4
                    })
                    .bg(p.bg_module_platform)
                    .text_color(p.label_secondary)
                    .child(label),
                cx.listener(move |app, _, w, cx| app.begin_shortcut_recording(id, w, cx)),
            ));
            let retry = draft.retry.is_some() && draft.error.is_some();
            return line.child(
                column()
                    .items_end()
                    .gap(px(6.))
                    .child(controls)
                    .when_some(draft.error.as_ref(), |column, error| {
                        column.child(
                            div()
                                .max_w(px(260.))
                                .text_size(px(12.))
                                .text_color(p.label_tertiary)
                                .child(error.clone()),
                        )
                    })
                    .when(retry, |column| {
                        column.child(dock_action_button(
                            div()
                                .id("shortcut-retry")
                                .role(gpui::Role::Button)
                                .tab_stop(can_edit)
                                .cursor_pointer()
                                .text_size(px(12.))
                                .text_color(p.label_secondary)
                                .child("重试保存"),
                            cx.listener(|app, _, _, cx| {
                                if let Some(edit) = app
                                    .shortcuts
                                    .draft
                                    .as_mut()
                                    .and_then(|draft| draft.retry.take())
                                {
                                    app.save_shortcut(edit, cx);
                                }
                            }),
                        ))
                    }),
            );
        }
        div().w_full().child(dock_action_button(
            line.id(id)
                .role(gpui::Role::Button)
                .aria_label(format!("修改{}快捷键", definition.label))
                .tab_stop(can_edit)
                .when(can_edit, |row| {
                    row.cursor_pointer()
                        .hover(|style| style.bg(p.hover))
                        .focus_visible(|style| {
                            style.border_1().border_color(p.state_business_primary)
                        })
                })
                .child(
                    div()
                        .flex_none()
                        .px(px(if keys.is_empty() { 16. } else { 12. }))
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .rounded(px(10.))
                        .bg(p.bg_module_platform)
                        .text_size(px(12.))
                        .text_color(if keys.is_empty() {
                            p.label_tertiary
                        } else {
                            p.label_secondary
                        })
                        .child(if keys.is_empty() {
                            "暂无快捷键".to_string()
                        } else {
                            keys.to_string()
                        }),
                ),
            cx.listener(move |app, _, w, cx| app.begin_shortcut_recording(id, w, cx)),
        ))
    }
}
