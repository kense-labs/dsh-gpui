//! OpenAI-compatible provider configuration over the official Host adapter.

use super::{permission_dialog::dialog_action, *};
use gpui::{AnyElement, Entity, Focusable, Subscription};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
enum Protocol {
    #[default]
    #[serde(rename = "openai-completions")]
    Chat,
    #[serde(rename = "openai-responses")]
    Responses,
}

impl Protocol {
    fn name(self) -> &'static str {
        match self {
            Self::Chat => "Chat Completions",
            Self::Responses => "Responses",
        }
    }
}

struct Preset {
    name: &'static str,
    keywords: &'static str,
    url: &'static str,
    protocol: Protocol,
}

const PRESETS: &[Preset] = &[
    Preset {
        name: "硅基流动",
        keywords: "siliconflow",
        url: "https://api.siliconflow.cn/v1",
        protocol: Protocol::Chat,
    },
    Preset {
        name: "深度求索",
        keywords: "deepseek",
        url: "https://api.deepseek.com",
        protocol: Protocol::Chat,
    },
    Preset {
        name: "OpenRouter",
        keywords: "",
        url: "https://openrouter.ai/api/v1",
        protocol: Protocol::Chat,
    },
    Preset {
        name: "OpenAI",
        keywords: "",
        url: "https://api.openai.com/v1",
        protocol: Protocol::Responses,
    },
    Preset {
        name: "Ollama",
        keywords: "本地",
        url: "http://localhost:11434/v1",
        protocol: Protocol::Chat,
    },
];

/// One provider's mark: local brand artwork, or a glyph when the brand is unknown.
#[derive(Clone, Copy)]
enum ProviderMark {
    Logo(&'static [u8]),
    Glyph(Glyph),
}

/// Resolve the mark for a configured provider (see `crate::provider_marks`).
fn provider_mark(name: &str, base_url: &str) -> ProviderMark {
    crate::provider_marks::provider_logo(name, base_url)
        .map(ProviderMark::Logo)
        .unwrap_or(ProviderMark::Glyph(Glyph::Globe))
}

/// Decoded provider marks, keyed by the static asset address.
///
/// `Image::from_bytes` decodes the PNG, so every frame would pay for it. The
/// address of the `include_bytes!` slice is stable, which makes it a sound cache
/// key without threading an index through the mapping.
fn provider_image(data: &'static [u8]) -> Arc<gpui::Image> {
    static CACHE: OnceLock<Mutex<HashMap<usize, Arc<gpui::Image>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = data.as_ptr() as usize;
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache
        .entry(key)
        .or_insert_with(|| {
            Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                data.to_vec(),
            ))
        })
        .clone()
}

fn provider_mark_element(mark: ProviderMark, cx: &mut Context<DshApp>) -> AnyElement {
    match mark {
        // `img()` keeps the artwork's own colors; `svg()` would rasterize it to
        // an alpha mask and paint the whole mark in one tint.
        ProviderMark::Logo(data) => gpui::img(provider_image(data))
            .size(px(16.))
            .flex_none()
            .object_fit(gpui::ObjectFit::Contain)
            .into_any_element(),
        ProviderMark::Glyph(glyph) => icon(glyph, 15., dsh(cx).label_secondary).into_any_element(),
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderModel {
    id: String,
    name: Option<String>,
    context_window: Option<u64>,
    max_tokens: Option<u64>,
    #[serde(alias = "input")]
    input_modalities: Option<Vec<String>>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Provider {
    id: String,
    name: String,
    base_url: String,
    protocol: Protocol,
    models: Vec<ProviderModel>,
    key_configured: bool,
    available: Option<bool>,
}

#[derive(Deserialize)]
struct SavedProviders {
    id: String,
    providers: Vec<Provider>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    List,
    Discover,
    Save,
    Remove,
}

pub(super) struct ProviderSettings {
    provider_search: Entity<Editor>,
    model_search: Entity<Editor>,
    name: Entity<Editor>,
    url: Entity<Editor>,
    key: Entity<Editor>,
    model: Entity<Editor>,
    _model_enter: Subscription,
    _form_edits: Vec<Subscription>,
    providers: Vec<Provider>,
    selected: Option<String>,
    preset: Option<usize>,
    protocol: Protocol,
    models: Vec<ProviderModel>,
    discovered: Vec<ProviderModel>,
    manual_entry: bool,
    key_configured: bool,
    clear_key: bool,
    confirm_remove: bool,
    loaded: bool,
    busy: Option<Operation>,
    feedback: Option<String>,
}

impl ProviderSettings {
    pub(super) fn new(w: &mut Window, cx: &mut Context<DshApp>) -> Self {
        let provider_search = editor("搜索服务商".into(), InputMode::Single, w, cx);
        let model_search = editor("搜索模型名称或 ID".into(), InputMode::Single, w, cx);
        let name = editor("例如 我的模型网关".into(), InputMode::Single, w, cx);
        let url = editor("https://example.com/v1".into(), InputMode::Single, w, cx);
        let key = editor("输入 API Key".into(), InputMode::Password, w, cx);
        let mut form_edits: Vec<_> = [&name, &provider_search, &model_search]
            .into_iter()
            .map(|field| {
                cx.subscribe(field, |_, _, event: &EditEvent, cx| {
                    if matches!(event, EditEvent::Changed) {
                        cx.notify();
                    }
                })
            })
            .collect();
        form_edits.push(cx.subscribe(&url, |app, _, event: &EditEvent, cx| {
            if matches!(event, EditEvent::Changed) {
                app.model_providers.discovered.clear();
                cx.notify();
            }
        }));
        form_edits.push(cx.subscribe(&key, |app, input, event: &EditEvent, cx| {
            if matches!(event, EditEvent::Changed) {
                app.model_providers.discovered.clear();
                if !input.read(cx).value().is_empty() {
                    app.model_providers.clear_key = false;
                }
                cx.notify();
            }
        }));
        let model = editor("模型 ID，例如 llama3.2".into(), InputMode::Single, w, cx);
        let model_enter = cx.subscribe_in(&model, w, |app, _, event: &EditEvent, w, cx| {
            if matches!(event, EditEvent::Submitted) {
                app.model_providers.add_model(w, cx);
            }
        });
        Self {
            provider_search,
            model_search,
            name,
            url,
            key,
            model,
            _model_enter: model_enter,
            _form_edits: form_edits,
            providers: Vec::new(),
            selected: None,
            preset: None,
            protocol: Protocol::default(),
            models: Vec::new(),
            discovered: Vec::new(),
            manual_entry: false,
            key_configured: false,
            clear_key: false,
            confirm_remove: false,
            loaded: false,
            busy: None,
            feedback: None,
        }
    }

    fn install_catalog(&mut self, providers: Vec<Provider>, w: &Window, cx: &mut Context<DshApp>) {
        let adding = self.selected.is_none()
            && (self.preset.is_some()
                || !self.models.is_empty()
                || [&self.name, &self.url, &self.key, &self.model]
                    .iter()
                    .any(|input| !input.read(cx).value().is_empty()));
        self.providers = providers;
        self.loaded = true;
        if adding {
            self.feedback = None;
        } else {
            let selected = self
                .selected
                .clone()
                .filter(|id| self.providers.iter().any(|provider| provider.id == *id))
                .or_else(|| self.providers.first().map(|provider| provider.id.clone()));
            self.load(selected, w, cx);
        }
    }

    fn load(&mut self, id: Option<String>, w: &Window, cx: &mut Context<DshApp>) {
        let saved = id
            .as_deref()
            .and_then(|id| self.providers.iter().find(|provider| provider.id == id));
        replace(&self.name, saved.map_or("", |p| p.name.as_str()), w, cx);
        replace(&self.url, saved.map_or("", |p| p.base_url.as_str()), w, cx);
        replace(&self.key, "", w, cx);
        replace(&self.model, "", w, cx);
        replace(&self.model_search, "", w, cx);
        self.protocol = saved.map_or_else(Protocol::default, |provider| provider.protocol);
        self.models = saved.map_or_else(Vec::new, |provider| provider.models.clone());
        self.key_configured = saved.is_some_and(|provider| provider.key_configured);
        self.selected = id;
        self.preset = None;
        self.discovered.clear();
        self.manual_entry = false;
        self.feedback = None;
        self.clear_key = false;
        self.confirm_remove = false;
        cx.notify();
    }

    fn load_preset(&mut self, index: usize, w: &Window, cx: &mut Context<DshApp>) {
        let preset = &PRESETS[index];
        self.load(None, w, cx);
        replace(&self.name, preset.name, w, cx);
        replace(&self.url, preset.url, w, cx);
        self.protocol = preset.protocol;
        self.preset = Some(index);
    }

    fn set_protocol(&mut self, protocol: Protocol, cx: &mut Context<DshApp>) {
        self.protocol = protocol;
        self.discovered.clear();
        cx.notify();
    }

    fn add_model(&mut self, w: &Window, cx: &mut Context<DshApp>) {
        if self.busy.is_some() {
            return;
        }
        let id = self.model.read(cx).value().trim().to_owned();
        if id.is_empty() || id.chars().any(char::is_whitespace) {
            self.feedback = Some("请输入不含空格的模型 ID。".into());
        } else if self.models.iter().any(|model| model.id == id) {
            self.feedback = Some("此模型已添加。".into());
        } else {
            self.models.push(ProviderModel {
                id,
                name: None,
                context_window: None,
                max_tokens: None,
                input_modalities: None,
            });
            replace(&self.model, "", w, cx);
            replace(&self.model_search, "", w, cx);
            self.feedback = None;
        }
        cx.notify();
    }

    fn draft(&self, cx: &App) -> Value {
        let mut draft = json!({
            "baseUrl": self.url.read(cx).value().trim(),
            "name": self.name.read(cx).value().trim(),
            "protocol": self.protocol,
            "clearKey": self.clear_key,
            "models": self.models,
        });
        if let Some(id) = &self.selected {
            draft["id"] = id.as_str().into();
        }
        let key = self.key.read(cx).value().trim();
        if !key.is_empty() {
            draft["apiKey"] = key.into();
        }
        draft
    }
}

impl DshApp {
    pub(super) fn load_startup_model_providers(
        &mut self,
        reply: &Value,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(value) = reply.get("modelProviders")
            && let Ok(providers) = serde_json::from_value::<Vec<Provider>>(value.clone())
        {
            let available = providers
                .iter()
                .any(|provider| provider.available.unwrap_or(true) && !provider.models.is_empty());
            self.model_providers.install_catalog(providers, w, cx);
            if available {
                self.welcome
                    .update(cx, |welcome, cx| welcome.use_saved_models(w, cx));
            }
        } else if self.show_settings && self.settings_page == SettingsPage::Providers {
            self.open_model_providers(w, cx);
        }
    }

    pub(super) fn open_model_providers(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.settings_page = SettingsPage::Providers;
        if !self.model_providers.loaded {
            if self.connected() {
                self.request_model_providers(Operation::List, w, cx);
            } else {
                self.model_providers.feedback = Some(
                    self.error
                        .clone()
                        .unwrap_or_else(|| "正在连接模型服务…".into()),
                );
            }
        }
        cx.notify();
    }

    fn request_model_providers(
        &mut self,
        action: Operation,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let blank_session = if action == Operation::Save
            && self.conversation.acp_open
            && !self.conversation.running
            && self.conversation.entries.is_empty()
        {
            self.active_session.clone()
        } else {
            None
        };
        let settings = &mut self.model_providers;
        if settings.busy.is_some() {
            return;
        }
        let mut draft = match action {
            Operation::List => json!({}),
            Operation::Remove => {
                let Some(id) = &settings.selected else { return };
                json!({ "id": id })
            }
            Operation::Save | Operation::Discover => settings.draft(cx),
        };
        if let Some(session) = blank_session {
            draft["sessionId"] = session.into();
        }
        settings.busy = Some(action);
        settings.feedback = None;
        for input in [
            &settings.name,
            &settings.url,
            &settings.key,
            &settings.model,
        ] {
            input.update(cx, |input, cx| {
                input.disabled = true;
                cx.notify();
            });
        }
        cx.notify();
        let command = match action {
            Operation::List => "list",
            Operation::Discover => "discover",
            Operation::Save => "save",
            Operation::Remove => "remove",
        };
        cx.spawn_in(w, async move |this, cx| {
            let result =
                bridge::invoke("model_providers", json!({"action":command,"draft":draft})).await;
            let _ = this.update_in(cx, |app, w, cx| {
                let settings = &mut app.model_providers;
                settings.busy = None;
                for input in [
                    &settings.name,
                    &settings.url,
                    &settings.key,
                    &settings.model,
                ] {
                    input.update(cx, |input, cx| {
                        input.disabled = false;
                        cx.notify();
                    });
                }
                let apply = |settings: &mut ProviderSettings,
                             result: Value,
                             cx: &mut Context<Self>| {
                    match action {
                        Operation::List => {
                            settings.install_catalog(serde_json::from_value(result)?, w, cx);
                        }
                        Operation::Discover => {
                            settings.discovered = serde_json::from_value(result)?;
                            if settings.discovered.is_empty() {
                                settings.feedback = Some("服务端没有返回模型，可手动添加。".into());
                            }
                        }
                        Operation::Save | Operation::Remove => {
                            let reply: SavedProviders = serde_json::from_value(result)?;
                            settings.providers = reply.providers;
                            settings.loaded = true;
                            let id = if action == Operation::Save {
                                replace(&settings.provider_search, "", w, cx);
                                Some(reply.id)
                            } else {
                                settings
                                    .providers
                                    .first()
                                    .map(|provider| provider.id.clone())
                            };
                            settings.load(id, w, cx);
                        }
                    }
                    Ok::<_, serde_json::Error>(())
                };
                match result {
                    Ok(value) => {
                        if apply(settings, value, cx).is_err() {
                            settings.feedback = Some("模型服务返回的数据无法读取。".into());
                        } else if matches!(action, Operation::Save | Operation::Remove) {
                            app.push_toast(
                                if action == Operation::Save {
                                    "模型服务已保存"
                                } else {
                                    "模型服务已删除"
                                },
                                cx,
                            );
                            if action == Operation::Save && app.active_session.is_none() {
                                app.new_session(w, cx);
                            }
                        }
                    }
                    Err(error) => settings.feedback = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_provider_sidebar(&self, compact: bool, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let state = &self.model_providers;
        let busy = state.busy.is_some();
        let query = state.provider_search.read(cx).value().trim().to_lowercase();
        let mut service_list = column()
            .id("provider-service-list")
            .flex_1()
            .min_h_0()
            .w_full()
            .gap(px(4.))
            .overflow_y_scroll();
        let matching_providers: Vec<_> = state
            .providers
            .iter()
            .filter(|provider| matches_query(&query, &[&provider.name, &provider.base_url]))
            .collect();
        let has_saved_matches = !matching_providers.is_empty();
        if has_saved_matches {
            service_list = service_list.child(list_label("已配置", cx));
        }
        for provider in matching_providers {
            let id = provider.id.clone();
            let selected = state.selected.as_deref() == Some(id.as_str());
            service_list = service_list.child(provider_row(
                format!("model-provider-{id}"),
                &provider.name,
                Some(provider_mark(&provider.name, &provider.base_url)),
                selected,
                busy,
                cx,
                move |app, w, cx| app.model_providers.load(Some(id.clone()), w, cx),
            ));
        }
        let mut presets = PRESETS
            .iter()
            .enumerate()
            .filter(|(_, preset)| {
                matches_query(&query, &[preset.name, preset.keywords, preset.url])
            })
            .peekable();
        if presets.peek().is_some() {
            service_list = service_list.child(list_label("常用服务商", cx));
        } else if !has_saved_matches {
            service_list = service_list.child(list_label("没有匹配的服务商", cx));
        }
        for (index, preset) in presets {
            service_list = service_list.child(provider_row(
                format!("provider-preset-{index}"),
                preset.name,
                Some(provider_mark(preset.name, preset.url)),
                state.preset == Some(index),
                busy,
                cx,
                move |app, w, cx| app.model_providers.load_preset(index, w, cx),
            ));
        }
        column()
            .min_h_0()
            .flex_none()
            .gap(px(8.))
            .when(compact, |list| {
                list.w_full()
                    .h(px(if state.loaded { 142. } else { 174. }))
                    .pb(px(8.))
                    .border_b(px(0.5))
                    .border_color(p.border_l2)
            })
            .when(!compact, |list| {
                list.w(px(184.))
                    .h_full()
                    .gap(px(10.))
                    .pr(px(12.))
                    .border_r(px(0.5))
                    .border_color(p.border_l2)
            })
            .child(search_field(&state.provider_search, cx))
            .child(service_list)
            .child(provider_row(
                "add-model-provider".into(),
                "添加服务商",
                Some(ProviderMark::Glyph(Glyph::Plus)),
                state.selected.is_none() && state.preset.is_none(),
                busy,
                cx,
                |app, w, cx| app.model_providers.load(None, w, cx),
            ))
            .when(!state.loaded, |sidebar| {
                sidebar.child(provider_row(
                    "reload-model-providers".into(),
                    "重新加载",
                    Some(ProviderMark::Glyph(Glyph::Redo)),
                    false,
                    busy,
                    cx,
                    |app, w, cx| app.request_model_providers(Operation::List, w, cx),
                ))
            })
    }

    fn render_provider_models(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let state = &self.model_providers;
        let busy = state.busy.is_some();
        let can_sync =
            crate::validation::valid_provider_url(state.url.read(cx).value().trim()) && !busy;
        let model_query = state.model_search.read(cx).value().trim().to_lowercase();
        let model_matches = |model: &ProviderModel| {
            matches_query(
                &model_query,
                &[&model.id, model.name.as_deref().unwrap_or("")],
            )
        };
        let mut models = column().w_full().gap(px(2.));
        let mut has_matches = false;
        for model in state.models.iter().filter(|model| model_matches(model)) {
            has_matches = true;
            let id = model.id.clone();
            models = models.child(
                row()
                    .w_full()
                    .min_w_0()
                    .h(px(34.))
                    .px(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .child(model.name.clone().unwrap_or_else(|| id.clone())),
                    )
                    .child(
                        provider_row(
                            format!("remove-provider-model-{id}"),
                            "移除",
                            None,
                            false,
                            busy,
                            cx,
                            move |app, _, cx| {
                                app.model_providers.models.retain(|model| model.id != id);
                                cx.notify();
                            },
                        )
                        .w_auto()
                        .flex_none(),
                    ),
            );
        }
        let mut matches = state
            .discovered
            .iter()
            .filter(|model| model_matches(model))
            .peekable();
        if matches.peek().is_some() {
            has_matches = true;
            models = models.child(
                div()
                    .mt(px(8.))
                    .text_size(px(12.))
                    .text_color(p.label_tertiary)
                    .child("从同步结果中选择要添加的模型"),
            );
            let mut candidates = column()
                .id("provider-model-results")
                .w_full()
                .max_h(px(180.))
                .overflow_y_scroll()
                .gap(px(2.));
            for model in matches {
                let selected = state.models.iter().any(|added| added.id == model.id);
                let model = model.clone();
                candidates = candidates.child(provider_row(
                    format!("discovered-model-{}", model.id),
                    &format!(
                        "{} {}",
                        if selected { "✓" } else { "＋" },
                        model.name.as_deref().unwrap_or(&model.id)
                    ),
                    None,
                    selected,
                    busy,
                    cx,
                    {
                        let model = model.clone();
                        move |app, _, cx| {
                            let models = &mut app.model_providers.models;
                            if let Some(index) =
                                models.iter().position(|other| other.id == model.id)
                            {
                                models.remove(index);
                            } else {
                                models.push(model.clone());
                            }
                            cx.notify();
                        }
                    },
                ));
            }
            models = models.child(candidates);
        }

        column()
            .w_full()
            .flex_none()
            .gap(px(8.))
            .child(
                row()
                    .w_full()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("模型"),
                    )
                    .child(dialog_action(
                        "sync-provider-models",
                        if state.busy == Some(Operation::Discover) {
                            "同步中…"
                        } else {
                            "同步模型"
                        },
                        false,
                        !can_sync,
                        0.,
                        cx,
                        |app, w, cx| app.request_model_providers(Operation::Discover, w, cx),
                    ))
                    .child(dialog_action(
                        "toggle-manual-provider-model",
                        if state.manual_entry {
                            "取消添加"
                        } else {
                            "＋ 手动添加"
                        },
                        false,
                        busy,
                        0.,
                        cx,
                        |app, w, cx| {
                            let state = &mut app.model_providers;
                            state.manual_entry = !state.manual_entry;
                            if state.manual_entry {
                                state.model.focus_handle(cx).focus(w, cx);
                            } else {
                                replace(&state.model, "", w, cx);
                            }
                            cx.notify();
                        },
                    )),
            )
            .when(state.manual_entry, |body| {
                body.child(
                    row()
                        .w_full()
                        .gap(px(6.))
                        .child(div().min_w_0().flex_1().child(input(&state.model, cx)))
                        .child(dialog_action(
                            "add-provider-model",
                            "添加",
                            false,
                            busy,
                            0.,
                            cx,
                            |app, w, cx| app.model_providers.add_model(w, cx),
                        )),
                )
            })
            .when(
                !state.models.is_empty() || !state.discovered.is_empty() || !model_query.is_empty(),
                |body| body.child(search_field(&state.model_search, cx)),
            )
            .when(has_matches, |body| body.child(models))
            .when(!has_matches, |body| {
                let (title, subtitle) = if model_query.is_empty() {
                    ("请选择模型", "同步服务端的模型列表，或手动输入模型 ID")
                } else {
                    ("没有匹配的模型", "试试其他模型名称或 ID")
                };
                body.child(
                    column()
                        .w_full()
                        .py(px(22.))
                        .items_center()
                        .gap(px(6.))
                        .child(icon(Glyph::Card, 20., p.label_caption))
                        .child(div().text_size(px(13.)).child(title))
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(p.label_secondary)
                                .child(subtitle),
                        ),
                )
            })
    }

    pub(super) fn render_model_providers(
        &self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let state = &self.model_providers;
        let busy = state.busy.is_some();
        let key_title = if state.clear_key {
            "API Key（保存时清除）"
        } else if state.key_configured {
            "API Key（已保存）"
        } else {
            "API Key"
        };
        let provider_name = state.name.read(cx).value().to_owned();
        let provider_url = state.url.read(cx).value().to_owned();
        // 官方：端点必须是可解析的 HTTP/HTTPS URL，语法错误阻止创建。
        let url_valid = crate::validation::valid_provider_url(&provider_url);
        let can_sync = url_valid && !busy;
        let can_save =
            can_sync && !state.models.is_empty() && !state.name.read(cx).value().trim().is_empty();
        let heading = row().gap(px(8.)).items_center().flex_none();
        let heading = if let Some(logo) =
            crate::provider_marks::provider_logo(&provider_name, &provider_url)
        {
            heading.child(
                gpui::img(provider_image(logo))
                    .size(px(20.))
                    .flex_none()
                    .object_fit(gpui::ObjectFit::Contain),
            )
        } else {
            heading
        }
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .child(if state.selected.is_some() || state.preset.is_some() {
                    provider_name
                } else {
                    "添加 OpenAI 兼容服务商".to_owned()
                }),
        );
        let details = column()
            .id("provider-details")
            .w_full()
            .min_w_0()
            .min_h_0()
            .when(!compact, |details| details.h_full())
            .flex_1()
            .overflow_y_scroll()
            .pr(px(4.))
            .gap(px(14.))
            .child(heading)
            .child(provider_field("服务商名称", input(&state.name, cx), cx))
            .child(provider_field("API 地址", input(&state.url, cx), cx))
            .when(!url_valid && !provider_url.trim().is_empty(), |details| {
                details.child(
                    div()
                        .id("provider-url-error")
                        .role(gpui::Role::Alert)
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(p.danger)
                        .child("请输入以 http:// 或 https:// 开头的有效地址"),
                )
            })
            .child(provider_field(
                "接口协议",
                row()
                    .child(
                        provider_row(
                            "provider-chat".into(),
                            Protocol::Chat.name(),
                            None,
                            matches!(state.protocol, Protocol::Chat),
                            busy,
                            cx,
                            |app, _, cx| app.model_providers.set_protocol(Protocol::Chat, cx),
                        )
                        .w_auto(),
                    )
                    .child(
                        provider_row(
                            "provider-responses".into(),
                            Protocol::Responses.name(),
                            None,
                            matches!(state.protocol, Protocol::Responses),
                            busy,
                            cx,
                            |app, _, cx| app.model_providers.set_protocol(Protocol::Responses, cx),
                        )
                        .w_auto(),
                    ),
                cx,
            ))
            .child(provider_field(
                key_title,
                row()
                    .child(div().flex_1().min_w_0().child(input(&state.key, cx)))
                    .when(state.key_configured, |key_row| {
                        key_row.child(dialog_action(
                            "provider-clear-key",
                            if state.clear_key { "保留" } else { "清除" },
                            false,
                            busy,
                            0.,
                            cx,
                            |app, w, cx| {
                                let settings = &mut app.model_providers;
                                settings.clear_key = !settings.clear_key;
                                settings.discovered.clear();
                                if settings.clear_key {
                                    replace(&settings.key, "", w, cx);
                                }
                                cx.notify();
                            },
                        ))
                    }),
                cx,
            ))
            .child(self.render_provider_models(cx))
            .children(state.feedback.as_ref().map(|message| {
                div()
                    .text_size(px(13.))
                    .text_color(p.label_secondary)
                    .child(message.clone())
            }))
            .child(
                row()
                    .w_full()
                    .flex_none()
                    .justify_end()
                    .gap(px(8.))
                    .when(state.selected.is_some(), |actions| {
                        actions.child(dialog_action(
                            "remove-model-provider",
                            if state.confirm_remove {
                                "确认删除"
                            } else {
                                "删除服务商"
                            },
                            false,
                            busy,
                            0.,
                            cx,
                            |app, w, cx| {
                                if app.model_providers.confirm_remove {
                                    app.request_model_providers(Operation::Remove, w, cx);
                                } else {
                                    app.model_providers.confirm_remove = true;
                                    cx.notify();
                                }
                            },
                        ))
                    })
                    .child(dialog_action(
                        "save-model-provider",
                        if state.busy == Some(Operation::Save) {
                            "保存中…"
                        } else {
                            "保存服务商"
                        },
                        true,
                        !can_save,
                        0.,
                        cx,
                        |app, w, cx| app.request_model_providers(Operation::Save, w, cx),
                    )),
            );
        column()
            .w_full()
            .h_full()
            .min_h_0()
            .gap(px(16.))
            .child(
                div()
                    .flex_none()
                    .text_size(px(20.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("模型服务"),
            )
            .child(
                row()
                    .when(compact, |panes| panes.flex_col())
                    .items_start()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .gap(px(if compact { 12. } else { 16. }))
                    .child(self.render_provider_sidebar(compact, cx))
                    .child(details),
            )
    }
}

fn provider_field(label: &'static str, field: impl IntoElement, cx: &App) -> gpui::Div {
    column()
        .w_full()
        .flex_none()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(13.))
                .text_color(dsh(cx).label_secondary)
                .child(label),
        )
        .child(field)
}

fn list_label(label: &'static str, cx: &App) -> gpui::Div {
    div()
        .flex_none()
        .px(px(8.))
        .pt(px(4.))
        .text_size(px(12.))
        .text_color(dsh(cx).label_caption)
        .child(label)
}

fn search_field(field: &Entity<Editor>, cx: &App) -> gpui::Div {
    let p = dsh(cx);
    row()
        .w_full()
        .h(px(32.))
        .flex_none()
        .px(px(8.))
        .gap(px(6.))
        .rounded(px(8.))
        .bg(p.hover)
        .child(icon(Glyph::Search, 14., p.label_caption))
        .child(div().flex_1().min_w_0().h(px(28.)).child(field.clone()))
}

fn matches_query(query: &str, values: &[&str]) -> bool {
    query.is_empty()
        || values
            .iter()
            .any(|value| value.to_lowercase().contains(query))
}

fn provider_row(
    id: String,
    label: &str,
    mark: Option<ProviderMark>,
    selected: bool,
    disabled: bool,
    cx: &mut Context<DshApp>,
    click: impl Fn(&mut DshApp, &mut Window, &mut Context<DshApp>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let p = dsh(cx);
    let button = row()
        .id(gpui::ElementId::Name(id.into()))
        .w_full()
        .min_w_0()
        .flex_none()
        .h(px(32.))
        .px(px(8.))
        .rounded(px(8.))
        .gap(px(8.))
        .role(gpui::Role::Button)
        .aria_label(label.to_owned())
        .aria_selected(selected)
        .tab_index(0)
        .tab_stop(!disabled)
        .when(selected, |button| button.bg(p.hover))
        .children(mark.map(|mark| provider_mark_element(mark, cx)))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .child(label.to_owned()),
        );
    if disabled {
        button
    } else {
        super::dock_controls::dock_action_button(
            button.cursor_pointer().hover(|style| style.bg(p.hover)),
            cx.listener(move |app, _, w, cx| click(app, w, cx)),
        )
    }
}
