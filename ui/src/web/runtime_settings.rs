use super::*;
use gpui::Entity;
use serde::{Deserialize, Serialize};

struct NumberSpec {
    key: &'static str,
    label: &'static str,
    hint: &'static str,
    integer: bool,
}

const SHELL_FIELDS: &[NumberSpec] = &[
    NumberSpec {
        key: "timeoutMs",
        label: "命令超时（毫秒）",
        hint: "单条命令允许运行多久，超时即终止。",
        integer: false,
    },
    NumberSpec {
        key: "maxOutputBytes",
        label: "单流输出上限（字节）",
        hint: "超出部分会转存到临时文件，而不是被丢弃。",
        integer: false,
    },
];
const LOOP_FIELDS: &[NumberSpec] = &[NumberSpec {
    key: "maxParallelToolCalls",
    label: "并行工具调用数",
    hint: "同一步内最多同时运行多少个可并行的调用。",
    integer: true,
}];

#[derive(Deserialize)]
struct NumberValue {
    value: f64,
    base: Option<f64>,
    overridden: bool,
}

impl NumberValue {
    fn default_text(&self) -> String {
        self.base
            .map_or_else(String::new, |number| number.to_string())
    }
}

#[derive(Deserialize)]
struct Snapshot {
    namespace: String,
    revision: u64,
    writable: bool,
    fields: HashMap<String, NumberValue>,
}

struct NumberField {
    spec: &'static NumberSpec,
    editor: Entity<Editor>,
    reset: bool,
    _edit: Subscription,
}

#[derive(Serialize)]
#[serde(tag = "op", content = "value", rename_all = "lowercase")]
enum Change {
    Set(f64),
    Unset,
}

impl NumberField {
    fn change(&self, accepted: &NumberValue, cx: &App) -> Result<Option<Change>, ()> {
        let text = self.editor.read(cx).value().trim();
        if self.reset || text.is_empty() {
            return Ok(accepted.overridden.then_some(Change::Unset));
        }
        if text == accepted.value.to_string() {
            return Ok(None);
        }
        let number: f64 = text.parse().map_err(|_| ())?;
        if !number.is_finite()
            || number <= 0.
            || (self.spec.integer && (number.fract() != 0. || number > 9_007_199_254_740_991.))
        {
            return Err(());
        }
        Ok(Some(Change::Set(number)))
    }
}

pub(super) struct RuntimeSettings {
    page: SettingsPage,
    identity: Rc<()>,
    fields: Vec<NumberField>,
    snapshot: Option<Snapshot>,
    loading: bool,
    saving: bool,
    error: Option<String>,
}

impl RuntimeSettings {
    fn command(&self) -> &'static str {
        if self.page == SettingsPage::Shell {
            "shell"
        } else {
            "agentLoop"
        }
    }

    fn changes(&self, snapshot: &Snapshot, cx: &App) -> Result<HashMap<&'static str, Change>, ()> {
        let mut changes = HashMap::new();
        for field in &self.fields {
            if let Some(value) = field.change(&snapshot.fields[field.spec.key], cx)? {
                changes.insert(field.spec.key, value);
            }
        }
        Ok(changes)
    }
}

impl DshApp {
    pub(super) fn prepare_runtime_settings(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let specs = match self.settings_page {
            SettingsPage::Shell if self.connected() => SHELL_FIELDS,
            SettingsPage::AgentLoop if self.connected() => LOOP_FIELDS,
            _ => {
                self.runtime_settings = None;
                return;
            }
        };
        let page = self.settings_page;
        if self
            .runtime_settings
            .as_ref()
            .is_some_and(|form| form.page == page)
        {
            return;
        }
        let fields = specs
            .iter()
            .map(|spec| {
                let editor = editor(spec.label.into(), InputMode::Single, w, cx);
                editor.update(cx, |input, _| {
                    input.set_placeholder("");
                    input.description = spec.hint.into();
                    input.font_size = 13.;
                });
                let edit = cx.subscribe(&editor, move |app, input, event: &EditEvent, cx| {
                    if *event != EditEvent::Changed {
                        return;
                    }
                    if let Some(form) = &mut app.runtime_settings
                        && let Some(field) =
                            form.fields.iter_mut().find(|field| field.editor == input)
                    {
                        if let Some(value) = form
                            .snapshot
                            .as_ref()
                            .and_then(|snap| snap.fields.get(spec.key))
                            && input.read(cx).value() != value.default_text()
                        {
                            field.reset = false;
                        }
                        form.error = None;
                        cx.notify();
                    }
                });
                NumberField {
                    spec,
                    editor,
                    reset: false,
                    _edit: edit,
                }
            })
            .collect();
        self.runtime_settings = Some(RuntimeSettings {
            page,
            fields,
            identity: Rc::new(()),
            snapshot: None,
            loading: true,
            saving: false,
            error: None,
        });
        self.request_runtime_settings(None, w, cx);
    }

    fn request_runtime_settings(
        &self,
        change: Option<Value>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &self.runtime_settings else {
            return;
        };
        let page = form.command();
        let identity = form.identity.clone();
        cx.spawn_in(w, async move |this, cx| {
            let result = bridge::invoke(
                "runtime_settings",
                json!({ "page": page, "change": change }),
            )
            .await
            .and_then(|value| {
                serde_json::from_value::<Option<Snapshot>>(value)
                    .map_err(|_| "运行设置返回的数据无法读取。".to_owned())
            });
            let _ = this.update_in(cx, |app, w, cx| {
                let Some(form) = app
                    .runtime_settings
                    .as_mut()
                    .filter(|form| Rc::ptr_eq(&form.identity, &identity))
                else {
                    return;
                };
                form.loading = false;
                form.saving = false;
                match result {
                    Ok(Some(snapshot))
                        if form
                            .fields
                            .iter()
                            .any(|field| !snapshot.fields.contains_key(field.spec.key)) =>
                    {
                        form.error = Some("运行设置返回的数据不完整。".into());
                    }
                    Ok(snapshot) => {
                        if let Some(snapshot) = &snapshot {
                            for field in &mut form.fields {
                                field.reset = false;
                                let value = snapshot.fields[field.spec.key].value.to_string();
                                field
                                    .editor
                                    .update(cx, |editor, cx| editor.set_value(value, w, cx));
                            }
                        }
                        form.snapshot = snapshot;
                        form.error = None;
                    }
                    Err(error) => form.error = Some(error),
                }
                let disabled = form.snapshot.as_ref().is_none_or(|snap| !snap.writable);
                for field in &form.fields {
                    field.editor.update(cx, |editor, cx| {
                        editor.disabled = disabled;
                        cx.notify();
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_runtime_settings(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &mut self.runtime_settings else {
            return;
        };
        let Some(snapshot) = form
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.writable && !form.saving)
        else {
            return;
        };
        let Ok(changes) = form.changes(snapshot, cx) else {
            return;
        };
        if changes.is_empty() {
            return;
        }
        let change = json!({ "namespace": snapshot.namespace, "revision": snapshot.revision, "fields": changes });
        form.saving = true;
        form.error = None;
        for field in &form.fields {
            field.editor.update(cx, |editor, cx| {
                editor.disabled = true;
                cx.notify();
            });
        }
        self.request_runtime_settings(Some(change), w, cx);
        cx.notify();
    }

    pub(super) fn render_runtime_settings(&self, w: &Window, cx: &Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let Some(form) = &self.runtime_settings else {
            return note("连接服务后可配置此插件。", cx);
        };
        let Some(snapshot) = &form.snapshot else {
            return note(
                if form.loading {
                    "正在读取设置…"
                } else {
                    form.error
                        .as_deref()
                        .unwrap_or("该插件当前未加载，暂时无法配置。")
                }
                .to_owned(),
                cx,
            );
        };
        let can_save = snapshot.writable
            && !form.saving
            && form
                .changes(snapshot, cx)
                .is_ok_and(|changes| !changes.is_empty());
        let mut controls = column().w_full().gap_0().when(!snapshot.writable, |form| {
            form.child(note("本部署的设置为只读。", cx))
        });
        for (index, field) in form.fields.iter().enumerate() {
            let spec = field.spec;
            let value = &snapshot.fields[spec.key];
            let change = field.change(value, cx);
            let invalid = change.is_err();
            let overridden = match change {
                Ok(Some(change)) => matches!(change, Change::Set(_)),
                Ok(None) => value.overridden,
                Err(()) => false,
            };
            let focused = field.editor.read(cx).focus.is_focused(w);
            let can_reset = snapshot.writable && !form.saving;
            let header = row()
                .w_full()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_weight(FontWeight::MEDIUM)
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
                                    format!("runtime-reset-{}", spec.key).into(),
                                ))
                                .role(gpui::Role::Button)
                                .tab_stop(can_reset)
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(p.label_secondary)
                                .when(can_reset, |button| {
                                    button
                                        .cursor_pointer()
                                        .hover(|style| style.text_color(p.label_primary))
                                })
                                .child("恢复默认"),
                            cx.listener(move |app, _, w, cx| {
                                let Some(form) = &mut app.runtime_settings else {
                                    return;
                                };
                                let Some(snap) = form
                                    .snapshot
                                    .as_ref()
                                    .filter(|snap| snap.writable && !form.saving)
                                else {
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
                                replace(&field.editor, snap.fields[spec.key].default_text(), w, cx);
                                form.error = None;
                                cx.notify();
                            }),
                        ))
                });
            controls = controls.child(
                column()
                    .w_full()
                    .py(px(12.))
                    .gap(px(6.))
                    .when(index > 0, |field| {
                        field.border_t(px(0.5)).border_color(p.border_l2)
                    })
                    .text_size(px(13.))
                    .line_height(px(19.5))
                    .text_color(p.label_primary)
                    .child(header)
                    .child(
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
                            .child(field.editor.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(if invalid {
                                p.state_error_primary
                            } else {
                                p.label_tertiary
                            })
                            .child(if invalid {
                                if spec.integer {
                                    "请输入正整数，留空表示使用默认值。"
                                } else {
                                    "请输入正数，留空表示使用默认值。"
                                }
                            } else {
                                spec.hint
                            }),
                    ),
            );
        }
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
                        .id("runtime-settings-save")
                        .role(gpui::Role::Button)
                        .tab_stop(can_save)
                        .h(px(32.))
                        .px(px(15.))
                        .rounded(px(12.))
                        .text_size(px(13.))
                        .line_height(px(19.5))
                        .bg(p.label_primary)
                        .text_color(p.bg_layer_3)
                        .when(can_save, |button| button.cursor_pointer())
                        .when(!can_save, |button| button.opacity(0.4))
                        .child(if form.saving {
                            "保存中…"
                        } else {
                            "保存"
                        }),
                    cx.listener(|app, _, w, cx| app.save_runtime_settings(w, cx)),
                )),
        )
    }
}
