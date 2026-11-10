//! Kense artwork shared by the KWork welcome and workspace.

use gpui::{Div, FontWeight, Image, ImageFormat, Img, div, img, prelude::*, px};
use std::sync::{Arc, OnceLock};

pub(crate) fn mark(size: f32, dark: bool) -> Img {
    static DARK: OnceLock<Arc<Image>> = OnceLock::new();
    static LIGHT: OnceLock<Arc<Image>> = OnceLock::new();
    let (cache, artwork): (_, &[u8]) = if dark {
        (&DARK, include_bytes!("../assets/kense-mark-dark.png"))
    } else {
        (&LIGHT, include_bytes!("../assets/kense-mark-light.png"))
    };
    let image =
        cache.get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Png, artwork.to_vec())));
    img(image.clone())
        .size(px(size))
        .flex_none()
        .object_fit(gpui::ObjectFit::Contain)
}

pub(crate) fn wordmark(height: f32, text_size: f32, dark: bool) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(height * 0.2))
        .h(px(height))
        .child(mark(height, dark))
        .child(
            div()
                .font_family("Montserrat")
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(text_size))
                .line_height(px(height))
                .child("KWork"),
        )
}
