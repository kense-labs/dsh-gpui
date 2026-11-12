//! A live activity label whose highlight sweep is independent of its text.
//!
//! KUI's `TextShimmer` replays a 0.24s fade-in (and a 0.16s fading ghost of the
//! previous string) whenever `set_text` sees different text. That suits a label
//! that changes occasionally, but the live process title embeds streaming
//! reasoning text, which changes on nearly every frame. Each change restarts the
//! fade, so the label never finishes fading in and the previous strings pile up
//! as ghosts — it reads as constant flicker.
//!
//! Official `TextShimmer` has no such transition. Its highlight lives in a
//! decorative layer whose CSS animation is never remounted when the text
//! changes (`packages/client/ui-primitives/src/TextShimmer.module.css` drives it
//! through `.text[data-shimmer-text]::after { content: attr(...) }`), so the
//! sweep keeps running on its own clock while the text is swapped underneath.
//!
//! This component reproduces that behaviour: the sweep phase is a pure function
//! of the mount time, so `set_text` only changes what is drawn. The sweep math
//! and the falloff stops mirror KUI's `text_shimmer` and the official
//! 1.5s / 0.3s-delay / hold-then-pause cycle.

use gpui::{HighlightStyle, SharedString, StyledText, prelude::*, *};
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

/// Band width in em, matching KUI's `BAND_EM` and the official mask gradient.
const BAND_EM: f32 = 2.2;
/// Official `animation-duration`.
const DEFAULT_DURATION: Duration = Duration::from_millis(1500);
/// Opacity of the resting text, from KUI's `faint.blend(ink.opacity(0.12))`.
const REST_INK: f32 = 0.12;

/// An activity label that keeps one continuous highlight while its text changes.
pub(crate) struct LiveShimmer {
    text: SharedString,
    font_size: f32,
    duration: Duration,
    active: bool,
    /// Mount time in monotonic milliseconds; the sweep phase is derived from it
    /// and deliberately *not* reset when the text changes.
    started_ms: f64,
    /// Whether the label is currently inside the viewport. Official `TextShimmer`
    /// tracks this too; a label scrolled out of view must not hold the renderer
    /// awake for a highlight nobody can see.
    in_view: bool,
}

impl LiveShimmer {
    /// `cx` is accepted for symmetry with KUI's `TextShimmer::new`; the sweep
    /// clock is read from the monotonic host clock at render time instead, so no
    /// executor reference is retained.
    pub(crate) fn new(text: impl Into<SharedString>, _cx: &App) -> Self {
        Self {
            text: text.into(),
            font_size: 13.,
            duration: DEFAULT_DURATION,
            active: true,
            started_ms: crate::bridge::monotonic_ms(),
            in_view: true,
        }
    }

    pub(crate) fn with_font_size(mut self, size: f32) -> Self {
        if size.is_finite() && size > 0. {
            self.font_size = size;
        }
        self
    }

    pub(crate) fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = duration.max(Duration::from_millis(100));
        self
    }

    /// Replace the label. This never touches the sweep clock, so a continuously
    /// growing string stays perfectly smooth instead of cross-fading per frame.
    pub(crate) fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        // `shape_line` requires a single line, and the live detail comes from
        // reasoning paragraphs that were normalized to spaces already.
        let text: SharedString = match text.into() {
            text if text.contains('\n') => text.replace('\n', " ").into(),
            text => text,
        };
        if text != self.text {
            self.text = text;
            cx.notify();
        }
    }

    /// Activity follows the owning turn; resuming restarts the sweep at the
    /// left, which is what remounting the official decoration layer does.
    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active != active {
            self.active = active;
            self.started_ms = crate::bridge::monotonic_ms();
            cx.notify();
        }
    }
}

impl Render for LiveShimmer {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = kui::primitives::family::Paint::get(cx);
        let reduced = cx.reduce_motion();
        // Scrolling the label out of view must stop the sweep. Without this the
        // entity keeps asking for frames for a highlight nobody can see, which
        // pegs the WebView's renderer while the transcript sits idle.
        let running =
            crate::anim::shimmer_should_animate(self.active, w.is_visible(), self.in_view, reduced);
        let seconds = self.duration.as_secs_f32();
        let age = ((crate::bridge::monotonic_ms() - self.started_ms).max(0.) / 1000.) as f32;
        let t = if running {
            // Keep frames coming while the sweep is visible, and stop as soon as
            // the owning turn settles.
            w.request_animation_frame();
            crate::anim::shimmer_sweep(age, seconds)
        } else {
            0.
        };

        let style = w.text_style();
        let shaped = w.text_system().shape_line(
            self.text.clone(),
            px(self.font_size),
            &[style.to_run(self.text.len())],
            None,
        );
        let width: f32 = shaped.width.into();
        let band = BAND_EM * self.font_size;
        let peak = t * width + (2. * t - 1.) * band;
        // Active text rests dim and the travelling band lifts graphemes to full
        // ink; idle text keeps its ordinary colour so a settled label still reads.
        let rest = if self.active {
            p.faint.blend(p.ink.opacity(REST_INK))
        } else {
            p.secondary
        };
        let base = rest;
        let styles = self.text.grapheme_indices(true).map(|(start, grapheme)| {
            let end = start + grapheme.len();
            let left: f32 = shaped.x_for_index(start).into();
            let right: f32 = shaped.x_for_index(end).into();
            let offset = ((left + right) * 0.5 - peak) / band;
            let strength = crate::anim::shimmer_falloff(offset);
            (
                start..end,
                HighlightStyle {
                    color: Some(base.blend(p.ink.opacity(strength))),
                    ..Default::default()
                },
            )
        });
        let viewport = w.viewport_size();
        let weak = cx.weak_entity();
        div()
            .flex_none()
            .whitespace_nowrap()
            .text_size(px(self.font_size))
            .text_color(p.secondary)
            .on_children_prepainted(move |bounds, _, cx| {
                let Some(bounds) = bounds.first() else {
                    return;
                };
                let margin = px(64.);
                let visible = bounds.left() < viewport.width + margin
                    && bounds.top() < viewport.height + margin
                    && bounds.right() > -margin
                    && bounds.bottom() > -margin;
                let _ = weak.update(cx, |shimmer, cx| {
                    if shimmer.in_view != visible {
                        shimmer.in_view = visible;
                        // Resuming from off-screen restarts the sweep so the label
                        // does not reappear mid-pass.
                        shimmer.started_ms = crate::bridge::monotonic_ms();
                        cx.notify();
                    }
                });
            })
            .child(StyledText::new(self.text.clone()).with_highlights(styles))
    }
}
