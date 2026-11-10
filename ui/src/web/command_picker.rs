//! Composer overlay shared by the `/model` and `/permission` commands.

use super::menu_groups::{MenuHeading, MenuViewport, heading_space};
use super::*;

pub(super) struct CommandPicker {
    input: gpui::Entity<Editor>,
    active: usize,
    viewport: MenuViewport,
    token: Option<String>,
    pub(super) settling: bool,
    model_error: Option<String>,
    _subscription: Subscription,
}

impl CommandPicker {
    pub(super) fn new(w: &mut Window, cx: &mut Context<DshApp>) -> Self {
        let input = editor("搜索…".into(), InputMode::Single, w, cx);
        input.update(cx, |editor, _| editor.font_size = 12.);
        let subscription = cx.subscribe_in(&input, w, |this, _, event: &EditEvent, _, cx| {
            if *event == EditEvent::Changed
                && this
                    .composer_menu
                    .is_some_and(ComposerMenu::is_command_picker)
            {
                this.command_picker.active = 0;
                this.command_picker.viewport.reset();
                cx.notify();
            }
        });
        Self {
            input,
            active: 0,
            viewport: MenuViewport::default(),
            token: None,
            settling: false,
            model_error: None,
            _subscription: subscription,
        }
    }

    fn highlight(&mut self, options: &[CommandChoice], active: usize) {
        let mut previous = None;
        let mut group_headers = 0;
        for option in options.iter().take(active + 1) {
            if let Some((group, _)) = option.group.as_ref()
                && previous != Some(*group)
            {
                group_headers += 1;
                previous = Some(*group);
            }
        }
        self.active = active;
        self.viewport.reveal(active + group_headers);
    }
}

struct CommandChoice {
    value: String,
    label: String,
    detail: Option<String>,
    group: Option<(usize, String)>,
    experimental: bool,
    current: bool,
}

impl DshApp {
    pub(super) fn open_command_picker(
        &mut self,
        menu: ComposerMenu,
        from_slash: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer_menu = None;
        replace(&self.command_picker.input, "", w, cx);
        self.command_picker.input.update(cx, |editor, _| {
            editor.set_placeholder(if menu == ComposerMenu::CommandModels {
                "搜索模型…"
            } else {
                "搜索…"
            });
        });
        self.command_picker.token = from_slash.then(|| self.composer.read(cx).value().to_owned());
        self.command_picker.settling = false;
        self.command_picker.model_error = None;
        self.command_picker.viewport.reset();
        self.composer_menu = Some(menu);
        if menu == ComposerMenu::PermissionChoices
            && self.conversation.permission_mode.state.is_none()
        {
            self.request_permission_mode();
        }
        self.highlight_current_command_option(menu, cx);
        self.focus_command_picker(w, cx);
    }

    pub(super) fn highlight_current_command_option(&mut self, menu: ComposerMenu, cx: &App) {
        let options = self.command_options(menu, cx);
        let active = options
            .iter()
            .position(|option| option.current)
            .unwrap_or(0);
        self.command_picker.highlight(&options, active);
    }

    pub(super) fn focus_command_picker(&self, w: &mut Window, cx: &mut Context<Self>) {
        self.command_picker
            .input
            .update(cx, |editor, cx| editor.focus(w, cx));
    }

    fn dismiss_command_picker(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.composer_menu = None;
        self.command_picker.settling = false;
        self.command_picker.token = None;
        self.focus_composer(w, cx);
        cx.notify();
    }

    pub(super) fn finish_command_picker(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.command_picker.token.as_deref() == Some(self.composer.read(cx).value()) {
            self.replace_composer_without_trigger("", w, cx);
        }
        self.dismiss_command_picker(w, cx);
    }

    fn choose_command_option(
        &mut self,
        menu: ComposerMenu,
        value: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if menu == ComposerMenu::PermissionChoices {
            self.choose_permission(value, w, cx);
        } else if !self.conversation.command_model_pending
            && self.conversation.model_menu_pending.is_none()
            && let Some(session) = self.active_session.as_ref()
        {
            self.conversation.command_model_pending = true;
            self.command_picker.settling = true;
            self.command_picker.model_error = None;
            invoke_with_reply(
                "set_config_option",
                "command_model",
                json!({"sessionId": session, "configId": "model", "value": value}),
            );
            cx.notify();
        }
    }

    pub(super) fn finish_command_model(
        &mut self,
        owner: Option<&str>,
        result: Result<Value, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = owner else {
            return;
        };
        let selected = self.active_session.as_deref() == Some(session);
        let picker_open = selected
            && self.composer_menu == Some(ComposerMenu::CommandModels)
            && self.command_picker.settling;
        let Some(conversation) = self.conversation_mut(session) else {
            return;
        };
        conversation.command_model_pending = false;
        match result.and_then(|value| {
            value["configOptions"]
                .as_array()
                .cloned()
                .ok_or_else(|| "无法读取模型选项。".into())
        }) {
            Ok(options) => {
                conversation.config_options = options;
                if selected {
                    self.rebuild_selectors(w, cx);
                    if picker_open {
                        self.finish_command_picker(w, cx);
                    }
                }
            }
            Err(error) if picker_open => self.command_picker.model_error = Some(error),
            Err(error) if selected => self.push_toast(error, cx),
            Err(_) => {}
        }
    }

    pub(super) fn handle_command_picker_key(
        &mut self,
        combo: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(menu) = self.composer_menu.filter(|menu| menu.is_command_picker()) else {
            return false;
        };
        if self.confirm_permission.is_some() {
            return false;
        }
        let options = self.command_options(menu, cx);
        let count = options.len();
        let active = self.command_picker.active.min(count.saturating_sub(1));
        let ready = !self.command_picker_busy(menu) && count > 0;
        match combo {
            "Escape" | "shift+Tab" => self.dismiss_command_picker(w, cx),
            "ArrowDown" | "ArrowUp" if ready => {
                let next = if combo == "ArrowDown" {
                    active + 1
                } else {
                    active + count - 1
                } % count;
                self.command_picker.highlight(&options, next);
                cx.notify();
            }
            "Enter" | "Tab" if ready => {
                self.choose_command_option(menu, options[active].value.clone(), w, cx)
            }
            "Enter" | "ArrowDown" | "ArrowUp" => {}
            _ => return false,
        }
        true
    }

    fn command_picker_busy(&self, menu: ComposerMenu) -> bool {
        if menu == ComposerMenu::PermissionChoices {
            self.conversation.permission_mode.changing()
                || self.conversation.permission_mode.loading()
        } else {
            self.conversation.command_model_pending
                || self.conversation.model_menu_pending.is_some()
        }
    }

    fn command_options(&self, menu: ComposerMenu, cx: &App) -> Vec<CommandChoice> {
        let query = self
            .command_picker
            .input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        if menu == ComposerMenu::CommandModels {
            let current = self
                .config_option("model")
                .and_then(|option| option["currentValue"].as_str());
            let groups = self.filtered_model_groups(&query);
            let ungrouped = groups.len() == 1 && groups[0].0 == "模型";
            return groups
                .into_iter()
                .enumerate()
                .flat_map(|(index, (label, models))| {
                    models.into_iter().map(move |model| CommandChoice {
                        current: current == Some(model.value.as_ref()),
                        value: model.value.to_string(),
                        label: model.label.to_string(),
                        detail: None,
                        group: (!ungrouped).then(|| (index, label.clone())),
                        experimental: false,
                    })
                })
                .collect();
        }
        let Some(state) = self.conversation.permission_mode.state.as_ref() else {
            return Vec::new();
        };
        state
            .options
            .iter()
            .filter(|option| {
                query.is_empty()
                    || option.label().to_lowercase().contains(&query)
                    || option
                        .detail()
                        .is_some_and(|detail| detail.to_lowercase().contains(&query))
            })
            .map(|option| CommandChoice {
                value: option.value.clone(),
                label: option.label().to_owned(),
                detail: option.detail().map(str::to_owned),
                group: None,
                experimental: option.value == "auto",
                current: option.value == state.current_value,
            })
            .collect()
    }

    pub(super) fn render_command_picker(
        &self,
        menu: ComposerMenu,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let mode = &self.conversation.permission_mode;
        let model = menu == ComposerMenu::CommandModels;
        let busy = self.command_picker_busy(menu);
        let options = self.command_options(menu, cx);
        let error = if model {
            self.command_picker.model_error.as_ref()
        } else {
            mode.error.as_ref()
        };
        let status = if !model && mode.loading() {
            Some("正在加载选项…")
        } else if busy {
            Some("正在应用…")
        } else if error.is_none() && options.is_empty() {
            Some(if !model {
                "无选项"
            } else if self.model_choice_groups("model").is_empty() {
                "暂无可用模型"
            } else {
                "没有匹配的模型。"
            })
        } else {
            None
        };
        let active = self
            .command_picker
            .active
            .min(options.len().saturating_sub(1));
        let mut list = column()
            .id("command-picker-options")
            .role(gpui::Role::ListBox)
            .aria_label(if model {
                "模型选项"
            } else {
                "权限选项"
            })
            .w_full()
            .min_h_0()
            .gap_0()
            .overflow_y_scroll()
            .track_scroll(&self.command_picker.viewport.scroll);
        let mut previous = None;
        let mut headings: Vec<MenuHeading> = Vec::new();
        for (index, option) in options.into_iter().enumerate() {
            if let Some((group, label)) = option.group.as_ref()
                && previous != Some(*group)
            {
                list = list.child(heading_space(headings.is_empty()));
                headings.push(MenuHeading {
                    child: index + headings.len(),
                    last_child: 0,
                    label: label.clone(),
                });
                previous = Some(*group);
            }
            let row_index = index + headings.len();
            if option.group.is_some()
                && let Some(heading) = headings.last_mut()
            {
                heading.last_child = row_index;
            }
            let value = option.value;
            list = list.child(
                row()
                    .id(gpui::ElementId::Name(
                        format!("command-choice-{value}").into(),
                    ))
                    .role(gpui::Role::ListBoxOption)
                    .aria_selected(index == active)
                    .w_full()
                    .flex_none()
                    .h(px(28.))
                    .px(px(7.))
                    .gap(px(6.))
                    .rounded(px(12.))
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(p.ink)
                    .when(index == active, |row| row.bg(p.hover))
                    .child(div().min_w_0().truncate().child(option.label))
                    .when(option.experimental, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .text_size(px(8.))
                                .text_color(p.faint)
                                .child("EXP"),
                        )
                    })
                    .children(option.detail.map(|detail| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(p.faint)
                            .child(detail)
                    }))
                    .when(option.current, |row| {
                        row.child(div().ml_auto().child(icon(Glyph::Check, 14., p.ink)))
                    })
                    .when(!busy, |row| {
                        row.cursor_pointer()
                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                if this.command_picker.active != index {
                                    this.command_picker.active = index;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, w, cx| {
                                this.choose_command_option(menu, value.clone(), w, cx)
                            }))
                    }),
            );
        }
        column()
            .w_full()
            .min_h_0()
            .gap_0()
            .child(
                div()
                    .mx(px(2.))
                    .mt(px(2.))
                    .mb(px(3.))
                    .px(px(7.))
                    .h(px(28.))
                    .flex_none()
                    .child(self.command_picker.input.clone()),
            )
            .children(status.map(|text| {
                div()
                    .px(px(8.))
                    .py(px(7.))
                    .text_size(px(12.))
                    .text_color(p.faint)
                    .child(text)
            }))
            .children(error.map(|error| {
                row()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(5.))
                    .text_size(px(12.))
                    .text_color(p.danger)
                    .child(div().flex_1().min_w_0().truncate().child(error.clone()))
                    .when(!model && mode.state.is_none(), |row| {
                        row.child(
                            Button::new("reload-permissions")
                                .label("重试")
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.request_permission_mode();
                                    cx.notify();
                                }))
                                .into_themed(cx),
                        )
                    })
            }))
            .child(self.command_picker.viewport.render(list, headings))
    }
}
