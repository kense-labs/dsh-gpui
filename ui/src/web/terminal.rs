//! GPUI view and input handler for the embedded PTY screen.

use super::bridge;
use crate::{
    terminal_keys::{TerminalKey, terminal_key},
    tokens::dsh,
};
use gpui::{
    Animation, AnimationExt, App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase,
    ElementInputHandler, EntityInputHandler, FocusHandle, FontWeight, KeyDownEvent, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, ScrollEvent,
    Size, Subscription, Task, UTF16Selection, Window, canvas, div, font, point, prelude::*, px,
    rgb, size,
};
use serde::Deserialize;
use std::{ops::Range, time::Duration};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(raw_module = "../terminal-screen.js")]
extern "C" {
    type TerminalEngine;

    #[wasm_bindgen(js_name = createTerminal)]
    fn create_terminal() -> TerminalEngine;
    #[wasm_bindgen(js_name = bindTerminal)]
    fn bind_terminal(screen: &TerminalEngine, id: &str);
    #[wasm_bindgen(js_name = unbindTerminal)]
    fn unbind_terminal(screen: &TerminalEngine);
    #[wasm_bindgen(js_name = writeScreen)]
    fn write_screen(screen: &TerminalEngine, data: &str) -> js_sys::Promise;
    #[wasm_bindgen(js_name = restoreTerminal)]
    fn restore_terminal(
        screen: &TerminalEngine,
        cols: u16,
        rows: u16,
        snapshot: &str,
    ) -> js_sys::Promise;
    #[wasm_bindgen(js_name = resizeScreen)]
    fn resize_screen(screen: &TerminalEngine, cols: u16, rows: u16) -> js_sys::Promise;
    #[wasm_bindgen(js_name = fitTerminal)]
    fn fit_terminal(screen: &TerminalEngine, cols: u16, rows: u16) -> js_sys::Promise;
    #[wasm_bindgen(js_name = terminalInput)]
    fn terminal_input(screen: &TerminalEngine, data: &str);
    #[wasm_bindgen(js_name = terminalPaste)]
    fn terminal_paste(screen: &TerminalEngine, data: &str);
    #[wasm_bindgen(js_name = terminalScroll)]
    fn terminal_scroll(screen: &TerminalEngine, lines: i32);
    #[wasm_bindgen(js_name = terminalMode)]
    fn terminal_mode(screen: &TerminalEngine, mode: &str) -> bool;
    #[wasm_bindgen(js_name = terminalFrame)]
    fn terminal_frame(screen: &TerminalEngine) -> JsValue;
    #[wasm_bindgen(js_name = terminalSelectStart)]
    fn terminal_select_start(
        screen: &TerminalEngine,
        col: u16,
        row: u16,
        clicks: u32,
        extend: bool,
        column: bool,
    );
    #[wasm_bindgen(js_name = terminalSelectMove)]
    fn terminal_select_move(screen: &TerminalEngine, col: u16, row: u16, edge: i32);
    #[wasm_bindgen(js_name = terminalSelectAll)]
    fn terminal_select_all(screen: &TerminalEngine);
    #[wasm_bindgen(js_name = terminalCopy)]
    fn terminal_copy(screen: &TerminalEngine) -> String;
    #[wasm_bindgen(js_name = terminalMouseActive)]
    fn terminal_mouse_active(screen: &TerminalEngine) -> bool;
    #[wasm_bindgen(js_name = terminalMouse)]
    fn terminal_mouse(
        screen: &TerminalEngine,
        col: u16,
        row: u16,
        x: u16,
        y: u16,
        button: u8,
        action: u8,
        modifiers: u8,
    );
    #[wasm_bindgen(js_name = terminalFocus)]
    fn terminal_focus(screen: &TerminalEngine, focused: bool);
    #[wasm_bindgen(js_name = terminalWheel)]
    fn terminal_wheel(
        screen: &TerminalEngine,
        col: u16,
        row: u16,
        x: u16,
        y: u16,
        delta: f32,
        cell_height: f32,
        precise: bool,
        modifiers: u8,
    ) -> bool;
}

const FONT: &str = "Menlo";
const FONT_SIZE: Pixels = px(13.);
const PADDING: Pixels = px(8.);
const BOLD: u8 = 1;
const ITALIC: u8 = 2;
const UNDERLINE: u8 = 4;
const STRIKE: u8 = 8;
const INVERSE: u8 = 16;
const DIM: u8 = 32;
const SELECTED: u8 = 64;

#[derive(Deserialize)]
pub(super) struct TerminalSnapshot {
    pub(super) title: String,
    pub(super) cols: u16,
    pub(super) rows: u16,
    pub(super) sequence: u64,
    pub(super) screen: String,
    pub(super) exit: Option<TerminalExit>,
}

#[derive(Deserialize)]
pub(super) struct TerminalExit {
    pub(super) code: Option<u32>,
}

#[derive(Deserialize)]
pub(super) struct TerminalUpdate {
    pub(super) sequence: u64,
    pub(super) operation: TerminalOperation,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum TerminalOperation {
    Output { data: String },
    Resize { cols: u16, rows: u16 },
    Exit { code: Option<u32> },
    Rename { title: String },
}

#[derive(Clone, Copy)]
struct GridBounds {
    bounds: Bounds<Pixels>,
    cell: Size<Pixels>,
}

#[derive(Clone, Copy)]
enum PointerGesture {
    Selection(Point<Pixels>),
    Report(MouseButton),
}

fn mouse_button(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Navigate(_) => None,
    }
}

fn mouse_modifiers(modifiers: Modifiers) -> u8 {
    u8::from(modifiers.control) | (u8::from(modifiers.alt) << 1) | (u8::from(modifiers.shift) << 2)
}

#[derive(Default, Deserialize)]
struct ScreenFrame {
    lines: Vec<Vec<CellRun>>,
    cursor: Option<(u16, u16, u8)>,
}

#[derive(Deserialize)]
struct CellRun(u16, u16, String, Option<u32>, Option<u32>, u8);

pub(super) struct TerminalScreen {
    engine: TerminalEngine,
    focus: FocusHandle,
    writable: bool,
    composing: String,
    scroll_remainder: f32,
    grid: (u16, u16),
    geometry: Option<GridBounds>,
    pointer: Option<PointerGesture>,
    drag_scroll: Option<Task<()>>,
    _focus_subscriptions: [Subscription; 3],
    frame: ScreenFrame,
    dirty: bool,
}

impl TerminalScreen {
    pub(super) fn new(w: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let on_focus = cx.on_focus(&focus, w, |this, w, cx| {
            terminal_focus(&this.engine, w.is_window_active());
            cx.notify();
        });
        let on_blur = cx.on_blur(&focus, w, |this, _, cx| {
            this.pointer = None;
            this.drag_scroll = None;
            terminal_focus(&this.engine, false);
            cx.notify();
        });
        let on_activation = cx.observe_window_activation(w, |this, w, cx| {
            if this.focus.is_focused(w) {
                terminal_focus(&this.engine, w.is_window_active());
                cx.notify();
            }
        });
        Self {
            engine: create_terminal(),
            focus,
            writable: false,
            composing: String::new(),
            scroll_remainder: 0.,
            grid: (80, 24),
            geometry: None,
            pointer: None,
            drag_scroll: None,
            _focus_subscriptions: [on_focus, on_blur, on_activation],
            frame: ScreenFrame::default(),
            dirty: true,
        }
    }

    pub(super) fn bind(&mut self, id: &str, cx: &mut Context<Self>) {
        bind_terminal(&self.engine, id);
        self.writable = true;
        cx.notify();
    }

    pub(super) fn unbind(&mut self, cx: &mut Context<Self>) {
        unbind_terminal(&self.engine);
        self.writable = false;
        self.composing.clear();
        self.pointer = None;
        self.drag_scroll = None;
        cx.notify();
    }

    pub(super) fn feed(&mut self, data: &str, cx: &mut Context<Self>) {
        self.refresh_after(write_screen(&self.engine, data), cx);
    }

    fn refresh_after(&self, update: js_sys::Promise, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let _ = wasm_bindgen_futures::JsFuture::from(update).await;
            let _ = this.update(cx, |this, cx| this.screen_changed(cx));
        })
        .detach();
    }

    pub(super) fn restore(&mut self, snapshot: &TerminalSnapshot) -> js_sys::Promise {
        self.grid = (snapshot.cols, snapshot.rows);
        restore_terminal(&self.engine, snapshot.cols, snapshot.rows, &snapshot.screen)
    }

    pub(super) fn observe_grid(&mut self, cols: u16, rows: u16, cx: &mut Context<Self>) {
        // After binding, these events acknowledge local fits and may arrive after a newer fit.
        if !self.writable && self.grid != (cols, rows) {
            self.grid = (cols, rows);
            self.refresh_after(resize_screen(&self.engine, cols, rows), cx);
        }
    }

    pub(super) fn restore_finished(&mut self, cx: &mut Context<Self>) {
        self.screen_changed(cx);
    }

    pub(super) fn focus(&self, w: &mut Window, cx: &mut App) {
        self.focus.focus(w, cx);
    }

    fn screen_changed(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        cx.notify();
    }

    fn fit(&mut self, bounds: Bounds<Pixels>, cell: Size<Pixels>, cx: &mut Context<Self>) {
        self.geometry = Some(GridBounds { bounds, cell });
        let cols = (f32::from(bounds.size.width) / f32::from(cell.width)) as u16;
        let rows = (f32::from(bounds.size.height) / f32::from(cell.height)) as u16;
        let grid = (cols.min(500), rows.min(200));
        if self.writable && cols >= 2 && rows >= 1 && self.grid != grid {
            self.grid = grid;
            self.refresh_after(fit_terminal(&self.engine, grid.0, grid.1), cx);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let is_mac = bridge::is_mac();
        let key = &event.keystroke;
        let mods = key.modifiers;
        if is_mac && mods.platform && !mods.alt && !mods.control && !mods.shift {
            match key.key.as_str() {
                "a" => {
                    terminal_select_all(&self.engine);
                    self.screen_changed(cx);
                }
                "c" => self.copy(cx),
                _ => return,
            }
            cx.stop_propagation();
            return;
        }
        if !is_mac && mods.control && mods.shift && !mods.platform && !mods.alt && key.key == "c" {
            self.copy(cx);
            cx.stop_propagation();
            return;
        }
        if !self.writable || !self.composing.is_empty() {
            return;
        }
        let application_cursor = terminal_mode(&self.engine, "applicationCursorKeysMode");
        match terminal_key(key, application_cursor, is_mac) {
            Some(TerminalKey::Input(data)) => terminal_input(&self.engine, &data),
            Some(TerminalKey::ScrollPage(direction)) => {
                let page = i32::from(self.grid.1.saturating_sub(1).max(1));
                terminal_scroll(&self.engine, direction * page);
            }
            None => return,
        }
        cx.stop_propagation();
        self.screen_changed(cx);
    }

    fn copy(&self, cx: &mut Context<Self>) {
        let text = terminal_copy(&self.engine);
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn selection_cell(&self, position: Point<Pixels>) -> Option<(u16, u16)> {
        let geometry = self.geometry?;
        let local = position - geometry.bounds.origin;
        let col = (f32::from(local.x) / f32::from(geometry.cell.width)).round() as u16;
        let row = (f32::from(local.y) / f32::from(geometry.cell.height)) as u16;
        Some((col.min(self.grid.0), row.min(self.grid.1 - 1)))
    }

    fn mouse_coordinates(&self, position: Point<Pixels>) -> Option<(u16, u16, u16, u16)> {
        let geometry = self.geometry?;
        let local = position - geometry.bounds.origin;
        let width = f32::from(geometry.cell.width);
        let height = f32::from(geometry.cell.height);
        let x = f32::from(local.x).clamp(0., (width * f32::from(self.grid.0) - 1.).max(0.));
        let y = f32::from(local.y).clamp(0., (height * f32::from(self.grid.1) - 1.).max(0.));
        Some(((x / width) as u16, (y / height) as u16, x as u16, y as u16))
    }

    fn report_mouse(&self, position: Point<Pixels>, button: u8, action: u8, modifiers: Modifiers) {
        if let Some((col, row, x, y)) = self.mouse_coordinates(position) {
            terminal_mouse(
                &self.engine,
                col,
                row,
                x,
                y,
                button,
                action,
                mouse_modifiers(modifiers),
            );
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        let is_mac = bridge::is_mac();
        if mouse_button(event.button).is_some() {
            self.focus(w, cx);
        }
        if terminal_mouse_active(&self.engine)
            && (is_mac || !event.modifiers.shift)
            && let Some(button) = mouse_button(event.button)
        {
            self.pointer = Some(PointerGesture::Report(event.button));
            self.report_mouse(event.position, button, 1, event.modifiers);
        } else if event.button == MouseButton::Left
            && let Some((col, row)) = self.selection_cell(event.position)
        {
            self.pointer = Some(PointerGesture::Selection(event.position));
            terminal_select_start(
                &self.engine,
                col,
                row,
                event.click_count as u32,
                event.modifiers.shift && !terminal_mouse_active(&self.engine),
                event.modifiers.alt,
            );
        } else {
            return;
        }
        self.screen_changed(cx);
        cx.stop_propagation();
    }

    fn mouse_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        match self.pointer {
            Some(PointerGesture::Selection(_)) if event.dragging() => {
                self.pointer = Some(PointerGesture::Selection(event.position));
                self.extend_selection(event.position, cx);
                if self.selection_scroll_speed(event.position) != 0 && self.drag_scroll.is_none() {
                    self.drag_scroll = Some(cx.spawn(async |this, cx| {
                        loop {
                            cx.background_executor()
                                .timer(Duration::from_millis(50))
                                .await;
                            let active = this.update(cx, |this, cx| this.drag_scroll_step(cx));
                            if !matches!(active, Ok(true)) {
                                break;
                            }
                        }
                    }));
                }
            }
            Some(PointerGesture::Report(button)) if event.pressed_button == Some(button) => {
                if let Some(button) = mouse_button(button) {
                    self.report_mouse(event.position, button, 32, event.modifiers);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        match self.pointer {
            Some(PointerGesture::Selection(position)) if event.button == MouseButton::Left => {
                if position != event.position {
                    self.extend_selection(event.position, cx);
                }
            }
            Some(PointerGesture::Report(button)) if event.button == button => {
                if let Some(button) = mouse_button(button) {
                    self.report_mouse(event.position, button, 0, event.modifiers);
                }
            }
            _ => return,
        }
        self.pointer = None;
        self.drag_scroll = None;
        cx.stop_propagation();
    }

    fn extend_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some((col, row)) = self.selection_cell(position) {
            terminal_select_move(
                &self.engine,
                col,
                row,
                self.selection_scroll_speed(position),
            );
            self.screen_changed(cx);
        }
    }

    fn selection_scroll_speed(&self, position: Point<Pixels>) -> i32 {
        let Some(geometry) = self.geometry else {
            return 0;
        };
        let top = geometry.bounds.origin.y;
        let bottom =
            top + (geometry.cell.height * f32::from(self.grid.1)).min(geometry.bounds.size.height);
        let distance = f32::from(position.y - position.y.clamp(top, bottom));
        if distance == 0. {
            return 0;
        }
        let scaled = distance.clamp(-50., 50.) / 50.;
        (14. * scaled + 0.5).floor() as i32 + if distance < 0. { -1 } else { 1 }
    }

    fn drag_scroll_step(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(PointerGesture::Selection(position)) = self.pointer else {
            return false;
        };
        let speed = self.selection_scroll_speed(position);
        if speed != 0 {
            terminal_scroll(&self.engine, speed);
            self.extend_selection(position, cx);
        }
        true
    }

    fn scroll(&mut self, event: &ScrollEvent, cell_height: Pixels, cx: &mut Context<Self>) {
        let cell_height = f32::from(cell_height);
        let pixels = -f32::from(event.delta.pixel_delta(px(cell_height)).y);
        let lines = pixels / cell_height;
        let precise = event.delta.precise();
        if let Some((col, row, x, y)) = self.mouse_coordinates(event.position)
            && terminal_wheel(
                &self.engine,
                col,
                row,
                x,
                y,
                if precise { pixels } else { lines },
                cell_height,
                precise,
                mouse_modifiers(event.modifiers),
            )
        {
            self.screen_changed(cx);
        } else {
            self.scroll_remainder += lines;
            let steps = self.scroll_remainder.trunc() as i32;
            if steps != 0 {
                self.scroll_remainder -= steps as f32;
                terminal_scroll(&self.engine, steps);
                self.screen_changed(cx);
            }
        }
        cx.stop_propagation();
    }
}

impl Render for TerminalScreen {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = dsh(cx);
        if self.dirty {
            self.frame =
                serde_wasm_bindgen::from_value(terminal_frame(&self.engine)).unwrap_or_default();
            self.dirty = false;
        }
        let text = w.text_system();
        let font_id = text.resolve_font(&font(FONT));
        let cell = size(
            text.ch_advance(font_id, FONT_SIZE)
                .unwrap_or(px(8.))
                .max(px(1.)),
            (text.ascent(font_id, FONT_SIZE) + text.descent(font_id, FONT_SIZE))
                .ceil()
                .max(px(1.)),
        );
        let view = cx.entity();
        let measure_view = view.clone();
        let focus = self.focus.clone();
        let focused = focus.is_focused(w) && w.is_window_active();
        let writable = self.writable;
        let cursor = self.frame.cursor;

        // The viewport mask stays inside the padded screen, even while the PTY still
        // has the previous grid during a resize. The body owns the rounded backdrop.
        let grid = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .children(self.frame.lines.iter().map(|runs| {
                div()
                    .relative()
                    .w_full()
                    .h(cell.height)
                    .flex_none()
                    .children(
                        runs.iter()
                            .map(|&CellRun(col, width, ref text, fg, bg, flags)| {
                                let mut fg = fg.map_or(theme.ink, |c| rgb(c).into());
                                let mut bg = bg.map_or(theme.bg_base, |c| rgb(c).into());
                                if flags & INVERSE != 0 {
                                    std::mem::swap(&mut fg, &mut bg);
                                }
                                if flags & DIM != 0 {
                                    fg = fg.opacity(0.5);
                                }
                                if flags & SELECTED != 0 {
                                    fg = theme.bg_base;
                                    bg = theme.ink;
                                }
                                div()
                                    .absolute()
                                    .top_0()
                                    .left(cell.width * col as f32)
                                    .w(cell.width * width as f32)
                                    .h(cell.height)
                                    .text_color(fg)
                                    .bg(bg)
                                    .when(flags & BOLD != 0, |run| {
                                        run.font_weight(FontWeight::BOLD)
                                    })
                                    .when(flags & ITALIC != 0, |run| run.italic())
                                    .when(flags & UNDERLINE != 0, |run| run.underline())
                                    .when(flags & STRIKE != 0, |run| run.line_through())
                                    .child(text.clone())
                            }),
                    )
            }))
            .children(cursor.map(|(col, row, style)| {
                let x = cell.width * col as f32;
                let y = cell.height * row as f32;
                let cursor = div().absolute().left(x).top(y).w(cell.width).h(cell.height);
                let cursor = if !focused {
                    cursor.border_1().border_color(theme.ink)
                } else {
                    match style {
                        3 | 4 => cursor.border_b_2().border_color(theme.ink),
                        5 | 6 => cursor.w(px(2.)).bg(theme.ink),
                        _ => cursor.bg(theme.ink.opacity(0.65)),
                    }
                };
                if focused && matches!(style, 0 | 1 | 3 | 5) && !cx.reduce_motion() {
                    cursor
                        .with_animation(
                            "terminal-cursor",
                            Animation::new(Duration::from_secs(1)).repeat(),
                            |cursor, t| cursor.opacity(if t < 0.5 { 1. } else { 0. }),
                        )
                        .into_any_element()
                } else {
                    cursor.into_any_element()
                }
            }))
            .children(
                cursor
                    .filter(|_| !self.composing.is_empty())
                    .map(|(col, row, _)| {
                        div()
                            .absolute()
                            .left(cell.width * col as f32)
                            .top(cell.height * row as f32)
                            .bg(theme.bg_base)
                            .underline()
                            .child(self.composing.clone())
                    }),
            )
            .child(
                canvas(
                    move |bounds, w, cx| {
                        let view = measure_view.clone();
                        w.defer(cx, move |_, cx| {
                            view.update(cx, |view, cx| view.fit(bounds, cell, cx))
                        });
                    },
                    move |bounds, _, w, cx| {
                        let drag_view = view.downgrade();
                        let release_view = drag_view.clone();
                        w.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture
                                && let Some(view) = drag_view.upgrade()
                                && view.read(cx).pointer.is_some()
                            {
                                view.update(cx, |view, cx| view.mouse_drag(event, cx));
                            }
                        });
                        w.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture
                                && let Some(view) = release_view.upgrade()
                                && view.read(cx).pointer.is_some()
                            {
                                view.update(cx, |view, cx| view.mouse_up(event, cx));
                            }
                        });
                        if writable {
                            let origin = cursor.map_or(bounds.origin, |(col, row, _)| {
                                bounds.origin
                                    + point(cell.width * col as f32, cell.height * row as f32)
                            });
                            w.handle_input(
                                &focus,
                                ElementInputHandler::new(Bounds { origin, size: cell }, view),
                                cx,
                            );
                        }
                    },
                )
                .absolute()
                .inset_0(),
            );

        div()
            .id("terminal-screen")
            .role(gpui::Role::MultilineTextInput)
            .aria_label("终端")
            .track_focus(&self.focus)
            .tab_index(0)
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .p(PADDING)
            .overflow_hidden()
            .text_color(theme.ink)
            .font_family(FONT)
            .text_size(FONT_SIZE)
            .line_height(cell.height)
            .whitespace_nowrap()
            .cursor(if terminal_mouse_active(&self.engine) {
                CursorStyle::Arrow
            } else {
                CursorStyle::IBeam
            })
            .on_any_mouse_down(cx.listener(|this, event, w, cx| this.mouse_down(event, w, cx)))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, _| {
                if this.pointer.is_none()
                    && event.pressed_button.is_none()
                    && terminal_mouse_active(&this.engine)
                {
                    this.report_mouse(event.position, 3, 32, event.modifiers);
                }
            }))
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .on_scroll(cx.listener(move |this, event: &ScrollEvent, _, cx| {
                this.scroll(event, cell.height, cx);
            }))
            .child(grid)
    }
}

impl EntityInputHandler for TerminalScreen {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text: Vec<_> = self.composing.encode_utf16().collect();
        let range = range.start.min(text.len())..range.end.min(text.len());
        *actual = Some(range.clone());
        Some(String::from_utf16_lossy(&text[range]))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.composing.encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.composing.is_empty()).then(|| 0..self.composing.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.composing.clear();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composing.clear();
        terminal_input(&self.engine, text);
        self.screen_changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composing = text.to_owned();
        cx.notify();
    }

    fn paste(&mut self, item: ClipboardItem, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = item.text() {
            terminal_paste(&self.engine, &text);
            self.screen_changed(cx);
        }
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
