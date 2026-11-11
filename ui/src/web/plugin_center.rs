//! Plugin configuration pages and the connected Host's inventory.

use super::*;
use super::{
    plugin_install::PluginTarget,
    plugin_versions::{VersionCheck, version_button},
    runtime_settings::RuntimePage,
};
use gpui::{Div, Entity, FocusHandle, Stateful};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, LazyLock};

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Config {
    Subagent,
    WebSearch,
    AgentLoop,
    Shell,
}

impl Config {
    const ORDER: [Self; 4] = [
        Self::Subagent,
        Self::WebSearch,
        Self::AgentLoop,
        Self::Shell,
    ];

    fn info(self) -> (&'static str, &'static str, &'static str, Artwork) {
        match self {
            Self::Subagent => (
                "plugin-config-subagent",
                "子智能体",
                "设置子智能体的递归层级、数量和模型。",
                Artwork::Subagent,
            ),
            Self::WebSearch => (
                "plugin-config-web-search",
                "网页搜索与获取",
                "选择网页搜索与网页获取使用的提供方。",
                Artwork::Search,
            ),
            Self::AgentLoop => (
                "plugin-config-agent-loop",
                "Agent 循环",
                "控制 Agent 派发工具调用的方式。",
                Artwork::Loop,
            ),
            Self::Shell => (
                "plugin-config-shell",
                "终端",
                "限制每条命令最多能跑多久、最多输出多少内容。",
                Artwork::Terminal,
            ),
        }
    }
}

#[derive(Clone)]
enum Detail {
    Config(Config),
    Bundle(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    entry_id: String,
    module_name: String,
    enabled: bool,
    fiber_phase: Option<String>,
    title: Option<String>,
    description: Option<String>,
    diagnostic: Option<String>,
    read_only_reason: Option<String>,
}

impl Entry {
    fn title(&self) -> &str {
        self.title.as_deref().unwrap_or_else(|| {
            let module = self
                .module_name
                .rsplit('/')
                .next()
                .unwrap_or(&self.module_name);
            [
                "cordis:",
                "cordis-plugin-",
                "dsh-host-",
                "dsh-client-",
                "dsh-",
            ]
            .iter()
            .find_map(|prefix| module.strip_prefix(prefix))
            .unwrap_or(module)
        })
    }

    fn status(&self) -> &'static str {
        match self.fiber_phase.as_deref() {
            Some("active") => "运行中",
            Some("failed") => "异常",
            Some("loading") => "加载中",
            Some("pending") => "等待依赖",
            Some("unloading") => "卸载中",
            _ if self.enabled => "未运行",
            _ => "已关闭",
        }
    }

    fn matches(&self, query: &str) -> bool {
        [
            self.title(),
            &self.entry_id,
            &self.module_name,
            self.description.as_deref().unwrap_or(""),
        ]
        .iter()
        .any(|value| value.to_lowercase().contains(query))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bundle {
    name: String,
    version: Option<String>,
    title: Option<String>,
    description: Option<String>,
    installed: bool,
    official: bool,
    enabled: bool,
    removable: bool,
    availability: String,
    read_only_reason: Option<String>,
    error: Option<String>,
    catalog_version: Option<String>,
    #[serde(default)]
    catalog_update: bool,
    #[serde(default)]
    updatable: bool,
    more_only: bool,
    rank: usize,
    rows: Vec<BundleRow>,
}

impl Bundle {
    fn title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.name)
    }

    fn lock(&self) -> Option<&str> {
        self.read_only_reason.as_deref().or_else(|| {
            if self.enabled {
                None
            } else if self.availability == "missing" && self.catalog_version.is_none() {
                Some("missing-bundle")
            } else {
                self.error.as_deref()
            }
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BundleRow {
    row_id: String,
    module_name: String,
    entry_id: Option<String>,
    title: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    configs: Vec<Config>,
    management_available: bool,
    entries: Vec<Entry>,
    bundles: Vec<Bundle>,
}

impl Snapshot {
    fn entry(&self, row: &BundleRow) -> Option<&Entry> {
        let id = row.entry_id.as_deref()?;
        self.entries.iter().find(|entry| entry.entry_id == id)
    }
}

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum Operation {
    Bundle {
        name: String,
        enabled: bool,
    },
    Entry {
        name: String,
        row_id: String,
        enabled: bool,
    },
    Remove {
        name: String,
    },
}

impl Operation {
    fn switch_id(&self) -> gpui::ElementId {
        let id = match self {
            Self::Bundle { name, .. } => format!("plugin-bundle-switch:{name}"),
            Self::Entry { name, row_id, .. } => format!("plugin-entry-switch:{name}:{row_id}"),
            Self::Remove { name } => format!("plugin-remove:{name}"),
        };
        gpui::ElementId::Name(id.into())
    }

    fn checked(&self, snapshot: &Snapshot) -> Option<bool> {
        let name = match self {
            Self::Bundle { name, .. } | Self::Entry { name, .. } => name,
            Self::Remove { .. } => return None,
        };
        let bundle = snapshot
            .bundles
            .iter()
            .find(|bundle| &bundle.name == name)?;
        match self {
            Self::Bundle { .. } => Some(bundle.enabled),
            Self::Entry { row_id, .. } => snapshot
                .entry(bundle.rows.iter().find(|row| &row.row_id == row_id)?)
                .map(|entry| entry.enabled),
            Self::Remove { .. } => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MutationReply {
    snapshot: Option<Snapshot>,
    outcome: MutationOutcome,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MutationOutcome {
    application: String,
    changed: Option<bool>,
    restart_reason: Option<String>,
    error: Option<String>,
}

impl MutationOutcome {
    fn notice(&self, removing: bool) -> Option<&'static str> {
        Some(match self.application.as_str() {
            "applied" if removing => "已卸载插件。",
            "applied" => return None,
            "restart-required" if removing && self.changed == Some(true) => {
                "已卸载插件。请重新连接 ACP 服务，使变更生效。"
            }
            "restart-required" if self.restart_reason.as_deref() == Some("no-hmr") => {
                "已保存插件变更。当前 ACP 配置在重新连接后应用。"
            }
            "restart-required" => "已保存插件变更。请重新连接 ACP 服务，使变更生效。",
            "overridden" => "已保存，但更高优先级的配置覆盖了此变更。",
            "cancelled" => "已取消插件操作。",
            _ => plugin_error(self.error.as_deref()),
        })
    }
}

fn plugin_error(code: Option<&str>) -> &'static str {
    match code {
        Some("install-running") => "请等待当前插件安装结束。",
        Some("management-required") => "插件管理依赖此组件，无法停用或卸载。",
        Some("unaddressable") => "此组件的配置无法通过当前 profile 修改。",
        Some("unknown-bundle" | "unknown-plugin") => "插件清单已变化，请刷新后重试。",
        Some("missing-bundle") => "当前 profile 缺少这个插件的安装文件。",
        Some("not-removable") => "当前 profile 无法卸载此插件。",
        Some("stop-profile" | "bundle-in-use") => {
            "插件仍在当前 ACP 服务中运行。请停用插件并重新连接，再卸载。"
        }
        Some("unsupported-platform") => "此插件不支持当前系统。",
        Some("incompatible-version") => "此插件与当前 Harness 版本不兼容。",
        _ => "Host 未接受插件变更。请刷新清单后重试。",
    }
}

struct RemoveConfirmation {
    name: String,
    cancel_focus: FocusHandle,
    confirm_focus: FocusHandle,
}

#[derive(Default)]
pub(super) struct PluginCenter {
    detail: Option<Detail>,
    more: bool,
    pending: Option<Rc<()>>,
    loaded: bool,
    inventory: Option<Snapshot>,
    inventory_error: Option<String>,
    mutation: Option<Operation>,
    switch_motion: Option<(Operation, DockSlide)>,
    confirm_remove: Option<RemoveConfirmation>,
    remove_trigger: Option<FocusHandle>,
    search: Option<(Entity<Editor>, Subscription)>,
    inventory_open: bool,
    expanded_entry: Option<String>,
    pub(super) versions: Option<VersionCheck>,
}

impl PluginCenter {
    pub(super) fn close_inventory(&mut self) {
        self.search = None;
        self.inventory_open = false;
        self.expanded_entry = None;
    }

    fn snapshot(&self) -> Option<&Snapshot> {
        self.inventory.as_ref()
    }
}

impl DshApp {
    pub(super) fn close_plugin_detail(&mut self) {
        self.plugin_center.detail = None;
        self.plugin_center.confirm_remove = None;
        self.plugin_center.versions = None;
        self.close_plugin_add_menu();
        self.runtime_settings = None;
        self.web_search_settings = None;
    }

    pub(super) fn refresh_plugin_center(&mut self, cx: &mut Context<Self>) {
        self.request_plugin_center(None, cx);
    }

    pub(super) fn can_install_plugins(&self) -> bool {
        self.connected()
            && self.plugin_center.pending.is_none()
            && self.plugin_center.inventory_error.is_none()
            && self
                .plugin_center
                .snapshot()
                .is_some_and(|snapshot| snapshot.management_available)
    }

    pub(super) fn activate_installed_plugin(&mut self, name: String, cx: &mut Context<Self>) {
        self.request_plugin_center(
            Some(Operation::Bundle {
                name,
                enabled: true,
            }),
            cx,
        );
    }

    fn request_plugin_center(&mut self, operation: Option<Operation>, cx: &mut Context<Self>) {
        if !self.connected()
            || self.plugin_center.pending.is_some()
            || operation.is_some() && self.plugin_install_busy()
        {
            return;
        }
        let request = Rc::new(());
        self.plugin_center.pending = Some(request.clone());
        self.plugin_center.mutation = operation.clone();
        cx.spawn(async move |this, cx| {
            let result = bridge::invoke("plugin_center", json!({"operation": operation})).await;
            let _ = this.update(cx, |app, cx| {
                if app
                    .plugin_center
                    .pending
                    .as_ref()
                    .is_some_and(|pending| Rc::ptr_eq(pending, &request))
                {
                    app.finish_plugin_request(result, cx);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_plugin_request(&mut self, result: Result<Value, String>, cx: &mut Context<Self>) {
        let center = &mut self.plugin_center;
        center.pending = None;
        center.loaded = true;
        let Some(operation) = center.mutation.take() else {
            match result.and_then(|value| {
                serde_json::from_value(value).map_err(|_| "无法读取返回的插件清单。".to_owned())
            }) {
                Ok(inventory) => {
                    center.inventory = inventory;
                    center.inventory_error = None;
                }
                Err(error) => center.inventory_error = Some(error),
            }
            return;
        };
        let reply = result.and_then(|value| {
            serde_json::from_value::<MutationReply>(value)
                .map_err(|_| "无法读取 Host 的插件变更结果。".to_owned())
        });
        let notice = match reply {
            Ok(reply) => {
                let removing = matches!(operation, Operation::Remove { .. });
                let notice = reply.outcome.notice(removing).map(str::to_owned);
                center.inventory_error = None;
                if let Some(snapshot) = reply.snapshot {
                    let previous = center
                        .inventory
                        .as_ref()
                        .and_then(|snapshot| operation.checked(snapshot));
                    if let (Some(previous), Some(checked)) =
                        (previous, operation.checked(&snapshot))
                        && checked != previous
                    {
                        let now = bridge::monotonic_ms();
                        let mut slide = DockSlide::with_curve(120., [0.25, 0.1, 0.25, 1.]);
                        slide.update(previous, now, false);
                        slide.update(checked, now, !cx.reduce_motion());
                        center.switch_motion = Some((operation, slide));
                    }
                    if removing
                        && reply.outcome.changed == Some(true)
                        && reply.outcome.application != "failed"
                    {
                        center.detail = None;
                    }
                    center.inventory = Some(snapshot);
                } else {
                    center.inventory_error = Some("变更结果已返回，暂时无法刷新插件清单。".into());
                }
                notice
            }
            Err(error) => {
                center.inventory_error = Some("无法确认当前插件状态，请刷新清单。".into());
                Some(error)
            }
        };
        if let Some(message) = notice {
            self.toasts.retain(|toast| toast.message != message);
            self.push_toast(message, cx);
        }
    }

    fn prepare_plugin_center(&mut self, cx: &mut Context<Self>) {
        if !self.connected() {
            self.plugin_center.pending = None;
            self.plugin_center.mutation = None;
            self.plugin_center.loaded = false;
            self.plugin_center.inventory = None;
            self.plugin_center.inventory_error = None;
        } else if !self.plugin_center.loaded {
            self.refresh_plugin_center(cx);
        }
    }

    pub(super) fn render_plugins_panel(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.prepare_plugin_center(cx);
        self.plugin_center
            .remove_trigger
            .get_or_insert_with(|| cx.focus_handle());
        let p = dsh(cx);
        let width = f32::from(w.viewport_size().width);
        let inset = if width < 768. {
            20.
        } else {
            (width * 0.04).clamp(24., 48.)
        };
        let detail = self.plugin_center.detail.clone();
        let list_only = detail.is_none() && !self.plugin_center.more;
        let scroll_id = match &detail {
            Some(Detail::Config(config)) => config.info().0.into(),
            Some(Detail::Bundle(name)) => {
                gpui::ElementId::Name(format!("plugin-detail:{name}").into())
            }
            None if self.plugin_center.more => "more-plugins".into(),
            None => "plugins-page".into(),
        };
        let root = column()
            .id(scroll_id)
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scroll()
            .px(px(inset))
            .pt(px(if bridge::is_mac() { 58. } else { 28. }))
            .pb(px(48.));
        let body = column().mx_auto().w_full().max_w(px(960.)).gap(px(32.));
        let body = match detail {
            Some(Detail::Config(config)) => {
                let (_, title, summary, artwork) = config.info();
                let fields = match config {
                    Config::WebSearch => {
                        self.prepare_web_search_settings(w, cx);
                        self.render_web_search_settings(w, cx)
                    }
                    config => {
                        let page = match config {
                            Config::Subagent => RuntimePage::Subagent,
                            Config::AgentLoop => RuntimePage::AgentLoop,
                            Config::Shell => RuntimePage::Shell,
                            Config::WebSearch => unreachable!(),
                        };
                        self.prepare_runtime_settings(page, w, cx);
                        self.render_runtime_settings(w, cx)
                    }
                };
                body.child(self.plugin_detail_heading(title, summary, artwork, None, cx))
                    .child(fields)
            }
            Some(Detail::Bundle(name)) => {
                let installed = self
                    .plugin_bundle(&name)
                    .filter(|bundle| bundle.updatable)
                    .and_then(|bundle| bundle.version.clone());
                if let Some(installed) = installed {
                    self.ensure_plugin_versions(&name, &installed, cx);
                }
                body.child(self.plugin_bundle_detail(&name, w, cx))
            }
            None if self.plugin_center.more => body
                .child(
                    row()
                        .gap(px(12.))
                        .child(self.plugin_back("插件", true, cx))
                        .child(plugin_heading("实验性插件", 20.)),
                )
                .child(self.plugin_bundle_group(true, true, w, cx)),
            None => {
                let header = row()
                    .w_full()
                    .items_start()
                    .gap(px(16.))
                    .justify_between()
                    .child(
                        column()
                            .min_w_0()
                            .gap(px(4.))
                            .child(plugin_heading("插件", 20.))
                            .child(note("安装、启用和配置插件", cx)),
                    )
                    .child(
                        row()
                            .flex_none()
                            .gap(px(8.))
                            .child(self.plugin_refresh_button(width >= 768., cx))
                            .child(self.render_plugin_install_trigger(w, width >= 768., cx)),
                    );
                let mut list = body.child(header);
                if let Some(status) = self.plugin_status(cx) {
                    list = list.child(status);
                }
                if let Some(snapshot) = self.plugin_center.snapshot() {
                    let configs = Config::ORDER
                        .into_iter()
                        .filter(|config| snapshot.configs.contains(config));
                    if !snapshot.configs.is_empty() {
                        list = list.child(
                            column()
                                .gap(px(8.))
                                .child(plugin_heading("内置插件配置", 14.))
                                .child(column().gap(px(2.)).children(configs.map(|config| {
                                    let (id, title, summary, artwork) = config.info();
                                    self.plugin_card(
                                        id.into(),
                                        title,
                                        summary,
                                        artwork,
                                        Detail::Config(config),
                                        None,
                                        cx,
                                    )
                                }))),
                        );
                    }
                    for official in [true, false] {
                        if snapshot
                            .bundles
                            .iter()
                            .any(|bundle| bundle.official == official)
                        {
                            list = list.child(self.plugin_bundle_group(official, false, w, cx));
                        }
                    }
                    if snapshot.configs.is_empty()
                        && snapshot.bundles.is_empty()
                        && snapshot.management_available
                    {
                        list = list.child(note("还没有安装任何插件。", cx));
                    }
                }
                list
            }
        };
        let body = body.when(
            !list_only && self.plugin_center.inventory_error.is_some(),
            |body| {
                body.child(
                    row()
                        .gap(px(8.))
                        .children(self.plugin_status(cx))
                        .child(self.plugin_refresh_button(true, cx)),
                )
            },
        );
        root.text_color(p.ink).child(body)
    }

    fn plugin_status(&self, cx: &App) -> Option<Div> {
        let message = if !self.connected() {
            "连接服务后可读取插件清单。"
        } else if let Some(error) = self.plugin_center.inventory_error.as_deref() {
            error
        } else if !self.plugin_center.loaded {
            "正在读取插件…"
        } else if self
            .plugin_center
            .snapshot()
            .is_none_or(|snapshot| !snapshot.management_available)
        {
            "本部署没有可管理的 profile，无法安装或启停插件。"
        } else {
            return None;
        };
        Some(note(message.to_owned(), cx))
    }

    fn plugin_bundle_group(
        &self,
        official: bool,
        more: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = dsh(cx);
        let Some(snapshot) = self.plugin_center.snapshot() else {
            return div();
        };
        let mut bundles: Vec<_> = snapshot
            .bundles
            .iter()
            .filter(|bundle| bundle.official == official && (more || !bundle.more_only))
            .collect();
        bundles.sort_by_key(|bundle| (bundle.rank, bundle.title().to_lowercase()));
        let header = row()
            .w_full()
            .gap(px(8.))
            .child(
                plugin_heading(
                    if official {
                        "实验性插件"
                    } else {
                        "已安装插件"
                    },
                    14.,
                )
                .flex_1(),
            )
            .when(official, |header| {
                header.child(super::dock_action_button(
                    row()
                        .id("more-official-plugins")
                        .role(gpui::Role::Button)
                        .tab_stop(true)
                        .h(px(28.))
                        .gap(px(4.))
                        .px(px(8.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .hover(|style| style.bg(p.hover))
                        .text_size(px(13.))
                        .child("更多")
                        .child(icon(Glyph::Arrow, 14., p.label_secondary)),
                    cx.listener(|app, _, _, cx| {
                        app.plugin_center.more = true;
                        cx.notify();
                    }),
                ))
            });
        column()
            .w_full()
            .gap(px(8.))
            .when(!more, |body| body.child(header))
            .child(
                column()
                    .gap(px(2.))
                    .children(bundles.into_iter().map(|bundle| {
                        let switch = self.bundle_switch(bundle, w, cx);
                        self.plugin_card(
                            gpui::ElementId::Name(bundle.name.clone().into()),
                            bundle.title(),
                            bundle.description.as_deref().unwrap_or(""),
                            Artwork::Default,
                            Detail::Bundle(bundle.name.clone()),
                            Some(switch),
                            cx,
                        )
                    })),
            )
    }

    fn plugin_bundle_detail(&self, name: &str, w: &mut Window, cx: &mut Context<Self>) -> Div {
        let p = dsh(cx);
        let Some((snapshot, bundle)) = self.plugin_center.snapshot().and_then(|snapshot| {
            snapshot
                .bundles
                .iter()
                .find(|bundle| bundle.name == name)
                .map(|bundle| (snapshot, bundle))
        }) else {
            return self.plugin_detail_heading(
                name,
                "当前清单中找不到这个插件包。",
                Artwork::Default,
                None,
                cx,
            );
        };
        let mut actions = row().flex_none().gap(px(8.));
        let can_update = self.can_install_plugins() && !self.plugin_install_busy();
        if let Some(version) = bundle
            .catalog_version
            .as_ref()
            .filter(|_| bundle.catalog_update)
        {
            let target = PluginTarget {
                name: bundle.name.clone(),
                title: bundle.title().to_owned(),
                from: Some(bundle.version.clone().unwrap_or_default()),
                version: version.clone(),
                registry: None,
            };
            actions = actions.child(version_button(
                "update-catalog-plugin",
                "更新",
                can_update,
                cx,
                move |app, w, cx| app.confirm_catalog_update(target.clone(), w, cx),
            ));
        } else if let Some(target) = self.latest_plugin_update(name, bundle.title()) {
            actions = actions.child(version_button(
                "update-registry-plugin",
                "更新",
                can_update,
                cx,
                move |app, w, cx| app.open_plugin_version_update(target.clone(), w, cx),
            ));
        }
        if bundle.removable || bundle.installed {
            let disabled = self.plugin_center.pending.is_some()
                || self.plugin_center.inventory_error.is_some()
                || self.plugin_install_busy()
                || !bundle.removable
                || bundle.read_only_reason.is_some();
            let name = bundle.name.clone();
            let button = row()
                .id("plugin-remove")
                .role(gpui::Role::Button)
                .aria_label(format!("卸载 {}", bundle.title()))
                .tab_stop(!disabled)
                .when_some(
                    self.plugin_center.remove_trigger.as_ref(),
                    |button, focus| button.track_focus(focus),
                )
                .h(px(32.))
                .px(px(12.))
                .gap(px(6.))
                .rounded(px(12.))
                .border(px(0.5))
                .border_color(p.state_error_primary.alpha(0.3))
                .text_size(px(13.))
                .text_color(p.state_error_primary)
                .child(icon(Glyph::Trash, 13., p.state_error_primary))
                .child("卸载");
            actions = actions.child(if disabled {
                button.opacity(0.5)
            } else {
                super::dock_action_button(
                    button.cursor_pointer().hover(|style| style.bg(p.hover)),
                    cx.listener(move |app, _, w, cx| {
                        let cancel_focus = cx.focus_handle();
                        cancel_focus.focus(w, cx);
                        app.plugin_center.confirm_remove = Some(RemoveConfirmation {
                            name: name.clone(),
                            cancel_focus,
                            confirm_focus: cx.focus_handle(),
                        });
                        cx.notify();
                    }),
                )
            });
        }
        actions = actions.child(self.bundle_switch(bundle, w, cx));
        let mut content = column()
            .w_full()
            .gap(px(12.))
            .child(note(bundle.name.clone(), cx))
            .when_some(bundle.version.as_ref(), |body, version| {
                body.child(note(format!("v{version}"), cx))
            })
            .when_some(bundle.lock(), |body, reason| {
                body.child(note(plugin_error(Some(reason)), cx))
            })
            .when_some(bundle.catalog_version.as_ref(), |body, version| {
                body.child(note(format!("官方目标版本 {version}"), cx))
            })
            .children(
                bundle
                    .updatable
                    .then(|| self.render_plugin_versions(name, bundle.title(), cx))
                    .flatten(),
            )
            .child(
                row()
                    .gap(px(8.))
                    .child(plugin_heading("包含的组件", 14.))
                    .child(note(format!("共 {} 个", bundle.rows.len()), cx)),
            );
        if bundle.rows.is_empty() {
            content = content.child(note("这个插件包不包含任何组件。", cx));
        }
        for part in &bundle.rows {
            let entry = snapshot.entry(part);
            let title = part
                .title
                .as_deref()
                .or_else(|| entry.map(Entry::title))
                .unwrap_or(&part.module_name);
            let enabled = entry.is_some_and(|entry| entry.enabled);
            let lock = if !bundle.enabled || entry.is_none() {
                Some("unaddressable")
            } else {
                entry.and_then(|entry| entry.read_only_reason.as_deref())
            };
            let action = Operation::Entry {
                name: bundle.name.clone(),
                row_id: part.row_id.clone(),
                enabled: !enabled,
            };
            let status = entry.map_or("已关闭", Entry::status);
            content = content.child(
                row()
                    .w_full()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(12.))
                    .border(px(0.5))
                    .border_color(p.border_l3)
                    .child(icon(Glyph::Bulb, 20., p.label_secondary))
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .gap(px(3.))
                            .child(plugin_heading(title, 13.).truncate())
                            .when_some(
                                part.description.as_deref().or_else(|| {
                                    entry.and_then(|entry| entry.description.as_deref())
                                }),
                                |column, description| {
                                    column.child(note(description.to_owned(), cx))
                                },
                            )
                            .when_some(
                                entry.and_then(|entry| entry.diagnostic.as_deref()),
                                |column, diagnostic| {
                                    column.child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(p.state_error_primary)
                                            .child(diagnostic.to_owned()),
                                    )
                                },
                            ),
                    )
                    .child(note(status, cx))
                    .child(self.plugin_switch(action, title, enabled, lock, w, cx)),
            );
        }
        column()
            .w_full()
            .gap(px(32.))
            .child(self.plugin_detail_heading(
                bundle.title(),
                bundle.description.as_deref().unwrap_or(""),
                Artwork::Default,
                Some(actions),
                cx,
            ))
            .child(content)
    }

    fn bundle_switch(&self, bundle: &Bundle, w: &mut Window, cx: &mut Context<Self>) -> Div {
        self.plugin_switch(
            Operation::Bundle {
                name: bundle.name.clone(),
                enabled: !bundle.enabled,
            },
            bundle.title(),
            bundle.enabled,
            bundle.lock(),
            w,
            cx,
        )
    }

    fn plugin_bundle(&self, name: &str) -> Option<&Bundle> {
        self.plugin_center
            .snapshot()?
            .bundles
            .iter()
            .find(|bundle| bundle.name == name)
    }

    fn apply_plugin_switch(
        &mut self,
        operation: Operation,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Operation::Bundle {
            name,
            enabled: true,
        } = &operation
            && let Some(bundle) = self.plugin_bundle(name)
            && bundle.availability == "missing"
            && bundle.lock().is_none()
            && let Some(version) = &bundle.catalog_version
        {
            self.open_catalog_plugin(
                PluginTarget {
                    name: name.clone(),
                    title: bundle.title().to_owned(),
                    from: None,
                    version: version.clone(),
                    registry: None,
                },
                w,
                cx,
            );
        } else {
            self.request_plugin_center(Some(operation), cx);
        }
    }

    fn plugin_switch(
        &self,
        operation: Operation,
        title: &str,
        checked: bool,
        lock: Option<&str>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = dsh(cx);
        let center = &self.plugin_center;
        let busy = center.mutation.as_ref() == Some(&operation);
        let disabled = lock.is_some()
            || center.pending.is_some()
            || center.inventory_error.is_some()
            || self.plugin_install_busy();
        let label = format!("启用 {title}");
        let id = operation.switch_id();
        let resting_offset = if checked { 16. } else { 0. };
        let offset = center
            .switch_motion
            .as_ref()
            .filter(|(target, _)| target.switch_id() == id)
            .map_or(resting_offset, |(_, slide)| {
                let (progress, running) = slide.sample(bridge::monotonic_ms());
                if running {
                    w.request_animation_frame();
                    16. * progress as f32
                } else {
                    resting_offset
                }
            });
        let switch = div()
            .id(id)
            .role(gpui::Role::Switch)
            .aria_toggled(checked.into())
            .aria_label(lock.map_or(label.clone(), |reason| {
                format!("{label}。{}", plugin_error(Some(reason)))
            }))
            .tab_stop(!disabled)
            .relative()
            .flex_none()
            .w(px(36.))
            .h(px(20.))
            .rounded_full()
            .bg(if checked {
                p.button_primary_fill
            } else {
                p.border_l3
            })
            .child(
                div()
                    .absolute()
                    .left(px(2. + offset))
                    .top(px(2.))
                    .size(px(16.))
                    .rounded_full()
                    .bg(if checked {
                        p.label_primary_foreground
                    } else {
                        p.switch_thumb
                    }),
            );
        let switch = if disabled {
            switch.opacity(0.5)
        } else {
            super::dock_action_button(
                switch
                    .cursor_pointer()
                    .focus_visible(|style| style.border_1().border_color(p.state_business_primary)),
                cx.listener(move |app, _, w, cx| app.apply_plugin_switch(operation.clone(), w, cx)),
            )
        };
        row()
            .flex_none()
            .gap(px(8.))
            .when(busy, |row| {
                row.child(
                    div()
                        .id("plugin-switch-progress")
                        .role(gpui::Role::Status)
                        .aria_label("正在应用插件变更")
                        .size(px(14.))
                        .child(icon(Glyph::Spinner, 14., p.label_secondary)),
                )
            })
            .child(switch)
    }

    fn plugin_refresh_button(&self, show_label: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.connected() && self.plugin_center.pending.is_none();
        let p = dsh(cx);
        super::dock_action_button(
            row()
                .id("refresh-plugins")
                .role(gpui::Role::Button)
                .aria_label("刷新")
                .tab_stop(enabled)
                .h(px(if show_label { 32. } else { 28. }))
                .when(show_label, |button| button.px(px(12.)).gap(px(6.)))
                .when(!show_label, |button| button.w(px(28.)))
                .justify_center()
                .rounded(px(if show_label { 12. } else { 8. }))
                .border(px(0.5))
                .border_color(p.border_l3)
                .when(enabled, |button| {
                    button.cursor_pointer().hover(|style| style.bg(p.hover))
                })
                .when(!enabled, |button| button.opacity(0.45))
                .child(
                    gpui::svg()
                        .data(include_bytes!("../../assets/refresh-outline.svg"))
                        .size(px(13.))
                        .text_color(p.label_secondary),
                )
                .when(show_label, |button| button.text_size(px(13.)).child("刷新")),
            cx.listener(|app, _, _, cx| app.refresh_plugin_center(cx)),
        )
    }

    fn plugin_card(
        &self,
        id: gpui::ElementId,
        title: &str,
        summary: &str,
        artwork: Artwork,
        detail: Detail,
        end: Option<Div>,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = dsh(cx);
        let link = super::dock_action_button(
            row()
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(format!("查看 {title}"))
                .tab_stop(true)
                .flex_1()
                .min_w_0()
                .p(px(8.))
                .gap(px(14.))
                .rounded(px(20.))
                .cursor_pointer()
                .focus_visible(|style| style.bg(p.hover))
                .child(plugin_artwork(artwork, cx))
                .child(
                    column()
                        .flex_1()
                        .min_w_0()
                        .gap(px(4.))
                        .child(plugin_heading(title, 14.).truncate())
                        .child(
                            div()
                                .truncate()
                                .text_size(px(13.))
                                .line_height(px(18.))
                                .text_color(p.label_tertiary)
                                .child(summary.to_owned()),
                        ),
                ),
            cx.listener(move |app, _, _, cx| {
                app.close_plugin_detail();
                app.plugin_center.detail = Some(detail.clone());
                cx.notify();
            }),
        );
        row()
            .self_stretch()
            .mx(px(-8.))
            .pr(px(8.))
            .rounded(px(20.))
            .hover(|style| style.bg(p.hover))
            .child(link)
            .children(end)
    }

    fn plugin_back(
        &self,
        text: &'static str,
        from_more: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = dsh(cx);
        super::dock_action_button(
            row()
                .id("plugins-back")
                .role(gpui::Role::Button)
                .aria_label(format!("返回{text}"))
                .tab_stop(true)
                .gap(px(6.))
                .cursor_pointer()
                .text_size(px(12.5))
                .line_height(px(20.))
                .text_color(p.label_tertiary)
                .hover(|style| style.text_color(p.label_primary))
                .child(kui::primitives::glyph::transformed(
                    Glyph::Down,
                    14.,
                    p.label_tertiary,
                    std::f32::consts::FRAC_PI_2,
                    1.,
                    (0., 0.),
                    1.,
                ))
                .child(text),
            cx.listener(move |app, _, _, cx| {
                app.close_plugin_detail();
                if from_more {
                    app.plugin_center.more = false;
                }
                cx.notify();
            }),
        )
    }

    fn plugin_detail_heading(
        &self,
        title: &str,
        summary: &str,
        artwork: Artwork,
        actions: Option<Div>,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = dsh(cx);
        let crumb = if self.plugin_center.more {
            "实验性插件"
        } else {
            "插件"
        };
        column()
            .items_start()
            .w_full()
            .child(self.plugin_back(crumb, false, cx))
            .child(
                row()
                    .w_full()
                    .mt(px(32.))
                    .gap(px(12.))
                    .justify_between()
                    .child(plugin_artwork(artwork, cx))
                    .children(actions),
            )
            .child(plugin_heading(title, 20.).mt(px(20.)))
            .child(
                div()
                    .mt(px(8.))
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .text_color(p.label_secondary)
                    .child(summary.to_owned()),
            )
    }

    fn cancel_plugin_remove(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.plugin_center.confirm_remove = None;
        if let Some(trigger) = &self.plugin_center.remove_trigger {
            trigger.focus(w, cx);
        }
        cx.notify();
    }

    pub(super) fn render_remove_confirmation(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let confirmation = self.plugin_center.confirm_remove.as_ref()?;
        let name = confirmation.name.clone();
        let cancel_focus = confirmation.cancel_focus.clone();
        let confirm_focus = confirmation.confirm_focus.clone();
        let tab_cancel = cancel_focus.clone();
        let tab_confirm = confirm_focus.clone();
        let title = self
            .plugin_center
            .snapshot()
            .and_then(|snapshot| snapshot.bundles.iter().find(|bundle| bundle.name == name))
            .map_or(name.as_str(), Bundle::title);
        let p = dsh(cx);

        Some(
            row()
                .id("plugin-remove-dialog")
                .role(gpui::Role::AlertDialog)
                .aria_label("卸载插件")
                .absolute()
                .inset_0()
                .justify_center()
                .items_center()
                .p(px(24.))
                .bg(p.bg_mask_1)
                .occlude()
                .capture_key_down(cx.listener(move |app, event: &gpui::KeyDownEvent, w, cx| {
                    match event.keystroke.key.as_str() {
                        "escape" => app.cancel_plugin_remove(w, cx),
                        "tab" => {
                            let next = if tab_cancel.is_focused(w) {
                                &tab_confirm
                            } else {
                                &tab_cancel
                            };
                            next.focus(w, cx);
                        }
                        _ => return,
                    }
                    w.prevent_default();
                    cx.stop_propagation();
                }))
                .child(
                    column()
                        .w(px(380.))
                        .max_w_full()
                        .p(px(20.))
                        .gap(px(12.))
                        .rounded(px(20.))
                        .bg(p.bg_layer_2)
                        .border(px(0.5))
                        .border_color(p.border_l3)
                        .child(plugin_heading("卸载插件", 20.))
                        .child(note(
                            format!("确定卸载 {title} 吗？这会将它从当前 profile 移除。"),
                            cx,
                        ))
                        .child(
                            row()
                                .w_full()
                                .justify_end()
                                .gap(px(8.))
                                .child(
                                    super::permission_dialog::dialog_action(
                                        "cancel-plugin-remove",
                                        "取消",
                                        false,
                                        false,
                                        64.,
                                        cx,
                                        |app, w, cx| app.cancel_plugin_remove(w, cx),
                                    )
                                    .track_focus(&cancel_focus),
                                )
                                .child(
                                    super::permission_dialog::dialog_action(
                                        "confirm-plugin-remove",
                                        "卸载",
                                        true,
                                        false,
                                        64.,
                                        cx,
                                        move |app, _, cx| {
                                            app.plugin_center.confirm_remove = None;
                                            app.request_plugin_center(
                                                Some(Operation::Remove { name: name.clone() }),
                                                cx,
                                            );
                                        },
                                    )
                                    .track_focus(&confirm_focus),
                                ),
                        ),
                ),
        )
    }

    pub(super) fn render_plugin_inventory(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        self.prepare_plugin_center(cx);
        let p = dsh(cx);
        let mut body = column().w_full().gap(px(12.));
        if let Some(status) = self.plugin_status(cx) {
            body = body.child(
                row()
                    .gap(px(8.))
                    .child(status)
                    .child(self.plugin_refresh_button(false, cx)),
            );
        }
        if self
            .plugin_center
            .snapshot()
            .is_none_or(|snapshot| !snapshot.management_available)
        {
            return body;
        }
        let (search, _) = self.plugin_center.search.get_or_insert_with(|| {
            let search = editor("搜索内置插件".into(), InputMode::Single, w, cx);
            let changes = cx.subscribe(&search, |_, _, event: &EditEvent, cx| {
                if *event == EditEvent::Changed {
                    cx.notify();
                }
            });
            (search, changes)
        });
        let search = search.clone();
        let query = search.read(cx).value().trim().to_lowercase();
        let searching = !query.is_empty();
        let snapshot = self.plugin_center.snapshot().expect("checked above");
        let mut entries: Vec<_> = snapshot
            .entries
            .iter()
            .filter(|entry| !searching || entry.matches(&query))
            .collect();
        entries.sort_by_key(|entry| entry.fiber_phase.as_deref() != Some("failed"));
        let open = searching || self.plugin_center.inventory_open;
        body = body
            .child(
                row()
                    .w_full()
                    .gap(px(8.))
                    .child(div().flex_1().child(input(&search, cx)))
                    .child(self.plugin_refresh_button(false, cx)),
            )
            .child(super::dock_action_button(
                row()
                    .id("inventory-toggle")
                    .role(gpui::Role::Button)
                    .aria_expanded(open)
                    .tab_stop(true)
                    .w_full()
                    .h(px(32.))
                    .gap(px(6.))
                    .rounded(px(8.))
                    .px(px(8.))
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .child(icon(
                        if open { Glyph::Up } else { Glyph::Down },
                        14.,
                        p.secondary,
                    ))
                    .child(plugin_heading("全局插件", 14.))
                    .child(note(entries.len().to_string(), cx)),
                cx.listener(|app, _, _, cx| {
                    app.plugin_center.inventory_open = !app.plugin_center.inventory_open;
                    cx.notify();
                }),
            ));
        if open {
            if entries.is_empty() {
                body = body.child(note("没有匹配的插件。", cx));
            } else {
                let viewport = f32::from(w.viewport_size().width);
                let two_columns = (viewport - 48.).min(800.) - 188. - 48. > 520.;
                let mut cards = column().gap(px(10.));
                if two_columns {
                    for pair in entries.chunks(2) {
                        let mut line = row().w_full().items_start().gap(px(10.));
                        for entry in pair {
                            line = line.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(self.plugin_entry_row(entry, cx)),
                            );
                        }
                        cards = cards
                            .child(line.when(pair.len() == 1, |line| line.child(div().flex_1())));
                    }
                } else {
                    cards = cards.children(
                        entries
                            .into_iter()
                            .map(|entry| self.plugin_entry_row(entry, cx)),
                    );
                }
                body = body.child(cards);
            }
        }
        body
    }

    fn plugin_entry_row(&self, entry: &Entry, cx: &mut Context<Self>) -> Div {
        let p = dsh(cx);
        let key = entry.entry_id.clone();
        let open = self.plugin_center.expanded_entry.as_ref() == Some(&key);
        let status_color = match entry.fiber_phase.as_deref() {
            Some("failed") => p.state_error_primary,
            Some("active") => p.state_business_primary,
            _ => p.label_tertiary,
        };
        let id = entry
            .entry_id
            .strip_prefix("include:")
            .unwrap_or(&entry.entry_id);
        column()
            .w_full()
            .gap_0()
            .rounded(px(20.))
            .overflow_hidden()
            .border(px(0.5))
            .border_color(if entry.fiber_phase.as_deref() == Some("failed") {
                p.state_error_primary.alpha(0.45)
            } else if open {
                p.border_l3
            } else {
                p.border_l4
            })
            .bg(p.bg_layer_2)
            .child(super::dock_action_button(
                column()
                    .id(gpui::ElementId::Name(format!("plugin-entry-{key}").into()))
                    .role(gpui::Role::Button)
                    .aria_expanded(open)
                    .tab_stop(true)
                    .w_full()
                    .min_h(px(52.))
                    .px(px(14.))
                    .py(px(12.))
                    .gap(px(2.))
                    .cursor_pointer()
                    .when(open, |card| card.rounded_t(px(20.)).bg(p.hover))
                    .when(!open, |card| card.rounded(px(20.)))
                    .hover(|style| style.bg(p.hover))
                    .child(
                        row()
                            .w_full()
                            .gap(px(8.))
                            .child(
                                plugin_heading(entry.title(), 14.)
                                    .flex_1()
                                    .min_w_0()
                                    .truncate(),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(12.))
                                    .text_color(status_color)
                                    .child(entry.status()),
                            )
                            .child(icon(
                                if open { Glyph::Up } else { Glyph::Down },
                                12.,
                                p.label_tertiary,
                            )),
                    )
                    .when_some(entry.description.clone(), |card, description| {
                        card.child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(p.label_tertiary)
                                .when(!open, |description| description.line_clamp(2))
                                .child(description),
                        )
                    })
                    .when(id != entry.title(), |card| {
                        card.child(
                            div()
                                .self_start()
                                .mt(px(4.))
                                .max_w_full()
                                .truncate()
                                .px(px(6.))
                                .py(px(1.))
                                .rounded(px(4.))
                                .bg(p.bg_module_platform)
                                .font_family("monospace")
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .text_color(p.label_secondary)
                                .child(id.to_owned()),
                        )
                    }),
                cx.listener(move |app, _, _, cx| {
                    let expanded = &mut app.plugin_center.expanded_entry;
                    *expanded = (expanded.as_ref() != Some(&key)).then(|| key.clone());
                    cx.notify();
                }),
            ))
            .when(open, |body| {
                body.child(
                    column()
                        .w_full()
                        .gap(px(4.))
                        .px(px(14.))
                        .py(px(10.))
                        .border_t(px(0.5))
                        .border_color(p.border_l2)
                        .rounded_b(px(20.))
                        .bg(p.bg_module_platform)
                        .child(note(format!("模块  {}", entry.module_name), cx))
                        .child(note(format!("条目 ID  {}", entry.entry_id), cx))
                        .when_some(entry.diagnostic.clone(), |body, diagnostic| {
                            body.child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(p.state_error_primary)
                                    .child(diagnostic),
                            )
                        }),
                )
            })
    }
}

fn plugin_heading(text: &str, size: f32) -> Div {
    div()
        .text_size(px(size))
        .line_height(px(if size == 20. { 28. } else { 20. }))
        .font_weight(FontWeight::MEDIUM)
        .child(text.to_owned())
}

#[derive(Clone, Copy)]
enum Artwork {
    Subagent,
    Search,
    Loop,
    Terminal,
    Default,
}

fn plugin_artwork(artwork: Artwork, cx: &App) -> Div {
    // The official artwork includes gradients and an HTML-backed conic ring.
    // Embed its transparent 3× render so GPUI keeps the source colors.
    static IMAGES: LazyLock<[Arc<gpui::Image>; 5]> = LazyLock::new(|| {
        let assets: [&[u8]; 5] = [
            include_bytes!("../../assets/plugin-artwork-subagent.png"),
            include_bytes!("../../assets/plugin-artwork-search.png"),
            include_bytes!("../../assets/plugin-artwork-loop.png"),
            include_bytes!("../../assets/plugin-artwork-terminal.png"),
            include_bytes!("../../assets/plugin-artwork-default.png"),
        ];
        assets.map(|bytes| {
            Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                bytes.to_vec(),
            ))
        })
    });
    let p = dsh(cx);
    row()
        .size(px(48.))
        .flex_none()
        .justify_center()
        .rounded(px(16.))
        .border(px(0.5))
        .border_color(p.border_l3)
        .child(gpui::img(IMAGES[artwork as usize].clone()).size(px(36.)))
}
