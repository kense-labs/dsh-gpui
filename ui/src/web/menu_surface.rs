//! The material shared by Harness-style menus.

use crate::{
    bridge,
    right_dock::FloatRect,
    tokens::{Tokens, elevation_prominent},
};
use gpui::{Bounds, BoxShadow, Corners, IntoElement, Pixels, Styled, canvas, px};

#[derive(Clone, Copy)]
pub(super) struct MenuCover {
    pub(super) rect: FloatRect,
    pub(super) radius: f32,
    pub(super) visual_only: bool,
}

impl MenuCover {
    pub(super) fn measured(bounds: Bounds<Pixels>, radius: f32, visual_only: bool) -> Self {
        Self {
            rect: FloatRect {
                x: bounds.origin.x.into(),
                y: bounds.origin.y.into(),
                width: bounds.size.width.into(),
                height: bounds.size.height.into(),
            },
            radius,
            visual_only,
        }
    }
}

pub(super) fn menu_material(tokens: Tokens, radius: f32) -> impl IntoElement {
    let base = bridge::has_mac_window_material().then_some(tokens.bg_base);
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let corners = Corners::from(px(radius)).clamp_radii_for_quad_size(bounds.size);
            let effect = gpui_wgpu::backdrop_quad(
                bounds,
                corners,
                tokens.menu_surface_fill,
                base,
                40. * window.scale_factor(),
            );
            window.paint_layer(bounds, |window| window.paint_quad(effect));
        },
    )
    .absolute()
    .inset_0()
}

pub(super) fn menu_elevation(tokens: Tokens) -> Vec<BoxShadow> {
    let mut shadows = elevation_prominent();
    for shadow in &mut shadows {
        // CSS box-shadow's blur radius is twice the Gaussian standard deviation.
        shadow.blur_radius *= 0.5;
        // Harness's `--dsw-elevation-prominent` is two almost invisible glows
        // (alpha .04/.05) plus a 0.5px hairline stroke. GPUI renders the
        // exterior-only variant far more densely than CSS, so the raw alpha
        // reads as a heavy dark halo around the menu. Compensate here while
        // keeping the official geometry and stroke.
        shadow.color.a *= 0.4;
    }
    gpui_wgpu::clip_backdrop_shadows(&mut shadows);
    // CSS paints the first listed shadow on top.
    shadows.reverse();
    let mut stroke = gpui_wgpu::backdrop_stroke(px(0.5), tokens.menu_stroke);
    stroke.color.a *= 0.5;
    shadows.push(stroke);
    shadows
}
