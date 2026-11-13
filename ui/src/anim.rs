//! DeepSeek Harness 官方动效常量与通用动画元素。
//! 时长/缓动严格对应官方 token：
//!   菜单进入 40ms ease-out、菜单退场 80ms ease-in、标准过渡 200ms ease-in-out(.4,0,.2,1)、
//!   布局过渡 300ms、鲸鱼游动 1.6s ease-in-out 循环、待命圆点 1s ease-in-out alternate 呼吸。
//! GPUI 的 with_animation 自带 prefers-reduced-motion 降级，与官方行为一致。

use gpui::{
    Animation, AnimationExt, Div, ElementId, IntoElement, div, ease_in_out, prelude::*, quadratic,
};
use std::time::Duration;

/// 菜单/浮层进入（--dsh-menu-enter）。
pub const MENU_ENTER: Duration = Duration::from_millis(40);
/// 菜单/浮层退场（--dsh-menu-exit）。
pub const MENU_EXIT: Duration = Duration::from_millis(80);
/// Context menus wait for the action catalogue before entering.
pub(crate) const CONTEXT_MENU_PREPARE_MS: f64 = 20.;
pub(crate) const CONTEXT_MENU_REPLACE_MS: f64 = 10.;

pub(crate) fn context_menu_enter_opacity(elapsed_ms: f64) -> f32 {
    let progress = (elapsed_ms / MENU_ENTER.as_millis() as f64).clamp(0., 1.);
    crate::right_dock::dock_easing(progress, 0., 0.58) as f32
}

pub(crate) fn context_menu_exit_opacity(elapsed_ms: f64, duration_ms: f64) -> f32 {
    let progress = (elapsed_ms / duration_ms).clamp(0., 1.);
    1. - crate::right_dock::dock_easing(progress, 0.42, 1.) as f32
}

/// 模态等标准过渡（--ds-transition-duration）。
pub const STANDARD: Duration = Duration::from_millis(200);
/// 面板/侧栏布局过渡（--ds-transition-duration-slow）。
pub const SLOW: Duration = Duration::from_millis(300);
/// 鲸鱼游动循环（hero-fish-swim）。
pub const FISH_SWIM: Duration = Duration::from_millis(1600);
/// 待命圆点呼吸（input-pending）。
pub const PENDING: Duration = Duration::from_millis(1000);

/// Toast 时序，对应官方 `Toast.tsx` 的 `HOLD_MS` / `FADE_MS` 与样式表里的
/// `dsh-toast-in 160ms`。淡出起点是 3s，因此总时长 4s。
pub const TOAST_IN_SECONDS: f32 = 0.16;
pub const TOAST_HOLD_SECONDS: f32 = 3.;
pub const TOAST_FADE_SECONDS: f32 = 1.;
pub const TOAST_TOTAL_SECONDS: f32 = TOAST_HOLD_SECONDS + TOAST_FADE_SECONDS;

/// How far a toast has travelled through its life, as `(opacity, rise_px)`.
///
/// Mirrors the official animation pair `dsh-toast-in 160ms ease-out` and
/// `dsh-toast-fade 1000ms ease` delayed by the hold (Toast.module.css). The
/// 6px entry offset is the same in the official keyframes.
pub fn toast_frame(age_seconds: f32) -> (f32, f32) {
    if age_seconds < TOAST_IN_SECONDS {
        let t = (age_seconds / TOAST_IN_SECONDS).clamp(0., 1.);
        (t, -6. * (1. - t))
    } else if age_seconds < TOAST_HOLD_SECONDS {
        (1., 0.)
    } else {
        let t = ((age_seconds - TOAST_HOLD_SECONDS) / TOAST_FADE_SECONDS).clamp(0., 1.);
        (1. - t, 0.)
    }
}

/// 菜单进入淡入：opacity 0→1，40ms ease-out。
/// 以零尺寸包裹层承担透明度动画（对 absolute 浮层无布局影响）。
pub fn menu_in<E: IntoElement + 'static>(
    id: impl Into<ElementId>,
    el: E,
) -> gpui::AnimationElement<Div> {
    div().child(el).with_animation(
        id,
        Animation::new(MENU_ENTER).with_easing(quadratic),
        |el, t| el.opacity(t.clamp(0., 1.)),
    )
}

/// 菜单退场淡出：opacity 1→0，80ms ease-in（二次方反向）。
pub fn menu_out<E: IntoElement + 'static>(
    id: impl Into<ElementId>,
    el: E,
) -> gpui::AnimationElement<Div> {
    div().child(el).with_animation(
        id,
        Animation::new(MENU_EXIT).with_easing(quadratic),
        |el, t| el.opacity((1. - t).clamp(0., 1.)),
    )
}

/// 模态进入淡入：opacity 0→1，200ms ease-in-out（遮罩与对话框同用）。
pub fn modal_in<E: IntoElement + 'static>(
    id: impl Into<ElementId>,
    el: E,
) -> gpui::AnimationElement<Div> {
    div().absolute().inset_0().child(el).with_animation(
        id,
        Animation::new(STANDARD).with_easing(ease_in_out),
        |el, t| el.opacity(t.clamp(0., 1.)),
    )
}

/// 待命圆点呼吸：opacity 0.35↔1，1s alternate。
pub fn pending_dot(id: impl Into<ElementId>, el: Div) -> gpui::AnimationElement<Div> {
    el.with_animation(
        id,
        Animation::new(PENDING)
            .repeat()
            .with_easing(gpui::pulsating_between(0.35, 1.0)),
        |el, t| el.opacity(t.clamp(0.35, 1.0)),
    )
}

/// 官方 hero-fish-swim 关键帧在相位 t∈[0,1] 的变换：
/// 0%/100% 无变换；35% rotate(-4deg) translate(-0.4,-0.9)；70% rotate(1.6deg) translate(0.3,0.2)。
pub fn fish_swim_phase(t: f32) -> (f32, f32, f32) {
    // 分段线性插值：(rotate_deg, dx, dy)
    let k1 = (t / 0.35).clamp(0., 1.);
    let (r1, x1, y1) = (-4. * k1, -0.4 * k1, -0.9 * k1);
    let k2 = ((t - 0.35) / 0.35).clamp(0., 1.);
    let (r2, x2, y2) = (
        r1 + (1.6 - r1) * k2,
        x1 + (0.3 - x1) * k2,
        y1 + (0.2 - y1) * k2,
    );
    let k3 = ((t - 0.70) / 0.30).clamp(0., 1.);
    (r2 * (1. - k3), x2 * (1. - k3), y2 * (1. - k3))
}

/// Official TextShimmer sweep phase for a label that has been active for
/// `age` seconds. Mirrors the official 1.5s cycle: hold at the left, sweep
/// linearly, then pause parked at the left.
///
/// This lives here rather than beside its renderer because the live process
/// title needs it on both the host and wasm targets, and it is pure arithmetic
/// worth testing directly.
pub const SHIMMER_HOLD_SECONDS: f32 = 0.12;
/// Idle pause after each sweep, as a fraction of the sweep duration.
pub const SHIMMER_PAUSE_RATIO: f32 = 0.32;

pub fn shimmer_sweep(age: f32, duration: f32) -> f32 {
    let duration = duration.max(0.1);
    let time = age - SHIMMER_HOLD_SECONDS;
    if time <= 0. {
        return 0.;
    }
    let local = time % (duration * (1. + SHIMMER_PAUSE_RATIO));
    if local < duration {
        local / duration
    } else {
        0.
    }
}

/// Whether a live activity label should keep requesting animation frames.
///
/// Every condition matters for power: an `active` label that is off-screen, in a
/// hidden window, or under reduced-motion must not ask for frames. Dropping the
/// viewport check is enough to pin the WebView's renderer at full CPU while the
/// transcript sits idle, because the entity keeps animating a highlight nobody
/// can see.
pub fn shimmer_should_animate(
    active: bool,
    window_visible: bool,
    in_view: bool,
    reduced_motion: bool,
) -> bool {
    active && window_visible && in_view && !reduced_motion
}

/// Source falloff stops for the moving highlight, in units of the band.
/// Mirrors the official `linear-gradient(105deg, transparent 0%, black 40% 60%,
/// transparent 100%)`.
pub fn shimmer_falloff(offset: f32) -> f32 {
    const STOPS: [(f32, f32); 7] = [
        (-1., 0.),
        (-0.5, 0.5),
        (-0.2, 0.9),
        (0., 1.),
        (0.2, 0.9),
        (0.5, 0.5),
        (1., 0.),
    ];
    if offset <= STOPS[0].0 || offset >= STOPS[6].0 {
        return 0.;
    }
    for pair in STOPS.windows(2) {
        let (start, start_value) = pair[0];
        let (end, end_value) = pair[1];
        if offset >= start && offset <= end {
            let span = (end - start).max(0.0001);
            return start_value + (end_value - start_value) * (offset - start) / span;
        }
    }
    0.
}

#[cfg(test)]
mod tests {
    use super::{
        SHIMMER_PAUSE_RATIO, TOAST_HOLD_SECONDS, TOAST_IN_SECONDS, TOAST_TOTAL_SECONDS,
        shimmer_falloff, shimmer_should_animate, shimmer_sweep, toast_frame,
    };

    #[test]
    fn toast_slides_in_holds_then_fades_on_the_official_clock() {
        // Starts transparent, 6px high, and settles to full opacity at rest.
        let (o0, r0) = toast_frame(0.);
        assert_eq!((o0, r0), (0., -6.));
        let (o1, r1) = toast_frame(TOAST_IN_SECONDS);
        assert!((o1 - 1.).abs() < 0.001);
        assert_eq!(r1, 0.);

        // Full opacity for the whole hold, positioned at rest.
        assert_eq!(toast_frame(1.5), (1., 0.));
        assert_eq!(toast_frame(TOAST_HOLD_SECONDS - 0.001), (1., 0.));

        // The fade starts at the hold and finishes exactly at the total.
        assert!((toast_frame(TOAST_HOLD_SECONDS + 0.5).0 - 0.5).abs() < 0.001);
        assert_eq!(toast_frame(TOAST_TOTAL_SECONDS).0, 0.);
        // Total is the official 3s hold plus the 1s fade.
        assert_eq!(TOAST_TOTAL_SECONDS, 4.);
    }

    #[test]
    fn a_label_animates_only_while_it_is_visible_and_active() {
        assert!(shimmer_should_animate(true, true, true, false));
        // Settled: nothing to draw.
        assert!(!shimmer_should_animate(false, true, true, false));
        // Scrolled out of the viewport.
        assert!(!shimmer_should_animate(true, true, false, false));
        // Host window hidden.
        assert!(!shimmer_should_animate(true, false, true, false));
        // Reduced motion is honoured.
        assert!(!shimmer_should_animate(true, true, true, true));
    }

    #[test]
    fn sweep_holds_then_moves_then_pauses() {
        // Holds at the left for the first 0.12s.
        assert_eq!(shimmer_sweep(0., 1.5), 0.);
        assert_eq!(shimmer_sweep(0.12, 1.5), 0.);
        // Linear sweep across the duration.
        assert!((shimmer_sweep(0.12 + 0.75, 1.5) - 0.5).abs() < 0.0001);
        // Then parked at the left during the pause, and repeating.
        let cycle = 1.5 * (1. + SHIMMER_PAUSE_RATIO);
        assert_eq!(shimmer_sweep(0.12 + 1.5 + 0.1, 1.5), 0.);
        assert!((shimmer_sweep(0.12 + cycle + 0.75, 1.5) - 0.5).abs() < 0.0001);
    }

    #[test]
    fn sweep_advances_through_one_pass_then_resets_for_the_pause() {
        // Strictly inside the sweep the highlight only moves forward. The loop
        // stops just short of `hold + duration`, where the value returns to 0 to
        // begin the pause.
        let mut previous = -1.;
        for step in 0..30 {
            let age = 0.12 + 1.5 * (step as f32 / 30.);
            let t = shimmer_sweep(age, 1.5);
            assert!(t >= previous, "sweep went backwards at age {age}");
            previous = t;
        }
        assert!(previous > 0.9, "sweep should nearly finish its pass");
        // Past the sweep it parks back at the left for the pause, then the next
        // cycle sweeps again. One cycle is `duration * (1 + pause ratio)` after
        // the initial hold.
        assert_eq!(shimmer_sweep(0.12 + 1.6, 1.5), 0.);
        assert!((shimmer_sweep(0.12 + 1.5 * 1.32 + 0.75, 1.5) - 0.5).abs() < 0.0001);
    }

    #[test]
    fn falloff_peaks_at_the_centre_and_vanishes_at_the_edges() {
        assert_eq!(shimmer_falloff(0.), 1.);
        assert_eq!(shimmer_falloff(0.2), 0.9);
        assert_eq!(shimmer_falloff(-0.5), 0.5);
        assert_eq!(shimmer_falloff(1.), 0.);
        assert_eq!(shimmer_falloff(-1.), 0.);
        // Symmetric around the centre.
        assert!((shimmer_falloff(0.35) - shimmer_falloff(-0.35)).abs() < 0.0001);
    }
}

/// The shared StateDot ring. Sidebar instances start at document time zero.
#[cfg(target_family = "wasm")]
pub(crate) fn state_spinner(edge: f32, started: f64, ink: gpui::Hsla) -> impl IntoElement {
    use gpui::px;
    gpui::canvas(
        |_, _, _| (),
        move |bounds, (), w, cx| {
            let radius = px(9.5 * edge / 24.);
            let center = bounds.center();
            let point_on_ring =
                |angle: f32| center + gpui::point(radius * angle.cos(), radius * angle.sin());
            let radii = gpui::point(radius, radius);
            let stroke = px(2. * edge / 24.);
            let mut track = gpui::PathBuilder::stroke(stroke);
            track.move_to(point_on_ring(0.));
            track.arc_to(
                radii,
                px(0.),
                false,
                true,
                point_on_ring(std::f32::consts::PI),
            );
            track.arc_to(radii, px(0.), false, true, point_on_ring(0.));
            track.close();
            if let Ok(track) = track.build() {
                w.paint_path(track, ink.opacity(0.25));
            }

            let (phase, dash) = if cx.reduce_motion() {
                (0., 0.5)
            } else {
                w.request_animation_frame();
                let phase = ((crate::bridge::monotonic_ms() - started).max(0.) / 1500.).fract();
                let t = 1. - (1. - phase * 2.).abs();
                (
                    phase,
                    crate::right_dock::cubic_bezier_easing(t, [0.42, 0., 0.58, 1.]) as f32,
                )
            };
            let begin = phase as f32 * std::f32::consts::TAU + dash * 6. / 9.5;
            let end = begin + (12. + dash * 12.) / 9.5;
            let mut arc = gpui::PathBuilder::stroke(stroke);
            arc.move_to(point_on_ring(begin));
            arc.arc_to(radii, px(0.), false, true, point_on_ring(end));
            if let Ok(arc) = arc.build() {
                w.paint_path(arc, ink);
            }
            for endpoint in [point_on_ring(begin), point_on_ring(end)] {
                let half = stroke / 2.;
                w.paint_quad(
                    gpui::fill(
                        gpui::Bounds::new(
                            endpoint - gpui::point(half, half),
                            gpui::size(stroke, stroke),
                        ),
                        ink,
                    )
                    .corner_radii(half),
                );
            }
        },
    )
    .size(px(edge))
}
