//! Workspace group rows, hover actions and native-path context menus.

use super::workspace::{composer_bg, menu_bg, path_name};
use super::*;
use gpui::{Transformation, svg};

impl DshApp {
    pub(super) fn workspace_name(&self, cwd: &str) -> String {
        self.workspace_aliases.get(cwd).cloned().unwrap_or_else(|| {
            if cwd.is_empty() {
                "默认工作区".to_string()
            } else {
                path_name(cwd)
            }
        })
    }

    pub(super) fn apply_workspace_preferences(&mut self, reply: Value) {
        self.workspace_aliases = reply
            .get("aliases")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(cwd, name)| {
                let name = name.as_str()?.trim();
                (!name.is_empty()).then(|| (cwd.clone(), name.to_string()))
            })
            .collect();
        self.hidden_workspaces = string_set(reply.get("hidden"));
        self.collapsed_workspaces = string_set(reply.get("collapsed"));
    }

    fn save_workspace_preferences(&self) {
        let mut hidden = self.hidden_workspaces.iter().cloned().collect::<Vec<_>>();
        let mut collapsed = self
            .collapsed_workspaces
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        hidden.sort();
        collapsed.sort();
        invoke(
            "save_workspace_preferences",
            json!({
                "preferences": {
                    "aliases": self.workspace_aliases,
                    "hidden": hidden,
                    "collapsed": collapsed,
                }
            }),
        );
    }

    pub(super) fn render_workspace_group(
        &self,
        cwd: String,
        sessions: Vec<SessionSummary>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = Paint::get(cx);
        let expanded = !self.collapsed_workspaces.contains(&cwd);
        let hovered = self.hovered_workspace.as_deref() == Some(cwd.as_str());
        let menu_open = self
            .workspace_context_menu
            .as_ref()
            .is_some_and(|(path, _)| path == &cwd);
        let revealed = hovered || menu_open;
        let active = self
            .sessions
            .iter()
            .find(|session| Some(session.id.as_str()) == self.active_session.as_deref())
            .is_some_and(|session| session.cwd == cwd);
        let label = self.workspace_name(&cwd);
        let for_hover = cwd.clone();
        let for_toggle = cwd.clone();
        let for_menu = cwd.clone();
        let for_create = cwd.clone();
        let enabled = self.connected() && !self.running && !self.creating_session;
        let folder_color = if active {
            gpui::rgb(0x80a9ea).into()
        } else {
            p.faint
        };
        let mut group = column().w_full().gap(px(2.)).child(
            row()
                .id(gpui::ElementId::Name(
                    format!("workspace-group-{cwd}").into(),
                ))
                .role(gpui::Role::Button)
                .aria_label(label.clone())
                .aria_expanded(expanded)
                .tab_index(0)
                .w_full()
                .h(px(32.))
                .flex_none()
                .px(px(8.))
                .gap(px(6.))
                .rounded(px(8.))
                .cursor_pointer()
                .when(revealed, |element| element.bg(p.hover))
                .hover(|style| style.bg(p.hover))
                .on_hover(cx.listener(move |this, inside: &bool, _, cx| {
                    let next = inside.then_some(for_hover.clone());
                    if this.hovered_workspace != next {
                        this.hovered_workspace = next;
                        cx.notify();
                    }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.workspace_context_menu = None;
                    if !this.collapsed_workspaces.remove(&for_toggle) {
                        this.collapsed_workspaces.insert(for_toggle.clone());
                    }
                    this.save_workspace_preferences();
                    cx.notify();
                }))
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        open_workspace_menu(this, &for_menu, event.position.y.into(), cx);
                    }),
                )
                .child(
                    div()
                        .size(px(16.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if revealed {
                            svg()
                                .data(include_bytes!("../../assets/workspace-triangle.svg"))
                                .size(px(14.))
                                .text_color(p.secondary)
                                .with_transformation(Transformation::default().with_rotation(
                                    gpui::radians(if expanded {
                                        std::f32::consts::FRAC_PI_2
                                    } else {
                                        0.
                                    }),
                                ))
                        } else {
                            svg()
                                .data(if expanded {
                                    include_bytes!("../../assets/workspace-folder-open.svg")
                                        .as_slice()
                                } else {
                                    include_bytes!("../../assets/workspace-folder.svg").as_slice()
                                })
                                .size(px(16.))
                                .text_color(folder_color)
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(14.))
                        .line_height(px(20.))
                        .text_color(p.ink)
                        .child(label),
                )
                .when(revealed, |element| {
                    let for_more = cwd.clone();
                    element
                        .child(
                            div()
                                .id(gpui::ElementId::Name(
                                    format!("workspace-more-{cwd}").into(),
                                ))
                                .role(gpui::Role::Button)
                                .aria_label("工作区更多操作")
                                .tab_index(0)
                                .size(px(24.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .cursor_pointer()
                                .hover(|style| style.text_color(p.ink))
                                .child(
                                    svg()
                                        .data(include_bytes!("../../assets/workspace-ellipsis.svg"))
                                        .size(px(16.))
                                        .text_color(p.secondary),
                                )
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(
                                        move |this, event: &gpui::MouseDownEvent, _, cx| {
                                            cx.stop_propagation();
                                            open_workspace_menu(
                                                this,
                                                &for_more,
                                                event.position.y.into(),
                                                cx,
                                            );
                                        },
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .id(gpui::ElementId::Name(
                                    format!("workspace-new-session-{cwd}").into(),
                                ))
                                .role(gpui::Role::Button)
                                .aria_label("在此工作区新建会话")
                                .tab_index(0)
                                .tab_stop(enabled)
                                .size(px(24.))
                                .ml(px(-4.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .when(enabled, |element| {
                                    element
                                        .cursor_pointer()
                                        .hover(|style| style.text_color(p.ink))
                                })
                                .when(!enabled, |element| element.opacity(0.4))
                                .child(
                                    svg()
                                        .data(include_bytes!("../../assets/new-chat-outline.svg"))
                                        .size(px(14.))
                                        .text_color(p.secondary),
                                )
                                .on_click(cx.listener(move |this, _, w, cx| {
                                    cx.stop_propagation();
                                    this.workspace_context_menu = None;
                                    if !this.connected() || this.running || this.creating_session {
                                        return;
                                    }
                                    replace(&this.workspace_editor, for_create.clone(), w, cx);
                                    this.collapsed_workspaces.remove(&for_create);
                                    this.start_service(w, cx);
                                })),
                        )
                }),
        );
        if expanded {
            for session in sessions {
                group = group.child(self.render_session_row(session, cx));
            }
        }
        group
    }

    pub(super) fn render_workspace_context_menu(
        &self,
        cwd: String,
        pointer_y: f32,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let for_rename = cwd.clone();
        let for_reveal = cwd.clone();
        let height = f32::from(w.viewport_size().height);
        let top = (pointer_y - 12.).clamp(52., (height - 142.).max(52.));
        column()
            .id("workspace-context-menu")
            .absolute()
            .left(px(self.sidebar_width - 10.))
            .top(px(top))
            .w(px(216.))
            .gap_0()
            .p(px(4.))
            .rounded(px(12.))
            .bg(menu_bg(cx))
            .shadow_lg()
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.workspace_context_menu = None;
                cx.notify();
            }))
            .child(workspace_menu_row(
                "rename",
                "重命名",
                include_bytes!("../../assets/workspace-edit.svg"),
                false,
                cx,
                move |this, w, cx| {
                    let name = this.workspace_name(&for_rename);
                    replace(&this.workspace_rename_editor, name, w, cx);
                    this.workspace_rename = Some(for_rename.clone());
                    this.workspace_context_menu = None;
                    this.workspace_rename_editor.update(cx, |editor, cx| {
                        editor.focus(w, cx);
                        editor.select_all(w, cx);
                    });
                    cx.notify();
                },
            ))
            .child(workspace_menu_row(
                "reveal",
                if cfg!(target_os = "macos") {
                    "在 Finder 中显示"
                } else {
                    "在文件夹中显示"
                },
                include_bytes!("../../assets/workspace-reveal.svg"),
                false,
                cx,
                move |this, _, cx| {
                    invoke(
                        "open_start_tool",
                        json!({"workspace": for_reveal, "tool": "files"}),
                    );
                    this.workspace_context_menu = None;
                    cx.notify();
                },
            ))
            .child(div().h(px(0.5)).mx(px(8.)).my(px(3.)).bg(p.line))
            .child(workspace_menu_row(
                "delete",
                "删除工作区",
                include_bytes!("../../assets/workspace-trash.svg"),
                true,
                cx,
                move |this, _, cx| {
                    this.workspace_delete = Some(cwd.clone());
                    this.workspace_context_menu = None;
                    cx.notify();
                },
            ))
    }

    pub(super) fn close_workspace_dialog(&mut self) {
        self.workspace_rename = None;
        self.workspace_delete = None;
    }

    fn workspace_rename_duplicate(&self, path: &str, name: &str) -> bool {
        self.sessions
            .iter()
            .any(|session| session.cwd != path && self.workspace_name(&session.cwd) == name)
            || (self.workspace != path && self.workspace_name(&self.workspace) == name)
            || self
                .workspace_aliases
                .keys()
                .any(|cwd| cwd != path && self.workspace_name(cwd) == name)
    }

    pub(super) fn confirm_workspace_rename(&mut self, cx: &App) {
        let Some(path) = self.workspace_rename.clone() else {
            return;
        };
        let name = self
            .workspace_rename_editor
            .read(cx)
            .value()
            .trim()
            .to_string();
        if name.is_empty()
            || self.workspace_aliases.get(&path) == Some(&name)
            || self.workspace_rename_duplicate(&path, &name)
        {
            return;
        }
        self.workspace_aliases.insert(path, name);
        self.workspace_rename = None;
        self.save_workspace_preferences();
    }

    pub(super) fn render_workspace_dialog(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = Paint::get(cx);
        let rename = self.workspace_rename.is_some();
        let path = self
            .workspace_rename
            .as_ref()
            .or(self.workspace_delete.as_ref())
            .cloned()
            .unwrap_or_default();
        let name = self.workspace_name(&path);
        let draft = self
            .workspace_rename_editor
            .read(cx)
            .value()
            .trim()
            .to_string();
        let duplicate =
            rename && !draft.is_empty() && self.workspace_rename_duplicate(&path, &draft);
        let blocked =
            draft.is_empty() || self.workspace_aliases.get(&path) == Some(&draft) || duplicate;
        let width = (f32::from(w.viewport_size().width) - 48.).clamp(0., 380.);
        div()
            .id("workspace-dialog")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000066))
            .occlude()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close_workspace_dialog();
                    cx.notify();
                }
            }))
            .child(
                column()
                    .id("workspace-dialog-panel")
                    .w(px(width))
                    .pb(px(24.))
                    .rounded(px(28.))
                    .bg(composer_bg(cx))
                    .shadow_lg()
                    .occlude()
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                    .child(super::permission_dialog::dialog_header(
                        if rename { "重命名工作区" } else { "删除工作区" },
                        "close-workspace-dialog",
                        cx,
                        |this, cx| {
                            this.close_workspace_dialog();
                            cx.notify();
                        },
                    ))
                    .child(
                        column()
                            .w_full()
                            .mt(px(20.))
                            .px(px(24.))
                            .when(rename, |element| {
                                element
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(44.))
                                            .px(px(14.))
                                            .flex()
                                            .items_center()
                                            .rounded(px(16.))
                                            .border(px(0.5))
                                            .border_color(p.line)
                                            .text_size(px(14.))
                                            .line_height(px(22.))
                                            .child(self.workspace_rename_editor.clone()),
                                    )
                                    .when(duplicate, |element| {
                                        element.child(
                                            div()
                                                .mt(px(8.))
                                                .text_size(px(12.))
                                                .line_height(px(18.))
                                                .text_color(p.danger)
                                                .child(format!("已存在名为“{draft}”的工作区。")),
                                        )
                                    })
                            })
                            .when(!rename, |element| {
                                element.child(
                                    div()
                                        .text_size(px(14.))
                                        .line_height(px(22.))
                                        .text_color(p.secondary)
                                        .child(format!(
                                            "仅从侧栏移除“{name}”分组，不会删除磁盘文件夹或会话数据。"
                                        )),
                                )
                            }),
                    )
                    .child(
                        row()
                            .w_full()
                            .mt(px(20.))
                            .px(px(24.))
                            .justify_end()
                            .gap(px(8.))
                            .child(super::permission_dialog::dialog_action(
                                "workspace-dialog-cancel",
                                "取消",
                                false,
                                false,
                                0.,
                                cx,
                                |this, _, cx| {
                                    this.close_workspace_dialog();
                                    cx.notify();
                                },
                            ))
                            .child(
                                super::permission_dialog::dialog_action(
                                    "workspace-dialog-confirm",
                                    if rename { "重命名" } else { "删除工作区" },
                                    rename,
                                    rename && blocked,
                                    0.,
                                    cx,
                                    move |this, _, cx| {
                                        if rename {
                                            this.confirm_workspace_rename(cx);
                                        } else {
                                            this.hidden_workspaces.insert(path.clone());
                                            this.workspace_delete = None;
                                            this.save_workspace_preferences();
                                        }
                                        cx.notify();
                                    },
                                )
                                .when(!rename, |element| element.text_color(p.danger)),
                            ),
                    ),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_workspace_dialog();
                cx.notify();
            }))
    }
}

fn open_workspace_menu(this: &mut DshApp, cwd: &str, pointer_y: f32, cx: &mut Context<DshApp>) {
    this.workspace_context_menu = Some((cwd.to_string(), pointer_y));
    this.show_workspace_view_options = false;
    this.show_workspace_menu = false;
    this.show_account_menu = false;
    this.composer_menu = None;
    cx.stop_propagation();
    cx.notify();
}

fn string_set(value: Option<&Value>) -> std::collections::HashSet<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn workspace_menu_row(
    id: &'static str,
    label: &'static str,
    artwork: &'static [u8],
    danger: bool,
    cx: &mut Context<DshApp>,
    action: impl Fn(&mut DshApp, &mut Window, &mut Context<DshApp>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let p = Paint::get(cx);
    let color = if danger { p.danger } else { p.ink };
    row()
        .id(gpui::ElementId::Name(format!("workspace-menu-{id}").into()))
        .role(gpui::Role::Button)
        .aria_label(label)
        .tab_index(0)
        .w_full()
        .h(px(32.))
        .flex_none()
        .px(px(8.))
        .gap(px(8.))
        .rounded(px(8.))
        .cursor_pointer()
        .text_size(px(14.))
        .line_height(px(20.))
        .text_color(color)
        .hover(|style| style.bg(p.hover))
        .child(
            svg()
                .data(artwork)
                .size(px(16.))
                .flex_none()
                .text_color(color),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
        .on_click(cx.listener(move |this, _, w, cx| action(this, w, cx)))
}
