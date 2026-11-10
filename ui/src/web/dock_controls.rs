//! Dock action buttons and their viewport-positioned hints.

use super::*;
use crate::right_dock::{FloatRect, dock_easing};
use gpui::{Bounds, FocusHandle, HoverListenerMode, Pixels, Point, canvas};
use std::{cell::Cell, rc::Rc, time::Duration};

const HOVER_DELAY: Duration = Duration::from_millis(500);
const FADE_MS: f64 = 150.;
const HEIGHT: f32 = 26.;
pub(super) const HINT_RADIUS: f32 = 8.;

pub(super) fn traverse_dock_focus(
    event: &gpui::KeyDownEvent,
    w: &mut Window,
    cx: &mut App,
) -> bool {
    let stroke = &event.keystroke;
    let mods = stroke.modifiers;
    if stroke.key != "tab" || mods.control || mods.alt || mods.platform {
        return false;
    }
    if mods.shift {
        w.focus_prev(cx);
    } else {
        w.focus_next(cx);
    }
    w.prevent_default();
    cx.stop_propagation();
    true
}

pub(crate) fn dock_action_button(
    button: gpui::Stateful<gpui::Div>,
    action: impl Fn(&(), &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let action = Rc::new(action);
    let click = action.clone();
    button
        // GPUI already translates Space on release into a click and cancels it on blur.
        .on_click(move |_, w, cx| click(&(), w, cx))
        .capture_key_down(move |event, w, cx| {
            let stroke = &event.keystroke;
            if stroke.key == "enter" && !stroke.modifiers.modified() && !w.default_prevented() {
                // Handle Enter before Div arms its keyboard click on release.
                action(&(), w, cx);
                w.prevent_default();
                cx.stop_propagation();
            }
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum DockControl {
    Close(DockPaneId, RightPanelTab),
    Add(DockPaneId),
    Split(DockPaneId),
    Fullscreen,
    Toggle { in_strip: bool },
    Return(DockPaneId),
    BrowserRefresh(u64),
    FilesRefresh(DockPaneId),
    DocumentRefresh(u64),
    DocumentWrap(u64),
    DocumentCopy(u64),
    DocumentNavigator(DockPaneId),
}

impl DockControl {
    fn id(self) -> &'static str {
        match self {
            Self::Close(..) => "right-tab-close",
            Self::Add(_) => "right-tab-add",
            Self::Split(_) => "start-split",
            Self::Fullscreen => "start-expand",
            Self::Toggle { .. } => "toggle-start-panel",
            Self::Return(_) => "float-dock",
            Self::BrowserRefresh(_) => "browser-refresh",
            Self::FilesRefresh(_) => "files-refresh",
            Self::DocumentRefresh(_) => "document-refresh",
            Self::DocumentWrap(_) => "document-wrap",
            Self::DocumentCopy(_) => "document-code-copy",
            Self::DocumentNavigator(_) => "document-navigator",
        }
    }

    fn command(self) -> Option<crate::shortcuts::Command> {
        use crate::shortcuts::Command;
        match self {
            Self::Close(..) => Some(Command::Close),
            Self::Split(_) => Some(Command::Split),
            Self::Fullscreen => Some(Command::Fullscreen),
            Self::Toggle { .. } => Some(Command::ToggleDock),
            Self::BrowserRefresh(_) | Self::FilesRefresh(_) | Self::DocumentRefresh(_) => {
                Some(Command::Refresh)
            }
            _ => None,
        }
    }
}

#[derive(Default)]
pub(super) struct DockControls {
    anchors: RefCell<HashMap<DockControl, Rc<Anchor>>>,
}

struct Anchor {
    focus: FocusHandle,
    state: Cell<HintState>,
}

#[derive(Clone, Copy, Default)]
struct HintState {
    mounted: bool,
    enabled: bool,
    label: &'static str,
    bounds: Option<Bounds<Pixels>>,
    hovered: bool,
    focused: bool,
    pinned: bool,
    space_held: bool,
    show_at: Option<f64>,
}

impl DockControls {
    pub(super) fn begin_frame(&self) {
        for anchor in self.anchors.borrow().values() {
            let mut state = anchor.state.get();
            state.mounted = false;
            anchor.state.set(state);
        }
    }

    pub(super) fn end_frame(&self) {
        self.anchors
            .borrow_mut()
            .retain(|_, anchor| anchor.state.get().mounted);
    }

    pub(super) fn dismiss_pinned(&self, outside: Option<Point<Pixels>>) -> bool {
        let mut dismissed = false;
        for anchor in self.anchors.borrow().values() {
            let mut state = anchor.state.get();
            let on_anchor = outside.is_some_and(|p| state.bounds.is_some_and(|b| b.contains(&p)));
            if state.pinned && !on_anchor {
                state.pinned = false;
                state.show_at = None;
                anchor.state.set(state);
                dismissed = true;
            }
        }
        dismissed
    }

    pub(super) fn hints(
        &self,
        shortcuts: &super::shortcut_settings::ShortcutUi,
        w: &Window,
        cx: &App,
    ) -> Vec<DockHint> {
        let now = bridge::monotonic_ms();
        self.anchors
            .borrow()
            .iter()
            .filter_map(|(control, anchor)| {
                let state = anchor.state.get();
                let show_at = state.show_at.filter(|start| *start <= now)?;
                let bounds = state.bounds.filter(|_| state.mounted && state.enabled)?;
                let keys = control
                    .command()
                    .map(|command| shortcuts.keycaps(command))
                    .unwrap_or_default();
                let opacity = if cx.reduce_motion() {
                    1.
                } else {
                    dock_easing(((now - show_at) / FADE_MS).clamp(0., 1.), 0.4, 0.2) as f32
                };
                Some(DockHint {
                    control: *control,
                    bounds: control_hint_rect(bounds, state.label, &keys, *control, w, cx),
                    label: state.label,
                    keys,
                    opacity,
                    pinned: state.pinned,
                })
            })
            .collect()
    }
}

pub(super) struct DockHint {
    control: DockControl,
    pub(super) pinned: bool,
    pub(super) bounds: FloatRect,
    pub(super) opacity: f32,
    label: &'static str,
    keys: Vec<String>,
}

impl DockHint {
    pub(super) fn render(self) -> gpui::Div {
        let rect = self.bounds;
        row()
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.width))
            .h(px(rect.height))
            .pl(px(7.))
            .pr(px(if self.keys.is_empty() { 7. } else { 5. }))
            .gap(px(8.))
            .opacity(self.opacity)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .child(self.label),
            )
            .when(!self.keys.is_empty(), |hint| {
                hint.child(
                    row()
                        .flex_none()
                        .gap(px(2.))
                        .children(self.keys.iter().map(|key| {
                            row()
                                .h(px(16.))
                                .min_w(px(16.))
                                .px(px(2.))
                                .rounded(px(4.))
                                .justify_center()
                                .bg(gpui::white().opacity(0.18))
                                .text_size(px(11.))
                                .line_height(px(14.))
                                .when(key == "⌥", |cap| {
                                    cap.child(
                                        gpui::svg()
                                            .data(include_bytes!(
                                                "../../assets/dock-option-key.svg"
                                            ))
                                            .size(px(11.))
                                            .text_color(gpui::white()),
                                    )
                                })
                                .when(key != "⌥", |cap| cap.child(key.clone()))
                        })),
                )
            })
    }
}

fn text_width(label: &str, size: f32, w: &Window, style: &gpui::TextStyle) -> f32 {
    w.text_system()
        .shape_line(
            label.to_owned().into(),
            px(size),
            &[style.to_run(label.len())],
            None,
        )
        .width()
        .into()
}

fn control_hint_rect(
    anchor: Bounds<Pixels>,
    label: &str,
    keys: &[String],
    control: DockControl,
    w: &Window,
    cx: &App,
) -> FloatRect {
    let viewport = w.viewport_size();
    let style = super::document_fonts::interface_style(w, cx);
    let width = 7.
        + text_width(label, 13., w, &style)
        + if keys.is_empty() {
            7.
        } else {
            let keys_width: f32 = keys
                .iter()
                .map(|key| (text_width(key, 11., w, &style) + 4.).max(16.))
                .sum();
            8. + keys_width + (keys.len() - 1) as f32 * 2. + 5.
        };
    hint_rect(
        anchor,
        width,
        viewport.width.into(),
        viewport.height.into(),
        matches!(control, DockControl::DocumentCopy(_)),
    )
}

fn hint_rect(
    anchor: Bounds<Pixels>,
    width: f32,
    viewport_width: f32,
    viewport_height: f32,
    above: bool,
) -> FloatRect {
    let width = width
        .min(viewport_width * 0.5)
        .min((viewport_width - 24.).max(0.));
    let height = HEIGHT.min((viewport_height - 24.).max(0.));
    let clamp_x = |x: f32| x.clamp(12., (viewport_width - 12. - width).max(12.));
    let clamp_y = |y: f32| y.clamp(12., (viewport_height - 12. - height).max(12.));
    let x = clamp_x(f32::from(anchor.center().x) - width * 0.5);
    let bottom = f32::from(anchor.bottom()) + 8.;
    let top = f32::from(anchor.top()) - 8. - height;
    let right = f32::from(anchor.right()) + 10.;
    let left = f32::from(anchor.left()) - 10. - width;
    let side_y = clamp_y(f32::from(anchor.center().y) - height * 0.5);
    let (x, y) = if above && top >= 12. {
        (x, top)
    } else if bottom + height <= viewport_height - 12. {
        (x, bottom)
    } else if top >= 12. {
        (x, top)
    } else if right + width <= viewport_width - 12. {
        (right, side_y)
    } else if left >= 12. {
        (left, side_y)
    } else {
        (x, clamp_y(bottom))
    };
    FloatRect {
        x,
        y,
        width,
        height,
    }
}

impl DshApp {
    pub(super) fn dock_button(
        &self,
        control: DockControl,
        label: &'static str,
        blocked: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let code_copy = matches!(control, DockControl::DocumentCopy(_));
        let hint_enabled = match control {
            DockControl::Close(pane, _) if !pane.is_floating() => {
                self.right_panel_visible() && !self.dock_tab_menus.is_open()
            }
            DockControl::Add(_)
            | DockControl::Split(_)
            | DockControl::Fullscreen
            | DockControl::Toggle { in_strip: true } => self.right_panel_visible(),
            _ => true,
        };
        let anchor = self
            .dock_controls
            .anchors
            .borrow_mut()
            .entry(control)
            .or_insert_with(|| {
                Rc::new(Anchor {
                    focus: cx.focus_handle().tab_index(0).tab_stop(true),
                    state: Cell::default(),
                })
            })
            .clone();
        let mut state = anchor.state.get();
        if !hint_enabled || state.label != label {
            state = HintState {
                focused: state.focused,
                hovered: state.hovered,
                show_at: (code_copy && hint_enabled)
                    .then_some(state.show_at)
                    .flatten(),
                ..HintState::default()
            };
        }
        state.mounted = true;
        state.enabled = hint_enabled;
        state.label = label;
        anchor.state.set(state);

        let measured = anchor.clone();
        let hovered = anchor.clone();
        let clicked = anchor.clone();
        let released = anchor.clone();
        let keyboard = anchor.clone();
        row()
            .id(control.id())
            .role(gpui::Role::Button)
            .aria_label(label)
            .when_some(
                control
                    .command()
                    .and_then(|command| self.shortcuts.aria(command)),
                |button, shortcut| button.aria_keyshortcuts(shortcut),
            )
            .track_focus(&anchor.focus)
            .relative()
            .size(px(28.))
            .flex_none()
            .justify_center()
            .rounded(px(8.))
            .when(
                matches!(
                    control,
                    DockControl::BrowserRefresh(_)
                        | DockControl::FilesRefresh(_)
                        | DockControl::DocumentRefresh(_)
                ),
                |button| {
                    button.child(super::browser::browser_focus_ring(
                        anchor.focus.clone(),
                        p.state_business_primary,
                    ))
                },
            )
            .when(
                !matches!(
                    control,
                    DockControl::BrowserRefresh(_)
                        | DockControl::FilesRefresh(_)
                        | DockControl::DocumentRefresh(_)
                ),
                |button| {
                    button.focus_visible(|style| {
                        style.shadow(vec![
                            gpui::BoxShadow::new(px(0.), px(0.), p.brand_accent)
                                .spread_radius(px(1.))
                                .inset(),
                        ])
                    })
                },
            )
            .when(!blocked, |button| {
                button.cursor_pointer().hover(|style| style.bg(p.hover))
            })
            .child(
                canvas(
                    move |bounds, w, cx| {
                        let mut state = measured.state.get();
                        let clipped = bounds.intersect(&w.content_mask().bounds);
                        let bounds = (clipped.size.width > px(0.) && clipped.size.height > px(0.))
                            .then_some(bounds);
                        let mut refresh = state.show_at.is_some() && state.bounds != bounds;
                        state.bounds = bounds;
                        let focused = measured.focus.is_focused(w);
                        if focused != state.focused {
                            state.focused = focused;
                            state.space_held = false;
                            if state.enabled && focused && w.last_input_was_keyboard() {
                                state.show_at = Some(bridge::monotonic_ms());
                                refresh = true;
                            } else if !focused && !state.pinned && !state.hovered {
                                refresh |= state.show_at.take().is_some();
                            }
                        }
                        measured.state.set(state);
                        if refresh {
                            w.defer(cx, |w, _| w.refresh());
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .when(bridge::has_any_pointer_hover(), |button| {
                button
                    .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
                    .on_hover(cx.listener(move |_, inside: &bool, w, cx| {
                        let mut state = hovered.state.get();
                        let inside = *inside && !w.last_input_was_touch();
                        state.hovered = inside;
                        if !state.enabled || state.pinned {
                            hovered.state.set(state);
                            return;
                        }
                        if inside {
                            let now = bridge::monotonic_ms();
                            if code_copy {
                                state.show_at = Some(now);
                                hovered.state.set(state);
                                cx.notify();
                                return;
                            }
                            if state.show_at.is_none_or(|start| start > now) {
                                let due = now + HOVER_DELAY.as_millis() as f64;
                                state.show_at = Some(due);
                                let pending = Rc::downgrade(&hovered);
                                cx.spawn(async move |this, cx| {
                                    cx.background_executor().timer(HOVER_DELAY).await;
                                    if pending
                                        .upgrade()
                                        .is_some_and(|a| a.state.get().show_at == Some(due))
                                    {
                                        let _ = this.update(cx, |_, cx| cx.notify());
                                    }
                                })
                                .detach();
                            }
                        } else if state.show_at.take().is_some() {
                            cx.notify();
                        }
                        hovered.state.set(state);
                    }))
            })
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, w, cx| {
                cx.stop_propagation();
                if !(bridge::is_mac() && event.modifiers().control) {
                    this.activate_dock_control(control, blocked, &clicked, w, cx);
                }
            }))
            .capture_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, w, cx| {
                if w.default_prevented() {
                    return;
                }
                if traverse_dock_focus(event, w, cx) {
                    return;
                }
                let stroke = &event.keystroke;
                let mods = stroke.modifiers;
                match stroke.key.as_str() {
                    "enter" if !blocked && !mods.modified() => {
                        if !event.is_held
                            || matches!(
                                control,
                                DockControl::BrowserRefresh(_)
                                    | DockControl::FilesRefresh(_)
                                    | DockControl::DocumentRefresh(_)
                            )
                        {
                            this.activate_dock_control(control, false, &keyboard, w, cx);
                        }
                    }
                    "space" if !blocked && !mods.modified() => {
                        let mut state = keyboard.state.get();
                        state.space_held = true;
                        keyboard.state.set(state);
                    }
                    _ => return,
                }
                w.prevent_default();
                cx.stop_propagation();
            }))
            .on_key_up(cx.listener(move |this, event: &gpui::KeyUpEvent, w, cx| {
                let mut state = released.state.get();
                if event.keystroke.key == "space" && state.space_held {
                    state.space_held = false;
                    released.state.set(state);
                    this.activate_dock_control(control, false, &released, w, cx);
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }))
    }

    fn activate_dock_control(
        &mut self,
        control: DockControl,
        blocked: bool,
        anchor: &Anchor,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut state = anchor.state.get();
        state.pinned = blocked && state.enabled && !state.pinned;
        if !matches!(control, DockControl::DocumentCopy(_)) {
            state.show_at = state.pinned.then(bridge::monotonic_ms);
        }
        anchor.state.set(state);
        cx.notify();
        if blocked {
            return;
        }
        match control {
            DockControl::Close(pane, tab) => self.close_right_tab(pane, tab, w, cx),
            DockControl::Add(pane) => self.open_right_tab(pane, RightPanelTab::Guide, w, cx),
            DockControl::Split(pane) => self.split_right_panel(pane, w, cx),
            DockControl::Fullscreen => self.toggle_right_fullscreen(w, cx),
            DockControl::Toggle { .. } => self.toggle_right_panel(w, cx),
            DockControl::Return(pane) => self.return_float(pane, w, cx),
            DockControl::BrowserRefresh(id) => self.browser_reload(id, w, cx),
            DockControl::FilesRefresh(pane) => self.refresh_dock_files(pane, cx),
            DockControl::DocumentRefresh(key) => self.reload_document(key, cx),
            DockControl::DocumentWrap(key) => self.toggle_document_wrap(key, cx),
            DockControl::DocumentCopy(key) => self.copy_document_code(key, cx),
            DockControl::DocumentNavigator(pane) => self.toggle_document_navigator(pane, cx),
        };
    }

    pub(super) fn render_dock_hints(
        &self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::Stateful<gpui::Div>> {
        let p = dsh(cx);
        self.dock_controls
            .hints(&self.shortcuts, w, cx)
            .into_iter()
            .map(|hint| {
                if hint.opacity < 1. {
                    w.request_animation_frame();
                }
                let id =
                    gpui::ElementId::Name(format!("dock-control-hint-{:?}", hint.control).into());
                let pinned = hint.pinned;
                hint.render()
                    .id(id)
                    .role(gpui::Role::Tooltip)
                    .when(pinned, |hint| {
                        hint.occlude().on_mouse_down_out(cx.listener(
                            |this, event: &gpui::MouseDownEvent, _, cx| {
                                if this.dock_controls.dismiss_pinned(Some(event.position)) {
                                    cx.notify();
                                }
                            },
                        ))
                    })
                    .rounded(px(HINT_RADIUS))
                    .bg(p.tooltip_bg)
                    .text_color(gpui::white())
            })
            .collect()
    }
}
