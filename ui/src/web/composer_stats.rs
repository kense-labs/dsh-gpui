//! Session statistics and their trigger-anchored composer popovers.

use super::menu_surface::{MenuCover, menu_elevation, menu_material};
use super::*;
use crate::process::{format_cache_hit_percent, format_tokens, format_tokens_per_second};
use gpui::{
    AvailableSpace, Bounds, DispatchPhase, MouseDownEvent, TextAlign, canvas, point, size, svg,
};

const PANEL_RADIUS: f32 = 16.;

struct StatReading {
    kind: StatsDialog,
    label: String,
    headline: String,
    rows: Vec<(&'static str, String)>,
}

impl StatsDialog {
    fn title(self) -> &'static str {
        match self {
            Self::Activity => "会话统计",
            Self::Usage => "Token 用量",
        }
    }

    fn icon(self, color: gpui::Hsla) -> gpui::Svg {
        let data: &[u8] = match self {
            Self::Activity => include_bytes!("../../assets/stats-gauge-outline.svg"),
            Self::Usage => include_bytes!("../../assets/stats-database-outline.svg"),
        };
        svg().data(data).size(px(14.)).flex_none().text_color(color)
    }
}

impl DshApp {
    fn activity_reading(&self) -> Option<StatReading> {
        let conversation = &self.conversation;
        let steps = if conversation.stats_steps > 0 {
            conversation.stats_steps
        } else {
            conversation
                .entries
                .iter()
                .filter(|entry| entry.role == EntryRole::Assistant)
                .count() as u64
        };
        if steps == 0 {
            return None;
        }
        let turns = if conversation.stats_turns > 0 {
            conversation.stats_turns
        } else {
            conversation
                .entries
                .iter()
                .filter(|entry| entry.role == EntryRole::User && entry.title.is_empty())
                .count() as u64
        };
        let headline = format!("{turns} 轮 {steps} 步");
        let mut rows = Vec::new();
        if conversation.stats_llm_ms > 0. {
            rows.push(("模型用时", format_stat_duration(conversation.stats_llm_ms)));
        }
        if conversation.stats_tool_ms > 0. {
            rows.push((
                "工具调用用时",
                format_stat_duration(conversation.stats_tool_ms),
            ));
        }
        if conversation.stats_ttft_steps > 0 {
            let avg = conversation.stats_ttft_ms / conversation.stats_ttft_steps as f64;
            rows.push(("首 token 平均（TTFT）", format_stat_duration(avg)));
        }
        let label = if conversation.decode_ms > 0. && conversation.decode_tokens > 0 {
            let speed = format!(
                "{} tok/s",
                format_tokens_per_second(
                    conversation.decode_tokens as f64 / (conversation.decode_ms / 1000.),
                )
            );
            let label = format!("{headline} · {speed}");
            rows.push(("输出速度（TPS）", speed));
            label
        } else {
            headline.clone()
        };
        Some(StatReading {
            kind: StatsDialog::Activity,
            label,
            headline,
            rows,
        })
    }

    fn usage_reading(&self) -> Option<StatReading> {
        let conversation = &self.conversation;
        let prompt = conversation.uncached_input_tokens
            + conversation.cache_read_tokens
            + conversation.cache_write_tokens;
        let billed = prompt + conversation.output_tokens;
        if billed == 0 {
            return None;
        }
        let mut rows = Vec::new();
        let mut label = format!("{} tok", format_tokens(billed));
        if let Some(percent) = format_cache_hit_percent(conversation.cache_read_tokens, prompt) {
            rows.push(("缓存命中", format!("{percent}%")));
            label.push_str(&format!(" · 缓存命中 {percent}%"));
        }
        rows.push(("输入", exact_tokens(conversation.uncached_input_tokens)));
        rows.push(("缓存读取", exact_tokens(conversation.cache_read_tokens)));
        if conversation.cache_write_tokens > 0 {
            rows.push(("缓存写入", exact_tokens(conversation.cache_write_tokens)));
        }
        rows.push(("输出", exact_tokens(conversation.output_tokens)));
        Some(StatReading {
            kind: StatsDialog::Usage,
            label,
            headline: format!("{} tok", exact_tokens(billed)),
            rows,
        })
    }

    pub(super) fn dismiss_stats_dialog(&mut self, cx: &mut Context<Self>) -> bool {
        if self.stats_dialog.take().is_none() {
            return false;
        }
        cx.notify();
        true
    }

    pub(super) fn stats_dialog_cover(&self) -> Option<MenuCover> {
        self.stats_dialog_bounds
            .get()
            .map(|bounds| MenuCover::measured(bounds, PANEL_RADIUS, false))
    }

    pub(super) fn render_composer_stats(
        &self,
        usage: Option<(u64, u64)>,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = dsh(cx);
        let delta = self.content_font_delta as f32;
        let mut pills = row()
            .relative()
            .id("composer-stats-row")
            .w_full()
            .min_h(px(28.))
            .px(px(4.))
            .gap(px(4.))
            .items_center()
            .justify_center();
        for reading in [self.activity_reading(), self.usage_reading()]
            .into_iter()
            .flatten()
        {
            let kind = reading.kind;
            let interactive = !reading.rows.is_empty();
            let open = interactive && self.stats_dialog == Some(kind);
            let color = if open {
                p.label_secondary
            } else {
                p.label_tertiary
            };
            let mut pill = row()
                .id(match kind {
                    StatsDialog::Activity => "activity-stat-pill",
                    StatsDialog::Usage => "usage-stat-pill",
                })
                .relative()
                .min_w_0()
                .items_center()
                .gap(px(6.))
                .px(px(8.))
                .py(px(1.))
                .rounded(px(8.))
                .text_size(px(12. + delta))
                .line_height(px(20. + delta))
                .text_color(color)
                .when(open, |pill| pill.bg(p.hover))
                .child(kind.icon(color))
                .child(div().min_w_0().truncate().child(reading.label.clone()));
            if interactive {
                pill = super::dock_action_button(
                    pill.role(gpui::Role::Button)
                        .aria_label(reading.label.clone())
                        .aria_expanded(open)
                        .tab_index(0)
                        .tab_stop(true)
                        .cursor_pointer()
                        .hover(|style| style.bg(p.hover).text_color(p.label_secondary)),
                    cx.listener(move |app, _, _, cx| {
                        app.stats_dialog = (app.stats_dialog != Some(kind)).then_some(kind);
                        cx.notify();
                    }),
                );
            }
            if open {
                let anchor = ComposerAnchor::default();
                let measured = anchor.clone();
                pill = pill.child(
                    canvas(
                        move |bounds, _, _| measured.set(Some(bounds)),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                );
                pills = pills
                    .child(pill)
                    .child(self.render_stats_panel(reading, anchor, w, cx));
            } else {
                pills = pills.child(pill);
            }
        }
        if let Some((used, limit)) = usage {
            let ratio = if limit > 0 {
                (used as f32 / limit as f32).clamp(0., 1.)
            } else {
                0.
            };
            pills = pills
                .child(context_meter_ring(ratio, p.label_secondary))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(p.label_tertiary)
                        .child(format!("{}%", (ratio * 100.).round() as u32)),
                );
        }
        pills
    }

    fn render_stats_panel(
        &self,
        reading: StatReading,
        anchor: ComposerAnchor,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = dsh(cx);
        let kind = reading.kind;
        let margin = px(12.);
        let top = px(if bridge::is_mac() {
            if self.shell_fullscreen(w) { 20. } else { 68. }
        } else {
            12.
        });
        let owner = cx.weak_entity();
        let cover = self.stats_dialog_bounds.clone();
        let mut card = column()
            .id(match kind {
                StatsDialog::Activity => "activity-stat-dialog",
                StatsDialog::Usage => "usage-stat-dialog",
            })
            .role(gpui::Role::Dialog)
            .aria_label(kind.title())
            .relative()
            .w_full()
            .occlude()
            .gap_0()
            .p(px(16.))
            .rounded(px(PANEL_RADIUS))
            .shadow(menu_elevation(p))
            .text_size(px(12.))
            .line_height(px(18.))
            .cursor(gpui::CursorStyle::Arrow)
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
                            .child(kind.icon(p.label_primary))
                            .child(kind.title()),
                    )
                    .child(div().text_align(TextAlign::Right).child(reading.headline)),
            )
            .child(
                div()
                    .mb(px(10.))
                    .border_t(px(0.5))
                    .border_color(p.border_l2),
            )
            .child(
                column()
                    .w_full()
                    .gap(px(6.))
                    .children(reading.rows.into_iter().map(move |(label, value)| {
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
                    })),
            );
        canvas(
            move |_, w, cx| {
                let anchor = anchor.get()?;
                let viewport = w.viewport_size();
                let gap = px(8.);
                let top = top.min(anchor.bottom() + gap).max(margin);
                let max_width = viewport.width - margin * 2.;
                let max_height = viewport.height - margin - top;
                if max_width <= px(0.) || max_height <= px(0.) {
                    return None;
                }
                card = card
                    .min_w(px(300.).min(max_width))
                    .max_w(px(440.).min(max_width))
                    .max_h(max_height)
                    .overflow_y_scroll();
                let mut card = card.into_any_element();
                let measured = card.layout_as_root(
                    size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                    w,
                    cx,
                );
                // Resolve percentage rows against the chosen width so the
                // heading and value columns extend to the panel's right inset.
                let measured = card.layout_as_root(
                    size(
                        AvailableSpace::Definite(measured.width),
                        AvailableSpace::MaxContent,
                    ),
                    w,
                    cx,
                );
                let x = anchor
                    .left()
                    .max(margin)
                    .min(viewport.width - margin - measured.width);
                let y = (anchor.top() - gap - measured.height)
                    .max(top)
                    .min(viewport.height - margin - measured.height);
                let origin = point(x, y);
                let bounds = Bounds::new(origin, measured);
                cover.set(Some(bounds));
                super::document_html::cover_all(MenuCover::measured(bounds, PANEL_RADIUS, false));
                w.defer_draw(card, origin, 2, None);
                Some((anchor, bounds))
            },
            move |_, placement, w, _| {
                let Some((anchor, panel)) = placement else {
                    return;
                };
                w.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture
                        && !anchor.contains(&event.position)
                        && !panel.contains(&event.position)
                        && let Some(owner) = owner.upgrade()
                    {
                        owner.update(cx, |app, cx| {
                            app.dismiss_stats_dialog(cx);
                        });
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}

fn exact_tokens(value: u64) -> String {
    let digits = value.to_string();
    let mut count = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            count.push(',');
        }
        count.push(digit);
    }
    count
}

/// Official `formatDuration`: under a minute → `45.2秒`, else `2分42秒`.
fn format_stat_duration(ms: f64) -> String {
    let seconds = ms / 1_000.;
    if seconds < 60. {
        let rounded = (seconds * 10.).round() / 10.;
        return format!("{rounded}秒");
    }
    let whole = seconds.round() as u64;
    format!("{}分{}秒", whole / 60, whole % 60)
}

fn context_meter_ring(ratio: f32, color: gpui::Hsla) -> gpui::Svg {
    let radius = 5.5f32;
    let circumference = 2. * std::f32::consts::PI * radius;
    let dash = (ratio.clamp(0., 1.) * circumference).max(if ratio > 0. { 1.5 } else { 0. });
    let data = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 14 14\">\
         <circle cx=\"7\" cy=\"7\" r=\"{radius}\" fill=\"none\" stroke=\"currentColor\" stroke-opacity=\"0.25\" stroke-width=\"1.6\"/>\
         <circle cx=\"7\" cy=\"7\" r=\"{radius}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.6\" stroke-linecap=\"round\" stroke-dasharray=\"{dash:.2} {circumference:.2}\" transform=\"rotate(-90 7 7)\"/>\
         </svg>"
    );
    svg()
        .data(data.as_bytes())
        .size(px(14.))
        .flex_none()
        .text_color(color)
}
