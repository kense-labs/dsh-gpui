//! Harness Welcome flow backed by write-only Host credential commands.

use crate::{
    state::{account_signed_in, is_stale_account, validate_api_key},
    web::invoke,
};
use gpui::{App, ClipboardItem, Context, FontWeight, Subscription, Window, div, prelude::*, px};
use kui::components::button::Button;
use kui::primitives::editor::{EditEvent, Editor, InputMode};
use kui::primitives::family::Paint;
use kui::primitives::glyph::{Glyph, icon};
use kui::primitives::ops_surface::{column, editor, input, note, replace, row};
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AuthPage {
    Entry,
    Key,
    SignIn,
}

#[derive(Clone, Copy)]
pub enum WelcomeEvent {
    Changed,
}

pub struct Welcome {
    pub auth: Option<Value>,
    pub profile: Option<Value>,
    pub balance: Option<Value>,
    pub avatar: Option<String>,
    pub avatar_url: Option<String>,
    pub page: AuthPage,
    pub busy: bool,
    pub copied: bool,
    pub error: Option<String>,
    api_key: gpui::Entity<Editor>,
    _subscriptions: Vec<Subscription>,
}

impl gpui::EventEmitter<WelcomeEvent> for Welcome {}

impl Welcome {
    pub fn new(w: &mut Window, cx: &mut Context<Self>) -> Self {
        let api_key = editor("输入 API 密钥".into(), InputMode::Password, w, cx);
        let submit = cx.subscribe_in(&api_key, w, |this, _, event: &EditEvent, _, cx| {
            if *event == EditEvent::Submitted {
                this.save_api_key(cx);
            }
        });
        Self {
            auth: None,
            profile: None,
            balance: None,
            avatar: None,
            avatar_url: None,
            page: AuthPage::Entry,
            busy: true,
            copied: false,
            error: None,
            api_key,
            _subscriptions: vec![submit],
        }
    }

    pub fn configured(&self) -> bool {
        self.auth.as_ref().is_some_and(|status| {
            status.pointer("/account/status").and_then(Value::as_str) == Some("credential-stored")
                || status
                    .pointer("/apiKey/configured")
                    .and_then(Value::as_bool)
                    == Some(true)
        })
    }

    pub fn signed_in(&self) -> bool {
        account_signed_in(self.auth.as_ref())
    }

    pub fn open(&mut self, page: AuthPage, cx: &mut Context<Self>) {
        self.page = page;
        self.error = None;
        self.copied = false;
        cx.notify();
    }

    pub fn handle_reply(
        &mut self,
        command: &'static str,
        result: Result<Value, String>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            "start_auth" | "auth_status" | "save_api_key" | "remove_api_key" => {
                self.busy = false;
                match result {
                    Ok(status) => {
                        self.apply_status(status);
                        self.error = None;
                        if command == "save_api_key" {
                            replace(&self.api_key, "", w, cx);
                        }
                        if self.signed_in() {
                            invoke("auth_profile", json!({}));
                        }
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            "start_sign_in" | "cancel_sign_in" | "sign_out" => {
                self.busy = false;
                match result {
                    Ok(account) => {
                        self.apply_account(account);
                        self.error = None;
                        if self.signed_in() {
                            invoke("auth_profile", json!({}));
                        }
                    }
                    Err(error) => {
                        self.error = Some(error);
                        if command == "start_sign_in" {
                            self.page = AuthPage::Entry;
                        }
                    }
                }
            }
            "auth_profile" => {
                if let Ok(details) = result {
                    self.profile = details.pointer("/profile/value").cloned();
                    self.balance = details.get("balance").cloned();
                    let avatar_url = self
                        .profile
                        .as_ref()
                        .and_then(|profile| profile.get("avatarUrl"))
                        .and_then(Value::as_str)
                        .filter(|url| url.starts_with("https://"))
                        .map(str::to_string);
                    if avatar_url != self.avatar_url
                        || (avatar_url.is_some() && self.avatar.is_none())
                    {
                        self.avatar = None;
                        self.avatar_url = avatar_url.clone();
                        if let Some(url) = avatar_url {
                            invoke("auth_avatar", json!({"url": url}));
                        }
                    }
                }
            }
            "auth_avatar" => {
                if let Ok(details) = result
                    && details.get("url").and_then(Value::as_str) == self.avatar_url.as_deref()
                {
                    self.avatar = details
                        .get("data")
                        .and_then(Value::as_str)
                        .filter(|data| data.starts_with("data:image/"))
                        .map(str::to_string);
                }
            }
            "open_sign_in" | "open_account_page" => {
                if let Err(error) = result {
                    self.error = Some(error);
                }
            }
            _ => {}
        }
        cx.emit(WelcomeEvent::Changed);
        cx.notify();
    }

    pub fn handle_event(&mut self, event: &Value, cx: &mut Context<Self>) {
        match event.get("kind").and_then(Value::as_str) {
            Some("account") => {
                if let Some(account) = event.get("state") {
                    self.apply_account(account.clone());
                    self.busy = false;
                    self.copied = false;
                    if self.signed_in() {
                        invoke("auth_profile", json!({}));
                    }
                }
            }
            Some("auth") => {
                self.busy = false;
                self.error = event
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| Some("Harness 账户服务不可用".to_string()));
            }
            _ => {}
        }
        cx.emit(WelcomeEvent::Changed);
        cx.notify();
    }

    fn apply_status(&mut self, status: Value) {
        if let Some(account) = status.get("account") {
            let stale = self
                .auth
                .as_ref()
                .and_then(|current| current.get("account"))
                .is_some_and(|current| is_stale_account(current, account));
            if stale {
                if let Some(current) = self.auth.as_mut()
                    && let Some(api_key) = status.get("apiKey")
                {
                    current["apiKey"] = api_key.clone();
                }
                return;
            }
            self.apply_account(account.clone());
        }
        self.auth = Some(status);
    }

    fn apply_account(&mut self, account: Value) {
        if self
            .auth
            .as_ref()
            .and_then(|status| status.get("account"))
            .is_some_and(|current| is_stale_account(current, &account))
        {
            return;
        }
        let attempt = account.get("attempt").cloned().unwrap_or(Value::Null);
        if let Some(status) = self.auth.as_mut() {
            status["account"] = account;
        } else {
            self.auth = Some(json!({
                "account": account,
                "apiKey": {"configured": false, "writable": true}
            }));
        }
        if !self.signed_in() {
            self.profile = None;
            self.balance = None;
            self.avatar = None;
            self.avatar_url = None;
        }
        let phase = attempt.get("phase").and_then(Value::as_str);
        if attempt.is_null() || phase == Some("cancelled") {
            if self.page == AuthPage::SignIn {
                self.page = AuthPage::Entry;
            }
        } else {
            self.page = AuthPage::SignIn;
        }
    }

    fn save_api_key(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let value = self.api_key.read(cx).value().trim().to_string();
        if let Err(error) = validate_api_key(&value) {
            self.error = Some(error.to_string());
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        invoke("save_api_key", json!({"value": value}));
        cx.notify();
    }

    pub fn start_sign_in(&mut self, cx: &mut Context<Self>) {
        self.page = AuthPage::SignIn;
        self.busy = true;
        self.error = None;
        self.copied = false;
        invoke("start_sign_in", json!({}));
        cx.notify();
    }

    fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        let attempt_id = self
            .auth
            .as_ref()
            .and_then(|auth| auth.pointer("/account/attempt/id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(attempt_id) = attempt_id {
            self.busy = true;
            invoke("cancel_sign_in", json!({"attemptId": attempt_id}));
            cx.notify();
        }
    }

    fn copy_sign_in_link(&mut self, cx: &mut Context<Self>) {
        let url = self
            .auth
            .as_ref()
            .and_then(|auth| auth.pointer("/account/attempt/authorizeUrl"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(url) = url {
            cx.write_to_clipboard(ClipboardItem::new_string(url));
            self.copied = true;
            cx.notify();
        }
    }

    /// Compact account controls for the workspace settings column.
    pub fn render_inline(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        let signed_in = self.signed_in();
        let configured = self
            .auth
            .as_ref()
            .and_then(|auth| auth.pointer("/apiKey/configured"))
            .and_then(Value::as_bool)
            == Some(true);
        let mut content = column().w_full().gap(px(10.));
        if self.page == AuthPage::Key {
            content = content
                .child(note("密钥只写入 Harness 凭据文件，不会回显到界面。", cx))
                .child(input(&self.api_key, cx))
                .child(
                    row()
                        .gap(px(6.))
                        .child(
                            Button::new("inline-save-api-key")
                                .label("保存")
                                .primary()
                                .loading(self.busy)
                                .disabled(
                                    self.busy || self.api_key.read(cx).value().trim().is_empty(),
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.save_api_key(cx)))
                                .into_themed(cx),
                        )
                        .child(
                            Button::new("inline-cancel-api-key")
                                .label("取消")
                                .ghost()
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, w, cx| {
                                    replace(&this.api_key, "", w, cx);
                                    this.open(AuthPage::Entry, cx);
                                }))
                                .into_themed(cx),
                        ),
                )
                .when(configured, |element| {
                    element.child(
                        Button::new("inline-remove-api-key")
                            .label("移除已保存的 Key")
                            .ghost()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.busy = true;
                                invoke("remove_api_key", json!({}));
                                cx.notify();
                            }))
                            .into_themed(cx),
                    )
                });
        } else if self.page == AuthPage::SignIn && !signed_in {
            let phase = self
                .auth
                .as_ref()
                .and_then(|auth| auth.pointer("/account/attempt/phase"))
                .and_then(Value::as_str)
                .unwrap_or("initializing");
            let waiting = phase == "waiting-browser";
            content = content
                .child(note(
                    if waiting {
                        "浏览器授权后可回到这里继续。"
                    } else {
                        "正在准备浏览器授权…"
                    },
                    cx,
                ))
                .when(waiting, |element| {
                    element.child(
                        row()
                            .gap(px(6.))
                            .child(
                                Button::new("inline-copy-sign-in")
                                    .label(if self.copied {
                                        "已复制链接"
                                    } else {
                                        "复制登录链接"
                                    })
                                    .secondary()
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.copy_sign_in_link(cx)),
                                    )
                                    .into_themed(cx),
                            )
                            .child(
                                Button::new("inline-open-sign-in")
                                    .icon_only(Glyph::Link)
                                    .label("重新打开浏览器")
                                    .ghost()
                                    .on_click(
                                        cx.listener(|_, _, _, _| invoke("open_sign_in", json!({}))),
                                    )
                                    .into_themed(cx),
                            ),
                    )
                })
                .child(
                    Button::new("inline-cancel-sign-in")
                        .label("取消登录")
                        .ghost()
                        .disabled(self.busy && !waiting)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_sign_in(cx)))
                        .into_themed(cx),
                );
        } else {
            content = content
                .child(
                    row()
                        .w_full()
                        .child(icon(Glyph::User, 16., p.secondary))
                        .child(div().flex_1().text_size(px(13.)).child(if signed_in {
                            "账户已登录"
                        } else {
                            "尚未登录"
                        }))
                        .child(div().size(px(6.)).rounded_full().bg(if signed_in {
                            p.success
                        } else {
                            p.faint
                        })),
                )
                .child(
                    row()
                        .w_full()
                        .child(icon(Glyph::Lock, 15., p.secondary))
                        .child(div().flex_1().text_size(px(13.)).child("API Key"))
                        .child(note(if configured { "已保存" } else { "未配置" }, cx)),
                )
                .child(
                    row()
                        .gap(px(6.))
                        .child(
                            Button::new("inline-sign-in")
                                .label(if signed_in { "退出登录" } else { "登录" })
                                .secondary()
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if this.signed_in() {
                                        this.busy = true;
                                        invoke("sign_out", json!({}));
                                        cx.notify();
                                    } else {
                                        this.start_sign_in(cx);
                                    }
                                }))
                                .into_themed(cx),
                        )
                        .child(
                            Button::new("inline-configure-api-key")
                                .label(if configured {
                                    "更新 Key"
                                } else {
                                    "添加 Key"
                                })
                                .ghost()
                                .disabled(self.busy)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.open(AuthPage::Key, cx)),
                                )
                                .into_themed(cx),
                        ),
                );
        }
        content.when_some(self.error.clone(), |element, error| {
            element.child(
                div()
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(p.danger)
                    .child(error),
            )
        })
    }

    fn render_entry(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        column()
            .w_full()
            .child(dialog_heading("开始使用"))
            .child(dialog_description(
                "登录 DeepSeek 账号，或添加 API Key，即可开始使用。你的项目和文件保存在本地。",
                cx,
            ))
            .children(
                self.error
                    .clone()
                    .map(|error| dialog_error(error, p.danger)),
            )
            .child(
                dialog_actions()
                    .child(
                        Button::new("welcome-api-key")
                            .label("添加 API Key")
                            .secondary()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.open(AuthPage::Key, cx)))
                            .into_themed(cx),
                    )
                    .child(
                        Button::new("welcome-sign-in")
                            .label("登录")
                            .loading(self.busy && self.auth.is_none())
                            .disabled(self.busy)
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| this.start_sign_in(cx)))
                            .into_themed(cx),
                    ),
            )
    }

    fn render_key(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        let configured = self
            .auth
            .as_ref()
            .and_then(|auth| auth.pointer("/apiKey/configured"))
            .and_then(Value::as_bool)
            == Some(true);
        column()
            .w_full()
            .child(dialog_heading("添加 API Key"))
            .child(dialog_description(
                "密钥只写入 Harness 凭据文件，不会回显到界面。保存后仍需登录 DeepSeek 账号进入工作区。",
                cx,
            ))
            .child(
                div()
                    .w_full()
                    .px(px(24.))
                    .pt(px(18.))
                    .child(input(&self.api_key, cx)),
            )
            .children(self.error.clone().map(|error| dialog_error(error, p.danger)))
            .child(
                dialog_actions()
                    .pt(px(18.))
                    .child(
                        Button::new("back-to-login")
                            .label("返回")
                            .secondary()
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, w, cx| {
                                replace(&this.api_key, "", w, cx);
                                this.open(AuthPage::Entry, cx);
                            }))
                            .into_themed(cx),
                    )
                    .child(
                        Button::new("save-api-key")
                            .label(if configured { "更新 API Key" } else { "保存 API Key" })
                            .primary()
                            .loading(self.busy)
                            .disabled(self.busy || self.api_key.read(cx).value().trim().is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.save_api_key(cx)))
                            .into_themed(cx),
                    ),
            )
            .when(configured, |element| {
                element.child(
                    row()
                        .w_full()
                        .justify_center()
                        .pb(px(16.))
                        .child(
                            Button::new("remove-api-key")
                                .label("移除已保存的 API Key")
                                .ghost()
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.busy = true;
                                    invoke("remove_api_key", json!({}));
                                    cx.notify();
                                }))
                                .into_themed(cx),
                        ),
                )
            })
    }

    fn render_sign_in(&mut self, cx: &mut Context<Self>) -> gpui::Div {
        let p = Paint::get(cx);
        let attempt = self
            .auth
            .as_ref()
            .and_then(|auth| auth.pointer("/account/attempt"));
        let phase = if self.busy && attempt.is_none_or(Value::is_null) {
            "initializing"
        } else {
            attempt
                .and_then(|attempt| attempt.get("phase"))
                .and_then(Value::as_str)
                .unwrap_or("initializing")
        };
        let failed = matches!(phase, "expired" | "failed" | "cancelled");
        let waiting = matches!(
            phase,
            "initializing" | "waiting-browser" | "exchanging" | "committing"
        ) && !failed;
        let heading = match phase {
            "expired" => "登录已超时",
            "failed" | "cancelled" => "登录失败",
            _ => "等待登录",
        };
        let description = match phase {
            "waiting-browser" => "没有自动打开新页面？复制登录链接，手动打开登录页完成登录。",
            "expired" => "登录链接已过期，请重新登录。",
            "failed" | "cancelled" => "未能完成登录，请重试。",
            _ => "请在系统浏览器中完成 DeepSeek 账号授权。",
        };
        column()
            .w_full()
            .child(dialog_heading(heading))
            .child(dialog_description(description, cx))
            .when(phase == "waiting-browser", |element| {
                element.child(
                    row()
                        .w_full()
                        .px(px(24.))
                        .pt(px(12.))
                        .gap(px(8.))
                        .child(
                            Button::new("copy-sign-in")
                                .label(if self.copied {
                                    "已复制链接"
                                } else {
                                    "复制登录链接"
                                })
                                .with_icon(
                                    if self.copied {
                                        Glyph::Check
                                    } else {
                                        Glyph::Copy
                                    },
                                    false,
                                )
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| this.copy_sign_in_link(cx)))
                                .into_themed(cx),
                        )
                        .child(
                            Button::new("reopen-sign-in")
                                .icon_only(Glyph::Link)
                                .label("重新打开浏览器")
                                .ghost()
                                .on_click(
                                    cx.listener(|_, _, _, _| invoke("open_sign_in", json!({}))),
                                )
                                .into_themed(cx),
                        ),
                )
            })
            .children(
                self.error
                    .clone()
                    .map(|error| dialog_error(error, p.danger)),
            )
            .child(
                dialog_actions()
                    .child(
                        Button::new("cancel-sign-in")
                            .label("取消")
                            .secondary()
                            .loading(self.busy && phase != "initializing")
                            .disabled(self.busy || phase == "committing")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.auth.as_ref().is_some_and(|auth| {
                                    auth.pointer("/account/attempt/id")
                                        .and_then(Value::as_str)
                                        .is_some()
                                }) {
                                    this.cancel_sign_in(cx);
                                } else {
                                    this.open(AuthPage::Entry, cx);
                                }
                            }))
                            .into_themed(cx),
                    )
                    .child(
                        Button::new("retry-sign-in")
                            .label(if failed {
                                "重新登录"
                            } else {
                                "等待登录"
                            })
                            .loading(waiting)
                            .disabled(waiting || self.busy || phase == "committing")
                            .primary()
                            .on_click(cx.listener(|this, _, _, cx| this.start_sign_in(cx)))
                            .into_themed(cx),
                    ),
            )
    }
}

fn dialog_heading(title: &'static str) -> gpui::Div {
    div()
        .w_full()
        .px(px(24.))
        .pt(px(22.))
        .pb(px(12.))
        .text_size(px(16.))
        .line_height(px(24.))
        .font_weight(FontWeight::MEDIUM)
        .child(title)
}

fn dialog_description(description: &'static str, cx: &App) -> gpui::Div {
    div()
        .w_full()
        .px(px(24.))
        .text_size(px(14.))
        .line_height(px(22.))
        .text_color(Paint::get(cx).ink)
        .child(description)
}

fn dialog_error(error: String, color: gpui::Hsla) -> gpui::Div {
    div()
        .w_full()
        .px(px(24.))
        .pt(px(12.))
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(color)
        .child(error)
}

fn dialog_actions() -> gpui::Div {
    row()
        .w_full()
        .justify_between()
        .items_center()
        .gap(px(8.))
        .px(px(24.))
        .pt(px(24.))
        .pb(px(22.))
}

impl Render for Welcome {
    fn render(&mut self, _w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Paint::get(cx);
        let content = match self.page {
            AuthPage::Entry => self.render_entry(cx),
            AuthPage::Key => self.render_key(cx),
            AuthPage::SignIn => self.render_sign_in(cx),
        };
        column()
            .size_full()
            .bg(p.stage)
            .text_color(p.ink)
            .child(
                row()
                    .w_full()
                    .px(px(32.))
                    .py(px(24.))
                    .gap(px(10.))
                    .child(
                        gpui::svg()
                            .data(include_bytes!("../assets/deepseek-fish.svg"))
                            .w(px(28.))
                            .h(px(28. * 17.04 / 23.16))
                            .text_color(p.ink),
                    )
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("DeepSeek"),
                    )
                    .child(div().text_color(p.secondary).child("Harness")),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(24.))
                    .pb(px(64.))
                    .child(
                        div()
                            .w(px(440.))
                            .max_w_full()
                            .overflow_hidden()
                            .rounded(px(12.))
                            .border(px(0.5))
                            .border_color(p.line)
                            .bg(p.panel)
                            .shadow_lg()
                            .child(content),
                    ),
            )
    }
}
