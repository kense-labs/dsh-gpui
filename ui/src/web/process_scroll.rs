//! One independently scrolling, capped process body.

use super::workspace::conversation_bg;
use gpui::{
    App, Bounds, DispatchPhase, Div, FocusHandle, Hsla, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle, Window, canvas, div, fill,
    linear_color_stop, linear_gradient, point, prelude::*, px, size,
};
use std::{cell::RefCell, rc::Rc};

const FADE_HEIGHT: f32 = 24.;
const SCROLLBAR_WIDTH: f32 = 5.;
const EDGE_TOLERANCE: f32 = 1.;

#[derive(Clone)]
pub(super) struct ProcessViewport {
    scroll: ScrollHandle,
    focus: FocusHandle,
    state: Rc<RefCell<ScrollState>>,
}

#[derive(Default)]
struct ScrollState {
    floor: Option<f32>,
    following: bool,
    motion: Option<ScrollMotion>,
    drag_grab: Option<f32>,
}

struct ScrollMotion {
    started: f64,
    from: f32,
    to: f32,
}

impl ScrollState {
    fn advance(&mut self, top: f32, floor: f32, now: f64, reduced_motion: bool) -> Option<f32> {
        let previous = self.floor.replace(floor);
        if self.following && previous.is_some_and(|previous| previous != floor) {
            if reduced_motion || floor - top <= EDGE_TOLERANCE {
                self.motion = None;
                return (top != floor).then_some(floor);
            }
            self.motion = Some(ScrollMotion {
                started: now,
                from: top,
                to: floor,
            });
        }
        let motion = self.motion.as_ref()?;
        let progress = if reduced_motion {
            1.
        } else {
            ((now - motion.started) / 200.).clamp(0., 1.) as f32
        };
        let position = motion.from + (motion.to - motion.from) * (1. - (1. - progress).powi(3));
        if progress == 1. {
            self.motion = None;
        }
        Some(position)
    }
}

impl ProcessViewport {
    pub(super) fn new(live: bool, cx: &App) -> Self {
        let scroll = ScrollHandle::new();
        if live {
            scroll.scroll_to_bottom();
        }
        Self {
            scroll,
            focus: cx.focus_handle().tab_stop(true),
            state: Rc::new(RefCell::new(ScrollState {
                following: live,
                ..ScrollState::default()
            })),
        }
    }

    fn position(&self) -> (f32, f32) {
        let floor = self.scroll.max_offset().y.as_f32();
        let top = (-self.scroll.offset().y.as_f32()).clamp(0., floor);
        (top, floor)
    }

    fn user_scroll_to(&self, top: f32, floor: f32, target: f32, w: &mut Window) -> bool {
        let mut state = self.state.borrow_mut();
        state.motion = None;
        let target = target.clamp(0., floor);
        if target == top {
            return false;
        }
        self.scroll.set_offset(point(px(0.), px(-target)));
        state.following = floor - target <= EDGE_TOLERANCE;
        w.refresh();
        true
    }

    pub(super) fn body(&self, id: String, cap: f32, content: Div, cx: &App) -> Div {
        let wheel = self.clone();
        let keys = self.clone();
        let press = self.clone();
        let pane = self.clone();
        let p = crate::tokens::dsh(cx);
        let bg = conversation_bg(cx);
        let content = content
            .id("process-scroll-content")
            .w_full()
            .flex_none()
            .pr(px(SCROLLBAR_WIDTH))
            .on_scroll(move |event, w, cx| {
                let (top, floor) = wheel.position();
                let delta = event.delta.pixel_delta(w.line_height()).y.as_f32();
                if delta != 0. && wheel.user_scroll_to(top, floor, top - delta, w) {
                    cx.stop_propagation();
                }
            });
        div()
            .relative()
            .w_full()
            .flex_none()
            .child(
                div()
                    .id(gpui::ElementId::Name(id.into()))
                    .w_full()
                    .max_h(px(cap))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .track_focus(&self.focus)
                    .role(gpui::Role::ScrollView)
                    .aria_label("工作步骤")
                    .on_mouse_down(MouseButton::Left, move |_, _, _| {
                        press.state.borrow_mut().motion = None;
                    })
                    .on_key_down(move |event, w, cx| {
                        if event.keystroke.modifiers.modified() || w.default_prevented() {
                            return;
                        }
                        let (top, floor) = keys.position();
                        let line = w.line_height().as_f32();
                        let page = keys.scroll.bounds().size.height.as_f32();
                        let target = match event.keystroke.key.as_str() {
                            "up" => top - line,
                            "down" => top + line,
                            "pageup" => top - page,
                            "pagedown" => top + page,
                            "home" => 0.,
                            "end" => floor,
                            "space" if keys.focus.is_focused(w) => top + page,
                            _ => return,
                        };
                        if keys.user_scroll_to(top, floor, target, w) {
                            w.prevent_default();
                            cx.stop_propagation();
                        }
                    })
                    .child(content),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, w, cx| {
                        pane.paint(bounds, bg, p.scrollbar_l1, p.scrollbar_l1_hover, w, cx)
                    },
                )
                .absolute()
                .inset_0(),
            )
    }

    fn paint(
        &self,
        bounds: Bounds<Pixels>,
        bg: Hsla,
        thumb_color: Hsla,
        thumb_hover: Hsla,
        w: &mut Window,
        cx: &mut App,
    ) {
        let (top, floor) = self.position();
        let next = self.state.borrow_mut().advance(
            top,
            floor,
            crate::bridge::monotonic_ms(),
            cx.reduce_motion(),
        );
        if let Some(next) = next {
            self.scroll.set_offset(point(px(0.), px(-next)));
            if (next - top).abs() > 0.005 || self.state.borrow().motion.is_some() {
                w.request_animation_frame();
            }
        }

        let height = bounds.size.height.as_f32();
        if height <= 0. || floor <= EDGE_TOLERANCE {
            return;
        }
        let fade = px(FADE_HEIGHT.min(height * 0.5));
        if top > EDGE_TOLERANCE {
            paint_fade(
                w,
                Bounds {
                    size: size(bounds.size.width, fade),
                    ..bounds
                },
                bg,
                true,
            );
        }
        if floor - top > EDGE_TOLERANCE {
            paint_fade(
                w,
                Bounds {
                    origin: point(bounds.left(), bounds.bottom() - fade),
                    size: size(bounds.size.width, fade),
                },
                bg,
                false,
            );
        }

        let thumb_h = (height * height / (height + floor)).max(18.).min(height);
        let travel = height - thumb_h;
        let track = Bounds {
            origin: point(bounds.right() - px(SCROLLBAR_WIDTH), bounds.top()),
            size: size(px(SCROLLBAR_WIDTH), bounds.size.height),
        };
        let thumb = Bounds {
            origin: point(track.left(), track.top() + px(travel * top / floor)),
            size: size(track.size.width, px(thumb_h)),
        };
        let hovered = thumb.contains(&w.mouse_position());
        let color = if hovered || self.state.borrow().drag_grab.is_some() {
            thumb_hover
        } else {
            thumb_color
        };
        w.paint_quad(fill(thumb, color).corner_radii(px(SCROLLBAR_WIDTH * 0.5)));
        if travel > 0. {
            self.bind_scrollbar(track, thumb, travel, w);
        }
    }

    fn bind_scrollbar(
        &self,
        track: Bounds<Pixels>,
        thumb: Bounds<Pixels>,
        travel: f32,
        w: &mut Window,
    ) {
        let press = self.clone();
        let clipped_track = track.intersect(&w.content_mask().bounds);
        w.on_mouse_event(move |event: &MouseDownEvent, phase, w, cx| {
            if phase == DispatchPhase::Bubble
                && event.button == MouseButton::Left
                && clipped_track.contains(&event.position)
            {
                press.focus.focus(w, cx);
                let (top, floor) = press.position();
                let grab = if thumb.contains(&event.position) {
                    (event.position.y - thumb.top()).as_f32()
                } else {
                    thumb.size.height.as_f32() * 0.5
                };
                let target = (event.position.y - track.top()).as_f32() - grab;
                press.user_scroll_to(top, floor, target / travel * floor, w);
                press.state.borrow_mut().drag_grab = Some(grab);
                w.refresh();
                cx.stop_propagation();
            }
        });
        let motion = self.clone();
        let was_hovered = thumb.contains(&w.mouse_position());
        w.on_mouse_event(move |event: &MouseMoveEvent, phase, w, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let grab = motion.state.borrow().drag_grab;
            if event.pressed_button == Some(MouseButton::Left)
                && let Some(grab) = grab
            {
                let (top, floor) = motion.position();
                let target = (event.position.y - track.top()).as_f32() - grab;
                motion.user_scroll_to(top, floor, target / travel * floor, w);
                cx.stop_propagation();
            } else if was_hovered != thumb.contains(&event.position) {
                w.refresh();
            }
        });
        let release = self.clone();
        w.on_mouse_event(move |event: &MouseUpEvent, phase, w, _| {
            if phase == DispatchPhase::Bubble
                && event.button == MouseButton::Left
                && release.state.borrow_mut().drag_grab.take().is_some()
            {
                w.refresh();
            }
        });
    }
}

fn paint_fade(w: &mut Window, bounds: Bounds<Pixels>, bg: Hsla, top: bool) {
    let clear = bg.opacity(0.);
    let (start, end) = if top { (bg, clear) } else { (clear, bg) };
    w.paint_quad(fill(
        bounds,
        linear_gradient(
            180.,
            linear_color_stop(start, 0.),
            linear_color_stop(end, 1.),
        ),
    ));
}
