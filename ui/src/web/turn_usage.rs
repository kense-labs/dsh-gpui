//! Per-turn token usage: the official TurnUsagePanel action and its dialog.
//!
//! Official turn actions carry a database pill (`用量 {total}`) that opens a
//! details dialog: 本轮用量 with the exact total plus provider/model routes, cache
//! hit, uncached input, cached input, cache write and output buckets. The buckets
//! come from the session log per turn, so this module only renders them.

use super::menu_surface::{menu_elevation, menu_material};
use super::*;
use gpui::{TextAlign, svg};

const PANEL_RADIUS: f32 = 16.;

/// State of the open per-turn usage dialog.
pub(crate) struct TurnUsageDialog {
    pub(super) turn: u64,
    /// Where the pill sits (the dialog opens above it).
    pub(super) anchor: ComposerAnchor,
}

impl DshApp {
    /// Dismiss the open dialog; returns whether one was open.
    pub(super) fn dismiss_turn_usage(&mut self, cx: &mut Context<Self>) -> bool {
        if self.turn_usage_dialog.take().is_none() {
            return false;
        }
        cx.notify();
        true
    }

    /// Button that opens the dialog for the turn a closing answer belongs to.
    fn render_turn_usage_action(
        &self,
        message: &str,
        turn: u64,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let Some(usage) = self.conversation.turn_usage.get(&turn) else {
            return div();
        };
        let label = format!("用量 {}", crate::process::format_tokens(usage.total_tokens));
        let anchor: ComposerAnchor = Rc::new(std::cell::Cell::new(None));
        let open = self
            .turn_usage_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.turn == turn);
        let mut button = row()
            .id(gpui::ElementId::Name(
                format!("turn-usage-{turn}-{message}").into(),
            ))
            .role(gpui::Role::Button)
            .aria_label(format!("本轮用量 {label}"))
            .aria_expanded(open)
            .tab_index(0)
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(6.))
            .h(px(24.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(if open {
                p.label_secondary
            } else {
                p.label_tertiary
            })
            .when(open, |button| button.bg(p.hover))
            .hover(|style| style.bg(p.hover).text_color(p.label_secondary))
            .child(
                svg()
                    .data(include_bytes!("../../assets/stats-database-outline.svg") as &[u8])
                    .size(px(14.))
                    .text_color(p.label_tertiary),
            )
            .child(div().min_w_0().truncate().child(label));
        // Anchor the dialog from the click itself: a measuring canvas would be an
        // extra absolute child over the pill for no benefit.
        button = button.on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
            let position = event.position();
            anchor.set(Some(gpui::Bounds::new(
                gpui::point(position.x - px(20.), position.y - px(14.)),
                gpui::size(px(40.), px(28.)),
            )));
            let already = this
                .turn_usage_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.turn == turn);
            if already {
                this.turn_usage_dialog = None;
            } else {
                this.turn_usage_dialog = Some(TurnUsageDialog {
                    turn,
                    anchor: anchor.clone(),
                });
            }
            cx.notify();
        }));
        div().child(button)
    }

    /// Append the usage action (and its open dialog) to one turn's actions row.
    pub(super) fn turn_usage_actions(
        &self,
        message: &str,
        entry: &ChatEntry,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let turn = self
            .conversation
            .turn_tails
            .answer_turn(message)
            .or_else(|| self.conversation.turn_number_for(&entry.key))?;
        if !self.conversation.turn_usage.contains_key(&turn) {
            return None;
        }
        Some(self.render_turn_usage_action(message, turn, cx))
    }

    /// Root-level popover for the open per-turn usage dialog.
    ///
    /// The pill lives inside the scrolled transcript, so the dialog is painted by
    /// the page overlay (anchored above the pill's last measured bounds) instead
    /// of deferring a draw from clipped content.
    pub(super) fn render_turn_usage_overlay(
        &self,
        viewport_width: f32,
        viewport_height: f32,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let dialog = self.turn_usage_dialog.as_ref()?;
        let usage = self.conversation.turn_usage.get(&dialog.turn)?.clone();
        let anchor = dialog.anchor.get()?;
        let p = dsh(cx);
        let mut rows: Vec<(&'static str, String)> = Vec::new();
        if !usage.routes.is_empty() {
            rows.push(("提供方 / 模型", usage.routes.join(", ")));
        }
        if let Some(percent) = usage.cache_hit_percent() {
            rows.push(("缓存命中", format!("{percent}%")));
        }
        rows.push(("未缓存输入", exact_tokens(usage.input_tokens)));
        rows.push(("缓存读取", exact_tokens(usage.cache_read_tokens)));
        if usage.cache_write_tokens != 0 {
            rows.push(("缓存写入", exact_tokens(usage.cache_write_tokens)));
        }
        rows.push(("输出", exact_tokens(usage.output_tokens)));
        let left = f32::from(anchor.left()).min((viewport_width - 8. - 320.).max(8.));
        let panel = column()
            .id("turn-usage-dialog")
            .role(gpui::Role::Dialog)
            .aria_label("本轮用量")
            .absolute()
            .left(px(left))
            .bottom(px(viewport_height - f32::from(anchor.top()) + 8.))
            .w(px(320.))
            .occlude()
            .gap_0()
            .p(px(16.))
            .rounded(px(PANEL_RADIUS))
            .shadow(menu_elevation(p))
            .text_size(px(12.))
            .line_height(px(18.))
            .cursor(gpui::CursorStyle::Arrow)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(menu_material(p, PANEL_RADIUS))
            .child(
                row()
                    .w_full()
                    .justify_between()
                    .gap(px(16.))
                    .mb(px(8.))
                    .text_color(p.label_primary)
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        row()
                            .min_w_0()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                svg()
                                    .data(include_bytes!("../../assets/stats-database-outline.svg")
                                        as &[u8])
                                    .size(px(14.))
                                    .text_color(p.label_primary),
                            )
                            .child("本轮用量"),
                    )
                    .child(
                        div()
                            .text_align(TextAlign::Right)
                            .child(format!("{} tok", exact_tokens(usage.total_tokens))),
                    ),
            )
            .child(
                div()
                    .mb(px(10.))
                    .border_t(px(0.5))
                    .border_color(p.border_l2),
            )
            .child(column().w_full().gap(px(6.)).children(rows.into_iter().map(
                |(label, value)| {
                    row()
                        .w_full()
                        .justify_between()
                        .items_start()
                        .gap(px(16.))
                        .child(div().min_w_0().text_color(p.label_tertiary).child(label))
                        .child(
                            div()
                                .min_w_0()
                                .text_align(TextAlign::Right)
                                .text_color(p.label_secondary)
                                .child(value),
                        )
                },
            )));
        // The overlay covers the window so any outside press dismisses the panel;
        // the panel above stops propagation.
        let overlay = div()
            .id("turn-usage-overlay")
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("turn-usage-cover")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.dismiss_turn_usage(cx);
                        }),
                    ),
            )
            .child(panel);
        Some(overlay.into_any_element())
    }
}

fn exact_tokens(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}
