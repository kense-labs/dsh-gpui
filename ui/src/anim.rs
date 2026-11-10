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
