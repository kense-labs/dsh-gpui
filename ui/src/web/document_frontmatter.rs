//! The Markdown preview's leading YAML metadata.

use super::{
    document_selection::{SelectionLayout, normalize_line_endings},
    *,
};
use crate::markdown::frontmatter_bounds;
use gpui::{Hsla, Pixels, TextStyle};
use serde::Deserialize;
use std::{cell::Cell, cell::OnceCell, ops::Range, rc::Rc, rc::Weak};

#[wasm_bindgen(raw_module = "../document-frontmatter.js")]
extern "C" {
    #[wasm_bindgen(js_name = parseFrontmatter)]
    fn parse_fields(source: &str) -> js_sys::Promise;
}

#[derive(Deserialize)]
struct Field {
    key: String,
    value: String,
    block: bool,
}

#[derive(Default)]
pub(super) struct ParsedFrontmatter {
    requested: Cell<bool>,
    fields: OnceCell<Vec<Field>>,
}

impl ParsedFrontmatter {
    pub(super) fn finish(&self, result: JsValue) -> Option<usize> {
        let fields: Vec<Field> = serde_wasm_bindgen::from_value(result).ok()?;
        let mut texts = fields
            .iter()
            .flat_map(|field| [&field.key, &field.value])
            .filter(|text| !text.is_empty());
        let len = texts.next().map_or(0, |first| {
            normalize_line_endings(first).len()
                + texts
                    .map(|text| normalize_line_endings(text).len() + 1)
                    .sum::<usize>()
        });
        self.fields.set(fields).ok()?;
        Some(len)
    }
}

pub(super) struct Frontmatter {
    source: Range<usize>,
    body_start: usize,
    parsed: Rc<ParsedFrontmatter>,
}

impl Frontmatter {
    /// Reuse field parsing when pagination only appends to the Markdown body.
    pub(super) fn update(current: &mut Option<Self>, text: &str) -> bool {
        let Some((source, body_start)) = frontmatter_bounds(text) else {
            return current.take().is_some();
        };
        if let Some(current) = current.as_mut().filter(|current| current.source == source) {
            current.body_start = body_start;
            return false;
        }
        *current = Some(Self {
            source,
            body_start,
            parsed: Rc::default(),
        });
        true
    }

    pub(super) fn body<'a>(&self, document: &'a str) -> &'a str {
        &document[self.body_start..]
    }

    pub(super) fn request(
        &self,
        document: &str,
    ) -> Option<(js_sys::Promise, Weak<ParsedFrontmatter>, usize)> {
        if self.parsed.requested.replace(true) {
            return None;
        }
        let source = &document[self.source.clone()];
        Some((
            parse_fields(source),
            Rc::downgrade(&self.parsed),
            normalize_line_endings(source).len(),
        ))
    }

    pub(super) fn render(
        &self,
        document: &str,
        selection: &SelectionLayout,
        w: &Window,
        cx: &App,
    ) -> Option<gpui::Div> {
        let fields = self.parsed.fields.get();
        if fields.is_some_and(Vec::is_empty) {
            return None;
        }
        let p = dsh(cx);
        let mono = if bridge::is_mac() {
            "Menlo"
        } else {
            "monospace"
        };
        // CSS keeps a half-point border at Retina scale and a one-pixel border at 1x.
        let scale = w.scale_factor();
        let border = px((0.5 * scale).floor().max(1.) / scale);
        let box_ = column()
            .w_full()
            .min_w_0()
            .mb(px(16.))
            .px(px(12.))
            .py(px(10.))
            .border(border)
            .border_color(p.border_l2)
            .rounded(px(crate::tokens::Tokens::RADIUS_MD))
            .bg(p.bg_layer_1)
            .when_some(super::document_fonts::family(12., cx), |box_, family| {
                box_.font_family(family)
            })
            .text_size(px(12.))
            .line_height(px(20.))
            .text_color(p.label_primary);
        let Some(fields) = fields else {
            let source = &document[self.source.clone()];
            return Some(box_.child(cell(source, selection).font_family(mono)));
        };

        let width = key_width(fields, mono, p.label_secondary, w);
        Some(box_.gap(px(4.)).children(fields.iter().map(|field| {
            row()
                .w_full()
                .min_w_0()
                .items_start()
                .gap(px(16.))
                .child(
                    cell(&field.key, selection)
                        .w(width)
                        .flex_none()
                        .whitespace_nowrap()
                        .font_family(mono)
                        .text_color(p.label_secondary),
                )
                .child(
                    cell(&field.value, selection)
                        .flex_1()
                        .min_w_0()
                        .when(field.block, |value| value.font_family(mono)),
                )
        })))
    }
}

fn cell(text: &str, selection: &SelectionLayout) -> gpui::Div {
    div().min_w_0().when(!text.is_empty(), |cell| {
        cell.child(selection.authored_text(text, "\n"))
    })
}

fn key_width(fields: &[Field], font: &str, color: Hsla, w: &Window) -> Pixels {
    let style = TextStyle {
        font_family: font.to_owned().into(),
        color,
        ..Default::default()
    };
    let system = w.text_system();
    fields.iter().fold(px(0.), |width, field| {
        normalize_line_endings(&field.key)
            .split('\n')
            .fold(width, |width, line| {
                width.max(
                    system
                        .shape_line(
                            line.to_owned().into(),
                            px(12.),
                            &[style.to_run(line.len())],
                            None,
                        )
                        .width(),
                )
            })
    })
}
