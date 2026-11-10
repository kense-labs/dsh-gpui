//! File-resource tabs in the workspace dock.

use super::{
    browser::REFRESH_ICON,
    dock_controls::{DockControl, dock_action_button},
    dock_files::{file_sheet_icon, files_folder_icon, files_header},
    document_code::{CodeSpan, HighlightSession, display_source, language_for_path, render_code},
    document_frontmatter::Frontmatter,
    document_html,
    document_selection::{DocumentSelection, SelectionLayout},
    workspace::render_markdown,
    workspace_watch::WorkspaceWatch,
    *,
};
use crate::right_dock::workspace_document_path;
use gpui::{Pixels, canvas, deferred, point, svg};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    rc::{Rc, Weak},
};
use wasm_bindgen_futures::JsFuture;

const WRAP: &[u8] = include_bytes!("../../assets/document-wrap.svg");
const NOWRAP: &[u8] = include_bytes!("../../assets/document-nowrap.svg");
const CODE_COPY: &[u8] = include_bytes!("../../assets/document-copy.svg");
const CODE_COPIED: &[u8] = include_bytes!("../../assets/document-copied.svg");
const HTML_MAX_ASSETS: usize = 64;
const HTML_MAX_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Viewer {
    Html,
    Markdown,
    Code,
    Plain,
}

impl Viewer {
    fn for_path(path: &str) -> &'static [Self] {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        if name.rsplit_once('.').is_some_and(|(_, suffix)| {
            suffix.eq_ignore_ascii_case("html") || suffix.eq_ignore_ascii_case("htm")
        }) {
            return &[Self::Html, Self::Code, Self::Plain];
        }
        match language_for_path(path) {
            Some("markdown") => &[Self::Markdown, Self::Code, Self::Plain],
            Some(_) => &[Self::Code, Self::Plain],
            None => &[Self::Plain],
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Html => "HTML",
            Self::Markdown => "Markdown",
            Self::Code => "代码",
            Self::Plain => "纯文本",
        }
    }

    fn can_wrap(self) -> bool {
        matches!(self, Self::Code | Self::Plain)
    }
}

struct CodeSource {
    language: &'static str,
    highlighter: Rc<HighlightSession>,
    requested_revision: Option<u64>,
    tokens: Vec<Vec<CodeSpan>>,
    copied: Rc<Cell<bool>>,
}

impl CodeSource {
    fn new(language: &'static str) -> Self {
        Self {
            language,
            highlighter: Rc::new(HighlightSession::new()),
            requested_revision: None,
            tokens: Vec::new(),
            copied: Rc::new(Cell::new(false)),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextPage {
    absolute_path: String,
    version: String,
    offset: usize,
    text: String,
    lines: usize,
    eof: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HtmlSource {
    absolute_path: String,
    version: String,
    source: String,
}

struct HtmlPage {
    absolute_path: String,
    version: String,
    document: Option<JsValue>,
}

impl HtmlPage {
    async fn read(
        workspace: String,
        path: String,
        assets: Option<HtmlPack>,
    ) -> Result<Self, ReadFailure> {
        let value = bridge::invoke(
            "read_workspace_html",
            json!({"workspace":workspace, "path":path}),
        )
        .await
        .map_err(ReadFailure::from_host)?;
        let source: HtmlSource = serde_json::from_value(value)
            .map_err(|error| ReadFailure::Unavailable(error.to_string()))?;
        let document = if let Some(pack) = assets {
            pack.prepare(&source.source, &workspace, &path).await
        } else {
            JsFuture::from(document_html::prepare_static_html(&source.source))
                .await
                .ok()
        };
        Ok(Self {
            absolute_path: source.absolute_path,
            version: source.version,
            document,
        })
    }
}

#[derive(Deserialize)]
struct HtmlReference {
    kind: String,
    reference: String,
    path: Option<String>,
}

#[derive(Serialize)]
struct HtmlAsset {
    kind: String,
    reference: String,
    text: String,
}

struct HtmlAssetWatch {
    lease: Rc<WorkspaceWatch>,
    observed: Option<FileObservation>,
}

type HtmlAssetWatches = RefCell<Vec<HtmlAssetWatch>>;

struct HtmlPack {
    watches: Weak<HtmlAssetWatches>,
    shell: Option<u64>,
    first_watch_id: u64,
}

impl HtmlPack {
    async fn prepare(self, source: &str, workspace: &str, base_file: &str) -> Option<JsValue> {
        let references: Vec<HtmlReference> =
            serde_wasm_bindgen::from_value(document_html::asset_references(source)).ok()?;
        let mut assets = Vec::new();
        let mut bytes = source.len();
        for (index, reference) in references.into_iter().enumerate() {
            if index == HTML_MAX_ASSETS || self.watches.strong_count() == 0 {
                return None;
            }
            let path = reference.path.as_ref()?;
            let args = json!({"workspace":workspace, "baseFile":base_file, "path":path});
            if let Some(shell) = self.shell {
                let watch =
                    WorkspaceWatch::new(shell, self.first_watch_id.wrapping_add(index as u64));
                self.watches.upgrade()?.borrow_mut().push(HtmlAssetWatch {
                    lease: watch.clone(),
                    observed: None,
                });
                let result = watch
                    .follow("watch_workspace_html_asset", args.clone())
                    .await;
                let watches = self.watches.upgrade()?;
                if let Some(Ok(value)) = result
                    && let Ok(observation) = serde_json::from_value(value)
                    && let Some(entry) = watches.borrow_mut().last_mut()
                    && entry.observed.is_none()
                {
                    entry.observed = Some(observation);
                }
            }
            let value = bridge::invoke("read_workspace_html_asset", args)
                .await
                .ok()?;
            let file: HtmlSource = serde_json::from_value(value).ok()?;
            bytes += file.source.len();
            if bytes > HTML_MAX_BYTES {
                return None;
            }
            let text = file
                .source
                .strip_prefix('\u{feff}')
                .map(str::to_owned)
                .unwrap_or(file.source);
            assets.push(HtmlAsset {
                kind: reference.kind,
                reference: reference.reference,
                text,
            });
        }
        #[derive(Serialize)]
        struct Content<'a> {
            html: &'a str,
            assets: Vec<HtmlAsset>,
        }
        serde_wasm_bindgen::to_value(&Content {
            html: source,
            assets,
        })
        .ok()
    }
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(tag = "code", content = "detail", rename_all = "kebab-case")]
enum ReadFailure {
    NotFound,
    NotRegularFile,
    NotText,
    TooLarge,
    Changed,
    Unavailable(String),
}

impl ReadFailure {
    fn from_host(error: String) -> Self {
        serde_json::from_str(&error).unwrap_or(Self::Unavailable(error))
    }

    fn line(&self) -> String {
        match self {
            Self::NotFound => "文件不存在，可能已被移动或删除".into(),
            Self::NotRegularFile => "该路径不是普通文件，没有可显示的内容".into(),
            Self::NotText => "该格式文件暂时无法预览".into(),
            Self::TooLarge => "单页内容超过 2 MiB 上限，无法读取".into(),
            Self::Changed => "文件已更新，当前显示为旧内容".into(),
            Self::Unavailable(detail) => format!("读取失败：{detail}"),
        }
    }

    fn can_retry(&self) -> bool {
        matches!(self, Self::Changed | Self::Unavailable(_))
    }
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum FileSnapshot {
    Live {
        absolute_path: String,
        version: String,
    },
    Failed {
        failure: ReadFailure,
    },
}

#[derive(Deserialize)]
struct FileObservation {
    sequence: u64,
    #[serde(flatten)]
    snapshot: FileSnapshot,
}

struct DocumentWatch {
    lease: Rc<WorkspaceWatch>,
    pending: bool,
}

#[derive(Default)]
struct DocumentViewport {
    scroll: ScrollHandle,
    remembered_top: Cell<Pixels>,
    layout_max: Cell<Option<Pixels>>,
    scrolled: Cell<bool>,
}

impl DocumentViewport {
    fn record_scroll(&self) -> bool {
        if !self.scrolled.replace(false) {
            return false;
        }
        let Some(maximum) = self.layout_max.get() else {
            return false;
        };
        let top = (-self.scroll.offset().y).clamp(px(0.), maximum);
        // A layout clamp after a short read is not a new reading position.
        if top + px(1.) < maximum || top >= self.remembered_top.get() {
            self.remembered_top.set(top);
        }
        maximum > px(0.) && top + px(1.) >= maximum
    }

    fn needs_more(&self) -> bool {
        self.layout_max
            .get()
            .is_some_and(|maximum| self.remembered_top.get() > maximum + px(1.))
    }

    fn restore_after_layout(&self, can_load_more: bool, w: &mut Window) {
        let maximum = self.scroll.max_offset().y;
        let changed = self.layout_max.replace(Some(maximum)) != Some(maximum);
        let top = self.remembered_top.get().min(maximum);
        let offset = self.scroll.offset();
        if offset.y != -top {
            self.scroll.set_offset(point(offset.x, -top));
            w.request_animation_frame();
        } else if changed && can_load_more && self.needs_more() {
            w.request_animation_frame();
        }
    }
}

pub(super) struct DocumentTab {
    text: Option<TextPage>,
    html: Option<HtmlPage>,
    html_assets: Option<Rc<HtmlAssetWatches>>,
    failure: Option<ReadFailure>,
    request: Option<u64>,
    restart: bool,
    viewport: Rc<DocumentViewport>,
    pub(super) selection: Rc<DocumentSelection>,
    watch: Option<DocumentWatch>,
    observation: Option<FileObservation>,
    observed_path: Option<String>,
    version_at_read: Option<String>,
    wrap: bool,
    viewer: Viewer,
    code: Option<CodeSource>,
    frontmatter: Option<Frontmatter>,
    content_revision: u64,
    viewer_open: bool,
}

impl DocumentTab {
    fn new(path: &str, cx: &Context<DshApp>) -> Self {
        Self {
            text: None,
            html: None,
            html_assets: None,
            failure: None,
            request: None,
            restart: false,
            viewport: Rc::new(DocumentViewport::default()),
            selection: DocumentSelection::new(cx),
            watch: None,
            observation: None,
            observed_path: None,
            version_at_read: None,
            wrap: true,
            viewer: Viewer::for_path(path)[0],
            code: language_for_path(path).map(CodeSource::new),
            frontmatter: None,
            content_revision: 0,
            viewer_open: false,
        }
    }

    pub(super) fn suspend_watch(&mut self) {
        self.watch = None;
        self.observation = None;
        if self.html_assets.is_some() {
            self.retire_html();
        }
        self.selection.clear();
    }

    fn retire_html(&mut self) {
        self.html = None;
        self.html_assets = None;
        self.request = None;
        self.failure = None;
        self.restart = true;
    }

    fn observe(&mut self, observation: FileObservation) -> bool {
        if let Some(previous) = self.observation.as_mut() {
            if previous.sequence >= observation.sequence {
                return false;
            }
            if previous.snapshot == observation.snapshot {
                previous.sequence = observation.sequence;
                return false;
            }
        }
        if let FileSnapshot::Live { absolute_path, .. } = &observation.snapshot {
            self.observed_path = Some(absolute_path.clone());
        }
        self.observation = Some(observation);
        true
    }

    fn observed_version(&self) -> Option<&str> {
        match &self.observation.as_ref()?.snapshot {
            FileSnapshot::Live { version, .. } => Some(version),
            FileSnapshot::Failed { .. } => None,
        }
    }

    fn metadata_failure(&self) -> Option<&ReadFailure> {
        match &self.observation.as_ref()?.snapshot {
            FileSnapshot::Failed { failure } => Some(failure),
            FileSnapshot::Live { .. } => None,
        }
    }

    fn has_file_change(&self) -> bool {
        self.observed_version().is_some_and(|version| {
            self.html
                .as_ref()
                .map(|page| page.version.as_str())
                .or_else(|| self.text.as_ref().map(|page| page.version.as_str()))
                != Some(version)
                && self.version_at_read.as_deref() != Some(version)
        })
    }

    fn has_content(&self) -> bool {
        self.text.is_some() || self.html.is_some()
    }

    pub(super) fn shows_html(&self) -> bool {
        self.viewer == Viewer::Html
            && self
                .html
                .as_ref()
                .is_some_and(|page| page.document.is_some())
    }

    fn finish_html(&mut self, request: u64, result: Result<HtmlPage, ReadFailure>) -> bool {
        if self.viewer != Viewer::Html || self.request != Some(request) {
            return false;
        }
        self.request = None;
        match result {
            Ok(page) => self.html = Some(page),
            Err(failure) => self.failure = Some(failure),
        }
        true
    }

    fn finish(&mut self, request: u64, offset: usize, result: Result<Value, String>) -> bool {
        if self.request != Some(request) {
            return false;
        }
        self.request = None;
        self.viewport.layout_max.set(None);
        match result.map_err(ReadFailure::from_host).and_then(|value| {
            serde_json::from_value::<TextPage>(value)
                .map_err(|error| ReadFailure::Unavailable(error.to_string()))
        }) {
            Ok(page) if page.offset == offset => {
                self.failure = None;
                if offset == 1 {
                    self.text = Some(page);
                } else if let Some(text) = self.text.as_mut() {
                    if text.version != page.version {
                        self.restart = true;
                        return true;
                    }
                    if page.lines > 0 {
                        if text.lines > 0 {
                            text.text.push('\n');
                        }
                        text.text.push_str(&page.text);
                        text.lines += page.lines;
                    }
                    text.eof = page.eof;
                }
                self.content_revision = request;
                if let (Some(code), Some(text)) = (&self.code, &self.text)
                    && code.language == "markdown"
                    && Frontmatter::update(&mut self.frontmatter, &text.text)
                    && self.viewer == Viewer::Markdown
                {
                    self.selection.clear();
                }
            }
            Ok(_) => self.failure = Some(ReadFailure::Unavailable("返回的行号与请求不符".into())),
            Err(failure) => self.failure = Some(failure),
        }
        true
    }
}

impl DshApp {
    pub(super) fn update_html_preview_mode(&mut self, enabled: bool) {
        if self.developer_tools == enabled {
            return;
        }
        self.developer_tools = enabled;
        for tab in self.sidebar.documents.values_mut().chain(
            self.parked_sidebars
                .values_mut()
                .flat_map(|sidebar| sidebar.documents.values_mut()),
        ) {
            if tab.viewer == Viewer::Html {
                tab.retire_html();
            }
        }
    }

    pub(super) fn open_workspace_document(
        &mut self,
        pane: DockPaneId,
        path: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_session.is_none() || !workspace_document_path(&path) {
            return;
        }
        let opened = self
            .sidebar
            .surface
            .documents
            .iter()
            .find(|(_, opened)| **opened == path)
            .and_then(|(&key, _)| {
                let tab = RightPanelTab::Document(key);
                self.sidebar
                    .surface
                    .layout
                    .all_panes()
                    .find(|pane| pane.tabs().contains(&tab))
                    .map(|pane| (pane.id, tab))
            });
        if let Some((owner, tab)) = opened {
            if !owner.is_floating() {
                self.sidebar.surface.open = true;
            }
            self.activate_right_tab(owner, tab, w, cx);
            if !self.document_navigator_open(owner) {
                self.right_dock_focus[&owner].focus(w, cx);
            }
            return;
        }
        self.next_file_request = self.next_file_request.wrapping_add(1);
        let key = (self.next_file_request..)
            .find(|id| !self.sidebar.surface.documents.contains_key(id))
            .expect("there are unclaimed document keys");
        self.sidebar.surface.documents.insert(key, path);
        if let Some(previous) = self.sidebar.surface.layout.replace_document(pane, key) {
            self.close_document(previous);
            self.focus_dock_pane(pane, w, cx);
            cx.notify();
        } else {
            self.open_right_tab(pane, RightPanelTab::Document(key), w, cx);
        }
        let target = self.sidebar.surface.layout.focused();
        if !self.document_navigator_open(target) {
            self.right_dock_focus[&target].focus(w, cx);
        }
    }

    pub(super) fn close_document(&mut self, key: u64) {
        self.sidebar.surface.documents.remove(&key);
        self.sidebar.documents.remove(&key);
    }

    pub(super) fn sync_dock_documents(&mut self, cx: &mut Context<Self>) {
        let shown: Vec<_> = self
            .sidebar
            .surface
            .layout
            .all_panes()
            .filter_map(|pane| {
                if let RightPanelTab::Document(key) = pane.active()
                    && self.dock_pane_on_screen(pane.id, cx)
                {
                    Some(key)
                } else {
                    None
                }
            })
            .collect();
        for (key, tab) in &mut self.sidebar.documents {
            if !shown.contains(key) {
                tab.viewer_open = false;
                tab.selection.clear();
            }
        }
        for key in shown {
            let Some(path) = self.sidebar.surface.documents.get(&key) else {
                continue;
            };
            let tab = self
                .sidebar
                .documents
                .entry(key)
                .or_insert_with(|| DocumentTab::new(path, cx));
            if tab.watch.is_none() && self.browser_shell.is_some() {
                self.watch_document(key, cx);
                continue;
            }
            let at_end = tab.viewport.record_scroll();
            if tab.viewer == Viewer::Markdown && tab.text.is_some() {}
            if tab.request.is_some() || tab.watch.as_ref().is_some_and(|watch| watch.pending) {
                continue;
            }
            if tab.restart || tab.has_file_change() || (!tab.has_content() && tab.failure.is_none())
            {
                self.read_document(key, 1, cx);
            } else if tab.failure.is_none()
                && tab.metadata_failure().is_none()
                && let Some(text) = &tab.text
                && !text.eof
                && (at_end || tab.viewport.needs_more())
            {
                let offset = text.lines + 1;
                self.read_document(key, offset, cx);
            }
            self.highlight_document(key, cx);
            self.parse_document_frontmatter(key, cx);
        }
    }

    fn highlight_document(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(session) = self.active_session.clone() else {
            return;
        };
        let Some(tab) = self.sidebar.documents.get_mut(&key) else {
            return;
        };
        let Some(text) = &tab.text else { return };
        let revision = tab.content_revision;
        let Some(code) = tab
            .code
            .as_mut()
            .filter(|code| tab.viewer == Viewer::Code && code.requested_revision != Some(revision))
        else {
            return;
        };
        code.requested_revision = Some(revision);
        let source = display_source(&text.text);
        let request = code.highlighter.highlight(source, code.language);
        let highlighter = code.highlighter.clone();
        cx.spawn(async move |this, cx| {
            let Ok(result) = JsFuture::from(request).await else {
                return;
            };
            let Ok(tokens) = serde_wasm_bindgen::from_value(result) else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let visible = this.active_session.as_deref() == Some(session.as_str());
                let Some(code) = this
                    .sidebar_for_session(&session)
                    .and_then(|sidebar| sidebar.documents.get_mut(&key))
                    .filter(|tab| tab.content_revision == revision)
                    .and_then(|tab| tab.code.as_mut())
                    .filter(|code| Rc::ptr_eq(&code.highlighter, &highlighter))
                else {
                    return;
                };
                code.tokens = tokens;
                if visible {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn parse_document_frontmatter(&self, key: u64, cx: &mut Context<Self>) {
        let Some(session) = self.active_session.clone() else {
            return;
        };
        let Some(tab) = self
            .sidebar
            .documents
            .get(&key)
            .filter(|tab| tab.viewer == Viewer::Markdown)
        else {
            return;
        };
        let (Some(metadata), Some(text)) = (&tab.frontmatter, &tab.text) else {
            return;
        };
        let Some((request, parsed, verbatim_len)) = metadata.request(&text.text) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let Ok(result) = JsFuture::from(request).await else {
                return;
            };
            let Some(parsed) = parsed.upgrade() else {
                return;
            };
            let Some(fields_len) = parsed.finish(result) else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                if this.active_session.as_deref() == Some(session.as_str())
                    && let Some(tab) = this.sidebar.documents.get(&key)
                    && tab.viewer == Viewer::Markdown
                {
                    tab.selection.replace_leading_text(verbatim_len, fields_len);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn watch_document(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(session) = self.active_session.clone() else {
            return;
        };
        let Some(shell) = self.browser_shell else {
            return;
        };
        let Some(path) = self.sidebar.surface.documents.get(&key) else {
            return;
        };
        let Some(tab) = self.sidebar.documents.get_mut(&key) else {
            return;
        };
        self.next_file_request = self.next_file_request.wrapping_add(1);
        let watch = WorkspaceWatch::new(shell, self.next_file_request);
        tab.watch = Some(DocumentWatch {
            lease: watch.clone(),
            pending: true,
        });
        let args = json!({"workspace": self.sidebar.workspace, "path": path});
        cx.spawn(async move |this, cx| {
            let Some(result) = watch.follow("watch_workspace_file", args).await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let mounted = this.active_session.as_deref() == Some(session.as_str());
                let Some(tab) = this
                    .sidebar_for_session(&session)
                    .and_then(|sidebar| sidebar.documents.get_mut(&key))
                    .filter(|tab| {
                        tab.watch
                            .as_ref()
                            .is_some_and(|owner| owner.lease.id() == watch.id())
                    })
                else {
                    return;
                };
                if let Some(owner) = tab.watch.as_mut() {
                    owner.pending = false;
                }
                if let Ok(value) = result
                    && let Ok(observation) = serde_json::from_value(value)
                {
                    tab.observe(observation);
                }
                if mounted {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn workspace_file_changed(&mut self, mut event: Value, cx: &mut Context<Self>) {
        let Some(id) = self.workspace_watch_id(&event) else {
            return;
        };
        let Ok(observation) =
            serde_json::from_value::<FileObservation>(event["observation"].take())
        else {
            return;
        };
        for tab in self.sidebar.documents.values_mut() {
            if tab
                .watch
                .as_ref()
                .is_some_and(|watch| watch.lease.id() == id)
            {
                if tab.observe(observation) {
                    cx.notify();
                }
                return;
            }
            if let Some(watches) = &tab.html_assets
                && let Some(watch) = watches
                    .borrow_mut()
                    .iter_mut()
                    .find(|watch| watch.lease.id() == id)
            {
                if watch
                    .observed
                    .as_ref()
                    .is_some_and(|previous| previous.sequence >= observation.sequence)
                {
                    return;
                }
                let changed = watch.observed.as_ref().is_none_or(|previous| {
                    previous.snapshot != observation.snapshot
                        || matches!(observation.snapshot, FileSnapshot::Failed { .. })
                });
                watch.observed = Some(observation);
                if changed {
                    tab.restart = true;
                    cx.notify();
                }
                return;
            }
        }
    }

    pub(super) fn reload_document(&mut self, key: u64, cx: &mut Context<Self>) {
        self.read_document(key, 1, cx);
        cx.notify();
    }

    fn load_more_document(&mut self, key: u64, cx: &mut Context<Self>) {
        if let Some(tab) = self.sidebar.documents.get(&key)
            && tab.request.is_none()
            && let Some(text) = &tab.text
            && !text.eof
        {
            self.read_document(key, text.lines + 1, cx);
            cx.notify();
        }
    }

    fn read_document(&mut self, key: u64, offset: usize, cx: &mut Context<Self>) {
        let Some(session) = self.active_session.clone() else {
            return;
        };
        let Some(path) = self.sidebar.surface.documents.get(&key) else {
            return;
        };
        let tab = self
            .sidebar
            .documents
            .entry(key)
            .or_insert_with(|| DocumentTab::new(path, cx));
        self.next_file_request = self.next_file_request.wrapping_add(1);
        let request = self.next_file_request;
        if offset == 1 {
            tab.viewport.record_scroll();
            tab.version_at_read = tab.observed_version().map(str::to_owned);
            tab.text = None;
            tab.html = None;
            tab.html_assets = None;
            tab.frontmatter = None;
            tab.selection.clear();
            if let Some(code) = tab.code.as_mut() {
                *code = CodeSource::new(code.language);
            }
            tab.viewport.layout_max.set(None);
        }
        tab.request = Some(request);
        tab.restart = false;
        tab.failure = None;
        if tab.viewer == Viewer::Html {
            let assets = self.developer_tools.then(|| {
                let watches = Rc::new(RefCell::new(Vec::new()));
                tab.html_assets = Some(watches.clone());
                let first_watch_id = self.next_file_request.wrapping_add(1);
                self.next_file_request =
                    self.next_file_request.wrapping_add(HTML_MAX_ASSETS as u64);
                HtmlPack {
                    watches: Rc::downgrade(&watches),
                    shell: self.browser_shell,
                    first_watch_id,
                }
            });
            let workspace = self.sidebar.workspace.clone();
            let path = path.clone();
            cx.spawn(async move |this, cx| {
                let result = HtmlPage::read(workspace, path, assets).await;
                let _ = this.update(cx, |this, cx| {
                    let visible = this.active_session.as_deref() == Some(session.as_str());
                    if let Some(sidebar) = this.sidebar_for_session(&session)
                        && let Some(tab) = sidebar.documents.get_mut(&key)
                        && tab.finish_html(request, result)
                        && visible
                    {
                        cx.notify();
                    }
                });
            })
            .detach();
            return;
        }
        let args = json!({"workspace": self.sidebar.workspace, "path": path, "offset": offset});
        cx.spawn(async move |this, cx| {
            let result = bridge::invoke("read_workspace_text", args).await;
            let _ = this.update(cx, |this, cx| {
                let visible = this.active_session.as_deref() == Some(session.as_str());
                if let Some(sidebar) = this.sidebar_for_session(&session)
                    && let Some(tab) = sidebar.documents.get_mut(&key)
                    && tab.finish(request, offset, result)
                    && visible
                {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn toggle_document_wrap(&mut self, key: u64, cx: &mut Context<Self>) {
        if let Some(tab) = self.sidebar.documents.get_mut(&key) {
            tab.wrap = !tab.wrap;
            tab.viewport.layout_max.set(None);
            cx.notify();
        }
    }

    pub(super) fn render_document(
        &self,
        pane: DockPaneId,
        key: u64,
        width: f32,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let path = self
            .sidebar
            .surface
            .documents
            .get(&key)
            .map_or("", String::as_str);
        let tab = self.sidebar.documents.get(&key);
        let path_label = tab
            .and_then(|tab| {
                tab.observed_path
                    .as_deref()
                    .or_else(|| tab.html.as_ref().map(|page| page.absolute_path.as_str()))
                    .or_else(|| tab.text.as_ref().map(|page| page.absolute_path.as_str()))
            })
            .unwrap_or(path);
        let banner = tab.filter(|tab| tab.has_content()).and_then(|tab| {
            tab.metadata_failure().map(ReadFailure::line).or_else(|| {
                tab.has_file_change()
                    .then(|| "文件已更新，当前显示为旧内容".into())
            })
        });
        let wrap = tab.is_none_or(|tab| tab.wrap);
        let choices = Viewer::for_path(path);
        let viewer = tab.map_or(choices[0], |tab| tab.viewer);
        let open = self.document_navigator_open(pane);
        let header = files_header(path_label, cx)
            .when(choices.len() > 1, |header| {
                header.child(self.document_viewer_button(key, viewer, cx))
            })
            .when(viewer.can_wrap(), |header| {
                header.child(
                    self.dock_button(
                        DockControl::DocumentWrap(key),
                        if wrap { "取消换行" } else { "自动换行" },
                        false,
                        cx,
                    )
                    .aria_label("自动换行")
                    .child(
                        svg()
                            .data(if wrap { NOWRAP } else { WRAP })
                            .size(px(15.))
                            .text_color(p.label_secondary),
                    ),
                )
            })
            .child(
                self.dock_button(DockControl::DocumentRefresh(key), "重新读取文件", false, cx)
                    .child(
                        svg()
                            .data(REFRESH_ICON)
                            .size(px(15.))
                            .text_color(p.label_secondary),
                    ),
            )
            .when(!self.sidebar.workspace.is_empty(), |header| {
                header.child(
                    self.dock_button(
                        DockControl::DocumentNavigator(pane),
                        if open {
                            "隐藏文件树"
                        } else {
                            "显示文件树"
                        },
                        false,
                        cx,
                    )
                    .aria_expanded(open)
                    .child(files_folder_icon(open, p.label_secondary).size(px(15.))),
                )
            })
            .relative()
            .when(tab.is_some_and(|tab| tab.viewer_open), |header| {
                header.child(deferred(
                    self.render_document_viewer_menu(key, viewer, choices, cx),
                ))
            });

        let preview = self.render_document_body(pane, key, tab, w, cx);
        let mut content = row().relative().flex_1().min_h_0().w_full().gap_0();
        if open {
            let fill = width < 400. || f32::from(w.viewport_size().width) < 768.;
            let push = !fill && width >= 560.;
            let maximum = (width - if push { 336. } else { 120. }).max(0.);
            let navigator = column()
                .min_h_0()
                .h_full()
                .gap_0()
                .bg(p.bg_base)
                .child(self.render_workspace_tree(pane, Some(path), cx));
            content = if fill {
                content.child(navigator.w_full())
            } else {
                let card = navigator
                    .w(px(260_f32.min(maximum)))
                    .rounded(px(16.))
                    .overflow_hidden()
                    .shadow(crate::tokens::elevation_prominent());
                if push {
                    content
                        .child(preview)
                        .child(card.m(px(8.)).h_auto().self_stretch())
                } else {
                    content.child(preview).child(
                        card.absolute()
                            .top(px(8.))
                            .bottom(px(8.))
                            .right(px(8.))
                            .h_auto()
                            .when(viewer == Viewer::Html, |card| {
                                card.child(document_html::cover(key, 16.))
                            }),
                    )
                }
            };
        } else {
            content = content.child(preview);
        }

        column()
            .relative()
            .size_full()
            .min_h_0()
            .gap_0()
            .bg(p.bg_base)
            .when_some(banner, |preview, message| {
                preview.child(
                    row()
                        .w_full()
                        .flex_none()
                        .items_center()
                        .gap(px(10.))
                        .px(px(10.))
                        .py(px(6.))
                        .bg(p.bg_layer_2)
                        .border_b(px(0.5))
                        .border_color(p.border_l1)
                        .text_color(p.label_secondary)
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .child(div().min_w_0().child(message))
                        .child(self.document_retry_button(key, "重新载入", true, cx)),
                )
            })
            .child(header)
            .child(content)
            .when(tab.is_some_and(|tab| tab.viewer_open), |page| {
                page.child(
                    div()
                        .id("document-viewer-backdrop")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.dismiss_document_viewer(cx);
                            }),
                        ),
                )
            })
    }

    fn render_document_body(
        &self,
        pane: DockPaneId,
        key: u64,
        tab: Option<&DocumentTab>,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let body = div()
            .id("document-body")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .h_full();
        if let Some(tab) = tab {
            if tab.viewer == Viewer::Html {
                if let Some(page) = &tab.html
                    && let Some(content) = &page.document
                {
                    return body.relative().child(self.html_document_frame(
                        pane,
                        key,
                        content.clone(),
                        tab.viewer_open,
                        w,
                        cx,
                    ));
                }
                if tab.html.is_some() || matches!(tab.failure, Some(ReadFailure::NotText)) {
                    return body.child(
                        div()
                            .id("html-preview-error")
                            .role(gpui::Role::Alert)
                            .p(px(10.))
                            .text_color(p.label_secondary)
                            .child("无法预览这份 HTML 文档"),
                    );
                }
            }
            if let Some(text) = &tab.text {
                let selection = SelectionLayout::new(tab.selection.clone());
                let code = tab.code.as_ref().filter(|_| tab.viewer == Viewer::Code);
                let nowrap = !tab.wrap && tab.viewer != Viewer::Markdown;
                let mut source = column()
                    .gap_0()
                    .when(nowrap, |source| source.flex_none().min_w_full())
                    .when(!nowrap, |source| source.w_full().min_w_0());
                if tab.viewer == Viewer::Markdown {
                    let family =
                        super::document_fonts::family(14. + self.content_font_delta as f32, cx);
                    source = source
                        .px(px(12.))
                        .py(px(10.))
                        .when_some(family, |source, family| source.font_family(family));
                    let markdown = if let Some(metadata) = &tab.frontmatter {
                        source = source.children(metadata.render(&text.text, &selection, w, cx));
                        metadata.body(&text.text)
                    } else {
                        &text.text
                    };
                    source = source.child(render_markdown(
                        markdown,
                        &format!("document-{key}"),
                        self.copied_mark.as_deref(),
                        &self.conversation.wrapped_code,
                        self.content_font_delta,
                        Some(&selection),
                        cx,
                    ));
                } else if let Some(code) = code {
                    source = source.child(render_code(
                        display_source(&text.text),
                        &code.tokens,
                        &selection,
                        tab.wrap,
                        self.content_font_delta,
                        cx,
                    ));
                } else {
                    let delta = self.content_font_delta as f32;
                    source = source.child(
                        div()
                            .px(px(18.))
                            .py(px(8.))
                            .font_family(if bridge::is_mac() {
                                "Menlo"
                            } else {
                                "monospace"
                            })
                            .text_size(px(11. + delta))
                            .line_height(px(19. + delta))
                            .when(!tab.wrap, |block| block.whitespace_nowrap())
                            .child(SelectionLayout::text(
                                Some(&selection),
                                text.text.clone(),
                                "",
                                gpui::StyledText::new,
                            )),
                    );
                }
                if let Some(failure) = &tab.failure {
                    source = source.child(
                        row()
                            .px(px(10.))
                            .py(px(6.))
                            .gap(px(10.))
                            .items_center()
                            .text_size(px(self.dock_title_font_size()))
                            .text_color(p.label_secondary)
                            .child(div().min_w_0().child(failure.line()))
                            .child(self.document_retry_button(key, "重试", false, cx)),
                    );
                } else if !text.eof {
                    let pending = tab.request.is_some();
                    let more = div()
                        .id("document-more")
                        .role(gpui::Role::Button)
                        .self_start()
                        .mx(px(10.))
                        .my(px(8.))
                        .px(px(10.))
                        .py(px(4.))
                        .rounded(px(8.))
                        .border(px(0.5))
                        .border_color(p.border_l2)
                        .bg(p.bg_layer_2)
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(if pending {
                            p.label_tertiary
                        } else {
                            p.label_secondary
                        })
                        .child(if pending {
                            "文档渲染中..."
                        } else {
                            "加载更多"
                        });
                    let more = if pending {
                        more
                    } else {
                        dock_action_button(
                            more.tab_stop(true)
                                .cursor_pointer()
                                .hover(|style| style.bg(p.bg_layer_3)),
                            cx.listener(move |this, _, _, cx| this.load_more_document(key, cx)),
                        )
                    };
                    source = source.child(more);
                }
                let pointer = selection.clone();
                let menu = selection.clone();
                let drag_scroll = tab.viewport.clone();
                let wheel = tab.viewport.clone();
                let viewport = tab.viewport.clone();
                let can_load_more = !text.eof
                    && tab.request.is_none()
                    && tab.failure.is_none()
                    && tab.metadata_failure().is_none();
                let body = body
                    .when(!nowrap, |body| body.overflow_y_scroll())
                    .when(nowrap, |body| body.flex().flex_row().overflow_scroll())
                    .track_focus(&tab.selection.focus)
                    .tab_stop(true)
                    .cursor(gpui::CursorStyle::IBeam)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |_, event, w, cx| {
                            pointer.mouse_down(event, w, cx);
                        }),
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |_, event, w, cx| {
                            menu.context_menu(event, w, cx);
                        }),
                    )
                    .track_scroll(&tab.viewport.scroll)
                    .on_scroll(move |event, _, _| {
                        if event.delta.pixel_delta(px(1.)).y != px(0.) {
                            wheel.scrolled.set(true);
                        }
                    })
                    .child(
                        source
                            .relative()
                            .child(
                                canvas(
                                    move |_, w, _| viewport.restore_after_layout(can_load_more, w),
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            )
                            .child(selection.event_layer(tab.viewport.scroll.clone(), move || {
                                drag_scroll.scrolled.set(true);
                            })),
                    );
                return if let Some(code) = code {
                    column()
                        .id("document-code")
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .h_full()
                        .gap_0()
                        .child(self.document_code_toolbar(key, code, cx))
                        .child(body)
                } else {
                    body
                };
            }
            if let Some(failure) = &tab.failure {
                return body.child(self.render_document_error(key, tab.viewer, failure, cx));
            }
        }
        body.flex()
            .items_center()
            .justify_center()
            .text_size(px(self.dock_title_font_size()))
            .text_color(p.label_secondary)
            .child("文档渲染中...")
    }

    fn render_document_error(
        &self,
        key: u64,
        viewer: Viewer,
        error: &ReadFailure,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        column()
            .items_center()
            .justify_center()
            .gap(px(16.))
            .px(px(24.))
            .text_size(px(self.dock_title_font_size()))
            .text_color(p.label_secondary)
            .size_full()
            .pb(gpui::relative(0.12))
            .child(file_sheet_icon(36.).opacity(0.6))
            .child(
                if viewer == Viewer::Html && matches!(error, ReadFailure::TooLarge) {
                    "单页内容超过 32 MB 上限，无法读取".into()
                } else {
                    error.line()
                },
            )
            .when(error.can_retry(), |status| {
                status.child(dock_action_button(
                    row()
                        .id("document-retry")
                        .role(gpui::Role::Button)
                        .tab_stop(true)
                        .gap(px(6.))
                        .h(px(32.))
                        .px(px(12.))
                        .rounded(px(12.))
                        .border(px(0.5))
                        .border_color(p.border_l2)
                        .cursor_pointer()
                        .hover(|style| style.bg(p.hover))
                        .text_color(p.label_primary)
                        .child(
                            svg()
                                .data(REFRESH_ICON)
                                .size(px(14.))
                                .text_color(p.label_primary),
                        )
                        .child("重试"),
                    cx.listener(move |this, _, _, cx| this.reload_document(key, cx)),
                ))
            })
    }

    fn document_retry_button(
        &self,
        key: u64,
        label: &'static str,
        reload: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        dock_action_button(
            row()
                .id(if reload {
                    "document-reload-now"
                } else {
                    "document-retry-page"
                })
                .role(gpui::Role::Button)
                .tab_stop(true)
                .flex_none()
                .px(px(10.))
                .h(px(28.))
                .items_center()
                .rounded(px(8.))
                .border(px(0.5))
                .border_color(p.border_l2)
                .cursor_pointer()
                .bg(p.bg_layer_2)
                .hover(|style| style.bg(p.bg_layer_3))
                .text_color(p.label_primary)
                .text_size(px(13.))
                .line_height(px(18.))
                .child(label),
            cx.listener(move |this, _, _, cx| {
                if reload {
                    this.reload_document(key, cx);
                } else {
                    this.load_more_document(key, cx);
                }
            }),
        )
    }

    fn document_code_toolbar(
        &self,
        key: u64,
        code: &CodeSource,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let p = dsh(cx);
        let copied = code.copied.get();
        let label = if copied { "已复制" } else { "复制" };
        row()
            .w_full()
            .h(px(42.))
            .flex_none()
            .pt(px(10.))
            .pb(px(8.))
            .pl(px(22.))
            .pr(px(18.))
            .items_center()
            .justify_between()
            .child(
                div()
                    .font_family(if bridge::is_mac() {
                        "Menlo"
                    } else {
                        "monospace"
                    })
                    .text_size(px(11.))
                    .line_height(px(18.))
                    .text_color(p.label_tertiary)
                    .child(code.language),
            )
            .child(
                self.dock_button(DockControl::DocumentCopy(key), label, false, cx)
                    .size(px(24.))
                    .child(
                        svg()
                            .data(if copied { CODE_COPIED } else { CODE_COPY })
                            .size(px(14.))
                            .text_color(p.label_secondary),
                    ),
            )
    }

    pub(super) fn copy_document_code(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(tab) = self
            .sidebar
            .documents
            .get(&key)
            .filter(|tab| tab.viewer == Viewer::Code)
        else {
            return;
        };
        let (Some(text), Some(code)) = (&tab.text, &tab.code) else {
            return;
        };
        if code.copied.get() {
            return;
        }
        let copied = Rc::downgrade(&code.copied);
        let request = bridge::copy_text_checked(display_source(&text.text));
        cx.spawn(async move |this, cx| {
            if !JsFuture::from(request)
                .await
                .ok()
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                return;
            }
            let Some(current) = copied.upgrade() else {
                return;
            };
            if current.replace(true) {
                return;
            }
            drop(current);
            let _ = this.update(cx, |_, cx| cx.notify());
            cx.background_executor()
                .timer(std::time::Duration::from_secs(1))
                .await;
            if let Some(current) = copied.upgrade() {
                current.set(false);
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
    }

    fn document_viewer_button(
        &self,
        key: u64,
        viewer: Viewer,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        dock_action_button(
            row()
                .id("document-viewer")
                .role(gpui::Role::Button)
                .aria_label("打开方式")
                .aria_expanded(
                    self.sidebar
                        .documents
                        .get(&key)
                        .is_some_and(|tab| tab.viewer_open),
                )
                .tab_stop(true)
                .h(px(28.))
                .px(px(6.))
                .flex_none()
                .rounded(px(8.))
                .cursor_pointer()
                .hover(|style| style.bg(p.hover))
                .text_size(px(12.))
                .text_color(p.label_secondary)
                .child(viewer.label()),
            cx.listener(move |this, _, _, cx| {
                if let Some(tab) = this.sidebar.documents.get_mut(&key) {
                    tab.viewer_open = !tab.viewer_open;
                    cx.notify();
                }
            }),
        )
    }

    pub(super) fn dismiss_document_viewer(&mut self, cx: &mut Context<Self>) -> bool {
        let mut dismissed = false;
        for tab in self.sidebar.documents.values_mut() {
            dismissed |= std::mem::take(&mut tab.viewer_open);
        }
        if dismissed {
            cx.notify();
        }
        dismissed
    }

    fn render_document_viewer_menu(
        &self,
        key: u64,
        viewer: Viewer,
        choices: &'static [Viewer],
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        column()
            .id("document-viewer-menu")
            .absolute()
            .top(px(34.))
            .right(px(if viewer.can_wrap() { 98. } else { 66. }))
            .occlude()
            .w(px(150.))
            .p(px(4.))
            .gap_0()
            .rounded(px(12.))
            .bg(p.bg_layer_2)
            .shadow(crate::tokens::elevation_prominent())
            .when(viewer == Viewer::Html, |menu| {
                menu.child(document_html::cover(key, 12.))
            })
            .children(choices.iter().copied().map(|next| {
                let label = next.label();
                dock_action_button(
                    row()
                        .id(label)
                        .role(gpui::Role::Button)
                        .tab_stop(true)
                        .h(px(28.))
                        .px(px(8.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .when(next == viewer, |row| row.bg(p.hover_active))
                        .hover(|style| style.bg(p.hover))
                        .text_size(px(13.))
                        .text_color(p.label_primary)
                        .child(label),
                    cx.listener(move |this, _, _, cx| {
                        if let Some(tab) = this.sidebar.documents.get_mut(&key) {
                            if tab.viewer != next {
                                if tab.viewer == Viewer::Html || next == Viewer::Html {
                                    tab.text = None;
                                    tab.retire_html();
                                }
                                tab.viewer = next;
                                tab.selection.clear();
                                tab.viewport.layout_max.set(None);
                                if let Some(code) = tab.code.as_mut() {
                                    code.copied = Rc::new(Cell::new(false));
                                }
                            }
                            tab.viewer_open = false;
                            cx.notify();
                        }
                    }),
                )
            }))
    }
}
