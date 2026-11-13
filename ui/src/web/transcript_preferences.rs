//! Work-step preferences in the General settings page.

use super::menu_surface::{menu_elevation, menu_material};
use super::*;
use gpui::{Bounds, Div, FocusHandle, Pixels, canvas, svg};
use std::{cell::Cell, rc::Rc};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Preference {
    Display,
    Timing,
    Replies,
    /// Official LinkOpeningRow: Chat HTTP(S) links open in the sidebar browser or
    /// the system default browser.
    Links,
}

impl Preference {
    fn labels(self) -> (&'static str, &'static str, &'static [&'static str]) {
        match self {
            Self::Display => (
                "工作步骤展示",
                "选择希望看到多少工具调用细节",
                &["简洁", "标准", "详细", "完全展开"],
            ),
            Self::Timing => (
                "工作步骤收起时机",
                "选择何时自动收起工作步骤",
                &["回答结束后", "下次有新消息时"],
            ),
            Self::Replies => (
                "收起后显示的回复",
                "工作步骤收起后，保留哪些助手回复",
                &["全部回复", "仅最终回答"],
            ),
            Self::Links => (
                "网页链接默认打开方式",
                "对话中网页链接的打开位置",
                &["应用内侧边栏", "默认浏览器"],
            ),
        }
    }

    fn selected(self, app: &DshApp) -> usize {
        match self {
            Self::Display => match app.work_details {
                WorkDetailsMode::Compact => 0,
                WorkDetailsMode::Standard => 1,
                WorkDetailsMode::Detailed => 2,
                WorkDetailsMode::Verbose => 3,
            },
            Self::Timing => usize::from(app.collapse_next_input),
            Self::Replies => usize::from(!app.keep_folded_replies),
            Self::Links => usize::from(app.link_open_external),
        }
    }
}

struct Trigger {
    focus: FocusHandle,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

struct OpenMenu {
    field: Preference,
    rows: Vec<FocusHandle>,
    walked: Option<usize>,
}

pub(super) struct TranscriptPreferences {
    triggers: [Trigger; 4],
    menu: Option<OpenMenu>,
}

impl TranscriptPreferences {
    pub(super) fn new(cx: &App) -> Self {
        Self {
            triggers: std::array::from_fn(|_| Trigger {
                focus: cx.focus_handle().tab_stop(true),
                bounds: Rc::default(),
            }),
            menu: None,
        }
    }

    pub(super) fn clear_menu(&mut self) {
        self.menu = None;
    }

    fn trigger(&self, field: Preference) -> &Trigger {
        &self.triggers[field as usize]
    }
}

impl DshApp {
    pub(super) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.transcript_preferences.clear_menu();
        self.plugin_center.close_inventory();
        self.show_settings = false;
        cx.notify();
    }

    fn toggle_transcript_menu(
        &mut self,
        field: Preference,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ui = &mut self.transcript_preferences;
        if ui.menu.as_ref().is_some_and(|menu| menu.field == field) {
            ui.clear_menu();
        } else {
            ui.menu = Some(OpenMenu {
                field,
                rows: field.labels().2.iter().map(|_| cx.focus_handle()).collect(),
                walked: None,
            });
        }
        ui.trigger(field).focus.focus(w, cx);
        cx.notify();
    }

    fn choose_transcript_preference(
        &mut self,
        index: usize,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.transcript_preferences.menu.take() else {
            return;
        };
        let field = menu.field;
        self.transcript_preferences
            .trigger(field)
            .focus
            .focus(w, cx);
        if index != field.selected(self) {
            match field {
                Preference::Display => {
                    self.work_details = [
                        WorkDetailsMode::Compact,
                        WorkDetailsMode::Standard,
                        WorkDetailsMode::Detailed,
                        WorkDetailsMode::Verbose,
                    ][index];
                }
                Preference::Timing => self.collapse_next_input = index == 1,
                Preference::Replies => self.keep_folded_replies = index == 0,
                Preference::Links => self.link_open_external = index == 1,
            }
            self.save_workspace_preferences(cx);
        }
        cx.notify();
    }

    pub(super) fn transcript_menu_key(
        &mut self,
        combo: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.settings_page != SettingsPage::General {
            return false;
        }
        let ui = &mut self.transcript_preferences;
        let Some(menu) = &ui.menu else { return false };
        let trigger = ui.trigger(menu.field).focus.clone();
        let focused = menu.rows.iter().position(|row| row.is_focused(w));
        let owns_focus = focused.is_some() || trigger.is_focused(w);
        if combo == "Escape" || (combo == "shift+Tab" && owns_focus) {
            ui.clear_menu();
            if owns_focus {
                trigger.focus(w, cx);
            }
            cx.notify();
            return true;
        }
        if !owns_focus {
            return false;
        }
        if combo == "Tab"
            && let Some(index) = focused
        {
            self.choose_transcript_preference(index, w, cx);
            return true;
        }
        let menu = ui.menu.as_mut().unwrap();
        let count = menu.rows.len();
        let from = focused.or(menu.walked);
        let next = match combo {
            "Tab" | "Home" => 0,
            "End" => count - 1,
            "ArrowDown" => from.map_or(0, |index| (index + 1) % count),
            "ArrowUp" => from.map_or(count - 1, |index| (index + count - 1) % count),
            _ => return false,
        };
        menu.walked = Some(next);
        menu.rows[next].focus(w, cx);
        cx.notify();
        true
    }

    pub(super) fn render_transcript_preferences(&self, w: &Window, cx: &mut Context<Self>) -> Div {
        let fields = [
            Preference::Display,
            Preference::Timing,
            Preference::Replies,
            Preference::Links,
        ];
        column().w_full().gap_0().children(
            fields
                .into_iter()
                .filter(|field| {
                    *field != Preference::Replies || self.work_details != WorkDetailsMode::Verbose
                })
                .map(|field| self.render_transcript_preference(field, w, cx)),
        )
    }

    fn render_transcript_preference(
        &self,
        field: Preference,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = dsh(cx);
        let (title, description, options) = field.labels();
        let selected = options[field.selected(self)];
        let trigger = self.transcript_preferences.trigger(field);
        let bounds = trigger.bounds.clone();
        let open = self
            .transcript_preferences
            .menu
            .as_ref()
            .is_some_and(|menu| menu.field == field);
        let gutter = if bridge::is_mac() || w.viewport_size().width >= px(768.) {
            48.
        } else {
            16.
        };
        row()
            .w_full()
            .items_center()
            .gap(px(8.))
            .py(px(16.))
            .border_b(px(0.5))
            .border_color(p.border_l2)
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .pr(px(gutter))
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .line_height(px(22.))
                            .text_color(p.label_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(p.label_tertiary)
                            .child(description),
                    ),
            )
            .child(dock_action_button(
                row()
                    .id(("transcript-preference", field as usize))
                    .relative()
                    .role(gpui::Role::Button)
                    .aria_label(format!("{title}，{selected}"))
                    .aria_expanded(open)
                    .track_focus(&trigger.focus)
                    .flex_none()
                    .h(px(36.))
                    .px(px(14.))
                    .gap(px(12.))
                    .rounded(px(12.))
                    .bg(p.bg_module_platform)
                    .hover(|style| style.bg(p.hover))
                    .when(
                        trigger.focus.is_focused(w) && bridge::keyboard_focus_visible(),
                        |row| {
                            row.shadow(vec![
                                gpui::BoxShadow::new(px(0.), px(0.), p.state_business_primary)
                                    .spread_radius(px(2.)),
                            ])
                        },
                    )
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .text_color(p.label_primary)
                    .cursor_pointer()
                    .child(selected)
                    .child(
                        svg()
                            .data(include_bytes!("../../assets/header-chevron.svg"))
                            .size(px(14.))
                            .text_color(p.label_primary),
                    )
                    .child(
                        canvas(
                            move |measured, w, _| {
                                let old = bounds.replace(Some(measured));
                                if open && old != Some(measured) {
                                    w.request_animation_frame();
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    ),
                cx.listener(move |app, _, w, cx| app.toggle_transcript_menu(field, w, cx)),
            ))
    }

    pub(super) fn render_transcript_preference_menu(
        &self,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        let menu = self.transcript_preferences.menu.as_ref()?;
        let field = menu.field;
        let bounds = self.transcript_preferences.trigger(field).bounds.clone();
        let anchor = bounds.get()?;
        let options = field.labels().2;
        let selected = field.selected(self);
        let viewport = w.viewport_size();
        let margin = px(12.);
        let gap = px(4.);
        let available_width = viewport.width - margin * 2.;
        let available_height = viewport.height - margin * 2.;
        if available_width <= px(0.) || available_height <= px(0.) {
            return None;
        }
        let width = px(144.).min(available_width);
        let height = px(8. + 34. * options.len() as f32).min(available_height);
        let left = (anchor.right() - width)
            .max(margin)
            .min(viewport.width - margin - width);
        let below = (anchor.bottom() + gap).max(margin);
        let above = anchor.top() - gap - height;
        let top = if below + height <= viewport.height - margin {
            below
        } else if above >= margin {
            above.min(viewport.height - margin - height)
        } else {
            below.clamp(margin, (viewport.height - margin - height).max(margin))
        };
        let p = dsh(cx);
        Some(
            column()
                .id("transcript-preference-menu")
                .role(gpui::Role::Menu)
                .aria_label(field.labels().0)
                .absolute()
                .left(left)
                .top(top)
                .w(width)
                .h(height)
                .p(px(4.))
                .gap_0()
                .rounded(px(16.))
                .child(menu_material(p, 16.))
                .shadow(menu_elevation(p))
                .occlude()
                .on_mouse_down_out(
                    cx.listener(move |app, event: &gpui::MouseDownEvent, _, cx| {
                        if !bounds
                            .get()
                            .is_some_and(|anchor| anchor.contains(&event.position))
                        {
                            app.transcript_preferences.clear_menu();
                            cx.notify();
                        }
                    }),
                )
                .child(
                    column()
                        .id("transcript-preference-menu-rows")
                        .gap_0()
                        .min_h_0()
                        .w_full()
                        .flex_1()
                        .overflow_y_scroll()
                        .children(options.iter().zip(&menu.rows).enumerate().map(
                            |(index, (&label, focus))| {
                                dock_action_button(
                                    row()
                                        .id(("transcript-option", index))
                                        .role(gpui::Role::MenuItem)
                                        .aria_label(label)
                                        .aria_selected(index == selected)
                                        .track_focus(focus)
                                        .w_full()
                                        .h(px(34.))
                                        .flex_none()
                                        .px(px(8.))
                                        .gap(px(6.))
                                        .rounded(px(12.))
                                        .text_size(px(13.))
                                        .line_height(px(20.))
                                        .text_color(p.label_primary)
                                        .cursor_pointer()
                                        .hover(|style| style.bg(p.hover))
                                        .focus_visible(|style| style.bg(p.hover))
                                        .when(
                                            focus.is_focused(w) && bridge::keyboard_focus_visible(),
                                            |row| row.bg(p.hover),
                                        )
                                        .child(div().flex_1().min_w_0().truncate().child(label))
                                        .when(index == selected, |row| {
                                            row.child(
                                                svg()
                                                    .data(include_bytes!(
                                                        "../../assets/question-check.svg"
                                                    ))
                                                    .size(px(14.))
                                                    .text_color(p.label_primary),
                                            )
                                        }),
                                    cx.listener(move |app, _, w, cx| {
                                        app.choose_transcript_preference(index, w, cx)
                                    }),
                                )
                            },
                        )),
                ),
        )
    }
}
