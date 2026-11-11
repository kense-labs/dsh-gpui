//! Version discovery and exact updates for registry-installed bundles.

use super::{
    plugin_install::{PluginTarget, registry_label},
    *,
};
use gpui::Div;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
enum VersionReply {
    Listed(VersionList),
    Refused {
        problem: String,
        #[serde(default)]
        attempts: Vec<Attempt>,
    },
}

#[derive(Deserialize)]
struct VersionList {
    installed: String,
    registry: Option<String>,
    versions: Vec<Version>,
    omitted: usize,
    attempts: Vec<Attempt>,
}

#[derive(Deserialize)]
struct Attempt {
    registry: Option<String>,
    problem: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Version {
    version: String,
    published_at: Option<String>,
    tags: Vec<String>,
    prerelease: bool,
    deprecated: Option<String>,
    bundle: bool,
    incompatible: Option<Incompatible>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Incompatible {
    runtime_version: String,
    requirements: std::collections::BTreeMap<String, String>,
}

impl Version {
    fn installable(&self) -> bool {
        self.bundle && self.incompatible.is_none()
    }

    fn requirements(&self) -> Option<String> {
        self.incompatible.as_ref().map(|issue| {
            issue
                .requirements
                .iter()
                .map(|(name, range)| format!("{name} {range}"))
                .collect::<Vec<_>>()
                .join("、")
        })
    }

    fn publication(&self) -> Option<&str> {
        self.published_at
            .as_deref()
            .and_then(|value| value.split('T').next())
    }
}

impl VersionList {
    fn candidate(&self) -> Option<&Version> {
        self.versions.iter().find(|version| {
            version.installable() && !version.prerelease && version.deprecated.is_none()
        })
    }

    fn target(&self, name: &str, title: &str, version: &Version) -> PluginTarget {
        PluginTarget {
            name: name.to_owned(),
            title: title.to_owned(),
            from: Some(self.installed.clone()),
            version: version.version.clone(),
            registry: self.registry.clone(),
        }
    }
}

pub(super) struct VersionCheck {
    name: String,
    installed: String,
    request: Rc<()>,
    result: Option<VersionReply>,
    checked_at: Option<String>,
    expanded: bool,
    prereleases: bool,
}

impl DshApp {
    pub(super) fn ensure_plugin_versions(
        &mut self,
        name: &str,
        installed: &str,
        cx: &mut Context<Self>,
    ) {
        if self
            .plugin_center
            .versions
            .as_ref()
            .is_none_or(|check| check.name != name || check.installed != installed)
            && !self.plugin_install_busy()
            && self.can_install_plugins()
        {
            self.check_plugin_versions(name.to_owned(), installed.to_owned(), cx);
        }
    }

    fn check_plugin_versions(&mut self, name: String, installed: String, cx: &mut Context<Self>) {
        if self.plugin_install_busy() || !self.can_install_plugins() {
            return;
        }
        let request = Rc::new(());
        let same = self
            .plugin_center
            .versions
            .as_ref()
            .filter(|check| check.name == name && check.installed == installed);
        self.plugin_center.versions = Some(VersionCheck {
            name: name.clone(),
            installed,
            request: request.clone(),
            result: None,
            checked_at: None,
            expanded: same.is_some_and(|check| check.expanded),
            prereleases: same.is_some_and(|check| check.prereleases),
        });
        cx.spawn(async move |this, cx| {
            let result = bridge::invoke(
                "plugin_install",
                json!({
                    "operation":{"action":"versions","name":name}
                }),
            )
            .await;
            let _ = this.update(cx, |app, cx| {
                let Some(check) = app
                    .plugin_center
                    .versions
                    .as_mut()
                    .filter(|check| Rc::ptr_eq(&check.request, &request))
                else {
                    return;
                };
                check.result = Some(
                    result
                        .ok()
                        .and_then(|value| serde_json::from_value(value).ok())
                        .filter(|reply| match reply {
                            VersionReply::Listed(list) => list.installed == check.installed,
                            VersionReply::Refused { .. } => true,
                        })
                        .unwrap_or(VersionReply::Refused {
                            problem: "unknown".to_owned(),
                            attempts: Vec::new(),
                        }),
                );
                check.checked_at = Some(bridge::time_label(bridge::now_ms()));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn latest_plugin_update(&self, name: &str, title: &str) -> Option<PluginTarget> {
        let check = self
            .plugin_center
            .versions
            .as_ref()
            .filter(|check| check.name == name)?;
        let VersionReply::Listed(list) = check.result.as_ref()? else {
            return None;
        };
        Some(list.target(name, title, list.candidate()?))
    }

    pub(super) fn render_plugin_versions(
        &self,
        name: &str,
        title: &str,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let check = self
            .plugin_center
            .versions
            .as_ref()
            .filter(|check| check.name == name)?;
        let p = dsh(cx);
        let (summary, detail, glyph, tone, count) = match &check.result {
            None => (
                "正在检查更新".into(),
                None,
                Glyph::Spinner,
                p.label_secondary,
                0,
            ),
            Some(VersionReply::Refused { problem, attempts }) => (
                if problem == "network" {
                    "无法连接安装源".into()
                } else {
                    "无法检查更新".into()
                },
                attempts.last().map(attempt_text),
                Glyph::Close,
                p.state_error_primary,
                attempts.len(),
            ),
            Some(VersionReply::Listed(list)) => {
                if let Some(candidate) = list.candidate() {
                    (
                        format!("可更新到 {}", candidate.version),
                        candidate.publication().map(|date| format!("发布于 {date}")),
                        Glyph::Download,
                        p.state_business_primary,
                        list.versions.len(),
                    )
                } else if list.versions.is_empty() {
                    (
                        "已是最新版本".into(),
                        check
                            .checked_at
                            .as_ref()
                            .map(|time| format!("上次检查于 {time}")),
                        Glyph::Check,
                        p.state_success_primary,
                        0,
                    )
                } else if let Some(blocked) = list
                    .versions
                    .iter()
                    .find(|version| !version.prerelease && version.incompatible.is_some())
                {
                    let detail = blocked
                        .incompatible
                        .as_ref()
                        .zip(blocked.requirements())
                        .map(|(issue, requirements)| {
                            format!(
                                "{} 需要 {requirements}，当前 DSH 为 {}",
                                blocked.version, issue.runtime_version
                            )
                        });
                    (
                        "新版本需要更新 DSH".into(),
                        detail,
                        Glyph::Alert,
                        p.state_warn_primary,
                        list.versions.len(),
                    )
                } else {
                    (
                        "没有可直接安装的新版本".into(),
                        Some("新版本已弃用、为预发布版本或没有声明组合包，展开查看".to_owned()),
                        Glyph::Alert,
                        p.state_warn_primary,
                        list.versions.len(),
                    )
                }
            }
        };
        let can_check =
            check.result.is_some() && self.can_install_plugins() && !self.plugin_install_busy();
        let request_name = check.name.clone();
        let installed = check.installed.clone();
        let failed = matches!(check.result, Some(VersionReply::Refused { .. }));
        let heading = row()
            .w_full()
            .justify_between()
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("版本"),
            )
            .child(version_button(
                "recheck-plugin-versions",
                if failed { "重试" } else { "重新检查" },
                can_check,
                cx,
                move |app, _, cx| {
                    app.check_plugin_versions(request_name.clone(), installed.clone(), cx)
                },
            ));
        let summary = row()
            .id("plugin-version-summary")
            .w_full()
            .gap(px(10.))
            .p(px(12.))
            .child(
                row()
                    .size(px(28.))
                    .flex_none()
                    .justify_center()
                    .rounded(px(8.))
                    .bg(tone.alpha(0.12))
                    .child(icon(glyph, 14., tone)),
            )
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(summary),
                    )
                    .when_some(detail, |col, detail| col.child(note(detail, cx))),
            );
        let summary = if count == 0 {
            summary.role(gpui::Role::Status)
        } else {
            let label = if check.expanded {
                "收起".to_owned()
            } else if failed {
                "详情".to_owned()
            } else {
                format!("{count} 个新版本")
            };
            super::dock_action_button(
                summary
                    .role(gpui::Role::Button)
                    .aria_expanded(check.expanded)
                    .tab_stop(true)
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .child(note(label, cx))
                    .child(
                        gpui::svg()
                            .data(include_bytes!("../../assets/header-chevron.svg"))
                            .size(px(12.))
                            .text_color(p.label_secondary)
                            .with_transformation(gpui::Transformation::default().with_rotation(
                                gpui::radians(if check.expanded {
                                    std::f32::consts::PI
                                } else {
                                    0.
                                }),
                            )),
                    ),
                cx.listener(|app, _, _, cx| {
                    if let Some(check) = &mut app.plugin_center.versions {
                        check.expanded = !check.expanded;
                        cx.notify();
                    }
                }),
            )
        };
        let mut panel = column()
            .w_full()
            .rounded(px(12.))
            .border(px(0.5))
            .border_color(p.border_l2)
            .overflow_hidden()
            .child(summary);
        if count != 0 && check.expanded {
            panel = match check.result.as_ref() {
                Some(VersionReply::Listed(list)) => {
                    panel.child(self.plugin_version_rows(name, title, check, list, cx))
                }
                Some(VersionReply::Refused { attempts, .. }) => panel.child(
                    column().px(px(12.)).pb(px(12.)).gap(px(6.)).children(
                        attempts
                            .iter()
                            .map(|attempt| note(attempt_text(attempt), cx)),
                    ),
                ),
                None => panel,
            }
        }
        Some(column().w_full().gap(px(8.)).child(heading).child(panel))
    }

    fn plugin_version_rows(
        &self,
        name: &str,
        title: &str,
        check: &VersionCheck,
        list: &VersionList,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = dsh(cx);
        let mut rows = column().w_full().px(px(12.)).pb(px(12.)).gap(px(6.));
        let can_update = self.can_install_plugins() && !self.plugin_install_busy();
        for version in list
            .versions
            .iter()
            .filter(|version| check.prereleases || !version.prerelease)
        {
            let tags = row()
                .min_w_0()
                .gap(px(5.))
                .flex_wrap()
                .child(version_number(&version.version))
                .children(version.tags.iter().map(|tag| version_tag(tag, cx)))
                .when(version.prerelease, |row| {
                    row.child(version_tag("预发布", cx))
                })
                .when(version.deprecated.is_some(), |row| {
                    row.child(version_tag("已弃用", cx))
                });
            let detail = version
                .requirements()
                .map(|range| format!("需要 {range}"))
                .or_else(|| {
                    (!version.bundle).then(|| "这个版本没有声明组合包，无法安装".to_owned())
                })
                .or_else(|| version.deprecated.clone());
            let mut line = row()
                .w_full()
                .min_h(px(38.))
                .gap(px(8.))
                .border_t(px(0.5))
                .border_color(p.border_l2)
                .child(
                    column()
                        .flex_1()
                        .min_w_0()
                        .py(px(6.))
                        .gap(px(3.))
                        .child(tags)
                        .when_some(detail, |col, detail| col.child(note(detail, cx))),
                )
                .when_some(version.publication(), |row, date| {
                    row.child(note(date.to_owned(), cx))
                });
            if version.installable() {
                let target = list.target(name, title, version);
                line = line.child(version_button(
                    gpui::ElementId::Name(format!("update-plugin-{}", version.version).into()),
                    "更新",
                    can_update,
                    cx,
                    move |app, w, cx| app.open_plugin_version_update(target.clone(), w, cx),
                ));
            }
            rows = rows.child(line);
        }
        let source = registry_name(list.registry.as_deref());
        let mut meta = row()
            .w_full()
            .gap(px(12.))
            .flex_wrap()
            .child(note(format!("安装源 {source}"), cx))
            .when(list.attempts.len() > 1, |row| {
                row.child(version_tag("备选源", cx))
            })
            .when_some(check.checked_at.as_ref(), |row, time| {
                row.child(note(format!("检查于 {time}"), cx))
            });
        if list.versions.iter().any(|version| version.prerelease) {
            meta = meta.child(super::dock_action_button(
                row()
                    .id("plugin-versions-prereleases")
                    .role(gpui::Role::CheckBox)
                    .aria_toggled(check.prereleases.into())
                    .tab_stop(true)
                    .gap(px(6.))
                    .cursor_pointer()
                    .child(
                        div()
                            .size(px(14.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(p.border_l3)
                            .when(check.prereleases, |box_| {
                                box_.child(icon(Glyph::Check, 12., p.ink))
                            }),
                    )
                    .child(note("显示预发布版本", cx)),
                cx.listener(|app, _, _, cx| {
                    if let Some(check) = &mut app.plugin_center.versions {
                        check.prereleases = !check.prereleases;
                        cx.notify();
                    }
                }),
            ));
        }
        rows.child(
            row()
                .w_full()
                .h(px(34.))
                .gap(px(5.))
                .border_t(px(0.5))
                .border_color(p.border_l2)
                .child(version_number(&list.installed))
                .child(version_tag("已安装", cx)),
        )
        .when(list.omitted > 0, |rows| {
            rows.child(note(
                format!("另有 {} 个比已安装版本新的版本未列出", list.omitted),
                cx,
            ))
        })
        .child(meta)
    }
}

pub(super) fn version_button(
    id: impl Into<gpui::ElementId>,
    title: &'static str,
    enabled: bool,
    cx: &mut Context<DshApp>,
    action: impl Fn(&mut DshApp, &mut Window, &mut Context<DshApp>) + 'static,
) -> impl IntoElement {
    let p = dsh(cx);
    let button = row()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(title)
        .tab_stop(enabled)
        .flex_none()
        .h(px(32.))
        .px(px(10.))
        .gap(px(5.))
        .rounded(px(10.))
        .border(px(0.5))
        .border_color(p.border_l2)
        .text_size(px(13.))
        .child(
            gpui::svg()
                .data(include_bytes!("../../assets/refresh-outline.svg"))
                .size(px(13.))
                .text_color(p.ink),
        )
        .child(title);
    if enabled {
        super::dock_action_button(
            button.cursor_pointer().hover(|style| style.bg(p.hover)),
            cx.listener(move |app, _, w, cx| action(app, w, cx)),
        )
    } else {
        button.opacity(0.5)
    }
}

fn version_tag(title: &str, cx: &App) -> Div {
    div()
        .px(px(5.))
        .rounded(px(5.))
        .border(px(0.5))
        .border_color(dsh(cx).border_l2)
        .text_size(px(10.))
        .line_height(px(15.))
        .text_color(dsh(cx).label_secondary)
        .child(title.to_owned())
}

fn version_number(version: &str) -> Div {
    div()
        .font_family("monospace")
        .text_size(px(13.))
        .child(version.to_owned())
}

fn registry_name(value: Option<&str>) -> String {
    value.map_or_else(|| "默认安装源".into(), registry_label)
}

fn attempt_text(attempt: &Attempt) -> String {
    let state = match attempt.problem.as_deref() {
        Some("network") => "无法连接",
        Some("not-found") => "未找到",
        Some(_) => "检查失败",
        None => "已读取",
    };
    format!("{} · {state}", registry_name(attempt.registry.as_deref()))
}
