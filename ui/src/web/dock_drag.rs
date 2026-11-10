//! Pointer capture and feedback for the right dock's in-window tab moves.

use super::{start_panel::RIGHT_DOCK_HEADER_HEIGHT, *};
use crate::right_dock::{
    DockDrop, DockDropResult, DockPane, DockSide, FloatRect, dock_easing, drag_threshold,
    strip_drop_slot,
};
use gpui::{
    Animation, Bounds, DispatchPhase, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    canvas, svg,
};
use std::{cell::RefCell, rc::Rc, time::Duration};

pub(super) struct DockTabGesture {
    pane: DockPaneId,
    tab: RightPanelTab,
    origin: Point<Pixels>,
    dragging: bool,
    target: Option<DockDrop>,
}

struct PaneGeometry {
    id: DockPaneId,
    bounds: Option<Bounds<Pixels>>,
    chips: Vec<Option<Bounds<Pixels>>>,
    split_allowed: bool,
}

#[derive(Clone)]
pub(super) struct DockGeometry(Rc<RefCell<Vec<PaneGeometry>>>);

impl DockGeometry {
    pub(super) fn new(panes: &[DockPane]) -> Self {
        Self(Rc::new(RefCell::new(
            panes
                .iter()
                .map(|pane| PaneGeometry {
                    id: pane.id,
                    bounds: None,
                    chips: vec![None; pane.tabs().len()],
                    split_allowed: false,
                })
                .collect(),
        )))
    }

    pub(super) fn measure_pane(&self, id: DockPaneId, split_allowed: bool) -> gpui::Canvas<()> {
        let geometry = self.clone();
        canvas(
            move |bounds, _, _| {
                if let Some(pane) = geometry
                    .0
                    .borrow_mut()
                    .iter_mut()
                    .find(|pane| pane.id == id)
                {
                    pane.bounds = Some(bounds);
                    pane.split_allowed = split_allowed;
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    pub(super) fn measure_chip(&self, id: DockPaneId, index: usize) -> gpui::Canvas<()> {
        let geometry = self.clone();
        canvas(
            move |bounds, _, _| {
                if let Some(pane) = geometry
                    .0
                    .borrow_mut()
                    .iter_mut()
                    .find(|pane| pane.id == id)
                {
                    pane.chips[index] = Some(bounds);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    pub(super) fn chip_bounds(&self, id: DockPaneId, index: usize) -> Option<Bounds<Pixels>> {
        self.0
            .borrow()
            .iter()
            .find(|pane| pane.id == id)
            .and_then(|pane| pane.chips.get(index).copied().flatten())
    }

    fn hit(
        &self,
        position: Point<Pixels>,
        layout: &crate::right_dock::RightDock,
        gesture: &DockTabGesture,
    ) -> Option<DockDrop> {
        let panes = self.0.borrow();
        let pane = panes
            .iter()
            .find(|pane| pane.bounds.is_some_and(|bounds| bounds.contains(&position)))?;
        let bounds = pane.bounds?;
        if position.y < bounds.origin.y + px(RIGHT_DOCK_HEADER_HEIGHT) {
            let tabs = layout.pane(pane.id)?.tabs();
            if pane.chips.len() != tabs.len() || pane.chips.iter().any(Option::is_none) {
                return None;
            }
            let source_index = (pane.id == gesture.pane)
                .then(|| tabs.iter().position(|tab| *tab == gesture.tab))
                .flatten();
            let slot = strip_drop_slot(
                pane.chips
                    .iter()
                    .flatten()
                    .map(|bounds| f32::from(bounds.origin.x + bounds.size.width * 0.5)),
                f32::from(position.x),
                source_index,
            )?;
            return Some(DockDrop::Strip {
                pane: pane.id,
                slot,
            });
        }
        if pane.split_allowed && layout.panes().len() == 1 {
            let side = if position.x < bounds.origin.x + bounds.size.width * 0.5 {
                DockSide::Left
            } else {
                DockSide::Right
            };
            Some(DockDrop::Split {
                pane: pane.id,
                side,
            })
        } else {
            Some(DockDrop::Center(pane.id))
        }
    }

    pub(super) fn pointer_capture(self, cx: &Context<DshApp>) -> gpui::Canvas<()> {
        let view = cx.weak_entity();
        canvas(
            |_, _, _| {},
            move |surface, _, w, _| {
                let move_view = view.clone();
                let move_geometry = self.clone();
                w.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture
                        && let Some(view) = move_view.upgrade()
                        && view.read(cx).dock_tab_gesture.is_some()
                    {
                        view.update(cx, |this, cx| this.move_dock_tab(event, &move_geometry, cx));
                    }
                });
                w.on_mouse_event(move |event: &MouseUpEvent, phase, w, cx| {
                    if phase == DispatchPhase::Capture
                        && event.button == MouseButton::Left
                        && let Some(view) = view.upgrade()
                        && view.read(cx).dock_tab_gesture.is_some()
                    {
                        view.update(cx, |this, cx| {
                            this.release_dock_tab(event.position, surface, &self, w, cx)
                        });
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}

impl DshApp {
    pub(super) fn press_dock_tab(
        &mut self,
        pane: DockPaneId,
        tab: RightPanelTab,
        origin: Point<Pixels>,
    ) {
        if self.right_panel_visible() {
            self.dock_tab_gesture = Some(DockTabGesture {
                pane,
                tab,
                origin,
                dragging: false,
                target: None,
            });
        }
    }

    fn move_dock_tab(
        &mut self,
        event: &MouseMoveEvent,
        geometry: &DockGeometry,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() {
            self.cancel_dock_tab_drag(cx);
            return;
        }
        let Some(gesture) = self.dock_tab_gesture.as_mut() else {
            return;
        };
        let delta = event.position - gesture.origin;
        let starting = !gesture.dragging;
        if starting && !drag_threshold(f32::from(delta.x), f32::from(delta.y)) {
            return;
        }
        gesture.dragging = true;
        let target = geometry.hit(event.position, &self.sidebar.surface.layout, gesture);
        if starting || gesture.target != target {
            gesture.target = target;
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn release_dock_tab(
        &mut self,
        position: Point<Pixels>,
        surface: Bounds<Pixels>,
        geometry: &DockGeometry,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(gesture) = self
            .dock_tab_gesture
            .take()
            .filter(|gesture| gesture.dragging)
        else {
            return;
        };
        let layout = &mut self.sidebar.surface.layout;
        let destination = match geometry.hit(position, layout, &gesture) {
            Some(target) => match layout.drop_tab(gesture.pane, gesture.tab, target) {
                Some(DockDropResult::Focus(pane)) => Some(pane),
                _ => None,
            },
            None if !surface.contains(&position) => layout.float_tab(
                gesture.pane,
                gesture.tab,
                FloatRect::at_drop(position.x.into(), position.y.into()),
            ),
            None => None,
        };
        if let Some(pane) = destination {
            self.activate_right_tab(pane, gesture.tab, w, cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn cancel_dock_tab_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let dragging = self
            .dock_tab_gesture
            .take()
            .is_some_and(|gesture| gesture.dragging);
        if dragging {
            cx.notify();
        }
        dragging
    }

    pub(super) fn dock_drop_hint_pane(&self) -> Option<DockPaneId> {
        match self.dock_tab_gesture.as_ref()?.target? {
            DockDrop::Center(pane) | DockDrop::Split { pane, .. } => Some(pane),
            DockDrop::Strip { .. } => None,
        }
    }

    pub(super) fn is_dragged_dock_tab(&self, pane: DockPaneId, tab: RightPanelTab) -> bool {
        self.dock_tab_gesture
            .as_ref()
            .is_some_and(|gesture| gesture.dragging && gesture.pane == pane && gesture.tab == tab)
    }

    pub(super) fn dock_tab_caret(&self, pane: DockPaneId) -> Option<usize> {
        match self.dock_tab_gesture.as_ref()?.target? {
            DockDrop::Strip { pane: target, slot } if pane == target => Some(slot),
            _ => None,
        }
    }

    pub(super) fn dock_drop_hint(&self, id: DockPaneId, cx: &App) -> Option<gpui::AnyElement> {
        let drop = self.dock_tab_gesture.as_ref()?.target?;
        let hint = match drop {
            DockDrop::Center(pane) if pane == id => {
                dock_hint_card(None, true, cx).into_any_element()
            }
            DockDrop::Split { pane, side } if pane == id => row()
                .size_full()
                .child(dock_hint_card(
                    Some(DockSide::Left),
                    side == DockSide::Left,
                    cx,
                ))
                .child(dock_hint_card(
                    Some(DockSide::Right),
                    side == DockSide::Right,
                    cx,
                ))
                .into_any_element(),
            _ => return None,
        };
        let scrim = div()
            .absolute()
            .inset_0()
            .bg(dsh(cx).bg_base.opacity(0.72))
            .child(hint);
        Some(if cx.reduce_motion() {
            scrim.into_any_element()
        } else {
            scrim
                .with_animation(
                    "dock-drop-preview",
                    Animation::new(Duration::from_millis(140))
                        .with_easing(|progress| dock_easing(f64::from(progress), 0., 0.58) as f32),
                    |scrim, progress| scrim.opacity(progress),
                )
                .into_any_element()
        })
    }
}

pub(super) fn dock_tab_separator(caret: bool, visible: bool, cx: &App) -> gpui::Div {
    let p = dsh(cx);
    row()
        .w(px(10.))
        .h(px(28.))
        .flex_none()
        .items_center()
        .justify_center()
        .children((caret || visible).then(|| {
            div()
                .w(px(if caret { 2. } else { 0.5 }))
                .h(px(if caret { 20. } else { 14. }))
                .bg(if caret { p.brand_accent } else { p.border_l4 })
        }))
}

fn dock_hint_card(side: Option<DockSide>, active: bool, cx: &App) -> gpui::Div {
    let p = dsh(cx);
    let (glyph, label) = match side {
        Some(DockSide::Left) => (LEFT_HINT, "左分栏"),
        Some(DockSide::Right) => (RIGHT_HINT, "右分栏"),
        None => (CENTER_HINT, "移到这里"),
    };
    div()
        .h_full()
        .flex_1()
        .min_w_0()
        .p(px(8.))
        .when(side == Some(DockSide::Left), |outer| outer.pr(px(4.)))
        .when(side == Some(DockSide::Right), |outer| outer.pl(px(4.)))
        .child(
            column()
                .size_full()
                .justify_center()
                .items_center()
                .gap(px(8.))
                .p(px(12.))
                .rounded(px(12.))
                .overflow_hidden()
                .border(px(1.5))
                .border_dashed()
                .border_color(if active { p.brand_accent } else { p.border_l2 })
                .when(active, |card| card.bg(p.brand_accent.opacity(0.08)))
                .text_color(if active { p.secondary } else { p.faint })
                .child(svg().data(glyph).size(px(20.)))
                .child(
                    div()
                        .max_w_full()
                        .truncate()
                        .text_size(px(13.))
                        .line_height(px(18.2))
                        .font_weight(FontWeight::MEDIUM)
                        .child(label),
                ),
        )
}

const CENTER_HINT: &[u8] = include_bytes!("../../assets/dock-drop-center.svg");
const LEFT_HINT: &[u8] = include_bytes!("../../assets/dock-drop-left.svg");
const RIGHT_HINT: &[u8] = include_bytes!("../../assets/dock-drop-right.svg");
