//! Shell launcher attached to a terminal guide card.

use super::menu_surface::{menu_elevation, menu_material};
use super::*;
use crate::right_dock::FloatRect;
use gpui::{Bounds, FocusHandle, KeyDownEvent, Pixels, canvas};
use serde::Deserialize;
use std::{cell::Cell, rc::Rc};

const ROW_HEIGHT: f32 = 34.;
const PADDING: f32 = 4.;
const MARGIN: f32 = 12.;
pub(super) const MENU_RADIUS: f32 = 16.;

#[derive(Deserialize)]
pub(super) struct ShellChoice {
    name: String,
    path: String,
}

pub(super) async fn load_shells() -> Result<Vec<ShellChoice>, String> {
    let value = bridge::invoke("terminal_shells", json!({})).await?;
    serde_json::from_value(value).map_err(|error| error.to_string())
}

pub(super) fn preferred_path(shells: &[ShellChoice]) -> Option<&str> {
    let previous = bridge::preferred_terminal_shell();
    shells
        .iter()
        .find(|shell| shell.path == previous)
        .or_else(|| shells.first())
        .map(|shell| shell.path.as_str())
}

#[derive(Default)]
pub(super) struct ShellMenuState {
    anchors: HashMap<DockPaneId, ShellTrigger>,
    open: Option<ShellMenu>,
}

struct ShellTrigger {
    focus: FocusHandle,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

struct ShellMenu {
    pane: DockPaneId,
    request: String,
    rows: Vec<ShellRow>,
}

enum ShellAction {
    Launch(String),
    Retry,
}

struct ShellRow {
    label: String,
    checked: bool,
    action: Option<(FocusHandle, ShellAction)>,
}

impl ShellRow {
    fn message(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            checked: false,
            action: None,
        }
    }
}

impl DshApp {
    pub(super) fn update_terminal_shell_menu(&mut self, cx: &Context<Self>) {
        let layout = &self.sidebar.surface.layout;
        let guides = layout
            .all_panes()
            .filter(|pane| pane.active() == RightPanelTab::Guide)
            .map(|pane| pane.id);
        let menu = &mut self.terminal_shell_menu;
        menu.anchors.retain(|id, _| {
            layout
                .pane(*id)
                .is_some_and(|pane| pane.active() == RightPanelTab::Guide)
        });
        for id in guides {
            menu.anchors.entry(id).or_insert_with(|| ShellTrigger {
                focus: cx.focus_handle().tab_stop(true),
                bounds: Rc::default(),
            });
        }
        let open_pane = menu.open.as_ref().map(|open| open.pane);
        if let Some(pane) = open_pane
            && (!self.terminal_shell_menu.anchors.contains_key(&pane)
                || !self.dock_pane_on_screen(pane, cx))
        {
            self.terminal_shell_menu.open = None;
        }
    }

    pub(super) fn terminal_shell_trigger(
        &self,
        pane: DockPaneId,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = dsh(cx);
        let anchor = &self.terminal_shell_menu.anchors[&pane];
        let bounds = anchor.bounds.clone();
        let expanded = self
            .terminal_shell_menu
            .open
            .as_ref()
            .is_some_and(|menu| menu.pane == pane);
        row()
            .id("terminal-shell-trigger")
            .role(gpui::Role::Button)
            .aria_label("选择 Shell")
            .aria_expanded(expanded)
            .track_focus(&anchor.focus)
            .size(px(20.))
            .flex_none()
            .relative()
            .rounded(px(4.))
            .justify_center()
            .occlude()
            .cursor_pointer()
            .hover(|style| style.bg(p.hover))
            .focus_visible(|style| style.border_1().border_color(p.state_business_primary))
            .child(icon(Glyph::Down, 14., p.faint))
            .child(
                canvas(
                    |bounds, w, _| {
                        let visible = bounds.intersect(&w.content_mask().bounds);
                        (visible.size.width > px(0.) && visible.size.height > px(0.))
                            .then_some(bounds)
                    },
                    move |_, visible, w, _| {
                        if bounds.replace(visible) != visible {
                            w.request_animation_frame();
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .on_key_down(cx.listener(|this, event, w, cx| this.shell_menu_key(event, w, cx)))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, w, cx| {
                if bridge::is_mac() && event.modifiers().control {
                    return;
                }
                if this
                    .terminal_shell_menu
                    .open
                    .as_ref()
                    .is_some_and(|menu| menu.pane == pane)
                {
                    this.dismiss_terminal_shell_menu(true, w, cx);
                } else {
                    this.open_terminal_shell_menu(pane, w, cx);
                }
                cx.stop_propagation();
            }))
    }

    fn open_terminal_shell_menu(
        &mut self,
        pane: DockPaneId,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(focus) = self
            .terminal_shell_menu
            .anchors
            .get(&pane)
            .map(|anchor| anchor.focus.clone())
        else {
            return;
        };
        self.focus_dock_pane(pane, w, cx);
        focus.focus(w, cx);
        self.dismiss_dock_tab_menu(false, w, cx);
        let request = bridge::terminal_id();
        self.terminal_shell_menu.open = Some(ShellMenu {
            pane,
            request: request.clone(),
            rows: vec![ShellRow::message("正在读取 Shell…")],
        });
        cx.spawn_in(w, async move |this, cx| {
            let choices = load_shells().await;
            let _ = this.update_in(cx, |app, w, cx| {
                let Some(menu) = app
                    .terminal_shell_menu
                    .open
                    .as_mut()
                    .filter(|menu| menu.request == request)
                else {
                    return;
                };
                menu.rows = match choices {
                    Ok(shells) if shells.is_empty() => vec![ShellRow::message("没有可用的 Shell")],
                    Ok(shells) => {
                        let selected = preferred_path(&shells).map(str::to_owned);
                        shells
                            .into_iter()
                            .map(|shell| ShellRow {
                                label: shell.name,
                                checked: selected.as_ref() == Some(&shell.path),
                                action: Some((
                                    cx.focus_handle().tab_stop(false),
                                    ShellAction::Launch(shell.path),
                                )),
                            })
                            .collect()
                    }
                    Err(error) => vec![
                        ShellRow::message(format!(
                            "终端错误：{}",
                            error.replace(['\r', '\n'], " ")
                        )),
                        ShellRow {
                            label: "重试".into(),
                            checked: false,
                            action: Some((cx.focus_handle().tab_stop(false), ShellAction::Retry)),
                        },
                    ],
                };
                if let Some((focus, _)) = menu.rows.iter().find_map(|row| row.action.as_ref()) {
                    focus.focus(w, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn dismiss_terminal_shell_menu(
        &mut self,
        restore_focus: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.terminal_shell_menu.open.take() else {
            return false;
        };
        if restore_focus && let Some(anchor) = self.terminal_shell_menu.anchors.get(&menu.pane) {
            anchor.focus.focus(w, cx);
        }
        cx.notify();
        true
    }

    fn activate_shell_row(&mut self, index: usize, w: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.terminal_shell_menu.open.as_ref() else {
            return;
        };
        let Some((_, action)) = menu.rows.get(index).and_then(|row| row.action.as_ref()) else {
            return;
        };
        let pane = menu.pane;
        match action {
            ShellAction::Launch(path) => {
                let path = path.clone();
                bridge::remember_terminal_shell(&path);
                self.dismiss_terminal_shell_menu(false, w, cx);
                self.new_terminal(pane, Some(path), w, cx);
            }
            ShellAction::Retry => self.open_terminal_shell_menu(pane, w, cx),
        }
    }

    fn shell_menu_key(&mut self, event: &KeyDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.terminal_shell_menu.open.as_ref() else {
            return;
        };
        let stroke = &event.keystroke;
        if stroke.modifiers.alt || stroke.modifiers.control || stroke.modifiers.platform {
            return;
        }
        let enabled = menu
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| row.action.as_ref().map(|(focus, _)| (index, focus)))
            .collect::<Vec<_>>();
        let focused = enabled.iter().position(|(_, focus)| focus.is_focused(w));
        match (stroke.key.as_str(), stroke.modifiers.shift) {
            ("escape", false) | ("tab", true) => {
                self.dismiss_terminal_shell_menu(true, w, cx);
            }
            ("tab", false) if let Some(index) = focused => {
                self.activate_shell_row(enabled[index].0, w, cx);
            }
            ("down" | "up" | "home" | "end" | "tab", _) if !enabled.is_empty() => {
                let last = enabled.len() - 1;
                let next = match stroke.key.as_str() {
                    "down" => focused.map_or(0, |index| (index + 1) % enabled.len()),
                    "up" => focused.map_or(last, |index| (index + last) % enabled.len()),
                    "end" => last,
                    _ => 0,
                };
                enabled[next].1.focus(w, cx);
            }
            _ => return,
        }
        w.prevent_default();
        cx.stop_propagation();
    }

    pub(super) fn terminal_shell_menu_rect(&self, w: &Window, cx: &App) -> Option<FloatRect> {
        let menu = self.terminal_shell_menu.open.as_ref()?;
        let bounds = self
            .terminal_shell_menu
            .anchors
            .get(&menu.pane)?
            .bounds
            .get()?;
        let viewport = w.viewport_size();
        let style = super::document_fonts::interface_style(w, cx);
        let width = menu
            .rows
            .iter()
            .map(|row| {
                let text = w.text_system().shape_line(
                    row.label.clone().into(),
                    px(13.),
                    &[style.to_run(row.label.len())],
                    None,
                );
                f32::from(text.width()) + 2. * PADDING + 16. + if row.checked { 20. } else { 0. }
            })
            .fold(144., f32::max)
            .ceil()
            .min(360.)
            .min(f32::from(viewport.width) - 2. * MARGIN);
        let top = super::start_panel::RIGHT_DOCK_HEADER_HEIGHT.max(MARGIN);
        let bottom = f32::from(viewport.height) - MARGIN;
        let anchor_below = f32::from(bounds.bottom()) + PADDING;
        let anchor_above = f32::from(bounds.top()) - PADDING;
        let below = (bottom - anchor_below).max(0.);
        let above = (anchor_above - top).max(0.);
        let height = 2. * PADDING + ROW_HEIGHT * menu.rows.len() as f32;
        let place_below = height <= below || below >= above;
        let height = height.min(if place_below { below } else { above });
        (width > 0. && height > 0.).then(|| FloatRect {
            x: (f32::from(bounds.right()) - width)
                .clamp(MARGIN, f32::from(viewport.width) - MARGIN - width),
            y: if place_below {
                anchor_below
            } else {
                anchor_above - height
            },
            width,
            height,
        })
    }

    pub(super) fn render_terminal_shell_menu(
        &self,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let menu = self.terminal_shell_menu.open.as_ref()?;
        let rect = self.terminal_shell_menu_rect(w, cx)?;
        let anchor = self.terminal_shell_menu.anchors[&menu.pane].bounds.clone();
        let p = dsh(cx);
        Some(
            column()
                .id("terminal-shell-menu")
                .role(gpui::Role::Menu)
                .absolute()
                .left(px(rect.x))
                .top(px(rect.y))
                .w(px(rect.width))
                .h(px(rect.height))
                .p(px(PADDING))
                .gap_0()
                .rounded(px(MENU_RADIUS))
                .child(menu_material(p, MENU_RADIUS))
                .shadow(menu_elevation(p))
                .occlude()
                .on_mouse_down_out(
                    cx.listener(move |this, event: &gpui::MouseDownEvent, w, cx| {
                        if !anchor
                            .get()
                            .is_some_and(|bounds| bounds.contains(&event.position))
                        {
                            this.dismiss_terminal_shell_menu(false, w, cx);
                        }
                    }),
                )
                .on_key_down(cx.listener(|this, event, w, cx| this.shell_menu_key(event, w, cx)))
                .child(
                    column()
                        .id("terminal-shell-menu-rows")
                        .gap_0()
                        .min_h_0()
                        .w_full()
                        .flex_1()
                        .overflow_y_scroll()
                        .children(menu.rows.iter().enumerate().map(|(index, option)| {
                            row()
                                .id(("terminal-shell", index))
                                .role(gpui::Role::MenuItem)
                                .aria_label(option.label.clone())
                                .aria_selected(option.checked)
                                .h(px(ROW_HEIGHT))
                                .w_full()
                                .flex_none()
                                .px(px(8.))
                                .gap(px(6.))
                                .rounded(px(12.))
                                .text_size(px(13.))
                                .line_height(px(20.))
                                .text_color(p.ink)
                                .when(option.action.is_none(), |row| {
                                    row.opacity(0.4)
                                        .cursor(gpui::CursorStyle::OperationNotAllowed)
                                })
                                .when_some(option.action.as_ref(), |row, (focus, _)| {
                                    row.track_focus(focus)
                                        .cursor_pointer()
                                        .hover(|style| style.bg(p.hover))
                                        .focus_visible(|style| style.bg(p.hover))
                                        .on_click(cx.listener(move |this, _, w, cx| {
                                            this.activate_shell_row(index, w, cx)
                                        }))
                                })
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .truncate()
                                        .child(option.label.clone()),
                                )
                                .when(option.checked, |row| {
                                    row.child(icon(Glyph::Check, 14., p.ink))
                                })
                        })),
                )
                .into_any_element(),
        )
    }
}
