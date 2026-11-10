//! Scroll viewports with the same bounded sticky headings as Harness MenuGroup.

use super::*;
use gpui::{AvailableSpace, Pixels, canvas, point, size};

#[derive(Default)]
pub(super) struct MenuViewport {
    pub(super) scroll: ScrollHandle,
    reveal: Rc<Cell<Option<usize>>>,
}

impl MenuViewport {
    pub(super) fn reset(&self) {
        self.reveal.set(None);
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    pub(super) fn reveal(&self, child: usize) {
        self.reveal.set(Some(child));
        self.scroll.scroll_to_item(child);
    }

    pub(super) fn render(&self, list: impl IntoElement, headings: Vec<MenuHeading>) -> gpui::Div {
        let scroll = self.scroll.clone();
        let reveal = self.reveal.clone();
        column()
            .relative()
            .w_full()
            .min_h_0()
            .gap_0()
            .overflow_hidden()
            .child(list)
            .child(
                canvas(
                    move |_, w, cx| {
                        let viewport = scroll.bounds();
                        if viewport.size.height <= px(0.) {
                            return Vec::new();
                        }
                        let offset = scroll.offset();
                        // On first mount ScrollHandle has no previous viewport to
                        // scroll against. Resolve that request once layout is known.
                        if let Some(child) = reveal.take()
                            && let Some(item) = scroll.bounds_for_item(child)
                        {
                            let y = if item.top() + offset.y < viewport.top() {
                                viewport.top() - item.top()
                            } else if item.bottom() + offset.y > viewport.bottom() {
                                viewport.bottom() - item.bottom()
                            } else {
                                offset.y
                            }
                            .clamp(-scroll.max_offset().y, px(0.));
                            if y != offset.y {
                                scroll.set_offset(point(offset.x, y));
                                w.request_animation_frame();
                            }
                        }
                        let p = dsh(cx);
                        headings
                            .into_iter()
                            .filter_map(|heading| {
                                let natural_top =
                                    scroll.bounds_for_item(heading.child)?.top() + offset.y;
                                let section_bottom =
                                    scroll.bounds_for_item(heading.last_child)?.bottom() + offset.y;
                                let (top, stuck) = heading_position(
                                    natural_top,
                                    section_bottom,
                                    viewport.top(),
                                    viewport.bottom(),
                                )?;
                                let mut label = div()
                                    .w(viewport.size.width)
                                    .h(px(22.))
                                    .px(px(7.))
                                    .pt(px(4.))
                                    .text_size(px(11.))
                                    .line_height(px(16.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(p.label_tertiary)
                                    .truncate()
                                    .when(stuck, |label| {
                                        label
                                            .bg(p.menu_group_header.opacity(0.94))
                                            .block_mouse_except_scroll()
                                    })
                                    .child(heading.label)
                                    .into_any_element();
                                label.prepaint_as_root(
                                    point(viewport.left(), top),
                                    size(
                                        AvailableSpace::Definite(viewport.size.width),
                                        AvailableSpace::MinContent,
                                    ),
                                    w,
                                    cx,
                                );
                                Some(label)
                            })
                            .collect::<Vec<_>>()
                    },
                    |_, mut labels, w, cx| {
                        for label in &mut labels {
                            label.paint(w, cx);
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
    }
}

pub(super) struct MenuHeading {
    pub(super) child: usize,
    pub(super) last_child: usize,
    pub(super) label: String,
}

pub(super) fn heading_space(first: bool) -> gpui::Div {
    div()
        .flex_none()
        .h(px(22.))
        .when(!first, |heading| heading.mt(px(3.)))
}

fn heading_position(
    natural_top: Pixels,
    section_bottom: Pixels,
    viewport_top: Pixels,
    viewport_bottom: Pixels,
) -> Option<(Pixels, bool)> {
    let top = natural_top.max(viewport_top).min(section_bottom - px(22.));
    (top + px(22.) > viewport_top && top < viewport_bottom)
        .then_some((top, natural_top < viewport_top))
}
