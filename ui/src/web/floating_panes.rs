//! Independent, viewport-positioned windows for tabs pulled out of the dock.

use super::{
    dock_controls::{DockControl, traverse_dock_focus},
    dock_strip::{dock_chip, dock_chip_title},
    start_panel::{RIGHT_DOCK_HEADER_HEIGHT, dock_sidebar_glyph, dock_tab_icon},
    *,
};
use crate::right_dock::{FloatPane, FloatRect, FloatViewport, SpringAxis, drag_threshold};
use gpui::{
    CursorStyle, DispatchPhase, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, canvas,
    svg,
};
use std::collections::VecDeque;

pub(super) const FLOAT_RADIUS: f32 = 28.;

pub(super) struct FloatGrip {
    hovered: bool,
    handle_hovered: bool,
    fade: DockSlide,
    opacity: f32,
}

impl Default for FloatGrip {
    fn default() -> Self {
        Self {
            hovered: false,
            handle_hovered: false,
            fade: DockSlide::with_timing(120., (0., 0.58)),
            opacity: 0.,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FloatDrag {
    Move,
    Resize,
}

pub(super) struct FloatGesture {
    pane: DockPaneId,
    mode: FloatDrag,
    origin: Point<Pixels>,
    initial: FloatRect,
    pressed: FloatRect,
    drawn: FloatRect,
    moved: bool,
    samples: VecDeque<(f64, FloatRect)>,
}

impl FloatGesture {
    fn reached(
        &self,
        position: Point<Pixels>,
        area: FloatViewport,
        reduce_motion: bool,
    ) -> (FloatRect, FloatRect) {
        let delta = position - self.origin;
        let dx = f32::from(delta.x);
        let dy = f32::from(delta.y);
        let initial = self.initial;
        let (drawn, legal) = match self.mode {
            FloatDrag::Move => {
                let desired = FloatRect {
                    x: initial.x + dx,
                    y: initial.y + dy,
                    ..initial
                };
                (area.resisted(desired), area.project(desired))
            }
            FloatDrag::Resize => {
                let resized = area.bounded_resize(FloatRect {
                    width: initial.width + dx,
                    height: initial.height + dy,
                    ..initial
                });
                (resized, area.project(resized))
            }
        };
        (if reduce_motion { legal } else { drawn }, legal)
    }

    fn sample(&mut self, now: f64, drawn: FloatRect) {
        self.drawn = drawn;
        self.samples.push_back((now, drawn));
        while self
            .samples
            .front()
            .is_some_and(|(time, _)| now - time > 100.)
        {
            self.samples.pop_front();
        }
    }

    fn velocity(&self) -> (f32, f32) {
        let Some(((start, from), (end, to))) = self.samples.front().zip(self.samples.back()) else {
            return (0., 0.);
        };
        if start == end {
            return (0., 0.);
        }
        let scale = (1000. / (end - start)) as f32;
        ((to.x - from.x) * scale, (to.y - from.y) * scale)
    }
}

pub(super) struct FloatReturn {
    pane: DockPaneId,
    area: FloatViewport,
    target: FloatRect,
    x: SpringAxis,
    y: SpringAxis,
    started: f64,
    last: f64,
}

impl FloatReturn {
    fn rect(&self) -> FloatRect {
        FloatRect {
            x: self.x.position,
            y: self.y.position,
            ..self.target
        }
    }
}

impl DshApp {
    pub(super) fn floats_on_screen(&self, _cx: &App) -> bool {
        self.active_session.is_some()
            && self.page == WorkspacePage::Conversation
            && !self.show_settings
            && !self.shortcuts.open
            && !self.show_welcome
    }

    pub(super) fn dock_pane_on_screen(&self, pane: DockPaneId, cx: &App) -> bool {
        self.floats_on_screen(cx) && (pane.is_floating() || self.right_panel_visible())
    }

    pub(super) fn update_float_panes(&mut self, w: &Window, cx: &Context<Self>) {
        let layout = &self.sidebar.surface.layout;
        self.right_dock_focus
            .retain(|id, _| layout.pane(*id).is_some());
        for pane in layout.all_panes() {
            self.right_dock_focus
                .entry(pane.id)
                .or_insert_with(|| cx.focus_handle());
        }
        let shown = self.floats_on_screen(cx);
        if !shown || !w.is_window_active() {
            self.float_gesture = None;
            self.float_return = None;
            if !shown {
                self.float_grips.clear();
            }
            return;
        }
        let now = bridge::monotonic_ms();
        self.float_grips.retain(|id, _| layout.pane(*id).is_some());
        for float in layout.floats() {
            let pane = float.pane.id;
            let grip = self.float_grips.entry(pane).or_default();
            let resizing = self
                .float_gesture
                .as_ref()
                .is_some_and(|gesture| gesture.pane == pane && gesture.mode == FloatDrag::Resize);
            let (opacity, running) = grip.fade.update(
                resizing || grip.hovered || grip.handle_hovered,
                now,
                !cx.reduce_motion(),
            );
            grip.opacity = opacity;
            if running {
                w.request_animation_frame();
            }
        }
        if let Some(settling) = self.float_return.as_mut() {
            let elapsed = now - settling.last;
            settling.last = now;
            let area = float_viewport(w);
            let unchanged = area == settling.area
                && layout.floats().iter().any(|float| {
                    float.pane.id == settling.pane && area.project(float.rect) == settling.target
                });
            if unchanged && elapsed <= 250. && now - settling.started < 900. {
                let settled_x = settling.x.step(settling.target.x, elapsed);
                let settled_y = settling.y.step(settling.target.y, elapsed);
                if !(settled_x && settled_y) {
                    w.request_animation_frame();
                    return;
                }
            }
            self.float_return = None;
        }
    }

    fn float_grip_hover(
        &mut self,
        pane: DockPaneId,
        handle: bool,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let grip = self.float_grips.entry(pane).or_default();
        let state = if handle {
            &mut grip.handle_hovered
        } else {
            &mut grip.hovered
        };
        if *state != hovered {
            *state = hovered;
            cx.notify();
        }
    }

    pub(super) fn cancel_float_drag(&mut self, cx: &mut Context<Self>) -> bool {
        if self.float_gesture.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    fn float_rect(&self, float: &FloatPane, area: FloatViewport) -> FloatRect {
        let pane = float.pane.id;
        if let Some(gesture) = self
            .float_gesture
            .as_ref()
            .filter(|gesture| gesture.pane == pane)
        {
            gesture.drawn
        } else if let Some(settling) = self
            .float_return
            .as_ref()
            .filter(|settling| settling.pane == pane)
        {
            settling.rect()
        } else {
            area.project(float.rect)
        }
    }

    /// Bottom to top, with the panel under the pointer above the committed stack.
    pub(super) fn float_layers(&self, w: &Window) -> Vec<(&FloatPane, FloatRect)> {
        let area = float_viewport(w);
        let floats = self.sidebar.surface.layout.floats();
        let lifted = self
            .float_gesture
            .as_ref()
            .map(|gesture| gesture.pane)
            .or_else(|| self.float_return.as_ref().map(|settling| settling.pane));
        floats
            .iter()
            .filter(|float| Some(float.pane.id) != lifted)
            .chain(floats.iter().filter(|float| Some(float.pane.id) == lifted))
            .map(|float| (float, self.float_rect(float, area)))
            .collect()
    }

    fn start_float_drag(
        &mut self,
        pane: DockPaneId,
        mode: FloatDrag,
        origin: Point<Pixels>,
        w: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(float) = self
            .sidebar
            .surface
            .layout
            .floats()
            .iter()
            .find(|float| float.pane.id == pane)
        else {
            return;
        };
        let area = float_viewport(w);
        let shown = self.float_rect(float, area);
        self.float_return = None;
        let initial = if mode == FloatDrag::Move && !cx.reduce_motion() {
            area.unresisted(shown)
        } else {
            shown
        };
        let mut gesture = FloatGesture {
            pane,
            mode,
            origin,
            initial,
            pressed: area.project(shown),
            drawn: shown,
            moved: false,
            samples: VecDeque::new(),
        };
        gesture.sample(bridge::monotonic_ms(), shown);
        self.float_gesture = Some(gesture);
        cx.notify();
    }

    fn update_float_drag(&mut self, event: &MouseMoveEvent, w: &Window, cx: &mut Context<Self>) {
        if !event.dragging() {
            self.cancel_float_drag(cx);
            return;
        }
        if let Some(gesture) = &mut self.float_gesture {
            let delta = event.position - gesture.origin;
            gesture.moved |= drag_threshold(delta.x.into(), delta.y.into());
            let (drawn, _) = gesture.reached(event.position, float_viewport(w), cx.reduce_motion());
            gesture.sample(bridge::monotonic_ms(), drawn);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn finish_float_drag(&mut self, event: &MouseUpEvent, w: &mut Window, cx: &mut Context<Self>) {
        let Some(mut gesture) = self.float_gesture.take() else {
            return;
        };
        let pane = gesture.pane;
        let Some(tab) = self
            .sidebar
            .surface
            .layout
            .pane(pane)
            .map(|pane| pane.active())
        else {
            return;
        };
        let delta = event.position - gesture.origin;
        if gesture.mode == FloatDrag::Move
            && !gesture.moved
            && !drag_threshold(delta.x.into(), delta.y.into())
            && event.click_count == 2
            && !(bridge::is_mac() && event.modifiers.control)
            && self.begin_terminal_title(pane, tab, w, cx)
        {
            cx.stop_propagation();
            return;
        }
        let area = float_viewport(w);
        let now = bridge::monotonic_ms();
        let (drawn, legal) = gesture.reached(event.position, area, cx.reduce_motion());
        gesture.sample(now, drawn);
        if gesture.pressed != legal {
            self.sidebar.surface.layout.set_float_rect(pane, legal);
        }
        self.focus_dock_pane(pane, w, cx);
        if drawn != legal && !cx.reduce_motion() {
            let (vx, vy) = gesture.velocity();
            self.float_return = Some(FloatReturn {
                pane,
                area,
                target: legal,
                x: SpringAxis::new(drawn.x, legal.x, vx),
                y: SpringAxis::new(drawn.y, legal.y, vy),
                started: now,
                last: now,
            });
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn return_float(
        &mut self,
        pane: DockPaneId,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((target, tab)) = self.sidebar.surface.layout.return_float(pane) {
            self.sidebar.surface.open = true;
            self.activate_right_tab(target, tab, w, cx);
        }
    }

    pub(super) fn render_float_panes(&self, w: &Window, cx: &mut Context<Self>) -> gpui::Div {
        let mut layer = div().absolute().inset_0();
        for (float, rect) in self.float_layers(w) {
            layer = layer.child(self.render_float_pane(float, rect, w, cx));
        }
        layer.child(float_pointer_capture(cx))
    }

    fn render_float_pane(
        &self,
        float: &FloatPane,
        rect: FloatRect,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let pane = float.pane.id;
        let tab = float.pane.active();
        let font_size = self.dock_title_font_size();
        let group: gpui::SharedString = format!("right-dock-{pane:?}").into();
        let mut shadow = crate::tokens::elevation_prominent();
        shadow.insert(
            0,
            gpui::BoxShadow::new(px(0.), px(0.), p.border_l2).spread_radius(px(0.5)),
        );
        let grip = self.float_grips.get(&pane);
        let grip_opacity = grip.map_or(0., |grip| grip.opacity);
        let grip_ink = if grip.is_some_and(|grip| grip.handle_hovered) {
            p.label_tertiary
        } else {
            p.label_caption
        };
        column()
            .id(gpui::ElementId::Name(group))
            .track_focus(&self.right_dock_focus[&pane])
            .tab_stop(false)
            .when(
                matches!(
                    tab,
                    RightPanelTab::Browser(_) | RightPanelTab::Files | RightPanelTab::Document(_)
                ),
                |pane| {
                    pane.on_key_down(|event, w, cx| {
                        traverse_dock_focus(event, w, cx);
                    })
                },
            )
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.width))
            .h(px(rect.height))
            .gap_0()
            .overflow_hidden()
            .occlude()
            .rounded(px(FLOAT_RADIUS))
            .bg(p.bg_layer_2)
            .shadow(shadow)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.float_grip_hover(pane, false, *hovered, cx);
            }))
            .child(
                row()
                    .w_full()
                    .h(px(RIGHT_DOCK_HEADER_HEIGHT))
                    .pt(px(10.))
                    .pr(px(6.))
                    .gap(px(4.))
                    .flex_none()
                    .child(
                        row()
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .pl(px(10.))
                            .text_size(px(font_size))
                            .text_color(p.secondary)
                            .cursor(CursorStyle::OpenHand)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &gpui::MouseDownEvent, w, cx| {
                                    this.start_float_drag(
                                        pane,
                                        FloatDrag::Move,
                                        event.position,
                                        w,
                                        cx,
                                    );
                                }),
                            )
                            .child(
                                dock_chip()
                                    .child(dock_tab_icon(
                                        tab,
                                        &self.dock_tab_label(tab),
                                        p.secondary,
                                        cx,
                                    ))
                                    .child(
                                        if let Some(editor) =
                                            self.render_terminal_title_editor(pane, tab, cx)
                                        {
                                            div().flex_auto().min_w_0().child(editor)
                                        } else {
                                            dock_chip_title(
                                                self.dock_tab_label(tab),
                                                font_size,
                                                p.secondary,
                                                false,
                                                w,
                                                cx,
                                            )
                                        },
                                    ),
                            ),
                    )
                    .child(
                        self.dock_button(DockControl::Return(pane), "收回到侧边栏", false, cx)
                            .child(dock_sidebar_glyph().text_color(p.secondary)),
                    )
                    .when(self.sidebar.surface.layout.can_close(pane, tab), |header| {
                        header.child(
                            self.dock_button(DockControl::Close(pane, tab), "关闭", false, cx)
                                .child(
                                    svg()
                                        .data(include_bytes!("../../assets/dock-float-close.svg"))
                                        .size(px(15.))
                                        .text_color(p.secondary),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .id("float-body")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .capture_any_mouse_down(cx.listener(move |this, _, w, cx| {
                        this.focus_dock_pane(pane, w, cx);
                        cx.notify();
                    }))
                    .child(self.render_dock_body(pane, tab, rect.width, rect.height, w, cx)),
            )
            .child(
                div()
                    .id("float-resize")
                    .absolute()
                    .bottom_0()
                    .right_0()
                    .size(px(20.))
                    .cursor(CursorStyle::ResizeUpRightDownLeft)
                    .occlude()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        this.float_grip_hover(pane, true, *hovered, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, w, cx| {
                            this.start_float_drag(pane, FloatDrag::Resize, event.position, w, cx);
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom(px(4.))
                            .right(px(4.))
                            .size(px(24.))
                            .rounded_br(px(24.))
                            .border_b(px(1.5))
                            .border_r(px(1.5))
                            .border_color(grip_ink)
                            .opacity(grip_opacity),
                    ),
            )
    }
}

fn float_viewport(w: &Window) -> FloatViewport {
    let size = w.viewport_size();
    FloatViewport {
        top: 0.,
        width: f32::from(size.width),
        height: f32::from(size.height),
    }
}

fn float_pointer_capture(cx: &Context<DshApp>) -> gpui::Canvas<()> {
    let view = cx.weak_entity();
    canvas(
        |_, _, _| {},
        move |_, _, w, _| {
            let move_view = view.clone();
            w.on_mouse_event(move |event: &MouseMoveEvent, phase, w, cx| {
                if phase == DispatchPhase::Capture
                    && let Some(view) = move_view.upgrade()
                    && view.read(cx).float_gesture.is_some()
                {
                    view.update(cx, |this, cx| this.update_float_drag(event, w, cx));
                }
            });
            w.on_mouse_event(move |event: &MouseUpEvent, phase, w, cx| {
                if phase == DispatchPhase::Capture
                    && event.button == MouseButton::Left
                    && let Some(view) = view.upgrade()
                    && view.read(cx).float_gesture.is_some()
                {
                    view.update(cx, |this, cx| this.finish_float_drag(event, w, cx));
                }
            });
        },
    )
    .absolute()
    .inset_0()
}
