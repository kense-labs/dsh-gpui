//! The workspace tree shown in a Files pane.

use super::{
    browser::REFRESH_ICON,
    dock_controls::{DockControl, dock_action_button},
    workspace_watch::WorkspaceWatch,
    *,
};
use crate::files::WorkspaceFile;
use gpui::{canvas, fill, linear_color_stop, linear_gradient, svg};
use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;

const ROOT: &str = "";
const FOLDER_CLOSED: &[u8] = include_bytes!("../../assets/files-folder-closed.svg");
const FOLDER_OPEN: &[u8] = include_bytes!("../../assets/files-folder-open.svg");
const FILE_OTHER: &[u8] = include_bytes!("../../assets/files-other.svg");
const FILE_OTHER_FOLD: &[u8] = include_bytes!("../../assets/files-other-fold.svg");

/// One workspace search's client state (official Files search).
#[derive(Default)]
pub(crate) struct FilesSearchState {
    pub query: String,
    /// `(path, type)` pairs as the Host answered them; type is `file`/`directory`.
    pub matches: Vec<(String, String)>,
    pub truncated: bool,
    pub incomplete: bool,
    pub error: Option<String>,
    pub loading: bool,
    /// Latest request id; stale answers for older ids are ignored.
    pub request: u64,
    pub scroll: ScrollHandle,
}

impl FilesSearchState {
    /// Reset the results when the query becomes empty.
    pub fn clear(&mut self) {
        self.matches.clear();
        self.truncated = false;
        self.incomplete = false;
        self.error = None;
        self.loading = false;
    }
}

#[derive(Default)]
pub(super) struct DockFileTree {
    levels: HashMap<String, FileLevel>,
    expanded: HashSet<String>,
    scroll: ScrollHandle,
    shown: bool,
    navigator_open: bool,
    navigator_focus: Option<gpui::FocusHandle>,
    revealed: Option<String>,
    scroll_to_current: Rc<Cell<bool>>,
}

#[derive(Default)]
struct FileLevel {
    listing: FileListing,
    stale: bool,
    refresh_children: bool,
    watch: Option<Rc<WorkspaceWatch>>,
}

impl FileLevel {
    fn invalidate(&mut self) {
        self.stale = true;
        self.listing.cancel();
        self.refresh_children = false;
        self.watch = None;
    }

    fn refresh_tree(&mut self) {
        self.stale = true;
        self.refresh_children = true;
    }
}

impl DockFileTree {
    /// Expand every directory on the way to one path so a search result can be
    /// revealed in the tree.
    pub(super) fn expand_path(&mut self, path: &str) {
        self.revealed = Some(path.to_owned());
        let mut current = String::new();
        for segment in path.split('/') {
            if segment.is_empty() {
                continue;
            }
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(segment);
            if current != path {
                self.expanded.insert(current.clone());
            }
        }
    }

    pub(super) fn suspend(&mut self) {
        if std::mem::take(&mut self.shown) {
            self.levels.values_mut().for_each(FileLevel::invalidate);
        }
    }

    /// Open newly restored levels after their parents settle. Existing watches
    /// remain independent while an ancestor is being reread.
    fn collect_reads(&self, path: &str, can_open: bool, can_watch: bool, reads: &mut Vec<String>) {
        let Some(level) = self.levels.get(path) else {
            if can_open {
                reads.push(path.to_owned());
            }
            return;
        };
        let read = !level.listing.loading()
            && (can_open || level.watch.is_some())
            && (level.stale || (can_watch && level.watch.is_none()));
        if read {
            reads.push(path.to_owned());
        }
        let can_open_children = can_open && !read && !level.listing.loading();
        for child in &level.listing.entries {
            if child.is_directory && self.expanded.contains(&child.path) {
                self.collect_reads(&child.path, can_open_children, can_watch, reads);
            }
        }
    }

    fn reveal_current(&mut self, path: &str) {
        if self.revealed.as_deref() == Some(path) {
            return;
        }
        let mut parent = ROOT;
        for (index, _) in path.match_indices('/') {
            let next = &path[..index];
            let Some(level) = self.levels.get(parent) else {
                return;
            };
            if level.stale || level.listing.loading() {
                return;
            }
            if !level
                .listing
                .entries
                .iter()
                .any(|entry| entry.is_directory && entry.path == next)
            {
                self.revealed = Some(path.to_owned());
                return;
            }
            if self.expanded.insert(next.to_owned()) {
                return;
            }
            parent = next;
        }
        if let Some(level) = self.levels.get(parent)
            && !level.stale
            && !level.listing.loading()
        {
            self.revealed = Some(path.to_owned());
            self.scroll_to_current
                .set(level.listing.entries.iter().any(|entry| entry.path == path));
        }
    }

    fn finish(&mut self, path: &str, request: u64, reply: Result<Value, String>) -> bool {
        let Some(level) = self.levels.get_mut(path) else {
            return false;
        };
        let previous: Vec<_> = level
            .listing
            .entries
            .iter()
            .filter(|entry| entry.is_directory)
            .map(|entry| entry.path.clone())
            .collect();
        if !level.listing.finish(request, reply) {
            return false;
        }
        let removed = if level.listing.error.is_none() {
            sort_entries(&mut level.listing.entries);
            let present: HashSet<_> = level
                .listing
                .entries
                .iter()
                .filter(|entry| entry.is_directory)
                .map(|entry| entry.path.as_str())
                .collect();
            previous
                .into_iter()
                .filter(|path| !present.contains(path.as_str()))
                .collect()
        } else {
            Vec::new()
        };
        let children = if !level.stale && std::mem::take(&mut level.refresh_children) {
            level
                .listing
                .entries
                .iter()
                .filter(|entry| entry.is_directory && self.expanded.contains(&entry.path))
                .map(|entry| entry.path.clone())
                .collect()
        } else {
            Vec::new()
        };
        if !removed.is_empty() {
            let kept = |path: &str| !removed.iter().any(|dir| in_branch(path, dir));
            self.expanded.retain(|path| kept(path));
            self.levels.retain(|path, _| kept(path));
        }
        for child in children {
            self.levels.entry(child).or_default().refresh_tree();
        }
        true
    }
}

impl DshApp {
    pub(super) fn sync_dock_files(&mut self, cx: &mut Context<Self>) {
        let layout = &self.sidebar.surface.layout;
        self.sidebar.file_trees.retain(|id, _| {
            layout.pane(*id).is_some_and(|pane| {
                pane.tabs()
                    .iter()
                    .any(|tab| matches!(tab, RightPanelTab::Files | RightPanelTab::Document(_)))
            })
        });
        let shown: Vec<_> = layout
            .all_panes()
            .filter(|pane| {
                !self.sidebar.workspace.is_empty()
                    && (pane.active() == RightPanelTab::Files
                        || matches!(pane.active(), RightPanelTab::Document(_))
                            && self.document_navigator_open(pane.id))
                    && self.dock_pane_on_screen(pane.id, cx)
            })
            .map(|pane| (pane.id, pane.active()))
            .collect();
        for (pane, tree) in &mut self.sidebar.file_trees {
            if !shown.iter().any(|(id, _)| id == pane) {
                tree.suspend();
            }
        }
        for (pane, tab) in shown {
            let tree = self.sidebar.file_trees.entry(pane).or_default();
            tree.shown = true;
            if let RightPanelTab::Document(key) = tab
                && let Some(path) = self.sidebar.surface.documents.get(&key)
            {
                tree.reveal_current(path);
            }
            let mut reads = Vec::new();
            tree.collect_reads(ROOT, true, self.browser_shell.is_some(), &mut reads);
            self.read_file_levels(pane, reads, cx);
        }
    }

    pub(super) fn refresh_dock_files(&mut self, pane: DockPaneId, cx: &mut Context<Self>) {
        if let Some(tree) = self
            .sidebar
            .file_trees
            .get_mut(&pane)
            .filter(|tree| tree.shown)
        {
            tree.levels
                .entry(ROOT.to_owned())
                .or_default()
                .refresh_tree();
            cx.notify();
        }
    }

    pub(super) fn workspace_directory_changed(&mut self, event: &Value, cx: &mut Context<Self>) {
        let Some(id) = self.workspace_watch_id(event) else {
            return;
        };
        let level = self
            .sidebar
            .file_trees
            .values_mut()
            .filter(|tree| tree.shown)
            .flat_map(|tree| tree.levels.values_mut())
            .find(|level| level.watch.as_ref().is_some_and(|watch| watch.id() == id));
        if let Some(level) = level
            && !std::mem::replace(&mut level.stale, true)
        {
            cx.notify();
        }
    }

    fn toggle_file_directory(&mut self, pane: DockPaneId, path: &str, cx: &mut Context<Self>) {
        let Some(tree) = self.sidebar.file_trees.get_mut(&pane) else {
            return;
        };
        if tree.expanded.remove(path) {
            if let Some(RightPanelTab::Document(key)) = self
                .sidebar
                .surface
                .layout
                .pane(pane)
                .map(|pane| pane.active())
                && let Some(current) = self.sidebar.surface.documents.get(&key)
                && in_branch(current, path)
            {
                tree.revealed = Some(current.clone());
                tree.scroll_to_current.set(false);
            }
            for (key, level) in &mut tree.levels {
                if in_branch(key, path) {
                    level.invalidate();
                }
            }
        } else {
            tree.expanded.insert(path.to_owned());
        }
        cx.notify();
    }

    fn read_file_levels(&mut self, pane: DockPaneId, paths: Vec<String>, cx: &mut Context<Self>) {
        let Some(session) = self.active_session.clone() else {
            return;
        };
        let Some(tree) = self.sidebar.file_trees.get_mut(&pane) else {
            return;
        };
        for path in paths {
            self.next_file_request = self.next_file_request.wrapping_add(1);
            let request = self.next_file_request;
            let level = tree.levels.entry(path.clone()).or_default();
            level.stale = false;
            level.listing.start(path.clone(), request);
            let start_watch = self
                .browser_shell
                .filter(|_| level.watch.is_none())
                .map(|shell| {
                    let watch = WorkspaceWatch::new(shell, request);
                    level.watch = Some(watch.clone());
                    watch
                });
            let workspace = self.sidebar.workspace.clone();
            let session = session.clone();
            cx.spawn(async move |this, cx| {
                if let Some(watch) = start_watch
                    && watch
                        .follow(
                            "watch_workspace_directory",
                            json!({
                                "workspace": workspace, "directory": path,
                            }),
                        )
                        .await
                        .is_none()
                {
                    return;
                }
                let args = json!({
                    "workspace": workspace, "directory": path, "requestId": request, "tree": true,
                });
                let reply = bridge::invoke("list_workspace_files", args).await;
                let _ = this.update(cx, |this, cx| {
                    if this.active_session.as_deref() == Some(session.as_str())
                        && let Some(tree) = this.sidebar.file_trees.get_mut(&pane)
                        && tree.finish(&path, request, reply)
                    {
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    pub(super) fn document_navigator_open(&self, pane: DockPaneId) -> bool {
        self.sidebar
            .file_trees
            .get(&pane)
            .is_some_and(|tree| tree.navigator_open)
    }

    pub(super) fn show_document_navigator(
        &mut self,
        pane: DockPaneId,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_dock_pane(pane, w, cx);
        let tree = self.sidebar.file_trees.entry(pane).or_default();
        tree.navigator_open = true;
        tree.navigator_focus
            .get_or_insert_with(|| cx.focus_handle())
            .focus(w, cx);
        cx.notify();
    }

    pub(super) fn toggle_document_navigator(&mut self, pane: DockPaneId, cx: &mut Context<Self>) {
        let tree = self.sidebar.file_trees.entry(pane).or_default();
        tree.navigator_open = !tree.navigator_open;
        cx.notify();
    }

    pub(super) fn render_right_files(&self, pane: DockPaneId, cx: &mut Context<Self>) -> gpui::Div {
        let p = dsh(cx);
        let size = self.dock_title_font_size();
        let page = column()
            .size_full()
            .min_h_0()
            .gap_0()
            .bg(p.bg_base)
            .relative();
        let workspace = &self.sidebar.workspace;
        if workspace.is_empty() {
            return page.child(
                div()
                    .px(px(10.))
                    .py(px(12.))
                    .text_size(px(size))
                    .line_height(px(size * 1.6))
                    .text_color(p.label_secondary)
                    .child("这个会话没有工作区目录。"),
            );
        }
        let show_hidden = self.files_search_show_hidden;
        let header = files_header(workspace, cx)
            .child(
                div()
                    .id("files-show-hidden")
                    .role(gpui::Role::Button)
                    .aria_label("显示隐藏项")
                    .tab_index(0)
                    .px(px(6.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(if show_hidden {
                        p.label_primary
                    } else {
                        p.label_tertiary
                    })
                    .hover(|style| style.bg(p.hover))
                    .child(if show_hidden {
                        "隐藏项：显示"
                    } else {
                        "显示隐藏项"
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.files_search_show_hidden = !this.files_search_show_hidden;
                        if !this.files_search.query.trim().is_empty() {
                            this.run_files_search_for_ui(cx);
                        }
                        this.save_workspace_preferences(cx);
                        cx.notify();
                    })),
            )
            .child(
                self.dock_button(DockControl::FilesRefresh(pane), "重新读取", false, cx)
                    .group("files-refresh")
                    .child(
                        svg()
                            .data(REFRESH_ICON)
                            .size(px(15.))
                            .text_color(p.label_secondary)
                            .group_hover("files-refresh", |style| {
                                style.text_color(p.label_primary)
                            }),
                    ),
            )
            .children(self.render_workspace_open_split(Some(pane), workspace, cx));
        let search_bar = div()
            .w_full()
            .flex_none()
            .px(px(8.))
            .pb(px(6.))
            .flex()
            .gap(px(4.))
            .items_center()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(input(&self.files_search_editor, cx)),
            )
            .child(
                div()
                    .id("files-search-settings")
                    .role(gpui::Role::Button)
                    .aria_label("搜索设置")
                    .tab_index(0)
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_color(if self.files_search_settings_open {
                        p.label_primary
                    } else {
                        p.label_tertiary
                    })
                    .hover(|style| style.bg(p.hover))
                    .child(icon(Glyph::Settings, 15., p.faint))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.files_search_settings_open = !this.files_search_settings_open;
                        cx.notify();
                    })),
            );
        let page = page.child(header).child(search_bar).child(
            if self.files_search.query.trim().is_empty() {
                self.render_workspace_tree(pane, None, cx)
                    .into_any_element()
            } else {
                self.render_files_search(pane, cx)
            },
        );
        if self.files_search_settings_open {
            page.child(self.render_files_search_settings(cx))
        } else {
            page
        }
    }

    /// Official Files search results: shallow-first rows with the empty,
    /// truncated, incomplete, and failure notes below them.
    pub(super) fn render_files_search(
        &self,
        pane: DockPaneId,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = dsh(cx);
        let size = self.dock_title_font_size();
        let search = &self.files_search;
        let mut body = div()
            .id("files-search-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pl(px(8.))
            .pr(px(7.))
            .py(px(8.))
            .text_size(px(size))
            .line_height(px(size * 1.5))
            .text_color(p.label_primary)
            .track_scroll(&search.scroll);
        if let Some(error) = search.error.clone() {
            body = body.child(
                column()
                    .w_full()
                    .gap(px(6.))
                    .child(file_note(format!("搜索失败：{error}"), cx))
                    .child(
                        div()
                            .id("files-search-retry")
                            .role(gpui::Role::Button)
                            .aria_label("重试")
                            .tab_index(0)
                            .ml(px(10.))
                            .px(px(8.))
                            .py(px(2.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .text_size(px(12.))
                            .text_color(p.label_secondary)
                            .hover(|style| style.bg(p.hover))
                            .child("重试")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.files_search.request =
                                    this.files_search.request.wrapping_add(1);
                                this.run_files_search(cx);
                            })),
                    ),
            );
        } else {
            if search.loading && search.matches.is_empty() {
                body = body.child(file_note("正在搜索…", cx));
            }
            for (index, (path, kind)) in search.matches.iter().enumerate() {
                let is_directory = kind == "directory";
                let open_path = path.clone();
                let row = row()
                    .id(gpui::ElementId::Name(
                        format!("files-search-row-{index}").into(),
                    ))
                    .role(gpui::Role::Button)
                    .aria_label(path.clone())
                    .tab_index(0)
                    .w_full()
                    .min_w_0()
                    .gap(px(6.))
                    .px(px(6.))
                    .h(px(22.))
                    .rounded(px(6.))
                    .items_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .child(icon(
                        if is_directory {
                            Glyph::Folder
                        } else {
                            Glyph::FileText
                        },
                        14.,
                        p.faint,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(p.label_primary)
                            .child(path.clone()),
                    );
                body = body.child(if is_directory {
                    row.on_click(cx.listener(move |this, _, w, cx| {
                        if let Some(tree) = this.sidebar.file_trees.get_mut(&pane) {
                            tree.expand_path(&open_path);
                        }
                        replace(&this.files_search_editor, "", w, cx);
                        this.files_search.query.clear();
                        this.files_search.clear();
                        cx.notify();
                    }))
                } else {
                    row.on_click(cx.listener(move |this, _, w, cx| {
                        this.open_workspace_document(pane, open_path.clone(), w, cx);
                    }))
                });
            }
            if !search.loading && search.matches.is_empty() {
                body = body
                    .child(file_note("没有匹配项", cx))
                    .child(file_note(
                        "隐藏项、排除的文件和目录，以及被忽略文件（如 .gitignore）忽略的文件和目录不会显示。",
                        cx,
                    ));
            }
            if search.truncated {
                body = body.child(file_note(
                    format!("只显示前 {} 个结果，请缩小搜索范围。", search.matches.len()),
                    cx,
                ));
            }
            if search.incomplete {
                body = body.child(file_note(
                    "文件数超过搜索上限，或部分目录未能读取，因此结果可能不完整。",
                    cx,
                ));
            }
        }
        body.into_any_element()
    }

    /// Official Files search settings: exclusion rules, `.gitignore` choice, and
    /// the add/restore/remove gestures, drawn as a panel under the search bar.
    pub(super) fn render_files_search_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let p = dsh(cx);
        let mut card = column()
            .id("files-search-settings-panel")
            .w_full()
            .gap(px(8.))
            .p(px(12.))
            .rounded(px(12.))
            .bg(super::workspace::menu_bg(cx))
            .border(px(0.5))
            .border_color(p.border_l2)
            .shadow(elevation_prominent())
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                row()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(p.label_primary)
                            .child("排除的文件和目录"),
                    )
                    .child(
                        div()
                            .id("files-search-settings-close")
                            .role(gpui::Role::Button)
                            .aria_label("关闭")
                            .tab_index(0)
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|style| style.bg(p.hover))
                            .child(icon(Glyph::Close, 12., p.faint))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.files_search_settings_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(p.label_tertiary)
                    .child("只影响搜索结果，不影响文件树。"),
            );
        let mut rules_list = column().w_full().gap(px(2.));
        for rule in &self.files_search_rules {
            let path = rule.path.clone();
            let enabled = rule.enabled;
            let toggle_path = path.clone();
            let remove_path = path.clone();
            rules_list = rules_list.child(
                row()
                    .w_full()
                    .gap(px(6.))
                    .items_center()
                    .px(px(4.))
                    .h(px(24.))
                    .rounded(px(6.))
                    .hover(|style| style.bg(p.hover))
                    .child(
                        div()
                            .id(gpui::ElementId::Name(
                                format!("files-rule-toggle-{path}").into(),
                            ))
                            .role(gpui::Role::CheckBox)
                            .aria_label(format!("排除 {path}"))
                            .aria_selected(enabled)
                            .tab_index(0)
                            .size(px(16.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border(px(1.))
                            .border_color(if enabled { p.ink } else { p.faint })
                            .cursor_pointer()
                            .when(enabled, |checkbox| {
                                checkbox.bg(p.ink).child(icon(
                                    Glyph::Check,
                                    11.,
                                    super::workspace::conversation_bg(cx),
                                ))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                crate::search_rules::toggle_rule(
                                    &mut this.files_search_rules,
                                    &toggle_path,
                                );
                                this.files_search_rule_error = None;
                                this.run_files_search_for_ui(cx);
                                this.save_workspace_preferences(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(if enabled {
                                p.label_primary
                            } else {
                                p.label_tertiary
                            })
                            .child(path),
                    )
                    .child(
                        div()
                            .id(gpui::ElementId::Name(
                                format!("files-rule-remove-{remove_path}").into(),
                            ))
                            .role(gpui::Role::Button)
                            .aria_label(format!("删除 {remove_path}"))
                            .tab_index(0)
                            .size(px(20.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(|style| style.bg(p.hover))
                            .child(icon(Glyph::Close, 11., p.faint))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                crate::search_rules::remove_rule(
                                    &mut this.files_search_rules,
                                    &remove_path,
                                );
                                this.run_files_search_for_ui(cx);
                                this.save_workspace_preferences(cx);
                                cx.notify();
                            })),
                    ),
            );
        }
        card = card.child(
            div()
                .id("files-search-rules-scroll")
                .w_full()
                .max_h(px(180.))
                .overflow_y_scroll()
                .child(rules_list),
        );
        if let Some(error) = &self.files_search_rule_error {
            card = card.child(
                div()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(p.danger)
                    .child(error.clone()),
            );
        }
        card = card.child(
            row()
                .w_full()
                .gap(px(6.))
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(input(&self.files_search_rule_editor, cx)),
                )
                .child(
                    div()
                        .id("files-search-rule-add")
                        .role(gpui::Role::Button)
                        .aria_label("添加文件或目录")
                        .tab_index(0)
                        .px(px(8.))
                        .h(px(28.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(p.label_primary)
                        .bg(p.selector)
                        .hover(|style| style.bg(p.hover))
                        .child("添加文件或目录")
                        .on_click(cx.listener(|this, _, w, cx| {
                            let draft = this
                                .files_search_rule_editor
                                .read(cx)
                                .value()
                                .trim()
                                .to_owned();
                            match crate::search_rules::normalize_rule_path(&draft) {
                                Ok(path) => {
                                    crate::search_rules::add_rule(
                                        &mut this.files_search_rules,
                                        path,
                                    );
                                    this.files_search_rule_error = None;
                                    replace(&this.files_search_rule_editor, "", w, cx);
                                    this.run_files_search_for_ui(cx);
                                    this.save_workspace_preferences(cx);
                                }
                                Err(reason) => {
                                    this.files_search_rule_error = Some(reason.label().to_owned());
                                }
                            }
                            cx.notify();
                        })),
                ),
        );
        card = card
            .child(
                row()
                    .w_full()
                    .gap(px(6.))
                    .items_center()
                    .child({
                        let respect = self.files_search_respect_gitignore;
                        div()
                            .id("files-respect-gitignore")
                            .role(gpui::Role::CheckBox)
                            .aria_label("尊重忽略文件（.gitignore 等）")
                            .aria_selected(respect)
                            .tab_index(0)
                            .size(px(16.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border(px(1.))
                            .border_color(if respect { p.ink } else { p.faint })
                            .cursor_pointer()
                            .when(respect, |checkbox| {
                                checkbox.bg(p.ink).child(icon(
                                    Glyph::Check,
                                    11.,
                                    super::workspace::conversation_bg(cx),
                                ))
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.files_search_respect_gitignore =
                                    !this.files_search_respect_gitignore;
                                this.run_files_search_for_ui(cx);
                                this.save_workspace_preferences(cx);
                                cx.notify();
                            }))
                    })
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .line_height(px(18.))
                            .text_color(p.secondary)
                            .child("尊重忽略文件（.gitignore 等）"),
                    )
                    .child(
                        div()
                            .id("files-search-rules-restore")
                            .role(gpui::Role::Button)
                            .aria_label("还原默认列表")
                            .tab_index(0)
                            .px(px(8.))
                            .h(px(24.))
                            .rounded(px(6.))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .text_size(px(12.))
                            .text_color(p.label_secondary)
                            .hover(|style| style.bg(p.hover))
                            .child("还原默认列表")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.files_search_rules = crate::search_rules::default_rules();
                                this.files_search_rule_error = None;
                                this.run_files_search_for_ui(cx);
                                this.save_workspace_preferences(cx);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                row().w_full().justify_end().child(
                    div()
                        .id("files-search-settings-done")
                        .role(gpui::Role::Button)
                        .aria_label("关闭")
                        .tab_index(0)
                        .px(px(12.))
                        .h(px(26.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(p.label_primary)
                        .bg(p.selector)
                        .hover(|style| style.bg(p.hover))
                        .child("关闭")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.files_search_settings_open = false;
                            cx.notify();
                        })),
                ),
            );
        // Outside-click closes the panel; it sits above the file body.
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .id("files-search-settings-cover")
                    .absolute()
                    .inset_0()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.files_search_settings_open = false;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .absolute()
                    .left(px(8.))
                    .right(px(8.))
                    .top(px(80.))
                    .child(card),
            )
            .into_any_element()
    }

    pub(super) fn render_workspace_tree(
        &self,
        pane: DockPaneId,
        current: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let size = self.dock_title_font_size();
        let body = div()
            .id("files-tree-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .pl(px(8.))
            .pr(px(7.))
            .py(px(8.))
            .text_size(px(size))
            .line_height(px(size * 1.5))
            .text_color(p.label_primary);
        if let Some(tree) = self.sidebar.file_trees.get(&pane) {
            body.track_scroll(&tree.scroll)
                .when_some(tree.navigator_focus.as_ref(), |body, focus| {
                    body.track_focus(focus).tab_stop(false)
                })
                .child(self.render_file_level(tree, pane, ROOT, current, size, cx))
        } else {
            body.child(file_note("正在读取…", cx))
        }
    }

    fn render_file_level(
        &self,
        tree: &DockFileTree,
        pane: DockPaneId,
        path: &str,
        current: Option<&str>,
        size: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut rows = column().w_full().gap_0();
        let Some(level) = tree.levels.get(path) else {
            return rows.child(file_note("正在读取…", cx));
        };
        let listing = &level.listing;
        if listing.loading() && !listing.loaded() {
            return rows.child(file_note("正在读取…", cx));
        }
        if let Some(error) = &listing.error {
            rows = rows.child(file_note(file_error_note(error), cx));
        } else if listing.entries.is_empty() {
            rows = rows.child(file_note("空目录", cx));
        } else if !self.files_search_show_hidden
            && listing
                .entries
                .iter()
                .all(|entry| entry.name.starts_with('.'))
        {
            rows = rows.child(file_note(
                "该目录只有隐藏项，可点击「显示隐藏项」查看。",
                cx,
            ));
        }
        for entry in &listing.entries {
            if !self.files_search_show_hidden && entry.name.starts_with('.') {
                continue;
            }
            let expanded = entry.is_directory && tree.expanded.contains(&entry.path);
            let current_row = current == Some(entry.path.as_str());
            let mut row = self.render_file_entry(pane, entry, expanded, current_row, size, cx);
            if current_row && tree.scroll_to_current.get() {
                let pending = tree.scroll_to_current.clone();
                let scroll = tree.scroll.clone();
                row = row.relative().child(
                    canvas(
                        move |bounds, w, _| {
                            if !pending.replace(false) {
                                return;
                            }
                            let view = scroll.bounds();
                            let delta = if bounds.top() < view.top() {
                                view.top() - bounds.top()
                            } else if bounds.bottom() > view.bottom() {
                                view.bottom() - bounds.bottom()
                            } else {
                                return;
                            };
                            let offset = scroll.offset();
                            scroll.set_offset(gpui::point(offset.x, offset.y + delta));
                            w.request_animation_frame();
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                );
            }
            rows = rows.child(row);
            if expanded {
                rows = rows.child(div().pl(px(18.)).child(self.render_file_level(
                    tree,
                    pane,
                    &entry.path,
                    current,
                    size,
                    cx,
                )));
            }
        }
        if listing.truncated {
            rows = rows.child(file_note("条目太多，只显示了一部分。", cx));
        }
        rows
    }

    fn render_file_entry(
        &self,
        pane: DockPaneId,
        entry: &WorkspaceFile,
        expanded: bool,
        current: bool,
        size: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let path = entry.path.clone();
        let directory = entry.is_directory;
        let button = row()
            .id(gpui::ElementId::Name(format!("files-entry-{path}").into()))
            .items_center()
            .w_full()
            .h(px(size * 1.5 + 10.))
            .px(px(10.))
            .gap(px(6.))
            .rounded(px(12.))
            .when(directory, |row| row.aria_expanded(expanded))
            .when(directory, |row| {
                row.child(
                    svg()
                        .data(if expanded { FOLDER_OPEN } else { FOLDER_CLOSED })
                        .size(px(16.))
                        .flex_none()
                        .text_color(p.label_tertiary),
                )
            })
            .when(!directory && !entry.is_other, |row| {
                row.child(file_sheet_icon(16.))
            })
            .child(div().min_w_0().truncate().child(entry.name.clone()));
        if entry.is_other {
            return button.text_color(p.label_tertiary);
        }
        dock_action_button(
            button
                .role(gpui::Role::Button)
                .aria_label(entry.name.clone())
                .tab_index(0)
                .cursor_pointer()
                .when(current, |row| row.bg(p.hover_active))
                .hover(|style| style.bg(if current { p.hover_active } else { p.hover }))
                .focus_visible(|style| {
                    style.shadow(vec![
                        gpui::BoxShadow::new(px(0.), px(0.), p.state_business_primary)
                            .spread_radius(px(1.))
                            .inset(),
                    ])
                }),
            cx.listener(move |this, _, w, cx| {
                if directory {
                    this.toggle_file_directory(pane, &path, cx);
                } else {
                    this.open_workspace_document(pane, path.clone(), w, cx);
                }
            }),
        )
    }
}

pub(super) fn files_header(path: &str, cx: &App) -> gpui::Div {
    row()
        .h(px(38.))
        .w_full()
        .flex_none()
        .items_center()
        .pl(px(16.))
        .pr(px(6.))
        .gap(px(4.))
        .border_b(px(0.5))
        .border_color(dsh(cx).border_l3)
        .child(files_path_label(path, cx))
}

pub(super) fn file_sheet_icon(size: f32) -> gpui::Div {
    div()
        .relative()
        .size(px(size))
        .flex_none()
        .child(
            svg()
                .data(FILE_OTHER)
                .size_full()
                .text_color(gpui::rgb(0xcfd3d6)),
        )
        .child(
            svg()
                .data(FILE_OTHER_FOLD)
                .absolute()
                .inset_0()
                .text_color(gpui::rgb(0xa2a4a6)),
        )
}

pub(super) fn files_folder_icon(open: bool, color: gpui::Hsla) -> gpui::Svg {
    svg()
        .data(if open { FOLDER_OPEN } else { FOLDER_CLOSED })
        .size(px(16.))
        .text_color(color)
}

fn files_path_label(workspace: &str, cx: &App) -> gpui::Stateful<gpui::Div> {
    let p = dsh(cx);
    let split = workspace
        .trim_end_matches('/')
        .rfind('/')
        .map_or(0, |i| i + 1);
    let (parent, name) = workspace.split_at(split);
    let text_width = Rc::new(Cell::new(px(0.)));
    let measured = text_width.clone();
    row()
        .id("files-root-path")
        .relative()
        .flex_1()
        .min_w_0()
        .mr(px(12.))
        .justify_end()
        .overflow_hidden()
        .text_size(px(12.))
        .line_height(px(18.))
        .child(
            row()
                .relative()
                .flex_none()
                .mr_auto()
                .whitespace_nowrap()
                .gap_0()
                .child(div().text_color(p.label_tertiary).child(parent.to_owned()))
                .child(div().text_color(p.label_primary).child(name.to_owned()))
                .child(
                    canvas(
                        move |bounds, _, _| measured.set(bounds.size.width),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                ),
        )
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, w, _| {
                    if text_width.get() > bounds.size.width {
                        let fade = gpui::Bounds {
                            size: gpui::size(bounds.size.width.min(px(28.)), bounds.size.height),
                            ..bounds
                        };
                        w.paint_quad(fill(
                            fade,
                            linear_gradient(
                                90.,
                                linear_color_stop(p.bg_base, 0.),
                                linear_color_stop(p.bg_base.opacity(0.), 1.),
                            ),
                        ));
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
}

/// 官方：失败层按 not-found / outside-workspace / not-directory 分类文案。
fn file_error_note(error: &str) -> String {
    let lower = error.to_lowercase();
    if lower.contains("not found") || lower.contains("enoent") {
        "目录不存在".to_owned()
    } else if lower.contains("outside") {
        "路径不在当前工作区内".to_owned()
    } else if lower.contains("not a directory") || lower.contains("enotdir") {
        "目标不是目录".to_owned()
    } else {
        error.to_owned()
    }
}

fn file_note(label: impl Into<gpui::SharedString>, cx: &App) -> gpui::Div {
    div()
        .px(px(10.))
        .py(px(3.))
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(dsh(cx).label_tertiary)
        .child(label.into())
}

fn in_branch(path: &str, directory: &str) -> bool {
    path == directory
        || path
            .strip_prefix(directory)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn sort_entries(entries: &mut [WorkspaceFile]) {
    thread_local! {
        static COMPARE: js_sys::Function = {
            let options = js_sys::JSON::parse(r#"{"numeric":true,"sensitivity":"base"}"#)
                .expect("valid collator options");
            js_sys::Intl::Collator::new(&js_sys::Array::new(), &js_sys::Object::from(options)).compare()
        };
    }
    COMPARE.with(|compare| {
        entries.sort_by(|left, right| {
            right.is_directory.cmp(&left.is_directory).then_with(|| {
                compare
                    .call2(
                        &JsValue::UNDEFINED,
                        &left.name.as_str().into(),
                        &right.name.as_str().into(),
                    )
                    .ok()
                    .and_then(|order| order.as_f64())
                    .unwrap_or_default()
                    .total_cmp(&0.)
            })
        });
    });
}
