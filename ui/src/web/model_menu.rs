//! Model button menu navigation across its root, model, and effort panes.

use super::menu_groups::MenuViewport;
use super::*;

pub(super) struct ModelMenu {
    pub(super) scope: gpui::FocusHandle,
    pub(super) trigger: gpui::FocusHandle,
    pub(super) row: gpui::FocusHandle,
    pub(super) active: Option<usize>,
    pub(super) query: String,
    pub(super) viewport: MenuViewport,
    pub(super) chevron: DockSlide,
    pub(super) selection_focus: bool,
    _focus_subscriptions: [Subscription; 2],
}

impl ModelMenu {
    pub(super) fn new(w: &mut Window, cx: &mut Context<DshApp>) -> Self {
        let scope = cx.focus_handle();
        let trigger = cx.focus_handle();
        let leave_menu = cx.on_focus_out(&scope, w, |app, _, _, cx| {
            if app.composer_menu.is_some_and(ComposerMenu::is_model) {
                app.composer_menu = None;
                app.model_menu.active = None;
                cx.notify();
            }
        });
        let leave_trigger = cx.on_blur(&trigger, w, |app, _, cx| {
            if std::mem::take(&mut app.model_menu.selection_focus) {
                cx.notify();
            }
        });
        Self {
            scope,
            trigger,
            row: cx.focus_handle(),
            active: None,
            query: String::new(),
            viewport: MenuViewport::default(),
            chevron: DockSlide::with_curve(120., [0.25, 0.1, 0.25, 1.]),
            selection_focus: false,
            _focus_subscriptions: [leave_menu, leave_trigger],
        }
    }
}

impl DshApp {
    pub(super) fn model_menu_width(
        &self,
        pane: ComposerMenu,
        model: &str,
        w: &Window,
        cx: &App,
    ) -> gpui::Pixels {
        let style = super::document_fonts::interface_style(w, cx);
        let width = |label: &str, font_size: f32| {
            w.text_system()
                .shape_line(
                    label.to_owned().into(),
                    px(font_size),
                    &[style.to_run(label.len())],
                    None,
                )
                .width()
        };
        let content = if pane == ComposerMenu::Model {
            let model = width("模型", 13.) + width(model, 13.);
            let effort = self
                .current_config_label("reasoning_effort")
                .map_or(px(0.), |label| width("思考强度", 13.) + width(&label, 13.));
            model.max(effort) + px(40.)
        } else {
            self.model_menu_groups(pane, cx)
                .into_iter()
                .map(|(group, choices)| {
                    let group_width = width(&group, 11.) + px(14.);
                    choices
                        .into_iter()
                        .map(|choice| width(&choice.label, 13.) + px(34.))
                        .fold(group_width, gpui::Pixels::max)
                })
                .fold(px(0.), gpui::Pixels::max)
        };
        content + px(8.)
    }

    pub(super) fn config_option(&self, id: &str) -> Option<&Value> {
        self.conversation
            .config_options
            .iter()
            .find(|option| option["id"].as_str() == Some(id))
    }

    pub(super) fn model_menu_searchable(&self) -> bool {
        self.model_choice_groups("model")
            .iter()
            .map(|(_, choices)| choices.len())
            .sum::<usize>()
            > 4
    }

    pub(super) fn filtered_model_groups(&self, query: &str) -> Vec<(String, Vec<PickerOption>)> {
        let query = query.trim().to_lowercase();
        self.model_choice_groups("model")
            .into_iter()
            .filter_map(|(name, choices)| {
                let mut matches = choices
                    .into_iter()
                    .filter_map(|choice| {
                        crate::commands::fuzzy_name_rank(&query, &choice.label)
                            .map(|rank| (rank, choice))
                    })
                    .collect::<Vec<_>>();
                matches.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
                (!matches.is_empty()).then(|| {
                    (
                        name,
                        matches.into_iter().map(|(_, choice)| choice).collect(),
                    )
                })
            })
            .collect()
    }

    pub(super) fn model_menu_groups(
        &self,
        pane: ComposerMenu,
        cx: &App,
    ) -> Vec<(String, Vec<PickerOption>)> {
        if pane == ComposerMenu::ModelChoices {
            let input = self.model_filter_editor.read(cx);
            let query = if self.model_menu_searchable() {
                input.value()
            } else {
                ""
            };
            self.filtered_model_groups(query)
        } else {
            self.model_choice_groups("reasoning_effort")
        }
    }

    pub(super) fn toggle_model_menu(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.composer_menu = if self.composer_menu.is_some_and(ComposerMenu::is_model) {
            None
        } else {
            Some(ComposerMenu::Model)
        };
        self.model_menu.active = None;
        self.model_menu.selection_focus = false;
        self.model_menu.trigger.focus(w, cx);
        cx.notify();
    }

    pub(super) fn drill_model_menu(
        &mut self,
        pane: ComposerMenu,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if pane == ComposerMenu::ModelChoices {
            self.model_menu.query.clear();
            replace(&self.model_filter_editor, "", w, cx);
        }
        self.composer_menu = Some(pane);
        self.model_menu.viewport.reset();
        let groups = self.model_menu_groups(pane, cx);
        let index = self.current_model_menu_index(pane, &groups);
        self.highlight_model_menu(pane, &groups, index, w, cx);
    }

    fn current_model_menu_index(
        &self,
        pane: ComposerMenu,
        groups: &[(String, Vec<PickerOption>)],
    ) -> usize {
        if pane == ComposerMenu::Model {
            return 0;
        }
        let current = self
            .config_option(config_id(pane))
            .and_then(|option| option["currentValue"].as_str());
        groups
            .iter()
            .flat_map(|(_, choices)| choices)
            .position(|choice| Some(choice.value.as_ref()) == current)
            .unwrap_or(0)
    }

    fn back_model_menu(&mut self, pane: ComposerMenu, w: &mut Window, cx: &mut Context<Self>) {
        if pane == ComposerMenu::Model {
            self.composer_menu = None;
            self.model_menu.active = None;
            self.model_menu.trigger.focus(w, cx);
        } else {
            self.composer_menu = Some(ComposerMenu::Model);
            self.model_menu.active = Some(usize::from(pane == ComposerMenu::ReasoningChoices));
            self.model_menu.row.focus(w, cx);
        }
        cx.notify();
    }

    fn highlight_model_menu(
        &mut self,
        pane: ComposerMenu,
        groups: &[(String, Vec<PickerOption>)],
        index: usize,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.model_menu.active = Some(index);
        if pane != ComposerMenu::Model {
            let mut skipped = 0;
            let mut headers = 0;
            for (name, choices) in groups {
                headers += usize::from(groups.len() > 1 || name != "模型");
                skipped += choices.len();
                if index < skipped {
                    self.model_menu.viewport.reveal(index + headers);
                    break;
                }
            }
        }
        if pane == ComposerMenu::ModelChoices && self.model_menu_searchable() {
            self.model_filter_editor
                .update(cx, |editor, cx| editor.focus(w, cx));
        } else {
            self.model_menu.row.focus(w, cx);
        }
        cx.notify();
    }

    pub(super) fn hover_model_choice(
        &mut self,
        pane: ComposerMenu,
        index: usize,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.model_menu_busy() && self.model_menu.active != Some(index) {
            self.model_menu.active = Some(index);
            if pane != ComposerMenu::ModelChoices || !self.model_menu_searchable() {
                self.model_menu.row.focus(w, cx);
            }
            cx.notify();
        }
    }

    pub(super) fn choose_model_menu(
        &mut self,
        pane: ComposerMenu,
        value: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.model_menu_busy() {
            return;
        }
        let id = config_id(pane);
        let current = self
            .config_option(id)
            .and_then(|option| option["currentValue"].as_str());
        if current == Some(value.as_str()) {
            self.model_menu.selection_focus = true;
            self.back_model_menu(ComposerMenu::Model, w, cx);
        } else if let Some(session) = self.active_session.as_ref() {
            let args = json!({"sessionId": session, "configId": id, "value": value});
            self.conversation.model_menu_pending = Some((pane, value));
            self.model_menu.selection_focus = true;
            self.model_menu.trigger.focus(w, cx);
            invoke_with_reply("set_config_option", "button_model", args);
            cx.notify();
        }
    }

    pub(super) fn model_menu_busy(&self) -> bool {
        self.conversation.model_menu_pending.is_some() || self.conversation.command_model_pending
    }

    pub(super) fn finish_model_menu(
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
        let Some(conversation) = self.conversation_mut(session) else {
            return;
        };
        let Some((pane, _)) = conversation.model_menu_pending.take() else {
            return;
        };
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
                    if self.composer_menu.is_some_and(ComposerMenu::is_model) {
                        self.back_model_menu(ComposerMenu::Model, w, cx);
                    }
                }
            }
            Err(error) if selected => {
                if self.composer_menu == Some(pane)
                    && pane == ComposerMenu::ModelChoices
                    && self.model_menu_searchable()
                {
                    self.model_filter_editor
                        .update(cx, |editor, cx| editor.focus(w, cx));
                }
                self.push_toast(format!("模型操作失败：{error}"), cx);
            }
            Err(_) => {}
        }
    }

    pub(super) fn handle_model_menu_key(
        &mut self,
        combo: &str,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self.composer_menu.filter(|pane| pane.is_model()) else {
            return false;
        };
        if !self.model_menu.scope.contains_focused(w, cx) {
            return false;
        }
        if matches!(combo, "Escape" | "shift+Tab") {
            self.back_model_menu(pane, w, cx);
            return true;
        }
        if !matches!(combo, "ArrowDown" | "ArrowUp" | "Enter" | "Tab") {
            return false;
        }
        if self.model_menu_busy() {
            return true;
        }
        let groups = if pane == ComposerMenu::Model {
            Vec::new()
        } else {
            self.model_menu_groups(pane, cx)
        };
        let count = if pane == ComposerMenu::Model {
            1 + usize::from(self.current_config_label("reasoning_effort").is_some())
        } else {
            groups.iter().map(|(_, choices)| choices.len()).sum()
        };
        let in_pane = self.model_menu.row.is_focused(w)
            || (pane == ComposerMenu::ModelChoices
                && self.model_filter_editor.read(cx).focus.is_focused(w));
        let active = self
            .model_menu
            .active
            .filter(|index| in_pane && *index < count);
        match combo {
            "ArrowDown" | "ArrowUp" if count > 0 => {
                let next = if combo == "ArrowDown" {
                    active.map_or(0, |index| (index + 1) % count)
                } else {
                    active.map_or(count - 1, |index| (index + count - 1) % count)
                };
                self.highlight_model_menu(pane, &groups, next, w, cx);
            }
            "Tab" if active.is_none() && count > 0 => {
                let index = self.current_model_menu_index(pane, &groups);
                self.highlight_model_menu(pane, &groups, index, w, cx);
            }
            "Enter" | "Tab" if pane == ComposerMenu::Model && active.is_some() => {
                let target = if active == Some(0) {
                    ComposerMenu::ModelChoices
                } else {
                    ComposerMenu::ReasoningChoices
                };
                self.drill_model_menu(target, w, cx);
            }
            "Enter" | "Tab" if active.is_some() => {
                if let Some(choice) = groups
                    .into_iter()
                    .flat_map(|(_, choices)| choices)
                    .nth(active.unwrap())
                {
                    self.choose_model_menu(pane, choice.value.to_string(), w, cx);
                }
            }
            "ArrowDown" | "ArrowUp" => {}
            "Enter" if pane != ComposerMenu::Model => {}
            _ => return false,
        }
        true
    }
}

pub(super) fn config_id(pane: ComposerMenu) -> &'static str {
    if pane == ComposerMenu::ModelChoices {
        "model"
    } else {
        "reasoning_effort"
    }
}
