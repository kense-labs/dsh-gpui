//! The macOS system text face and optical sizes used by document headings.

use super::*;
use gpui::{FontFallbacks, Global, SharedString};
use js_sys::{Array, Uint8Array};
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen(raw_module = "../document-fonts.js")]
extern "C" {
    #[wasm_bindgen(js_name = prepareDocumentFonts)]
    fn prepare_document_fonts(source: Uint8Array) -> js_sys::Promise;
}

struct DocumentFonts {
    loaded: bool,
    chinese: Option<FontFallbacks>,
}

impl Global for DocumentFonts {}

pub(super) fn prepare(cx: &mut Context<DshApp>) {
    if !bridge::is_mac() || cx.try_global::<DocumentFonts>().is_some() {
        return;
    }
    cx.set_global(DocumentFonts {
        loaded: false,
        chinese: None,
    });
    cx.spawn(async move |this, cx| {
        let Some(source) = bridge::font_data("document_system_font").await else {
            return;
        };
        let Ok(result) = JsFuture::from(prepare_document_fonts(source)).await else {
            return;
        };
        let fonts = Array::from(&result)
            .iter()
            .map(|value| Uint8Array::new(&value).to_vec().into())
            .collect();
        let _ = this.update(cx, |_, cx| {
            let system = cx.text_system();
            if system.add_fonts(fonts).is_ok()
                && system
                    .all_font_names()
                    .iter()
                    .any(|name| name == "DSH System 17")
            {
                cx.global_mut::<DocumentFonts>().loaded = true;
                cx.refresh_windows();
                cx.notify();
            }
        });
    })
    .detach();
    cx.spawn(async move |this, cx| {
        let Some(source) = bridge::font_data("chinese_system_font").await else {
            return;
        };
        let _ = this.update(cx, |_, cx| {
            let system = cx.text_system();
            if system.add_fonts(vec![source.to_vec().into()]).is_ok()
                && system
                    .all_font_names()
                    .iter()
                    .any(|name| name == "PingFang SC")
            {
                cx.global_mut::<DocumentFonts>().chinese = Some(FontFallbacks::from_fonts(vec![
                    "PingFang SC".into(),
                    "KUI Input Sans".into(),
                ]));
                cx.refresh_windows();
                cx.notify();
            }
        });
    })
    .detach();
}

pub(crate) fn interface_family(cx: &App) -> Option<SharedString> {
    family(17., cx)
}

pub(super) fn interface_fallbacks(cx: &App) -> Option<FontFallbacks> {
    cx.try_global::<DocumentFonts>()?.chinese.clone()
}

pub(super) fn interface_style(w: &Window, cx: &App) -> gpui::TextStyle {
    let mut style = w.text_style();
    if let Some(family) = interface_family(cx) {
        style.font_family = family;
    }
    if let Some(fallbacks) = interface_fallbacks(cx) {
        style.font_fallbacks = Some(fallbacks);
    }
    style
}

pub(super) fn family(size: f32, cx: &App) -> Option<SharedString> {
    cx.try_global::<DocumentFonts>()?.loaded.then(|| {
        let optical = size.round().clamp(17., 25.) as u8;
        format!("DSH System {optical}").into()
    })
}
