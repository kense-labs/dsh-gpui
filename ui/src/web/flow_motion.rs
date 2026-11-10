//! Work-detail visibility transitions on the transcript's GPUI layout clock.

use gpui::{
    App, Bounds, Div, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, ScrollHandle, Window, canvas, div, point, prelude::*, px,
};
use std::{cell::Cell, collections::HashMap, rc::Rc};

const FADE_OUT_MS: f64 = 70.;
const CLOSE_DELAY_MS: f64 = 50.;
const RESIZE_MS: f64 = 110.;
pub(super) const SETTLED_MOTION_MS: f64 = 800.;

#[derive(Clone, Copy)]
struct Sample {
    size: f32,
    opacity: f32,
}

impl Sample {
    fn rest(visible: bool) -> Self {
        let share = if visible { 1. } else { 0. };
        Self {
            size: share,
            opacity: share,
        }
    }
}

struct Transition {
    start: f64,
    from: Sample,
    batched: bool,
    covered: f32,
}

impl Transition {
    fn sample(&self, visible: bool, now: f64) -> Sample {
        let elapsed = (now - self.start).max(0.);
        let size = if self.batched {
            fold_progress(elapsed)
        } else {
            easing((elapsed - CLOSE_DELAY_MS) / RESIZE_MS)
        };
        if visible {
            Sample {
                size: self.from.size + (1. - self.from.size) * size,
                opacity: self.from.opacity + (1. - self.from.opacity) * size,
            }
        } else {
            Sample {
                size: self.from.size * (1. - size),
                opacity: self.from.opacity * (1. - (elapsed / FADE_OUT_MS).clamp(0., 1.) as f32),
            }
        }
    }
}

fn fold_progress(elapsed: f64) -> f32 {
    let linear = ((elapsed - CLOSE_DELAY_MS) / RESIZE_MS).clamp(0., 1.) as f32;
    linear * linear * (3. - 2. * linear)
}

pub(super) struct FoldMotion {
    pub(super) clock: f64,
    pub(super) progress: f32,
    pub(super) covered: f32,
    pub(super) remaining: f32,
    pub(super) request_tail: bool,
    pub(super) boxes: Vec<FoldBox>,
}

pub(super) struct FoldBox {
    pub(super) removed: f32,
    pub(super) closing: bool,
    pub(super) below_viewport: bool,
    visual: Rc<Cell<FoldVisual>>,
}

impl FoldBox {
    pub(super) fn position(&self, shift: f32, travels_forward: bool) {
        let travelling = self.visual.get().travelling || (travels_forward && self.below_viewport);
        self.visual.set(FoldVisual {
            travelling,
            shift: if travelling { 0. } else { shift },
        });
    }
}

#[derive(Clone, Copy, Default)]
struct FoldVisual {
    travelling: bool,
    shift: f32,
}

pub(super) struct FlowFrame {
    pub(super) fold: Option<FoldMotion>,
    pub(super) closed_now: bool,
}

fn easing(fraction: f64) -> f32 {
    let fraction = fraction.clamp(0., 1.);
    // CSS cubic-bezier(.4, 0, .2, 1).
    let (mut low, mut high) = (0., 1.);
    for _ in 0..14 {
        let t = (low + high) * 0.5;
        let x = 1.2 * (1. - t) * (1. - t) * t + 0.6 * (1. - t) * t * t + t * t * t;
        if x < fraction {
            low = t;
        } else {
            high = t;
        }
    }
    let t = (low + high) * 0.5;
    (t * t * (3. - 2. * t)) as f32
}

#[derive(Default)]
struct Measurement {
    natural_height: Cell<f32>,
    bottom: Cell<Option<Pixels>>,
    visual: Rc<Cell<FoldVisual>>,
}

struct Seat {
    target: bool,
    transition: Option<Transition>,
    measure: Rc<Measurement>,
    extent: Option<f32>,
    seen: bool,
}

pub(super) struct Frame {
    pub(super) show: bool,
    pub(super) closing: bool,
    sample: Sample,
    moving: bool,
    measure: Rc<Measurement>,
}

impl Frame {
    pub(super) fn wrap(self, element: impl IntoElement, gap: f32) -> Div {
        let measure = self.measure;
        let bottom = measure.clone();
        let frame = div()
            .relative()
            .flex()
            .flex_col()
            .items_start()
            .w_full()
            .flex_none()
            .mt(px(gap * self.sample.size));
        let height = px(measure.natural_height.get() * self.sample.size);
        let visual = measure.visual.clone();
        let body = div().relative().w_full().flex_none().child(element).child(
            canvas(
                move |bounds, _, _| measure.natural_height.set(bounds.size.height.as_f32()),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        );
        let frame = if self.closing {
            frame.child(FoldInk {
                content: div()
                    .w_full()
                    .flex_none()
                    .h(height)
                    .overflow_hidden()
                    .child(body),
                visual,
                opacity: self.sample.opacity,
            })
        } else if self.moving {
            frame
                .h(height)
                .overflow_hidden()
                .opacity(self.sample.opacity)
                .child(body)
        } else {
            frame.child(body)
        };
        frame.child(
            canvas(
                move |bounds, _, _| bottom.bottom.set(Some(bounds.bottom())),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0(),
        )
    }
}

/// Offset the clipping box after layout, leaving its flow height on the fold clock.
struct FoldInk {
    content: Div,
    visual: Rc<Cell<FoldVisual>>,
    opacity: f32,
}

impl IntoElement for FoldInk {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for FoldInk {
    type RequestLayoutState = <Div as Element>::RequestLayoutState;
    type PrepaintState = (<Div as Element>::PrepaintState, Bounds<Pixels>);

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        w: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.content.request_layout(id, inspector, w, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        mut bounds: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        w: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let visual = self.visual.get();
        let offset = point(px(0.), px(visual.shift));
        bounds.origin += offset;
        self.content.style().opacity = Some(if visual.travelling { 1. } else { self.opacity });
        let prepaint = w.with_element_offset(offset, |w| {
            self.content.prepaint(id, inspector, bounds, layout, w, cx)
        });
        (prepaint, bounds)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        layout: &mut Self::RequestLayoutState,
        (prepaint, bounds): &mut Self::PrepaintState,
        w: &mut Window,
        cx: &mut App,
    ) {
        self.content
            .paint(id, inspector, *bounds, layout, prepaint, w, cx);
    }
}

#[derive(Default)]
pub(super) struct FlowMotion {
    seats: HashMap<String, Seat>,
    read_delta: f32,
    moving: bool,
    closed_now: bool,
    pending_input: bool,
    batch_start: Option<f64>,
    next_batch: Option<f64>,
    batch_has_closing: bool,
    batch_covered: f32,
    batch_remaining: f32,
    batch_boxes: Vec<FoldBox>,
    animated: bool,
    reduced: bool,
    now: f64,
    viewport: Bounds<Pixels>,
}

impl FlowMotion {
    pub(super) fn begin_frame(
        &mut self,
        animated: bool,
        reduced: bool,
        now: f64,
        viewport: Bounds<Pixels>,
    ) {
        self.animated = animated;
        self.reduced = reduced;
        if !animated {
            self.batch_start = None;
        }
        self.now = now;
        self.viewport = viewport;
        self.read_delta = 0.;
        self.moving = false;
        self.next_batch = None;
        self.batch_has_closing = false;
        self.batch_covered = 0.;
        self.batch_remaining = 0.;
        self.batch_boxes.clear();
        self.closed_now = false;
        for seat in self.seats.values_mut() {
            seat.seen = false;
        }
    }

    pub(super) fn new_input(&mut self) {
        self.pending_input = true;
    }

    pub(super) fn owns_tail(&self) -> bool {
        self.pending_input || self.batch_start.is_some()
    }

    pub(super) fn cancel_tail_request(&mut self) {
        self.pending_input = false;
    }

    pub(super) fn showed(&self, key: &str) -> bool {
        self.seats
            .get(key)
            .is_some_and(|seat| seat.target || seat.transition.is_some())
    }

    pub(super) fn update(
        &mut self,
        key: &str,
        visible: bool,
        gap: f32,
        allow_motion: bool,
    ) -> Frame {
        let animated = self.animated && allow_motion;
        let now = self.now;
        let seat = self.seats.entry(key.to_owned()).or_insert_with(|| Seat {
            target: visible,
            transition: None,
            measure: Rc::default(),
            extent: None,
            seen: false,
        });
        seat.seen = true;
        self.closed_now |= !animated && !visible && (seat.target || seat.transition.is_some());
        let previous = seat
            .transition
            .as_ref()
            .map_or(Sample::rest(seat.target), |transition| {
                transition.sample(seat.target, now)
            });
        if !animated || (visible && !seat.target && seat.transition.is_some()) {
            // A reopening cancels the obsolete closing box instead of keeping it
            // on the old batch's clock.
            seat.transition = None;
        } else if seat.target != visible {
            seat.measure.visual.set(FoldVisual::default());
            let extent = seat
                .extent
                .unwrap_or((seat.measure.natural_height.get() + gap) * previous.size);
            let covered = seat.measure.bottom.get().map_or(0., |bottom| {
                (extent + (self.viewport.top() - bottom).as_f32()).clamp(0., extent)
            });
            let mut start = now;
            if !visible {
                let batch = *self.batch_start.get_or_insert(now);
                start = if !self.reduced && batch < now {
                    batch + CLOSE_DELAY_MS + RESIZE_MS
                } else {
                    batch
                };
            } else if let Some(batch) = self.batch_start
                && batch < now
            {
                start = batch + CLOSE_DELAY_MS + RESIZE_MS;
            }
            seat.transition = Some(Transition {
                start,
                from: previous,
                batched: !visible || self.batch_start.is_some(),
                covered: if visible { 0. } else { covered },
            });
        }
        seat.target = visible;
        let was_moving = seat.transition.is_some();
        if visible
            && let (Some(batch), Some(transition)) = (self.batch_start, seat.transition.as_mut())
            && !transition.batched
            && transition.start <= batch
        {
            transition.from = transition.sample(true, batch);
            transition.start = batch;
            transition.batched = true;
        }
        let done = seat
            .transition
            .as_ref()
            .is_some_and(|motion| self.reduced || now >= motion.start + CLOSE_DELAY_MS + RESIZE_MS);
        let sample = if done {
            Sample::rest(visible)
        } else {
            seat.transition
                .as_ref()
                .map_or(Sample::rest(visible), |transition| {
                    transition.sample(visible, now)
                })
        };
        let natural = seat.measure.natural_height.get() + gap;
        let extent = natural * sample.size;
        if let Some(transition) = seat.transition.as_ref() {
            if self.batch_start == Some(transition.start) {
                self.batch_has_closing |= !visible;
                self.batch_covered += transition.covered;
                self.batch_remaining += if visible { natural - extent } else { -extent };
                self.batch_boxes.push(FoldBox {
                    removed: natural * transition.from.size - extent,
                    closing: !visible,
                    below_viewport: seat.measure.bottom.get().is_some_and(|bottom| {
                        bottom - px(seat.extent.unwrap_or(extent)) >= self.viewport.top()
                    }),
                    visual: seat.measure.visual.clone(),
                });
            } else if !visible && !done {
                self.next_batch = Some(transition.start);
            }
        }
        if done {
            seat.transition = None;
        }
        let moving = seat.transition.is_some();
        if animated
            && was_moving
            && seat
                .measure
                .bottom
                .get()
                .is_some_and(|bottom| bottom <= self.viewport.top() + px(0.5))
            && let Some(previous_extent) = seat.extent
        {
            self.read_delta += extent - previous_extent;
        }
        seat.extent = Some(extent);
        self.moving |= moving;
        Frame {
            show: visible || moving,
            closing: moving && !visible,
            sample,
            moving,
            measure: seat.measure.clone(),
        }
    }

    pub(super) fn finish_frame(
        &mut self,
        scroll: &ScrollHandle,
        follows_tail: bool,
        w: &mut Window,
    ) -> FlowFrame {
        // Detailed live steps stop having flat seats once their completed turn folds into a group.
        self.closed_now |= self
            .seats
            .values()
            .any(|seat| !seat.seen && (seat.target || seat.transition.is_some()));
        self.seats.retain(|_, seat| seat.seen);
        if self.moving {
            w.request_animation_frame();
        }
        let requested_tail = std::mem::take(&mut self.pending_input);
        let cancelled_batch = self.batch_start.is_some() && !self.batch_has_closing;
        let fold = self
            .batch_start
            .filter(|_| self.batch_has_closing)
            .map(|clock| FoldMotion {
                clock,
                progress: if self.reduced {
                    1.
                } else {
                    fold_progress(self.now - clock)
                },
                covered: self.batch_covered,
                remaining: self.batch_remaining,
                request_tail: requested_tail && follows_tail,
                boxes: std::mem::take(&mut self.batch_boxes),
            });
        if fold.as_ref().is_none_or(|fold| fold.progress >= 1.) {
            self.batch_start = self.next_batch;
        }
        if fold.is_some() {
            return FlowFrame {
                fold,
                closed_now: self.closed_now,
            };
        }
        if follows_tail && (requested_tail || (self.moving && !cancelled_batch)) {
            scroll.scroll_to_bottom();
        } else if self.read_delta.abs() > 0.005 {
            let offset = scroll.offset();
            scroll.set_offset(point(offset.x, offset.y - px(self.read_delta)));
        }
        FlowFrame {
            fold,
            closed_now: self.closed_now,
        }
    }
}
