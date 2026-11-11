//! The Host's registered web providers and their staged settings.

use super::*;
use gpui::Entity;
use serde::Deserialize;

const SEARCH_API: &str = "deepseek-search-api";
const LEGACY_SEARCH: &str = "deepseek-official";
const FETCH_API: &str = "deepseek-fetch-api";
const LOCAL_FETCH: &str = "http";
const SEARCH_GUIDE: &str = "https://api-docs.deepseek.com/zh-cn/guides/search";

#[derive(Clone, Copy)]
enum Capability {
    Search,
    Fetch,
}

impl Capability {
    const ALL: [Self; 2] = [Self::Search, Self::Fetch];

    fn key(self) -> &'static str {
        match self {
            Self::Search => "searchProvider",
            Self::Fetch => "fetchProvider",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Search => "网页搜索提供方",
            Self::Fetch => "网页获取提供方",
        }
    }

    fn slot(self) -> usize {
        self as usize
    }

    fn official(self) -> [&'static str; 2] {
        match self {
            Self::Search => [SEARCH_API, LEGACY_SEARCH],
            Self::Fetch => [LOCAL_FETCH, FETCH_API],
        }
    }
}

#[derive(Deserialize)]
struct Provider {
    id: String,
    title: Option<String>,
    description: Option<String>,
}

impl Provider {
    fn label(&self) -> String {
        match self.id.as_str() {
            SEARCH_API => "DeepSeek Search API".into(),
            LEGACY_SEARCH => "DeepSeek（旧版）".into(),
            FETCH_API => "DeepSeek Fetch API".into(),
            LOCAL_FETCH => "本机直接获取".into(),
            _ => self.title.clone().unwrap_or_else(|| self.id.clone()),
        }
    }
}

#[derive(Deserialize)]
struct ProviderChoice {
    value: Option<String>,
    base: Option<String>,
    options: Vec<Provider>,
}

impl ProviderChoice {
    fn value(&self) -> &str {
        self.value.as_deref().unwrap_or_default()
    }

    fn choices(&self, capability: Capability, selected: &str) -> Vec<PickerOption> {
        let mut options = Vec::new();
        if self.base.is_none() {
            options.push(PickerOption::new("", "自动选择"));
        }
        let official = capability.official();
        let mut providers: Vec<_> = self.options.iter().collect();
        providers.sort_by_key(|provider| {
            official
                .iter()
                .position(|id| *id == provider.id)
                .unwrap_or(2)
        });
        options.extend(
            providers
                .into_iter()
                .map(|provider| PickerOption::new(provider.id.clone(), provider.label())),
        );
        if !selected.is_empty() && !self.options.iter().any(|provider| provider.id == selected) {
            options.push(PickerOption::new(
                selected.to_owned(),
                format!("{selected}（未加载）"),
            ));
        }
        options
    }
}

#[derive(Deserialize)]
struct FieldValue {
    value: Option<Value>,
    base: Option<Value>,
    overridden: bool,
}

fn field_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

#[derive(Deserialize)]
struct Credential {
    #[serde(rename = "ref")]
    reference: String,
    configured: bool,
    writable: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    search: ProviderChoice,
    fetch: ProviderChoice,
    fields: HashMap<String, FieldValue>,
    credential: Option<Credential>,
    revisions: HashMap<String, u64>,
    writable: bool,
    providers_failed: bool,
}

impl Snapshot {
    fn choice(&self, capability: Capability) -> &ProviderChoice {
        match capability {
            Capability::Search => &self.search,
            Capability::Fetch => &self.fetch,
        }
    }
}

#[derive(Deserialize)]
struct Reply {
    snapshot: Option<Snapshot>,
    applied: Vec<String>,
    failure: Option<String>,
}

struct FieldSpec {
    key: &'static str,
    label: &'static str,
    hint: &'static str,
    invalid: &'static str,
}

const FIELDS: [FieldSpec; 3] = [
    FieldSpec {
        key: "count",
        label: "每次搜索结果数",
        hint: "每个查询返回的结果条数，须为 1–50 的整数，默认 10。此项不控制搜索调用次数。",
        invalid: "请填 1–50 的整数，留空表示使用默认值。",
    },
    FieldSpec {
        key: "baseURL",
        label: "Base URL",
        hint: "填写含 API 前缀的地址，程序会追加 /messages。留空时使用官方地址。",
        invalid: "请输入有效的 HTTP 或 HTTPS 地址。",
    },
    FieldSpec {
        key: "maxUses",
        label: "单次请求最多搜索次数",
        hint: "每次 Messages 请求允许的服务端搜索次数上限，须为正整数，默认 5。",
        invalid: "请填正整数，留空表示使用默认值。",
    },
];

struct Field {
    spec: &'static FieldSpec,
    editor: Entity<Editor>,
    reset: bool,
}

impl Field {
    fn change(&self, accepted: &FieldValue, cx: &App) -> Result<Option<Value>, ()> {
        let text = self.editor.read(cx).value().trim();
        if self.reset || text.is_empty() {
            return Ok(accepted.overridden.then_some(json!({"op":"unset"})));
        }
        if text == field_text(accepted.value.as_ref()) {
            return Ok(None);
        }
        let value = match self.spec.key {
            "baseURL" if text.starts_with("https://") || text.starts_with("http://") => json!(text),
            "baseURL" => return Err(()),
            key => {
                let number = text.parse::<u64>().map_err(|_| ())?;
                let limit = if key == "count" {
                    50
                } else {
                    9_007_199_254_740_991
                };
                if number == 0 || number > limit {
                    return Err(());
                }
                json!(number)
            }
        };
        Ok(Some(json!({"op":"set","value":value})))
    }
}

pub(super) struct WebSearchSettings {
    identity: Rc<()>,
    snapshot: Option<Snapshot>,
    draft: [Option<String>; 2],
    selectors: [Option<Entity<Select>>; 2],
    selectors_disabled: bool,
    _choices: Vec<Subscription>,
    fields: Vec<Field>,
    credential: Entity<Editor>,
    _edits: Vec<Subscription>,
    busy: bool,
    saving: bool,
    error: Option<String>,
}

impl WebSearchSettings {
    fn selectors_blocked(&self, snapshot: &Snapshot, cx: &App) -> bool {
        self.busy
            || !snapshot.writable
            || self.error.is_some()
            || self.field_changes(snapshot, cx).is_err()
    }

    fn selected<'a>(&'a self, snapshot: &'a Snapshot, capability: Capability) -> &'a str {
        self.draft[capability.slot()]
            .as_deref()
            .unwrap_or_else(|| snapshot.choice(capability).value())
    }

    fn field_changes(
        &self,
        snapshot: &Snapshot,
        cx: &App,
    ) -> Result<serde_json::Map<String, Value>, ()> {
        let mut edits = serde_json::Map::new();
        for field in &self.fields {
            if let Some(accepted) = snapshot.fields.get(field.spec.key)
                && let Some(edit) = field.change(accepted, cx)?
            {
                edits.insert(field.spec.key.into(), edit);
            }
        }
        Ok(edits)
    }

    fn change(&self, snapshot: &Snapshot, cx: &App) -> Result<Option<Value>, ()> {
        let fields = self.field_changes(snapshot, cx)?;
        let mut change = serde_json::Map::new();
        if snapshot.writable {
            for (index, name) in ["search", "fetch"].into_iter().enumerate() {
                if let Some(value) = &self.draft[index] {
                    change.insert(name.into(), json!(value));
                }
            }
            if !fields.is_empty() {
                change.insert("fields".into(), Value::Object(fields));
            }
        }
        let key = self.credential.read(cx).value().trim();
        if !key.is_empty()
            && self.selected(snapshot, Capability::Search) == LEGACY_SEARCH
            && let Some(credential) = snapshot.credential.as_ref().filter(|key| key.writable)
        {
            change.insert(
                "credential".into(),
                json!({"ref":credential.reference,"key":key}),
            );
        }
        if change.is_empty() {
            return Ok(None);
        }
        change.insert("revisions".into(), json!(snapshot.revisions));
        Ok(Some(Value::Object(change)))
    }

    fn set_editors_enabled(&self, cx: &mut App) {
        let snapshot = self.snapshot.as_ref();
        for field in &self.fields {
            let enabled = !self.busy
                && snapshot.is_some_and(|snapshot| {
                    snapshot.writable && snapshot.fields.contains_key(field.spec.key)
                });
            field.editor.update(cx, |input, cx| {
                input.disabled = !enabled;
                cx.notify();
            });
        }
        let enabled = !self.busy
            && snapshot
                .and_then(|snapshot| snapshot.credential.as_ref())
                .is_some_and(|credential| credential.writable);
        self.credential.update(cx, |input, cx| {
            input.disabled = !enabled;
            cx.notify();
        });
    }
}

impl DshApp {
    pub(super) fn prepare_web_search_settings(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.connected() {
            self.web_search_settings = None;
            return;
        }
        if let Some(form) = &self.web_search_settings {
            if form.snapshot.as_ref().is_some_and(|snapshot| {
                form.selectors_blocked(snapshot, cx) != form.selectors_disabled
            }) {
                self.build_web_selectors(w, cx);
            }
            return;
        }
        let mut edits = Vec::new();
        let mut fields = Vec::new();
        for spec in &FIELDS {
            let editor = editor(spec.label.into(), InputMode::Single, w, cx);
            editor.update(cx, |input, _| {
                input.set_placeholder("");
                input.font_size = 13.;
                input.description = spec.hint.into();
            });
            edits.push(
                cx.subscribe(&editor, move |app, input, event: &EditEvent, cx| {
                    if *event != EditEvent::Changed {
                        return;
                    }
                    if let Some(form) = &mut app.web_search_settings
                        && let Some(field) =
                            form.fields.iter_mut().find(|field| field.editor == input)
                    {
                        if let Some(value) = form
                            .snapshot
                            .as_ref()
                            .and_then(|snap| snap.fields.get(spec.key))
                        {
                            let text = input.read(cx).value();
                            if text != field_text(value.base.as_ref()) {
                                field.reset = false;
                            }
                            // An accepted value may be normalized on save. Its
                            // editor event must not dismiss another field's failure.
                            if text != field_text(value.value.as_ref()) {
                                form.error = None;
                            }
                        }
                        cx.notify();
                    }
                }),
            );
            fields.push(Field {
                spec,
                editor,
                reset: false,
            });
        }
        let credential = editor("API Key".into(), InputMode::Password, w, cx);
        credential.update(cx, |input, _| {
            input.set_placeholder("");
            input.font_size = 13.;
        });
        edits.push(
            cx.subscribe(&credential, |app, input, event: &EditEvent, cx| {
                if *event == EditEvent::Changed
                    && let Some(form) = &mut app.web_search_settings
                    && form.credential == input
                {
                    if !input.read(cx).value().is_empty() {
                        form.error = None;
                    }
                    cx.notify();
                }
            }),
        );
        self.web_search_settings = Some(WebSearchSettings {
            identity: Rc::new(()),
            snapshot: None,
            draft: [None, None],
            selectors: [None, None],
            selectors_disabled: false,
            _choices: Vec::new(),
            fields,
            credential,
            _edits: edits,
            busy: false,
            saving: false,
            error: None,
        });
        self.request_web_search_settings(None, w, cx);
    }

    fn build_web_selectors(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.web_search_settings else {
            return;
        };
        let Some(snapshot) = &form.snapshot else {
            return;
        };
        form.selectors_disabled = form.selectors_blocked(snapshot, cx);
        form._choices.clear();
        for capability in Capability::ALL {
            let selected = form.selected(snapshot, capability).to_owned();
            let options = snapshot.choice(capability).choices(capability, &selected);
            let disabled = form.selectors_disabled || options.is_empty();
            let select = cx.new(|cx| {
                Select::new(capability.key(), capability.label(), options, w, cx)
                    .with_value(&selected)
                    .with_placeholder("暂无可用提供方")
                    .with_disabled(disabled, cx)
            });
            form._choices.push(cx.subscribe_in(
                &select,
                w,
                move |app, select, event: &SelectionChanged, w, cx| {
                    let Some(form) = &mut app.web_search_settings else {
                        return;
                    };
                    let Some(snapshot) = &form.snapshot else {
                        return;
                    };
                    if form.busy || form.selectors[capability.slot()].as_ref() != Some(&select) {
                        return;
                    }
                    if let Some(id) = &event.0 {
                        form.draft[capability.slot()] = (id.as_ref()
                            != snapshot.choice(capability).value())
                        .then(|| id.to_string());
                    }
                    if form.selected(snapshot, Capability::Search) != LEGACY_SEARCH {
                        replace(&form.credential, "", w, cx);
                    }
                    cx.notify();
                },
            ));
            form.selectors[capability.slot()] = Some(select);
        }
    }

    fn request_web_search_settings(
        &mut self,
        change: Option<Value>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &mut self.web_search_settings else {
            return;
        };
        if form.busy {
            return;
        }
        form.busy = true;
        form.saving = change.is_some();
        form.error = None;
        form.set_editors_enabled(cx);
        let identity = form.identity.clone();
        self.build_web_selectors(w, cx);
        cx.notify();
        cx.spawn_in(w, async move |this, cx| {
            let result = bridge::invoke("web_settings", json!({"change":change}))
                .await
                .and_then(|value| {
                    serde_json::from_value::<Reply>(value)
                        .map_err(|_| "网页设置返回的数据无法读取。".to_owned())
                });
            let _ = this.update_in(cx, |app, w, cx| {
                let Some(form) = app
                    .web_search_settings
                    .as_mut()
                    .filter(|form| Rc::ptr_eq(&form.identity, &identity))
                else {
                    return;
                };
                form.busy = false;
                form.saving = false;
                match result {
                    Ok(reply) => {
                        if let Some(snapshot) = &reply.snapshot {
                            for capability in Capability::ALL {
                                let draft = &mut form.draft[capability.slot()];
                                if reply.applied.iter().any(|name| name == capability.key())
                                    || draft.as_deref() == Some(snapshot.choice(capability).value())
                                {
                                    *draft = None;
                                }
                            }
                            for field in &mut form.fields {
                                let old = form
                                    .snapshot
                                    .as_ref()
                                    .and_then(|old| old.fields.get(field.spec.key));
                                let clean =
                                    old.is_none_or(|value| field.change(value, cx) == Ok(None));
                                let accepted =
                                    reply.applied.iter().any(|key| key == field.spec.key);
                                if clean
                                    || accepted
                                    || !snapshot.fields.contains_key(field.spec.key)
                                {
                                    field.reset = false;
                                    let value = snapshot
                                        .fields
                                        .get(field.spec.key)
                                        .and_then(|value| value.value.as_ref());
                                    replace(&field.editor, field_text(value), w, cx);
                                }
                            }
                            let old_ref = form
                                .snapshot
                                .as_ref()
                                .and_then(|snap| snap.credential.as_ref())
                                .map(|credential| credential.reference.as_str());
                            let new_ref = snapshot
                                .credential
                                .as_ref()
                                .map(|key| key.reference.as_str());
                            if old_ref != new_ref
                                || form.selected(snapshot, Capability::Search) != LEGACY_SEARCH
                                || reply.applied.iter().any(|field| field == "credential")
                            {
                                replace(&form.credential, "", w, cx);
                            }
                        }
                        form.snapshot = reply.snapshot;
                        form.error = reply.failure;
                    }
                    Err(error) => form.error = Some(error),
                }
                form.set_editors_enabled(cx);
                app.build_web_selectors(w, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn save_web_search_settings(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.web_search_settings else {
            return;
        };
        let Some(snapshot) = &form.snapshot else {
            return;
        };
        if let Ok(Some(change)) = form.change(snapshot, cx) {
            self.request_web_search_settings(Some(change), w, cx);
        }
    }
}

impl DshApp {
    pub(super) fn render_web_search_settings(&self, w: &Window, cx: &Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let Some(form) = &self.web_search_settings else {
            return note("连接服务后可配置此插件。", cx);
        };
        let retry = |id| {
            Button::new(id)
                .label("重试")
                .ghost()
                .disabled(form.busy)
                .on_click(cx.listener(|app, _, w, cx| app.request_web_search_settings(None, w, cx)))
                .into_themed(cx)
        };
        let Some(snapshot) = &form.snapshot else {
            let message = form.error.as_deref().unwrap_or(if form.busy {
                "正在读取设置…"
            } else {
                "该插件当前未加载，暂时无法配置。"
            });
            return row()
                .gap(px(8.))
                .child(note(message.to_owned(), cx))
                .when(form.error.is_some(), |row| {
                    row.child(retry("web-settings-retry"))
                });
        };
        let mut controls = column().w_full().gap_0().when(!snapshot.writable, |body| {
            body.child(note("本部署的设置为只读。", cx))
        });
        for capability in Capability::ALL {
            let selected = form.selected(snapshot, capability);
            let provider = snapshot
                .choice(capability)
                .options
                .iter()
                .find(|provider| provider.id == selected);
            let mut section = column()
                .w_full()
                .gap(px(8.))
                .when(matches!(capability, Capability::Fetch), |section| {
                    section.mt(px(32.))
                })
                .children(form.selectors[capability.slot()].clone())
                .when_some(
                    provider.and_then(|provider| provider.description.clone()),
                    |section, description| {
                        section.child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(p.label_tertiary)
                                .child(description),
                        )
                    },
                )
                .when(snapshot.providers_failed, |section| {
                    section.child(
                        row()
                            .gap(px(8.))
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(p.state_error_primary)
                                    .child("无法读取提供方列表，此菜单可能不完整"),
                            )
                            .child(retry(capability.key())),
                    )
                });
            if matches!(capability, Capability::Search) && provider.is_some() {
                let panel = column()
                    .w_full()
                    .gap_0()
                    .px(px(16.))
                    .rounded(px(12.))
                    .bg(p.bg_module_platform);
                section = match selected {
                    SEARCH_API if snapshot.fields.contains_key("count") => section
                        .child(super::dock_action_button(
                            div()
                                .id("web-search-guide")
                                .role(gpui::Role::Link)
                                .tab_stop(true)
                                .cursor_pointer()
                                .text_size(px(12.))
                                .text_color(p.state_business_primary)
                                .child("网页搜索与获取指南"),
                            |_, _, _| bridge::open_url(SEARCH_GUIDE),
                        ))
                        .child(panel.child(self.render_web_field(
                            form,
                            snapshot,
                            &form.fields[0],
                            false,
                            w,
                            cx,
                        ))),
                    LEGACY_SEARCH if snapshot.fields.contains_key("maxUses") => {
                        let mut panel = panel;
                        if let Some(credential) = &snapshot.credential {
                            panel = panel.child(
                                column()
                                    .w_full()
                                    .py(px(12.))
                                    .gap(px(6.))
                                    .child(
                                        row()
                                            .w_full()
                                            .justify_between()
                                            .gap(px(8.))
                                            .child(
                                                div()
                                                    .text_size(px(13.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(p.label_primary)
                                                    .child("API Key"),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(p.label_tertiary)
                                                    .child(if credential.configured {
                                                        "已配置密钥"
                                                    } else {
                                                        "未配置密钥"
                                                    }),
                                            ),
                                    )
                                    .child(web_setting_input(&form.credential, false, w, cx))
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .line_height(px(18.))
                                            .text_color(p.label_tertiary)
                                            .child(format!(
                                                "使用 {}。默认与 DeepSeek 模型共用。",
                                                credential.reference
                                            )),
                                    ),
                            );
                        }
                        section.child(
                            panel
                                .child(self.render_web_field(
                                    form,
                                    snapshot,
                                    &form.fields[1],
                                    snapshot.credential.is_some(),
                                    w,
                                    cx,
                                ))
                                .child(self.render_web_field(
                                    form,
                                    snapshot,
                                    &form.fields[2],
                                    true,
                                    w,
                                    cx,
                                )),
                        )
                    }
                    _ => section,
                };
            }
            if provider.is_some()
                && !selected.is_empty()
                && !capability.official().contains(&selected)
            {
                section = section.child(note("请在该插件自己的设置中配置", cx));
            }
            controls = controls.child(section);
        }
        let can_save = !form.busy
            && form
                .change(snapshot, cx)
                .is_ok_and(|change| change.is_some());
        controls.child(
            row()
                .w_full()
                .pt(px(16.))
                .gap(px(8.))
                .when_some(form.error.clone(), |footer, error| {
                    footer.child(
                        div()
                            .flex_1()
                            .text_color(p.state_error_primary)
                            .text_size(px(12.))
                            .child(error),
                    )
                })
                .child(super::dock_action_button(
                    row()
                        .id("web-settings-save")
                        .role(gpui::Role::Button)
                        .tab_stop(can_save)
                        .h(px(32.))
                        .px(px(15.))
                        .rounded(px(12.))
                        .text_size(px(13.))
                        .bg(p.label_primary)
                        .text_color(p.bg_layer_3)
                        .when(can_save, |button| button.cursor_pointer())
                        .when(!can_save, |button| button.opacity(0.4))
                        .child(if form.saving {
                            "保存中…"
                        } else {
                            "保存"
                        }),
                    cx.listener(|app, _, w, cx| app.save_web_search_settings(w, cx)),
                )),
        )
    }

    fn render_web_field(
        &self,
        form: &WebSearchSettings,
        snapshot: &Snapshot,
        field: &Field,
        divider: bool,
        w: &Window,
        cx: &Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let spec = field.spec;
        let Some(value) = snapshot.fields.get(spec.key) else {
            return div();
        };
        let change = field.change(value, cx);
        let overridden = match &change {
            Ok(Some(edit)) => edit["op"] == "set",
            Ok(None) => value.overridden,
            Err(()) => false,
        };
        let enabled = snapshot.writable && !form.busy;
        let invalid = change.is_err();
        column()
            .w_full()
            .py(px(12.))
            .gap(px(6.))
            .when(divider, |field| {
                field.border_t(px(0.5)).border_color(p.border_l2)
            })
            .child(
                row()
                    .w_full()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.label_primary)
                            .child(spec.label),
                    )
                    .when(overridden, |header| {
                        header
                            .child(
                                div()
                                    .px(px(6.))
                                    .rounded(px(4.))
                                    .bg(p.hover)
                                    .text_size(px(12.))
                                    .child("已覆盖"),
                            )
                            .child(super::dock_action_button(
                                div()
                                    .id(gpui::ElementId::Name(
                                        format!("web-reset-{}", spec.key).into(),
                                    ))
                                    .role(gpui::Role::Button)
                                    .tab_stop(enabled)
                                    .text_size(px(12.))
                                    .text_color(p.label_secondary)
                                    .when(enabled, |button| button.cursor_pointer())
                                    .child("恢复默认"),
                                cx.listener(move |app, _, w, cx| {
                                    let Some(form) = &mut app.web_search_settings else {
                                        return;
                                    };
                                    let Some(snapshot) = form
                                        .snapshot
                                        .as_ref()
                                        .filter(|snap| snap.writable && !form.busy)
                                    else {
                                        return;
                                    };
                                    let Some(value) = snapshot.fields.get(spec.key) else {
                                        return;
                                    };
                                    let Some(field) = form
                                        .fields
                                        .iter_mut()
                                        .find(|field| field.spec.key == spec.key)
                                    else {
                                        return;
                                    };
                                    field.reset = true;
                                    replace(&field.editor, field_text(value.base.as_ref()), w, cx);
                                    form.error = None;
                                    cx.notify();
                                }),
                            ))
                    }),
            )
            .child(web_setting_input(&field.editor, invalid, w, cx))
            .child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(if invalid {
                        p.state_error_primary
                    } else {
                        p.label_tertiary
                    })
                    .child(if invalid { spec.invalid } else { spec.hint }),
            )
    }
}

fn web_setting_input(editor: &Entity<Editor>, invalid: bool, w: &Window, cx: &App) -> gpui::Div {
    let p = dsh(cx);
    let focused = editor.read(cx).focus.is_focused(w);
    div()
        .w_full()
        .h(px(34.))
        .px(px(12.))
        .rounded(px(12.))
        .border(px(0.5))
        .border_color(if invalid {
            p.state_error_primary
        } else if focused {
            p.state_business_primary
        } else {
            p.border_l4
        })
        .bg(p.bg_layer_3)
        .child(editor.clone())
}
