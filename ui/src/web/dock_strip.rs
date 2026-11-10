//! Scrolling and keyboard focus for the docked tab strip.

use super::{dock_controls::traverse_dock_focus, *};
use crate::right_dock::{
    DockPane,
    strip::{STRIP_FADE, StripGlide, strip_focus_index, strip_reveal_offset},
};
use gpui::{
    Bounds, ContentMask, FocusHandle, Hsla, IsZero, Pixels, ScrollEvent, ShapedLine, TextAlign,
    canvas, fill, linear_color_stop, linear_gradient, point,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const TITLE_FADE: f32 = 16.;
const TITLE_CLOSE_INSET: f32 = 14.;

#[derive(Default)]
struct StripScroll {
    mounted: bool,
    tabs: Vec<RightPanelTab>,
    active: Option<RightPanelTab>,
    focused: Option<RightPanelTab>,
    glide: Option<StripGlide>,
}

#[derive(Default)]
pub(super) struct DockStrip {
    scroll: ScrollHandle,
    motion: Rc<RefCell<StripScroll>>,
    focus: HashMap<RightPanelTab, FocusHandle>,
    hovered: Rc<Cell<Option<RightPanelTab>>>,
}

impl DockStrip {
    pub(super) fn suspend(&self) {
        let mut motion = self.motion.borrow_mut();
        motion.mounted = false;
        motion.focused = None;
        motion.glide = None;
        self.hovered.set(None);
    }

    fn sync_tabs(&mut self, tabs: &[RightPanelTab], cx: &App) {
        if self.hovered.get().is_some_and(|tab| !tabs.contains(&tab)) {
            self.hovered.set(None);
        }
        self.focus.retain(|tab, _| tabs.contains(tab));
        for &tab in tabs {
            self.focus.entry(tab).or_insert_with(|| cx.focus_handle());
        }
    }

    pub(super) fn chip_focus(&self, tab: RightPanelTab, active: bool) -> FocusHandle {
        self.focus[&tab].clone().tab_index(0).tab_stop(active)
    }

    pub(super) fn is_hovered(&self, tab: RightPanelTab) -> bool {
        self.hovered.get() == Some(tab)
    }

    pub(super) fn track_hover(
        &self,
        tab: RightPanelTab,
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        let hovered = self.hovered.clone();
        move |inside, w, _| {
            if *inside {
                hovered.set(Some(tab));
            } else if hovered.get() == Some(tab) {
                hovered.set(None);
            }
            w.refresh();
        }
    }

    pub(super) fn scroller(&self) -> gpui::Stateful<gpui::Div> {
        let scroll = self.scroll.clone();
        let motion = self.motion.clone();
        row()
            .id("right-dock-tab-list")
            .role(gpui::Role::TabList)
            .h(px(28.))
            .gap_0()
            .min_w_0()
            .overflow_hidden()
            .overflow_x_scroll()
            .restrict_scroll_to_axis()
            .track_scroll(&self.scroll)
            .on_key_down(|event: &gpui::KeyDownEvent, w, cx| {
                traverse_dock_focus(event, w, cx);
            })
            .on_scroll(move |event: &ScrollEvent, w, cx| {
                let delta = event.delta.pixel_delta(w.line_height());
                if !delta.x.is_zero() || (event.modifiers.shift && !delta.y.is_zero()) {
                    motion.borrow_mut().glide = None;
                }
                // Some mice keep a shifted vertical wheel on the y axis.
                if delta.x.is_zero() && event.modifiers.shift && !delta.y.is_zero() {
                    let x = (scroll.offset().x + delta.y).clamp(-scroll.max_offset().x, px(0.));
                    scroll.set_offset(point(x, px(0.)));
                    w.refresh();
                    cx.stop_propagation();
                }
            })
    }

    /// The overlay sits beside the scroller so its viewport never translates with the chips.
    pub(super) fn frame(
        &self,
        pane: &DockPane,
        leading_caret: bool,
        chips: gpui::Stateful<gpui::Div>,
        ground: gpui::Hsla,
    ) -> gpui::Div {
        let scroll = self.scroll.clone();
        let motion = self.motion.clone();
        let tabs = pane.tabs().to_vec();
        let active = pane.active();
        let focus = self.focus.clone();
        div().relative().min_w_0().h(px(28.)).child(chips).child(
            canvas(
                |_, _, _| {},
                move |bounds, _, w, cx| {
                    let offset = f32::from(-scroll.offset().x);
                    let maximum = f32::from(scroll.max_offset().x);
                    let width = f32::from(bounds.size.width);
                    if width <= 0. {
                        return;
                    }

                    paint_strip_fades(bounds, offset, maximum, ground, w);
                    let mut motion = motion.borrow_mut();
                    let focused = tabs
                        .iter()
                        .copied()
                        .find(|tab| focus[tab].contains_focused(w, cx));
                    let changed =
                        !motion.mounted || motion.active != Some(active) || motion.tabs != tabs;
                    let focus_changed = focused.is_some() && motion.focused != focused;
                    let now = bridge::monotonic_ms();

                    if changed || focus_changed {
                        let reveal = if changed {
                            active
                        } else {
                            focused.unwrap_or(active)
                        };
                        if let Some(index) = tabs.iter().position(|tab| *tab == reveal)
                            && let Some(chip) =
                                scroll.bounds_for_item(index * 2 + usize::from(leading_caret))
                        {
                            let target = strip_reveal_offset(
                                offset,
                                maximum,
                                width,
                                f32::from(chip.left() - bounds.left()),
                                f32::from(chip.right() - bounds.left()),
                            );
                            let animate = changed && motion.active.is_some() && !cx.reduce_motion();
                            motion.glide = (animate && target != offset)
                                .then(|| StripGlide::new(offset, target, now));
                            if !animate && target != offset {
                                scroll.set_offset(point(px(-target), px(0.)));
                                w.request_animation_frame();
                            }
                            motion.tabs.clone_from(&tabs);
                            motion.active = Some(active);
                            motion.mounted = true;
                        }
                    }
                    motion.focused = focused;
                    if let Some(glide) = motion.glide {
                        let (offset, running) = glide.sample(now);
                        let next = px(-offset.clamp(0., maximum));
                        if scroll.offset().x != next || running {
                            scroll.set_offset(point(next, px(0.)));
                            w.request_animation_frame();
                        }
                        motion.glide = running.then_some(glide);
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
    }
}

pub(super) fn dock_chip() -> gpui::Div {
    row()
        .relative()
        .flex_initial()
        .h(px(28.))
        // The official content-box widths are 80–170px before the 10px insets.
        .min_w(px(100.))
        .max_w(px(190.))
        .px(px(10.))
        .gap(px(5.))
}

pub(super) fn dock_chip_title(
    label: gpui::SharedString,
    font_size: f32,
    ink: Hsla,
    close_visible: bool,
    w: &Window,
    cx: &App,
) -> gpui::Div {
    // Titles supplied by the host can contain line breaks. The strip is a single line.
    let label = if label.contains(['\r', '\n', '\t']) {
        label.replace(['\r', '\n', '\t'], " ").into()
    } else {
        label
    };
    let mut run = super::document_fonts::interface_style(w, cx).to_run(label.len());
    run.color = ink;
    let line = Rc::new(
        w.text_system()
            .shape_line(label, px(font_size), &[run], None),
    );
    let width = line.width();
    let line_height = px(font_size * 1.4);
    let inset = if close_visible { TITLE_CLOSE_INSET } else { 0. };
    let scale = w.scale_factor();
    let columns = (TITLE_FADE * scale).round().max(1.) as u32;
    let fade_end =
        move |bounds: Bounds<Pixels>| (f32::from(bounds.right() - px(inset)) * scale).floor();
    // The official title treats differences of one CSS pixel as rounding noise.
    let fades = move |bounds: Bounds<Pixels>| {
        close_visible || width.round() > bounds.size.width.round() + px(1.)
    };

    let opaque_line = line.clone();
    let mut title = div()
        .relative()
        .flex_auto()
        .min_w_0()
        .w(width)
        .h(line_height)
        .overflow_hidden()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, w, cx| {
                    let right = if fades(bounds) {
                        px((fade_end(bounds) - columns as f32) / scale)
                    } else {
                        bounds.right()
                    };
                    paint_title_slice(&opaque_line, bounds, bounds.left(), right, w, cx);
                },
            )
            .absolute()
            .inset_0(),
        );

    // At the 100px chip floor the icon, gap and padding leave 59px for text.
    if close_visible || width > px(59.) {
        // GPUI clips glyphs with rectangles. Each device-pixel column has its own opacity,
        // so glyphs, including emoji, fade over the chip's current background.
        for column in 0..columns {
            let line = line.clone();
            title = title.child(
                div()
                    .absolute()
                    .inset_0()
                    .opacity((column as f32 + 0.5) / columns as f32)
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, w, cx| {
                                if fades(bounds) {
                                    let end = fade_end(bounds) - column as f32;
                                    let left = px((end - 1.) / scale);
                                    if left < bounds.left() + width {
                                        paint_title_slice(
                                            &line,
                                            bounds,
                                            left,
                                            px(end / scale),
                                            w,
                                            cx,
                                        );
                                    }
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    ),
            );
        }
    }
    title
}

fn paint_title_slice(
    line: &ShapedLine,
    bounds: Bounds<Pixels>,
    left: Pixels,
    right: Pixels,
    w: &mut Window,
    cx: &mut App,
) {
    if right > left {
        let mask = ContentMask {
            bounds: Bounds::from_corners(point(left, bounds.top()), point(right, bounds.bottom())),
        };
        w.with_content_mask(Some(mask), |w| {
            let _ = line.paint(
                bounds.origin,
                bounds.size.height,
                TextAlign::Left,
                None,
                w,
                cx,
            );
        });
    }
}

impl DshApp {
    pub(super) fn dock_title_font_size(&self) -> f32 {
        let body_size = self.content_font_size(14.);
        (body_size - 1.).min((body_size - 2.).max(13.))
    }

    pub(super) fn update_dock_strips(&mut self, cx: &App) {
        let layout = &self.sidebar.surface.layout;
        self.sidebar
            .tab_strips
            .retain(|id, _| layout.panes().iter().any(|pane| pane.id == *id));
        for pane in layout.panes() {
            let strip = self.sidebar.tab_strips.entry(pane.id).or_default();
            strip.sync_tabs(pane.tabs(), cx);
            if !self.right_dock_frame.visible {
                strip.suspend();
            }
        }
    }

    pub(super) fn activate_dock_chip(
        &mut self,
        pane: DockPaneId,
        tab: RightPanelTab,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let layout = &self.sidebar.surface.layout;
        if layout.focused() != pane || layout.active() != tab {
            self.activate_right_tab(pane, tab, w, cx);
            if matches!(tab, RightPanelTab::Terminal(_)) {
                return;
            }
        }
        self.focus_dock_chip(pane, tab, w, cx);
    }

    pub(super) fn focus_dock_chip(
        &self,
        pane: DockPaneId,
        tab: RightPanelTab,
        w: &mut Window,
        cx: &mut App,
    ) {
        if let Some(handle) = self
            .sidebar
            .tab_strips
            .get(&pane)
            .and_then(|strip| strip.focus.get(&tab))
        {
            handle.focus(w, cx);
        }
    }

    pub(super) fn focused_dock_tab(
        &self,
        pane: DockPaneId,
        w: &Window,
        cx: &App,
    ) -> Option<RightPanelTab> {
        let layout = self.sidebar.surface.layout.pane(pane)?;
        let focused = self.sidebar.tab_strips.get(&pane).and_then(|strip| {
            layout.tabs().iter().copied().find(|tab| {
                strip
                    .focus
                    .get(tab)
                    .is_some_and(|focus| focus.contains_focused(w, cx))
            })
        });
        Some(focused.unwrap_or_else(|| layout.active()))
    }

    pub(super) fn handle_dock_chip_key(
        &mut self,
        pane: DockPaneId,
        tab: RightPanelTab,
        event: &gpui::KeyDownEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stroke = &event.keystroke;
        let Some(strip) = self.sidebar.tab_strips.get(&pane) else {
            return;
        };
        if stroke.modifiers.modified()
            || !strip
                .focus
                .get(&tab)
                .is_some_and(|handle| handle.is_focused(w))
        {
            return;
        }
        if matches!(stroke.key.as_str(), "enter" | "space") {
            self.activate_dock_chip(pane, tab, w, cx);
        } else {
            let Some(pane) = self.sidebar.surface.layout.pane(pane) else {
                return;
            };
            let tabs = pane.tabs();
            let Some(index) = tabs.iter().position(|item| *item == tab) else {
                return;
            };
            let Some(next) = strip_focus_index(&stroke.key, index, tabs.len()) else {
                return;
            };
            strip.focus[&tabs[next]].focus(w, cx);
        }
        w.prevent_default();
        cx.stop_propagation();
    }
}

fn paint_strip_fades(
    bounds: Bounds<Pixels>,
    offset: f32,
    maximum: f32,
    ground: gpui::Hsla,
    w: &mut Window,
) {
    let start = offset > 1.;
    let end = maximum - offset > 1.;
    let available = if start && end {
        bounds.size.width / 2.
    } else {
        bounds.size.width
    };
    let width = available.min(px(STRIP_FADE));
    for (visible, x, angle) in [
        (start, bounds.left(), 90.),
        (end, bounds.right() - width, 270.),
    ] {
        if visible {
            let fade = Bounds {
                origin: point(x, bounds.top()),
                size: gpui::size(width, bounds.size.height),
            };
            w.paint_quad(fill(
                fade,
                linear_gradient(
                    angle,
                    linear_color_stop(ground, 0.),
                    linear_color_stop(ground.opacity(0.), 1.),
                ),
            ));
        }
    }
}
