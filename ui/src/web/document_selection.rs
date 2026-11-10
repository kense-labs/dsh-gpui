//! Selection of the text drawn by the GPUI document renderers.

use super::*;
use gpui::{
    Bounds, ClipboardItem, DispatchPhase, Element, ElementId, FocusHandle, GlobalElementId,
    InspectorElementId, LayoutId, LineLayout, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, SharedString, StyledText, TextLayout, canvas, fill, point, size,
};
use std::{
    borrow::Cow,
    ops::Range,
    rc::{Rc, Weak},
    sync::Arc,
};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy)]
enum SelectionUnit {
    Character,
    Word,
    Line,
}

struct Drag {
    seed: Range<usize>,
    unit: SelectionUnit,
    start: Point<Pixels>,
    position: Point<Pixels>,
}

#[derive(Default)]
struct SelectionState {
    endpoints: Option<(usize, usize)>,
    drag: Option<Drag>,
    moved: bool,
    layout: Weak<RefCell<Frame>>,
}

pub(super) struct DocumentSelection {
    pub(super) focus: FocusHandle,
    state: RefCell<SelectionState>,
}

impl DocumentSelection {
    pub(super) fn new(cx: &Context<DshApp>) -> Rc<Self> {
        Rc::new(Self {
            focus: cx.focus_handle(),
            state: RefCell::default(),
        })
    }

    pub(super) fn clear(&self) {
        *self.state.borrow_mut() = SelectionState::default();
    }

    pub(super) fn replace_leading_text(&self, before: usize, after: usize) {
        if before == 0 && after == 0 {
            return;
        }
        let mut state = self.state.borrow_mut();
        let Some((anchor, head)) = state.endpoints else {
            return;
        };
        // When the verbatim metadata unmounts, text in the Markdown body is still the same.
        let body_start = state.layout.upgrade().and_then(|layout| {
            let frame = layout.borrow();
            let [fallback, body, ..] = frame.spans.as_slice() else {
                return None;
            };
            (fallback.bytes.end == before).then_some(body.bytes.start)
        });
        state.endpoints = body_start
            .filter(|&start| anchor.max(head) >= start)
            .map(|start| {
                let next = if after == 0 {
                    0
                } else {
                    after + start - before
                };
                // A range endpoint in the replaced metadata moves before its new contents.
                let rebase = |index| {
                    if index < start {
                        0
                    } else {
                        index - start + next
                    }
                };
                (rebase(anchor), rebase(head))
            });
        state.drag = None;
        state.layout = Weak::new();
    }

    pub(super) fn cancel_drag(&self) {
        self.state.borrow_mut().drag = None;
    }

    fn range(&self) -> Range<usize> {
        self.state
            .borrow()
            .endpoints
            .map_or(0..0, |(anchor, head)| anchor.min(head)..anchor.max(head))
    }

    pub(super) fn dragged(&self) -> bool {
        self.state.borrow().moved
    }

    pub(super) fn shortcut(&self, combo: &str, w: &Window, cx: &mut Context<DshApp>) -> bool {
        if !self.focus.is_focused(w) || bridge::is_mac() != combo.starts_with("meta+") {
            return false;
        }
        let Some(layout) = self.state.borrow().layout.upgrade() else {
            return false;
        };
        match combo {
            "meta+a" | "meta+A" | "ctrl+a" | "ctrl+A" => {
                self.state.borrow_mut().endpoints = Some((0, layout.borrow().len()));
                cx.notify();
                true
            }
            "meta+c" | "meta+C" | "ctrl+c" | "ctrl+C" => {
                let selected = layout.borrow().text(self.range(), TextSource::Authored);
                if selected.is_empty() {
                    return false;
                }
                cx.write_to_clipboard(ClipboardItem::new_string(selected));
                true
            }
            _ => false,
        }
    }
}

#[derive(Default)]
struct Frame {
    spans: Vec<Span>,
}

struct Span {
    prefix: &'static str,
    bytes: Range<usize>,
    text: SharedString,
    line_endings: Option<LineEndings>,
    rows: Vec<VisualRow>,
}

enum TextSource {
    Shaped,
    Authored,
}

struct LineEndings {
    original: SharedString,
    hidden_cr: Vec<usize>,
}

impl LineEndings {
    fn slice(&self, range: Range<usize>) -> Option<&str> {
        let original = |offset| {
            if offset == self.original.len() - self.hidden_cr.len() {
                self.original.len()
            } else {
                offset
                    + self
                        .hidden_cr
                        .partition_point(|position| *position < offset)
            }
        };
        self.original
            .get(original(range.start)..original(range.end))
    }
}

pub(super) fn normalize_line_endings(text: &str) -> Cow<'_, str> {
    if text.contains('\r') {
        Cow::Owned(text.replace('\r', ""))
    } else {
        Cow::Borrowed(text)
    }
}

struct VisualRow {
    origin: Point<Pixels>,
    height: Pixels,
    line: Arc<LineLayout>,
    logical_start: usize,
    local: Range<usize>,
    start_x: Pixels,
    end_x: Pixels,
    newline: bool,
}

impl VisualRow {
    fn index(&self, x: Pixels, nearest: bool) -> usize {
        let x = x - self.origin.x + self.start_x;
        let index = if x <= self.start_x {
            self.local.start
        } else if x >= self.end_x {
            self.local.end
        } else if nearest {
            self.line.closest_index_for_x(x)
        } else {
            self.line.index_for_x(x).unwrap_or(self.local.end)
        };
        self.logical_start + index.clamp(self.local.start, self.local.end)
    }

    fn distance(&self, position: Point<Pixels>) -> (f32, f32) {
        let bottom = self.origin.y + self.height;
        let right = self.origin.x + self.end_x - self.start_x;
        let vertical = position.y - position.y.clamp(self.origin.y, bottom);
        let horizontal = position.x - position.x.clamp(self.origin.x, right);
        (vertical.abs().into(), horizontal.abs().into())
    }

    fn highlight(&self, selected: &Range<usize>, space: Pixels) -> Option<Bounds<Pixels>> {
        let start = selected
            .start
            .saturating_sub(self.logical_start)
            .max(self.local.start);
        let end = selected
            .end
            .saturating_sub(self.logical_start)
            .min(self.local.end);
        let newline = self.newline
            && selected.start <= self.logical_start + self.local.end
            && selected.end > self.logical_start + self.local.end;
        if start >= end && !newline {
            return None;
        }
        let left = self.line.x_for_index(start.min(self.local.end));
        let right = self.line.x_for_index(end) + if newline { space } else { px(0.) };
        (right > left).then(|| {
            Bounds::new(
                point(self.origin.x + left - self.start_x, self.origin.y),
                size(right - left, self.height),
            )
        })
    }
}

impl Frame {
    fn len(&self) -> usize {
        self.spans.last().map_or(0, |span| span.bytes.end)
    }

    fn index(&self, position: Point<Pixels>, nearest: bool) -> Option<usize> {
        let mut extents = self.spans.iter().filter_map(|span| {
            let first = span.rows.first()?;
            let last = span.rows.last()?;
            Some((first.origin.y, last.origin.y + last.height))
        });
        let (top, bottom) = extents.next()?;
        let (top, bottom) = extents.fold((top, bottom), |(top, bottom), (start, end)| {
            (top.min(start), bottom.max(end))
        });
        if position.y < top {
            return Some(0);
        }
        if position.y >= bottom {
            return Some(self.len());
        }
        self.spans
            .iter()
            .filter_map(|span| {
                let index = span
                    .rows
                    .partition_point(|row| row.origin.y + row.height <= position.y);
                span.rows.get(index).or_else(|| span.rows.last())
            })
            .min_by(|a, b| {
                let a = a.distance(position);
                let b = b.distance(position);
                a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1))
            })
            .map(|row| row.index(position.x, nearest))
    }

    fn text(&self, selected: Range<usize>, source: TextSource) -> String {
        if selected.is_empty() {
            return String::new();
        }
        let mut text = String::new();
        for span in &self.spans {
            let prefix_start = span.bytes.start - span.prefix.len();
            let from = selected.start.saturating_sub(prefix_start);
            let to = selected
                .end
                .saturating_sub(prefix_start)
                .min(span.prefix.len());
            if from < to {
                text.push_str(&span.prefix[from..to]);
            }
            let from = selected.start.saturating_sub(span.bytes.start);
            let to = selected
                .end
                .saturating_sub(span.bytes.start)
                .min(span.text.len());
            if from < to {
                let piece = match (&source, &span.line_endings) {
                    (TextSource::Authored, Some(original)) => original.slice(from..to),
                    _ => span.text.get(from..to),
                };
                if let Some(piece) = piece {
                    text.push_str(piece);
                }
            }
        }
        text
    }

    fn unit(&self, index: usize, unit: SelectionUnit) -> Range<usize> {
        if matches!(unit, SelectionUnit::Character) {
            return index..index;
        }
        let text = self.text(0..self.len(), TextSource::Shaped);
        unit_at(&text, index, unit)
    }
}

fn unit_at(text: &str, index: usize, unit: SelectionUnit) -> Range<usize> {
    if text.is_empty() || matches!(unit, SelectionUnit::Character) {
        return index..index;
    }
    let index = text.floor_char_boundary(index.min(text.len() - 1));
    match unit {
        SelectionUnit::Character => index..index,
        SelectionUnit::Word => text
            .split_word_bound_indices()
            .find_map(|(start, word)| {
                (index < start + word.len()).then_some(start..start + word.len())
            })
            .unwrap_or(index..index),
        SelectionUnit::Line => {
            let start = text[..index].rfind('\n').map_or(0, |position| position + 1);
            let end = text[index..]
                .find('\n')
                .map_or(text.len(), |position| index + position + 1);
            start..end
        }
    }
}

#[derive(Clone)]
pub(super) struct SelectionLayout {
    selection: Rc<DocumentSelection>,
    frame: Rc<RefCell<Frame>>,
}

impl SelectionLayout {
    pub(super) fn new(selection: Rc<DocumentSelection>) -> Self {
        Self {
            selection,
            frame: Rc::new(RefCell::new(Frame::default())),
        }
    }

    pub(super) fn span_count(&self) -> usize {
        self.frame.borrow().spans.len()
    }

    pub(super) fn text(
        selection: Option<&Self>,
        text: impl Into<SharedString>,
        prefix: &'static str,
        style: impl FnOnce(SharedString) -> StyledText,
    ) -> gpui::AnyElement {
        let text = text.into();
        let styled = style(text.clone());
        let Some(selection) = selection else {
            return styled.into_any_element();
        };
        selection.register_text(text, prefix, styled, None, None)
    }

    pub(super) fn authored_text(&self, text: &str, prefix: &'static str) -> gpui::AnyElement {
        let display = normalize_line_endings(text);
        let line_endings = matches!(display, Cow::Owned(_)).then(|| LineEndings {
            original: text.to_owned().into(),
            hidden_cr: text
                .match_indices('\r')
                .enumerate()
                .map(|(count, (byte, _))| byte - count)
                .collect(),
        });
        let display = SharedString::from(display.into_owned());
        let styled = StyledText::new(display.clone());
        self.register_text(display, prefix, styled, None, line_endings)
    }

    pub(super) fn numbered_text(
        &self,
        text: impl Into<SharedString>,
        number_color: gpui::Hsla,
        style: impl FnOnce(SharedString) -> StyledText,
    ) -> gpui::AnyElement {
        let text = text.into();
        let styled = style(text.clone());
        self.register_text(text, "", styled, Some(number_color), None)
    }

    fn register_text(
        &self,
        text: SharedString,
        prefix: &'static str,
        styled: StyledText,
        number_color: Option<gpui::Hsla>,
        line_endings: Option<LineEndings>,
    ) -> gpui::AnyElement {
        let mut frame = self.frame.borrow_mut();
        let index = frame.spans.len();
        let prefix = if index == 0 { "" } else { prefix };
        let start = frame.len() + prefix.len();
        frame.spans.push(Span {
            prefix,
            bytes: start..start + text.len(),
            text,
            line_endings,
            rows: Vec::new(),
        });
        SelectableText {
            styled,
            index,
            selection: self.clone(),
            number_color,
        }
        .into_any_element()
    }

    pub(super) fn clicked_link(&self, event: &gpui::ClickEvent) -> bool {
        !(self.selection.dragged() || bridge::is_mac() && event.modifiers().control)
    }

    pub(super) fn mouse_down(
        &self,
        event: &MouseDownEvent,
        w: &mut Window,
        cx: &mut Context<DshApp>,
    ) {
        if bridge::is_mac() && event.modifiers.control {
            self.context_menu(event, w, cx);
            return;
        }
        let frame = self.frame.borrow();
        let unit = if event.modifiers.shift {
            SelectionUnit::Character
        } else {
            match event.click_count {
                2 => SelectionUnit::Word,
                3.. => SelectionUnit::Line,
                _ => SelectionUnit::Character,
            }
        };
        let Some(index) = frame.index(event.position, matches!(unit, SelectionUnit::Character))
        else {
            return;
        };
        let mut state = self.selection.state.borrow_mut();
        let (seed, anchor, head) = if event.modifiers.shift
            && let Some((anchor, _)) = state.endpoints
        {
            (anchor..anchor, anchor, index)
        } else {
            let seed = frame.unit(index, unit);
            (seed.clone(), seed.start, seed.end)
        };
        state.endpoints = Some((anchor, head));
        state.moved = false;
        state.drag = Some(Drag {
            seed,
            unit,
            start: event.position,
            position: event.position,
        });
        drop(state);
        self.selection.focus.focus(w, cx);
        cx.notify();
    }

    pub(super) fn context_menu(
        &self,
        event: &MouseDownEvent,
        w: &mut Window,
        cx: &mut Context<DshApp>,
    ) {
        if !bridge::is_mac() {
            return;
        }
        let frame = self.frame.borrow();
        let range = self.selection.range();
        let Some(index) = frame.index(event.position, false) else {
            return;
        };
        if !range.contains(&index) {
            return;
        }
        let text = frame.text(range, TextSource::Authored);
        if text.is_empty() {
            return;
        }
        self.selection.cancel_drag();
        self.selection.focus.focus(w, cx);
        w.prevent_default();
        cx.stop_propagation();
        invoke(
            "show_document_copy_menu",
            json!({
                "text": text, "x": f32::from(event.position.x), "y": f32::from(event.position.y),
            }),
        );
    }

    fn drag_to(&self, position: Point<Pixels>, bounds: Bounds<Pixels>) {
        let frame = self.frame.borrow();
        let mut state = self.selection.state.borrow_mut();
        let Some(drag) = state.drag.as_mut() else {
            return;
        };
        drag.position = position;
        let position = point(
            position.x.clamp(
                bounds.left(),
                (bounds.right() - px(0.01)).max(bounds.left()),
            ),
            position
                .y
                .clamp(bounds.top(), (bounds.bottom() - px(0.01)).max(bounds.top())),
        );
        let Some(index) = frame.index(position, matches!(drag.unit, SelectionUnit::Character))
        else {
            return;
        };
        let unit = frame.unit(index, drag.unit);
        let endpoints = if index < drag.seed.start {
            (drag.seed.end, unit.start)
        } else {
            (drag.seed.start, unit.end.max(drag.seed.end))
        };
        let movement = drag.position - drag.start;
        if movement.x.abs() >= px(2.) || movement.y.abs() >= px(2.) {
            state.moved = true;
        }
        state.endpoints = Some(endpoints);
    }

    pub(super) fn event_layer(
        &self,
        scroll: ScrollHandle,
        record_scroll: impl Fn() + Clone + 'static,
    ) -> gpui::Canvas<()> {
        let prepare = self.clone();
        let events = self.clone();
        let paint_scroll = scroll.clone();
        canvas(
            move |_, w, _| {
                let bounds = scroll.bounds();
                let pointer = prepare
                    .selection
                    .state
                    .borrow()
                    .drag
                    .as_ref()
                    .map(|drag| drag.position);
                if let Some(position) = pointer {
                    prepare.drag_to(position, bounds);
                    let overshoot = position
                        - point(
                            position.x.clamp(bounds.left(), bounds.right()),
                            position.y.clamp(bounds.top(), bounds.bottom()),
                        );
                    let speed = overshoot.map(|distance| (distance * 0.4).clamp(px(-30.), px(30.)));
                    let offset = scroll.offset();
                    let maximum = scroll.max_offset();
                    let next = point(
                        (offset.x - speed.x).clamp(-maximum.x, px(0.)),
                        (offset.y - speed.y).clamp(-maximum.y, px(0.)),
                    );
                    if next != offset {
                        scroll.set_offset(next);
                        if next.y != offset.y {
                            record_scroll();
                        }
                        w.request_animation_frame();
                    }
                }
            },
            move |_, _, w, _| {
                events.selection.state.borrow_mut().layout = Rc::downgrade(&events.frame);
                let view = w.current_view();
                let viewport = paint_scroll.bounds();
                let down = events.selection.clone();
                w.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture
                        && event.button == MouseButton::Left
                        && !viewport.contains(&event.position)
                        && !down.range().is_empty()
                    {
                        down.clear();
                        cx.notify(view);
                    }
                });
                let moving = events.clone();
                w.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture
                        || moving.selection.state.borrow().drag.is_none()
                    {
                        return;
                    }
                    if event.dragging() {
                        moving.drag_to(event.position, viewport);
                    } else {
                        moving.selection.cancel_drag();
                    }
                    cx.notify(view);
                });
                let release = events.clone();
                w.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                        return;
                    }
                    let moved = release
                        .selection
                        .state
                        .borrow()
                        .drag
                        .as_ref()
                        .is_some_and(|drag| drag.position != event.position);
                    if moved {
                        release.drag_to(event.position, viewport);
                        cx.notify(view);
                    }
                    release.selection.cancel_drag();
                });
            },
        )
        .absolute()
        .inset_0()
    }
}

struct SelectableText {
    styled: StyledText,
    index: usize,
    selection: SelectionLayout,
    number_color: Option<gpui::Hsla>,
}

impl IntoElement for SelectableText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for SelectableText {
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
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        w: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        self.styled.request_layout(id, inspector, w, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        w: &mut Window,
        cx: &mut App,
    ) {
        self.styled.prepaint(id, inspector, bounds, state, w, cx);
        let mut frame = self.selection.frame.borrow_mut();
        let span = &mut frame.spans[self.index];
        span.rows = visual_rows(self.styled.layout(), span.bytes.start, &span.text);
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut (),
        prepaint: &mut (),
        w: &mut Window,
        cx: &mut App,
    ) {
        let selected = self.selection.selection.range();
        let frame = self.selection.frame.borrow();
        let span = &frame.spans[self.index];
        if let Some(color) = self.number_color {
            let clip = w.content_mask().bounds;
            let style = w.text_style();
            let font_size = style.font_size.to_pixels(w.rem_size());
            for (index, row) in span
                .rows
                .iter()
                .filter(|row| row.local.start == 0)
                .enumerate()
            {
                if row.origin.y >= clip.bottom() || row.origin.y + row.height <= clip.top() {
                    continue;
                }
                let label = (index + 1).to_string();
                let run = gpui::TextRun {
                    color,
                    ..style.to_run(label.len())
                };
                let line = w
                    .text_system()
                    .shape_line(label.into(), font_size, &[run], None);
                let origin = point(row.origin.x - px(12.) - line.width(), row.origin.y);
                let _ = line.paint(origin, row.height, gpui::TextAlign::Left, None, w, cx);
            }
        }
        if selected.is_empty()
            || selected.start >= span.bytes.end
            || selected.end <= span.bytes.start
        {
            self.styled
                .paint(id, inspector, bounds, state, prepaint, w, cx);
            return;
        }
        let layout = self.styled.layout();
        let _ = layout.paint_background(w, cx);
        let style = w.text_style();
        let space = w
            .text_system()
            .shape_line(
                " ".into(),
                style.font_size.to_pixels(w.rem_size()),
                &[style.to_run(1)],
                None,
            )
            .width();
        let ink = gpui::rgba(if super::workspace::dark(cx) {
            0xffffff6f
        } else {
            0x00000035
        });
        let clip = w.content_mask().bounds;
        for bounds in span
            .rows
            .iter()
            .filter_map(|row| row.highlight(&selected, space))
        {
            if clip.intersects(&bounds) {
                w.paint_quad(fill(bounds, ink));
            }
        }
        let _ = layout.paint_foreground(w, cx);
    }
}

fn visual_rows(layout: &TextLayout, start: usize, text: &str) -> Vec<VisualRow> {
    let mut rows = Vec::new();
    let mut origin = layout.bounds().origin;
    let mut logical = 0;
    let height = layout.line_height();
    for line in layout.line_layouts() {
        let mut row_start = 0;
        let mut start_x = px(0.);
        let unwrapped = &line.unwrapped_layout;
        for (end, end_x) in line
            .wrap_boundaries()
            .iter()
            .map(|boundary| {
                let glyph = &unwrapped.runs[boundary.run_ix].glyphs[boundary.glyph_ix];
                (glyph.index, glyph.position.x)
            })
            .chain([(unwrapped.len, unwrapped.width)])
        {
            rows.push(VisualRow {
                origin,
                height,
                line: unwrapped.clone(),
                logical_start: start + logical,
                local: row_start..end,
                start_x,
                end_x,
                newline: end == unwrapped.len && text.as_bytes().get(logical + end) == Some(&b'\n'),
            });
            row_start = end;
            start_x = end_x;
            origin.y += height;
        }
        logical += unwrapped.len + 1;
    }
    rows
}
