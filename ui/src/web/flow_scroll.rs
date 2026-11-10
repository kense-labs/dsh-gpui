//! Transcript layout and scroll anchoring on the same GPUI prepaint pass.

use super::WorkDetailsMode;
use super::flow_motion::FlowFrame;
use crate::history::fold_scroll::{FoldGeometry, FoldLanding};
use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, Hitbox, InspectorElementId,
    InteractiveElement, Interactivity, IntoElement, LayoutId, Pixels, ScrollHandle, Stateful,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, point, px,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

#[derive(Default, Clone)]
pub(super) struct FlowRows(Rc<RefCell<Vec<(String, LayoutId)>>>);

impl FlowRows {
    pub(super) fn row(&self, key: String, content: impl IntoElement) -> impl IntoElement {
        MeasuredRow {
            key: Some(key),
            content: content.into_any_element(),
            rows: self.clone(),
        }
    }
}

// Use the child's own layout node so measuring an anchor does not add a box or change its margins.
struct MeasuredRow {
    key: Option<String>,
    content: AnyElement,
    rows: FlowRows,
}

impl IntoElement for MeasuredRow {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for MeasuredRow {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        w: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let id = self.content.request_layout(w, cx);
        if let Some(key) = self.key.take() {
            self.rows.0.borrow_mut().push((key, id));
        }
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        w: &mut Window,
        cx: &mut App,
    ) {
        self.content.prepaint(w, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        w: &mut Window,
        cx: &mut App,
    ) {
        self.content.paint(w, cx);
    }
}

struct RowBounds {
    key: String,
    top: f32,
    bottom: f32,
}

struct Geometry {
    content: f32,
    height: f32,
    rows: Vec<RowBounds>,
}

impl Geometry {
    fn anchor_after_shrink(&self, next: &Self, top: f32) -> Option<f32> {
        let visible_end = top + self.height;
        let first = self
            .rows
            .iter()
            .position(|row| row.bottom > top && row.top < visible_end)?;
        let next_tops: HashMap<_, _> = next.rows.iter().map(|row| (&row.key, row.top)).collect();
        let visible = self.rows[first..]
            .iter()
            .take_while(|row| row.top < visible_end);
        let above = self.rows[..first]
            .iter()
            .rev()
            .take_while(|row| row.bottom > top - self.height);
        visible
            .chain(above)
            .find_map(|row| next_tops.get(&row.key).map(|y| top + y - row.top))
    }
}

#[derive(Default, Clone)]
pub(super) struct FlowScroll(Rc<RefCell<ScrollMemory>>);

#[derive(Default)]
struct ScrollMemory {
    geometry: Option<Geometry>,
    mode: Option<WorkDetailsMode>,
    room: f32,
    fold: Option<(f64, FoldLanding)>,
}

impl ScrollMemory {
    fn reconcile(
        &mut self,
        next: Geometry,
        mode: WorkDetailsMode,
        top: f32,
        follows_tail: bool,
        flow: &FlowFrame,
    ) -> Option<f32> {
        let mut landing = None;
        let deficit = |top: f32| (top + next.height - next.content).max(0.);
        if self.mode != Some(mode) {
            self.mode = Some(mode);
            self.room = 0.;
            self.fold = None;
            self.geometry = None;
        }
        if let Some(fold) = &flow.fold {
            let geometry = FoldGeometry {
                top,
                height: next.height,
                content: next.content,
                remaining: fold.remaining,
                progress: fold.progress,
            };
            let new_batch = self
                .fold
                .as_ref()
                .is_none_or(|(clock, _)| *clock != fold.clock);
            if new_batch {
                self.fold = Some((
                    fold.clock,
                    FoldLanding::new(geometry, fold.covered, follows_tail),
                ));
            }
            if let Some((_, state)) = self.fold.as_mut() {
                let (top, room) = state.sample(geometry, follows_tail, fold.request_tail);
                let displacement = state.displacement(top);
                let forward = (new_batch || fold.request_tail) && state.travels_forward();
                let mut removed_before = 0.;
                for member in &fold.boxes {
                    if member.closing {
                        member.position(displacement + removed_before, forward);
                    }
                    removed_before += member.removed;
                }
                landing = Some(top);
                self.room = room;
            }
            if fold.progress >= 1. {
                self.fold = None;
            }
        } else if self.fold.take().is_some() {
            // Reopening all closing rows releases the batch's landing immediately.
            self.room = self.room.min(deficit(top));
        } else if let Some(previous) = &self.geometry
            && flow.closed_now
            && previous.content > next.content + 0.5
        {
            let anchor = previous.anchor_after_shrink(&next, top);
            let floor = (next.content - next.height).max(0.);
            if follows_tail {
                let cap = (next.content - next.height / 2.).max(floor);
                let target = anchor.unwrap_or(cap).clamp(0., cap);
                self.room = if target > 0. { deficit(target) } else { 0. };
                landing = Some(target);
            } else if let Some(anchor) = anchor {
                landing = Some(anchor.clamp(0., floor));
                self.room = 0.;
            }
        } else {
            // New content and upward reading consume only the room they actually replace.
            self.room = self.room.min(deficit(top));
        }
        self.geometry = Some(next);
        landing
    }
}

pub(super) struct TranscriptScroll {
    interactivity: Interactivity,
    content: AnyElement,
    rows: FlowRows,
    scroll: ScrollHandle,
    memory: FlowScroll,
    mode: WorkDetailsMode,
    follows_tail: bool,
    flow: FlowFrame,
}

impl TranscriptScroll {
    pub(super) fn new(
        content: impl IntoElement,
        rows: FlowRows,
        scroll: &ScrollHandle,
        memory: FlowScroll,
        mode: WorkDetailsMode,
        follows_tail: bool,
        flow: FlowFrame,
    ) -> Stateful<Self> {
        Self {
            interactivity: Interactivity::new(),
            content: content.into_any_element(),
            rows,
            scroll: scroll.clone(),
            memory,
            mode,
            follows_tail,
            flow,
        }
        .id("conversation")
        .h_full()
        .px(px(32.))
        .overflow_y_scroll()
        .track_scroll(scroll)
    }
}

impl Styled for TranscriptScroll {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.interactivity.base_style
    }
}

impl InteractiveElement for TranscriptScroll {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl IntoElement for TranscriptScroll {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for TranscriptScroll {
    type RequestLayoutState = LayoutId;
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        self.interactivity.element_id.clone()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.interactivity.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        w: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, LayoutId) {
        let mut child = None;
        let id = self
            .interactivity
            .request_layout(id, inspector, w, cx, |style, w, cx| {
                let id = w.with_text_style(style.text_style().cloned(), |w| {
                    self.content.request_layout(w, cx)
                });
                child = Some(id);
                w.request_layout(style, [id], cx)
            });
        (id, child.unwrap())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut LayoutId,
        w: &mut Window,
        cx: &mut App,
    ) -> Option<Hitbox> {
        let mut content_size = w.layout_bounds(*child).size;
        let rows = self
            .rows
            .0
            .borrow_mut()
            .drain(..)
            .filter_map(|(key, layout)| {
                let row = w.layout_bounds(layout);
                (row.size.height > px(0.)).then(|| RowBounds {
                    key,
                    top: (row.top() - bounds.top()).as_f32(),
                    bottom: (row.bottom() - bounds.top()).as_f32(),
                })
            })
            .collect();
        let top = (-self.scroll.offset().y.as_f32()).clamp(0., self.scroll.max_offset().y.as_f32());
        let mut memory = self.memory.0.borrow_mut();
        let landing = memory.reconcile(
            Geometry {
                content: content_size.height.as_f32(),
                height: bounds.size.height.as_f32(),
                rows,
            },
            self.mode,
            top,
            self.follows_tail,
            &self.flow,
        );
        content_size.height += px(memory.room);
        drop(memory);

        self.interactivity.prepaint(
            id,
            inspector,
            bounds,
            content_size,
            w,
            cx,
            |_, offset, hitbox, w, cx| {
                // Honor GPUI's regular scrolling unless this frame owns a closing landing.
                let offset = landing.map_or(offset, |top| point(offset.x, px(-top)));
                self.scroll.set_offset(offset);
                w.with_element_offset(offset, |w| self.content.prepaint(w, cx));
                hitbox
            },
        )
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut LayoutId,
        hitbox: &mut Option<Hitbox>,
        w: &mut Window,
        cx: &mut App,
    ) {
        self.interactivity
            .paint(id, inspector, bounds, hitbox.as_ref(), w, cx, |_, w, cx| {
                self.content.paint(w, cx);
            });
    }
}
