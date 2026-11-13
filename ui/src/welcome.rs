//! Harness Welcome flow backed by write-only Host credential commands.

use crate::{
    brand, bridge,
    state::validate_api_key,
    tokens::{Tokens, dsh},
    web::{dock_action_button, invoke},
};
use gpui::{App, Context, FontWeight, Subscription, Window, div, prelude::*, px};
use kui::components::button::Button;
use kui::primitives::editor::{EditEvent, Editor, InputMode};
use kui::primitives::glyph::{Glyph, icon};
use kui::primitives::ops_surface::{column, editor, icon_btn, input, note, replace, row};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub enum WelcomeEvent {
    Changed,
    OpenModelProviders,
}

pub struct Welcome {
    configured: bool,
    editing_key: bool,
    busy: bool,
    checking_key: bool,
    error: Option<String>,
    skipped: bool,
    api_key: gpui::Entity<Editor>,
    _subscriptions: Vec<Subscription>,
}

impl gpui::EventEmitter<WelcomeEvent> for Welcome {}

impl Welcome {
    pub fn new(w: &mut Window, cx: &mut Context<Self>) -> Self {
        let api_key = editor("输入 API 密钥".into(), InputMode::Password, w, cx);
        let submit = cx.subscribe_in(
            &api_key,
            w,
            |this, _, event: &EditEvent, w, cx| match event {
                EditEvent::Submitted => this.save_api_key(cx),
                EditEvent::Changed => {
                    this.error = None;
                    this.refresh(cx);
                }
                EditEvent::Focused | EditEvent::Blurred => cx.notify(),
                EditEvent::Escape if !this.busy && this.editing_key => {
                    this.clear_api_key(w, cx);
                    this.refresh(cx);
                }
                _ => {}
            },
        );
        Self {
            configured: false,
            editing_key: false,
            busy: true,
            checking_key: true,
            error: None,
            skipped: false,
            api_key,
            _subscriptions: vec![
                submit,
                cx.observe_window_appearance(w, |_, _, cx| cx.notify()),
            ],
        }
    }

    pub fn configured(&self) -> bool {
        self.configured
    }

    pub fn should_show(&self) -> bool {
        !self.skipped && !self.configured()
    }

    pub fn use_saved_models(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.should_show() && self.api_key.read(cx).value().is_empty() {
            self.skip_key(w, cx);
        }
    }

    fn skip_key(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.busy || self.checking_key {
            self.clear_api_key(w, cx);
            self.skipped = true;
            self.refresh(cx);
        }
    }

    fn clear_api_key(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        replace(&self.api_key, "", w, cx);
        self.editing_key = false;
        self.error = None;
    }

    /// Official ApiKeyInput reveal control: only the unsaved draft can be
    /// revealed; stored keys stay write-only on the Host.
    fn toggle_key_reveal(&mut self, cx: &mut Context<Self>) {
        let revealed = !self.api_key.read(cx).revealed;
        self.api_key.update(cx, |editor, cx| {
            editor.revealed = revealed;
            cx.notify();
        });
        cx.notify();
    }

    fn refresh(&self, cx: &mut Context<Self>) {
        cx.emit(WelcomeEvent::Changed);
        cx.notify();
    }

    pub fn handle_reply(
        &mut self,
        command: &'static str,
        result: Result<Value, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = false;
        self.checking_key = false;
        match result {
            Ok(status) => {
                self.configured = status.pointer("/apiKey/configured") == Some(&Value::Bool(true));
                self.error = None;
                if command == "save_api_key" || command == "remove_api_key" {
                    self.clear_api_key(w, cx);
                }
            }
            Err(error) => self.error = Some(error),
        }
        self.refresh(cx);
    }

    pub fn handle_event(&mut self, event: &Value, cx: &mut Context<Self>) {
        self.busy = false;
        self.checking_key = false;
        self.error = Some(
            event["detail"]
                .as_str()
                .unwrap_or("Harness 凭据服务不可用")
                .to_owned(),
        );
        cx.notify();
    }

    fn save_api_key(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let value = self.api_key.read(cx).value().trim().to_string();
        if let Err(error) = validate_api_key(&value) {
            self.error = Some(error.to_string());
            self.refresh(cx);
            return;
        }
        self.busy = true;
        self.error = None;
        invoke("save_api_key", json!({"value": value}));
        self.refresh(cx);
    }

    pub fn render_inline(&self, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let mut content = column()
            .w_full()
            .gap(px(10.))
            .child(
                row()
                    .w_full()
                    .gap(px(8.))
                    .child(icon(Glyph::Lock, 15., p.secondary))
                    .child(div().flex_1().text_size(px(14.)).child("DeepSeek API Key"))
                    .child(note(
                        if self.configured() {
                            "已配置"
                        } else {
                            "未配置"
                        },
                        cx,
                    )),
            )
            .child(note("API Key 存储在本机 Harness 凭据文件中。", cx));
        if self.editing_key {
            let draft = self.api_key.read(cx);
            let revealed = draft.revealed;
            let has_value = !draft.value().is_empty();
            let reveal_label = if revealed {
                "隐藏密钥"
            } else {
                "显示密钥"
            };
            let mut key_row = row()
                .w_full()
                .gap(px(6.))
                .items_center()
                .child(div().flex_1().min_w_0().child(input(&self.api_key, cx)));
            if has_value {
                key_row = key_row.child(icon_btn(
                    "api-key-reveal",
                    reveal_label,
                    Glyph::Eye,
                    cx,
                    |this: &mut Self, _, cx| this.toggle_key_reveal(cx),
                ));
            }
            content = content.child(key_row).child(
                row()
                    .gap(px(6.))
                    .child(
                        Button::new("inline-save-api-key")
                            .label("保存")
                            .primary()
                            .loading(self.busy)
                            .disabled(self.busy || self.api_key.read(cx).value().trim().is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.save_api_key(cx)))
                            .into_themed(cx),
                    )
                    .child(
                        Button::new("inline-cancel-api-key")
                            .label("取消")
                            .ghost()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.clear_api_key(w, cx);
                                this.refresh(cx);
                            }))
                            .into_themed(cx),
                    ),
            );
        } else {
            content = content.child(
                row()
                    .gap(px(6.))
                    .child(
                        Button::new("inline-configure-api-key")
                            .label(if self.configured() {
                                "更新 API Key"
                            } else {
                                "添加 API Key"
                            })
                            .secondary()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.editing_key = true;
                                this.error = None;
                                cx.on_next_frame(w, |this, w, cx| {
                                    if this.editing_key {
                                        this.api_key.read(cx).focus.clone().focus(w, cx);
                                    }
                                });
                                this.refresh(cx);
                            }))
                            .into_themed(cx),
                    )
                    .when(self.configured(), |actions| {
                        actions.child(
                            Button::new("inline-remove-api-key")
                                .label("移除 API Key")
                                .ghost()
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.busy = true;
                                    invoke("remove_api_key", json!({}));
                                    this.refresh(cx);
                                }))
                                .into_themed(cx),
                        )
                    }),
            );
        }
        content.children(
            self.error
                .clone()
                .map(|error| div().text_size(px(12.)).text_color(p.danger).child(error)),
        )
    }

    fn render_key(&self, p: Tokens, w: &Window, cx: &mut Context<Self>) -> (gpui::Div, gpui::Div) {
        let editor = self.api_key.read(cx);
        let can_save = !self.busy && !editor.value().trim().is_empty();
        let configuring = self.busy && !self.checking_key;
        let focused = editor.focus.is_focused(w);
        let content = column()
            .w(px(440.))
            .max_w_full()
            .items_center()
            .gap(px(40.))
            .child(welcome_heading(
                "配置模型后开始使用",
                Some("使用 DeepSeek API Key，或添加 OpenAI 兼容服务"),
                p,
            ))
            .child(
                div()
                    .relative()
                    .w_full()
                    .child(
                        row()
                            .w_full()
                            .h(px(48.))
                            .px(px(14.))
                            .rounded(px(12.))
                            .border_1()
                            .border_color(if focused {
                                p.label_primary
                            } else {
                                p.border_l4
                            })
                            .child(self.api_key.clone()),
                    )
                    .children(self.error.clone().map(|error| {
                        welcome_error(error, p)
                            .absolute()
                            .top(px(56.))
                            .left_0()
                            .w_full()
                    })),
            );
        let actions = welcome_actions()
            .child(dock_action_button(
                welcome_button("save-api-key", "保存并继续", true, !can_save, p, cx),
                cx.listener(move |this, _, _, cx| {
                    if can_save {
                        this.save_api_key(cx);
                    }
                }),
            ))
            .child(dock_action_button(
                welcome_button(
                    "configure-model-providers",
                    "配置模型服务",
                    false,
                    configuring,
                    p,
                    cx,
                ),
                cx.listener(|this, _, w, cx| {
                    if !this.busy || this.checking_key {
                        this.skip_key(w, cx);
                        cx.emit(WelcomeEvent::OpenModelProviders);
                    }
                }),
            ))
            .child(dock_action_button(
                welcome_button("skip-key", "稍后配置", false, configuring, p, cx),
                cx.listener(|this, _, w, cx| this.skip_key(w, cx)),
            ));
        (content, actions)
    }
}

fn welcome_actions() -> gpui::Div {
    column()
        .relative()
        .w(px(240.))
        .h(px(164.))
        .flex_none()
        .gap(px(16.))
}

fn welcome_heading(title: &'static str, description: Option<&'static str>, p: Tokens) -> gpui::Div {
    column()
        .items_center()
        .text_center()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(20.))
                .line_height(px(32.))
                .font_weight(FontWeight::NORMAL)
                .child(title),
        )
        .children(description.map(|description| {
            div()
                .text_size(px(16.))
                .line_height(px(22.))
                .font_weight(FontWeight::LIGHT)
                .text_color(p.label_secondary)
                .child(description)
        }))
}

fn welcome_error(error: String, p: Tokens) -> gpui::Div {
    div()
        .text_size(px(14.))
        .line_height(px(22.))
        .text_color(p.label_primary)
        .child(error)
}

fn welcome_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    disabled: bool,
    p: Tokens,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let (fill, hover, border, ink) = if primary {
        (
            p.button_primary_fill,
            p.button_primary_hover,
            gpui::transparent_black(),
            p.label_primary_inverted,
        )
    } else {
        (
            p.button_elevated,
            p.button_floating_hover,
            p.border_l2,
            p.label_primary,
        )
    };
    row()
        .id(id)
        .relative()
        .role(gpui::Role::Button)
        .aria_label(label)
        .tab_index(0)
        .tab_stop(!disabled)
        .w_full()
        .h(px(44.))
        .flex_none()
        .justify_center()
        .rounded(px(10.))
        .border_1()
        .border_color(border)
        .bg(fill)
        .text_color(ink)
        .font_family(if crate::bridge::is_windows() {
            "Segoe UI"
        } else {
            ".SystemUIFont"
        })
        .when_some(
            crate::web::document_fonts::interface_family(cx),
            |button, family| button.font_family(family),
        )
        .font_weight(FontWeight::MEDIUM)
        .text_size(px(14.))
        .line_height(px(22.))
        .when(disabled, |button| button.opacity(0.4).cursor_not_allowed())
        .when(!disabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(hover))
                .focus_visible(|style| {
                    style.shadow(vec![
                        gpui::BoxShadow::new(px(0.), px(0.), p.label_primary).spread_radius(px(5.)),
                        gpui::BoxShadow::new(px(0.), px(0.), p.stage).spread_radius(px(3.)),
                    ])
                })
        })
        .child(label)
}

impl Render for Welcome {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dark = matches!(
            w.appearance(),
            gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
        );
        let p = if dark {
            Tokens::dark()
        } else {
            Tokens::light()
        };
        let (content, actions) = self.render_key(p, w, cx);
        let tint = if bridge::has_mac_window_material() {
            match w.appearance() {
                gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark => {
                    gpui::rgb(0x18191c).opacity(0.5).into()
                }
                _ => gpui::white().opacity(0.4),
            }
        } else {
            p.stage
        };
        column()
            .size_full()
            .overflow_hidden()
            .items_center()
            .px(px(16.))
            .pt(px(74.))
            .pb(px(82.))
            .bg(tint)
            .text_color(p.label_primary)
            .font_family("Montserrat")
            .child(brand::wordmark(40., 34., dark))
            .child(
                row()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .justify_center()
                    .child(content),
            )
            .child(actions)
    }
}
