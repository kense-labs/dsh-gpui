//! One child webview per browser occurrence, with at most one visible in each pane.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
#[cfg(target_os = "macos")]
use std::sync::{Arc, atomic::AtomicBool};
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};
use tauri::{
    AppHandle, Emitter, Manager, Runtime, State, Webview, WebviewUrl, Window, WindowEvent,
    webview::{
        NewWindowResponse, PageLoadEvent, PageLoadPayload, PermissionResponse, PlatformWebview,
        WebviewBuilder,
    },
};

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub(crate) use macos::input::{install_guest_input, uninstall_guest_input};

#[derive(Default)]
pub struct BrowserService {
    shell: AtomicU64,
    state: Mutex<BrowserState>,
    #[cfg(target_os = "macos")]
    html_focus: Mutex<HtmlPreviewFocus>,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Default)]
struct HtmlPreviewFocus {
    shell: u64,
    revision: u32,
    key: Option<u64>,
}

#[cfg(target_os = "macos")]
impl HtmlPreviewFocus {
    fn update(&mut self, current_shell: u64, next: Self) {
        if next.shell == current_shell
            && (self.shell != next.shell || self.revision < next.revision)
        {
            *self = next;
        }
    }
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) fn set_html_preview_focus(
    webview: Webview,
    shell: u64,
    revision: u32,
    key: Option<u64>,
) {
    if webview.label() != "main" {
        return;
    }
    let service = webview.state::<BrowserService>();
    if let Ok(mut focus) = service.html_focus.lock() {
        focus.update(
            service.shell.load(Ordering::Acquire),
            HtmlPreviewFocus {
                shell,
                revision,
                key,
            },
        );
    }
}

#[derive(Default)]
struct BrowserState {
    shell: u64,
    revision: u64,
    visible: HashSet<u64>,
    closed: HashSet<u64>,
    origin: Option<url::Origin>,
    entries: HashMap<u64, BrowserEntry>,
}

#[derive(Clone, Copy)]
struct BrowserId {
    shell: u64,
    key: u64,
}

struct BrowserEntry {
    address: url::Url,
    navigation: u64,
    guest: BrowserGuest,
}

#[derive(Clone)]
struct BrowserGuest {
    #[cfg(target_os = "macos")]
    storage: BrowserStorage,
    #[cfg(target_os = "macos")]
    terminated: Arc<AtomicBool>,
}

impl BrowserGuest {
    fn new(_owner: &BrowserOwner, _previous: Option<&Self>) -> Self {
        Self {
            #[cfg(target_os = "macos")]
            storage: _previous.map_or_else(|| _owner.storage(), |guest| guest.storage.clone()),
            #[cfg(target_os = "macos")]
            terminated: Arc::default(),
        }
    }

    fn is_terminated(&self) -> bool {
        #[cfg(target_os = "macos")]
        return self.terminated.load(Ordering::Acquire);
        #[cfg(not(target_os = "macos"))]
        false
    }
}

impl BrowserState {
    fn accept_presentation(&mut self, revision: u64) -> bool {
        if revision <= self.revision {
            return false;
        }
        self.revision = revision;
        true
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub struct BrowserBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl BrowserBounds {
    fn intersects(self, other: Self) -> bool {
        self.x < other.x + other.width
            && self.x + self.width > other.x
            && self.y < other.y + other.height
            && self.y + self.height > other.y
    }

    fn valid(self) -> bool {
        self.width > 0.
            && self.height > 0.
            && [self.x, self.y, self.width, self.height]
                .into_iter()
                .all(|coordinate| coordinate.abs() <= 1_000_000.)
    }
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct BrowserClip {
    #[serde(flatten)]
    bounds: BrowserBounds,
    radius: f64,
    #[serde(default)]
    visual_only: bool,
}

impl BrowserClip {
    fn valid(self) -> bool {
        self.bounds.valid() && (0. ..=1_000_000.).contains(&self.radius)
    }
}

#[derive(Clone, Debug, Deserialize)]
struct BrowserLayout {
    bounds: BrowserBounds,
    #[serde(default)]
    frame_clip: Option<BrowserClip>,
    #[serde(default)]
    covers: Vec<BrowserClip>,
}

impl BrowserLayout {
    fn valid(&self) -> bool {
        self.bounds.valid()
            && self.frame_clip.is_none_or(BrowserClip::valid)
            && self.covers.iter().copied().all(BrowserClip::valid)
    }
}

#[derive(Debug, Deserialize)]
pub struct BrowserPresentation {
    key: u64,
    address: String,
    navigation: u64,
    #[serde(flatten)]
    layout: BrowserLayout,
}

#[derive(Debug, Deserialize)]
pub struct BrowserOwner {
    session: String,
    #[cfg(target_os = "macos")]
    cwd: String,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum BrowserStorage {
    Workspace(std::path::PathBuf),
    Session(String),
}

impl BrowserOwner {
    fn valid(&self) -> bool {
        !self.session.is_empty() && self.session.encode_utf16().count() <= 4096 - "session:".len()
    }

    #[cfg(target_os = "macos")]
    fn storage(&self) -> BrowserStorage {
        let cwd = std::path::Path::new(&self.cwd);
        if cwd.is_absolute()
            && let Ok(canonical) = cwd.canonicalize()
            && canonical.is_dir()
        {
            BrowserStorage::Workspace(canonical)
        } else {
            BrowserStorage::Session(self.session.clone())
        }
    }
}

#[derive(Serialize)]
pub struct BrowserPresentationResult {
    key: u64,
    navigation: u64,
    target: Option<BrowserTarget>,
    error: Option<String>,
}

#[derive(Serialize)]
struct BrowserTarget {
    address: String,
    title: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrowserAction {
    Back,
    Forward,
    Reload,
}

fn browser_label(id: BrowserId) -> String {
    format!("sidebar-browser-{}-{}", id.shell, id.key)
}

pub(crate) fn current_browser_shell<R: Runtime>(app: &AppHandle<R>) -> u64 {
    app.state::<BrowserService>().shell.load(Ordering::Acquire)
}

fn sync_browser_shell<R: Runtime>(app: &AppHandle<R>, state: &mut BrowserState, shell: u64) {
    if state.shell == shell {
        return;
    }
    let old = std::mem::replace(
        state,
        BrowserState {
            shell,
            ..Default::default()
        },
    );
    for key in old.entries.into_keys() {
        if let Err(error) = close_child(
            app,
            BrowserId {
                shell: old.shell,
                key,
            },
        ) {
            eprintln!("{error}");
        }
    }
}

fn invalidate_browser_shell<R: Runtime>(app: &AppHandle<R>) {
    app.state::<BrowserService>()
        .shell
        .fetch_add(1, Ordering::AcqRel);
    crate::workspace::watch::retire_shell(app);
    let app = app.clone();
    // Do not wait on a presentation that is itself waiting for the window thread.
    tauri::async_runtime::spawn(async move {
        let service = app.state::<BrowserService>();
        if let Ok(mut state) = service.state.lock() {
            sync_browser_shell(&app, &mut state, current_browser_shell(&app));
        }
    });
}

pub(crate) fn shell_page_load<R: Runtime>(webview: &Webview<R>, page: &PageLoadPayload<'_>) {
    if webview.label() == "main" && matches!(page.event(), PageLoadEvent::Started) {
        invalidate_browser_shell(webview.app_handle());
    }
}

pub(crate) fn shell_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if window.label() == "main" && matches!(event, WindowEvent::Destroyed) {
        invalidate_browser_shell(window.app_handle());
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn shell_process_terminated<R: Runtime>(webview: &Webview<R>) {
    if webview.label() == "main" {
        invalidate_browser_shell(webview.app_handle());
    }
    // Preserve the runtime's default reload for the shell and legacy webviews.
    // Native Sidebar guests consume termination in their own delegate.
    if let Err(error) = webview.reload() {
        eprintln!("{error}");
    }
}

#[tauri::command]
pub fn browser_shell_generation(webview: Webview) -> Result<u64, String> {
    if webview.label() != "main" {
        return Err("浏览器只能由主界面打开".into());
    }
    Ok(current_browser_shell(webview.app_handle()))
}

fn normalize_url(raw: &str) -> Result<url::Url, String> {
    let trimmed =
        raw.trim_matches(|ch: char| ch == '\u{feff}' || (ch.is_whitespace() && ch != '\u{0085}'));
    if trimmed.is_empty() {
        return Err("请输入地址。".into());
    }
    if trimmed.encode_utf16().count() > 16 * 1024 {
        return Err("这个地址无效或过长。".into());
    }
    let explicit_scheme = trimmed.split_once(':').is_some_and(|(prefix, rest)| {
        let mut bytes = prefix.bytes();
        let is_scheme = bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || b"+.-".contains(&byte));
        let port = rest.split(['/', '?', '#']).next().unwrap_or_default();
        is_scheme && (port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()))
    });
    let candidate = if explicit_scheme {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = url::Url::parse(&candidate).map_err(|_| "这个地址无效或过长。")?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("地址不能包含用户名或密码。".into());
    }
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("只支持 HTTP 和 HTTPS 地址；本地文件请使用文档预览。".into());
    }
    Ok(parsed)
}

fn present_child<R: Runtime>(
    app: &AppHandle<R>,
    id: BrowserId,
    presentation: &BrowserPresentation,
    address: &url::Url,
    navigate: bool,
    application_origin: Option<url::Origin>,
    _guest: &BrowserGuest,
) -> Result<(), String> {
    let layout = &presentation.layout;
    let label = browser_label(id);
    #[cfg(target_os = "macos")]
    let native = macos::supports_clipping();
    #[cfg(not(target_os = "macos"))]
    let native = false;
    if let Some(webview) = app.get_webview(&label) {
        #[cfg(target_os = "macos")]
        if native {
            return macos::present(
                &webview,
                id,
                layout.clone(),
                navigate.then(|| (presentation.navigation, address.clone())),
                application_origin,
                _guest.clone(),
            );
        }
        if navigate {
            if webview.url().ok().as_ref() == Some(address) {
                webview
                    .reload()
                    .map_err(|error| format!("刷新失败：{error}"))?;
            } else {
                webview
                    .navigate(address.clone())
                    .map_err(|error| format!("导航失败：{error}"))?;
            }
        }
        return present_surface(&webview, layout);
    }
    let window = app
        .get_window("main")
        .or_else(|| {
            app.get_webview_window("main")
                .map(|window| window.as_ref().window())
        })
        .ok_or_else(|| "主窗口不可用".to_string())?;
    let popup_app = app.clone();
    let popup_origin = application_origin.clone();
    let navigation_origin = application_origin.clone();
    // Register the native page observer before starting the first request.
    let initial_url = if native {
        url::Url::parse("about:blank").expect("valid blank URL")
    } else {
        address.clone()
    };
    let builder = WebviewBuilder::new(label, WebviewUrl::External(initial_url))
        .focused(false)
        .on_permission_request(|_, _| PermissionResponse::Deny)
        .on_navigation(move |address| allowed_target(address, navigation_origin.as_ref()))
        .on_new_window(move |address, _| {
            if allowed_target(&address, popup_origin.as_ref()) {
                emit_browser_popup(&popup_app, id, &address);
            }
            NewWindowResponse::Deny
        })
        .on_page_load(move |webview, payload| {
            if !native {
                emit_browser_page(
                    &webview,
                    id,
                    payload.url(),
                    matches!(payload.event(), PageLoadEvent::Started),
                );
            }
        });
    let builder = if native {
        builder
    } else {
        builder.on_download(|_, _| false)
    };
    let bounds = layout.bounds;
    // Start clear of the canvas until the frame and its input mask are installed.
    let position = tauri::LogicalPosition::new(-bounds.width - 1., 0.);
    let size = tauri::LogicalSize::new(bounds.width.max(1.), bounds.height.max(1.));
    #[cfg(target_os = "macos")]
    let webview = macos::add_child(
        window,
        builder,
        position,
        size,
        _guest.storage.clone(),
        id.shell,
    )?;
    #[cfg(not(target_os = "macos"))]
    let webview = window
        .add_child(builder, position, size)
        .map_err(|error| format!("创建内嵌浏览器失败：{error}"))?;
    #[cfg(target_os = "macos")]
    if native {
        return macos::present(
            &webview,
            id,
            layout.clone(),
            Some((presentation.navigation, address.clone())),
            application_origin,
            _guest.clone(),
        );
    }
    present_surface(&webview, layout)
}

fn emit_browser_popup<R: Runtime>(app: &AppHandle<R>, id: BrowserId, address: &url::Url) {
    let _ = app.emit_to(
        "main",
        "acp-event",
        json!({"kind": "browser_popup", "shell": id.shell, "key": id.key, "address": address.as_str()}),
    );
}

fn close_child<R: Runtime>(app: &AppHandle<R>, id: BrowserId) -> Result<(), String> {
    if let Some(webview) = app.get_webview(&browser_label(id)) {
        #[cfg(target_os = "macos")]
        macos::detach(&webview);
        webview
            .close()
            .map_err(|error| format!("关闭浏览器失败：{error}"))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn browser_terminated<R: Runtime>(app: AppHandle<R>, id: BrowserId, guest: &BrowserGuest) {
    if guest.terminated.swap(true, Ordering::AcqRel) {
        return;
    }
    let terminated = guest.terminated.clone();
    // Presentation can be waiting for AppKit while holding this lock.
    tauri::async_runtime::spawn(async move {
        let service = app.state::<BrowserService>();
        let Ok(mut state) = service.state.lock() else {
            return;
        };
        if state.shell != id.shell || current_browser_shell(&app) != id.shell {
            return;
        }
        let Some(entry) = state
            .entries
            .get(&id.key)
            .filter(|entry| Arc::ptr_eq(&entry.guest.terminated, &terminated))
        else {
            return;
        };
        let navigation = entry.navigation;
        if let Err(error) = close_child(&app, id) {
            eprintln!("{error}");
        }
        state.visible.remove(&id.key);
        let _ = app.emit_to(
            "main",
            "acp-event",
            json!({
                "kind": "browser_terminated",
                "shell": id.shell,
                "key": id.key,
                "navigation": navigation,
            }),
        );
    });
}

fn present_surface<R: Runtime>(webview: &Webview<R>, layout: &BrowserLayout) -> Result<(), String> {
    let mut bounds = layout.bounds;
    if layout.frame_clip.is_some() {
        bounds.height -= 20.;
    }
    if bounds.height <= 0.
        || layout
            .covers
            .iter()
            .any(|cover| !cover.visual_only && bounds.intersects(cover.bounds))
    {
        return webview
            .hide()
            .map_err(|error| format!("隐藏浏览器失败：{error}"));
    }
    webview
        .set_bounds(tauri::Rect {
            position: tauri::LogicalPosition::new(bounds.x, bounds.y).into(),
            size: tauri::LogicalSize::new(bounds.width.max(1.), bounds.height.max(1.)).into(),
        })
        .map_err(|error| format!("定位浏览器失败：{error}"))?;
    webview
        .show()
        .map_err(|error| format!("显示浏览器失败：{error}"))
}

fn allowed_target(address: &url::Url, application_origin: Option<&url::Origin>) -> bool {
    matches!(address.scheme(), "http" | "https")
        && address.username().is_empty()
        && address.password().is_none()
        && application_origin != Some(&address.origin())
}

fn emit_browser_page<R: Runtime>(
    webview: &Webview<R>,
    id: BrowserId,
    address: &url::Url,
    loading: bool,
) {
    let app = webview.app_handle().clone();
    let title = address.host_str().unwrap_or_default().to_owned();
    let address = address.as_str().to_owned();
    let _ = webview.with_webview(move |view| {
        let (can_back, can_forward) = browser_history_availability(&view);
        let _ = app.emit_to("main", "acp-event", json!({"kind": "browser_page", "shell": id.shell, "key": id.key,
            "address": address, "title": title, "loading": loading, "canBack": can_back, "canForward": can_forward}));
    });
}

fn browser_history_availability(view: &PlatformWebview) -> (bool, bool) {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `PlatformWebview::inner` returns the `WKWebView` that Tauri owns;
        // `cast` only retypes that pointer and the borrow ends with the temporary,
        // so no reference outlives the webview.
        let view: &objc2_web_kit::WKWebView = unsafe { &*view.inner().cast() };
        // SAFETY: `view` is a live `WKWebView`; each is a read-only Objective-C
        // accessor taking no arguments and returning a plain boolean, so the
        // selector and return type match the declared method.
        let back = unsafe { view.canGoBack() };
        // SAFETY: as above, for the forward accessor.
        let forward = unsafe { view.canGoForward() };
        return (back, forward);
    }
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    {
        use webkit2gtk::WebViewExt;
        let view = view.inner();
        return (view.can_go_back(), view.can_go_forward());
    }
    #[cfg(windows)]
    {
        // SAFETY: `view` is a live Tauri-owned WebView2 widget; the COM
        // pointer is only read inside this block.
        let core = unsafe { view.controller().CoreWebView2() };
        if let Ok(view) = core {
            let mut back = Default::default();
            let mut forward = Default::default();
            // SAFETY: both BOOL receivers live in this stack frame and stay
            // valid for the duration of each call.
            let can_back = unsafe { view.CanGoBack(&mut back) }.is_ok() && back.as_bool();
            let can_forward =
                unsafe { view.CanGoForward(&mut forward) }.is_ok() && forward.as_bool();
            return (can_back, can_forward);
        }
    }
    #[allow(unreachable_code)]
    (false, false)
}

fn move_browser_history(view: &PlatformWebview, forward: bool) {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: same retype of the Tauri-owned `WKWebView` as in
        // `browser_history_availability`; the borrow is confined to this block.
        let view: &objc2_web_kit::WKWebView = unsafe { &*view.inner().cast() };
        // SAFETY: `view` is a live `WKWebView`; navigation selectors take no
        // arguments and only enqueue a load in the webview, whose result is
        // deliberately discarded.
        if forward {
            // SAFETY: as above, for the forward selector.
            let _ = unsafe { view.goForward() };
        } else {
            // SAFETY: as above, for the back selector.
            let _ = unsafe { view.goBack() };
        }
    }
    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    {
        use webkit2gtk::WebViewExt;
        let view = view.inner();
        if forward {
            view.go_forward();
        } else {
            view.go_back();
        }
    }
    #[cfg(windows)]
    {
        // SAFETY: `view` is a live Tauri-owned WebView2 widget; the COM
        // pointer is only read inside this block.
        let core = unsafe { view.controller().CoreWebView2() };
        if let Ok(view) = core {
            if forward {
                // SAFETY: navigation enqueues a load on the live webview; its
                // result is deliberately discarded.
                let _ = unsafe { view.GoForward() };
            } else {
                // SAFETY: as above, for the backward navigation.
                let _ = unsafe { view.GoBack() };
            }
        }
    }
}

#[tauri::command]
pub async fn present_embedded_browser(
    app: AppHandle,
    service: State<'_, BrowserService>,
    shell: u64,
    revision: u64,
    presentations: Vec<BrowserPresentation>,
    owner: Option<BrowserOwner>,
) -> Result<Vec<BrowserPresentationResult>, String> {
    let requested: HashSet<_> = presentations.iter().map(|item| item.key).collect();
    if requested.len() != presentations.len()
        || presentations.iter().any(|item| !item.layout.valid())
    {
        return Err("浏览器面板布局无效".into());
    }
    if !presentations.is_empty() && owner.as_ref().is_none_or(|owner| !owner.valid()) {
        return Err("浏览器所属会话无效".into());
    }
    let mut state = service.state.lock().map_err(|_| "浏览器状态锁失败")?;
    if current_browser_shell(&app) != shell {
        return Ok(Vec::new());
    }
    sync_browser_shell(&app, &mut state, shell);
    if !state.accept_presentation(revision) {
        return Ok(Vec::new());
    }
    let obsolete: Vec<_> = state.visible.difference(&requested).copied().collect();
    for key in obsolete {
        if let Some(webview) = app.get_webview(&browser_label(BrowserId { shell, key })) {
            webview
                .hide()
                .map_err(|error| format!("隐藏浏览器失败：{error}"))?;
        }
        state.visible.remove(&key);
    }
    let Some(owner) = owner else {
        return Ok(Vec::new());
    };
    if state.origin.is_none() && !presentations.is_empty() {
        state.origin = app
            .get_webview("main")
            .and_then(|webview| webview.url().ok())
            .map(|url| url.origin());
    }
    Ok(presentations
        .into_iter()
        .filter_map(|presentation| {
            let key = presentation.key;
            let id = BrowserId { shell, key };
            if state.closed.contains(&key) {
                return None;
            }
            let navigation = presentation.navigation;
            let mut target = None;
            let outcome = (|| {
                let address = normalize_url(&presentation.address)?;
                if !allowed_target(&address, state.origin.as_ref()) {
                    return Err("不能在嵌入浏览器中打开 DSH 应用自身。".into());
                }
                target = Some(BrowserTarget {
                    address: address.as_str().to_owned(),
                    title: address.host_str().unwrap_or_default().to_owned(),
                });
                let previous = state.entries.get(&key);
                let terminated = previous.is_some_and(|entry| entry.guest.is_terminated());
                if terminated && previous.is_some_and(|entry| navigation <= entry.navigation) {
                    return Ok(());
                }
                let guest = if let Some(entry) = previous.filter(|_| !terminated) {
                    entry.guest.clone()
                } else {
                    BrowserGuest::new(&owner, previous.map(|entry| &entry.guest))
                };
                if terminated {
                    close_child(&app, id)?;
                }
                let navigate = terminated
                    || previous.is_none_or(|entry| {
                        entry.address != address || entry.navigation != navigation
                    });
                present_child(
                    &app,
                    id,
                    &presentation,
                    &address,
                    navigate,
                    state.origin.clone(),
                    &guest,
                )?;
                state.entries.insert(
                    key,
                    BrowserEntry {
                        address,
                        navigation,
                        guest,
                    },
                );
                state.visible.insert(key);
                Ok(())
            })();
            Some(BrowserPresentationResult {
                key,
                navigation,
                target,
                error: outcome.err(),
            })
        })
        .collect())
}

#[tauri::command]
pub async fn embedded_browser_action(
    app: AppHandle,
    service: State<'_, BrowserService>,
    shell: u64,
    key: u64,
    navigation: u64,
    action: BrowserAction,
) -> Result<(), String> {
    if current_browser_shell(&app) != shell {
        return Ok(());
    }
    let state = service.state.lock().map_err(|_| "浏览器状态锁失败")?;
    if state.shell != shell
        || state
            .entries
            .get(&key)
            .is_none_or(|entry| entry.navigation != navigation)
    {
        return Ok(());
    }
    let webview = app
        .get_webview(&browser_label(BrowserId { shell, key }))
        .ok_or("浏览器页面尚未打开")?;
    match action {
        BrowserAction::Reload => webview.reload(),
        BrowserAction::Back | BrowserAction::Forward => webview.with_webview(move |view| {
            move_browser_history(&view, matches!(action, BrowserAction::Forward));
        }),
    }
    .map_err(|error| format!("浏览器操作失败：{error}"))
}

#[tauri::command]
pub fn focus_browser_shell(webview: Webview, shell: u64) -> Result<(), String> {
    if webview.label() != "main" {
        return Err("浏览器子视图不能设置应用焦点".into());
    }
    if current_browser_shell(webview.app_handle()) != shell {
        return Ok(());
    }
    webview.set_focus().map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn close_embedded_browser(
    app: AppHandle,
    service: State<'_, BrowserService>,
    shell: u64,
    key: u64,
) -> Result<(), String> {
    let mut state = service.state.lock().map_err(|_| "浏览器状态锁失败")?;
    if current_browser_shell(&app) != shell {
        return Ok(());
    }
    sync_browser_shell(&app, &mut state, shell);
    // Closing one tab must not discard another tab's in-flight presentation.
    state.closed.insert(key);
    close_child(&app, BrowserId { shell, key })?;
    state.visible.remove(&key);
    state.entries.remove(&key);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn preview_focus_rejects_old_revisions_and_replaced_shells() {
        let focus = |shell, revision, key| HtmlPreviewFocus {
            shell,
            revision,
            key,
        };
        let mut state = HtmlPreviewFocus::default();
        state.update(1, focus(1, 2, Some(8)));
        state.update(1, focus(1, 1, None));
        assert_eq!(state.key, Some(8));
        state.update(1, focus(1, 3, None));
        state.update(1, focus(1, 2, Some(8)));
        assert_eq!(state.key, None);
        state.update(2, focus(2, 1, Some(9)));
        state.update(2, focus(1, 4, Some(8)));
        assert_eq!((state.shell, state.revision, state.key), (2, 1, Some(9)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn browser_storage_groups_canonical_cwds_and_isolates_ungrouped_sessions() {
        let root = std::env::current_dir().unwrap();
        let storage = |session: &str, cwd: &std::path::Path| {
            BrowserOwner {
                session: session.into(),
                cwd: cwd.to_str().unwrap().into(),
            }
            .storage()
        };
        let workspace = storage("first", &root);
        assert_eq!(
            workspace,
            BrowserStorage::Workspace(root.canonicalize().unwrap())
        );
        assert_eq!(workspace, storage("second", &root.join(".")));
        assert_ne!(workspace, storage("first", root.parent().unwrap()));
        let ungrouped = std::path::Path::new("");
        assert_eq!(
            storage("first", ungrouped),
            BrowserStorage::Session("first".into())
        );
        assert_ne!(storage("first", ungrouped), storage("second", ungrouped));
    }

    #[test]
    fn browser_presentation_accepts_fractional_bounds_and_rounded_occluders() {
        let presentation: BrowserPresentation = serde_json::from_value(json!({
            "key": 7,
            "address": "https://example.com/",
            "navigation": 2,
            "bounds": {"x": 450.5, "y": 76.0, "width": 300.0, "height": 200.0},
            "frame_clip": null,
            "covers": [{"x": 460.0, "y": 80.0, "width": 144.0, "height": 110.0, "radius": 16.0}]
        }))
        .unwrap();
        assert!(presentation.layout.valid());
        assert_eq!(presentation.layout.bounds.x, 450.5);
        assert_eq!(presentation.layout.covers[0].radius, 16.);
        assert!(
            presentation
                .layout
                .bounds
                .intersects(presentation.layout.covers[0].bounds)
        );
    }

    #[test]
    fn stale_presentations_cannot_switch_back_to_the_previous_browser() {
        let mut state = BrowserState::default();
        assert!(state.accept_presentation(4));
        assert!(!state.accept_presentation(3));
        assert!(!state.accept_presentation(4));
        assert!(state.accept_presentation(5));
        assert_ne!(
            browser_label(BrowserId { shell: 1, key: 1 }),
            browser_label(BrowserId { shell: 1, key: 2 })
        );
        assert_ne!(
            browser_label(BrowserId { shell: 1, key: 1 }),
            browser_label(BrowserId { shell: 2, key: 1 })
        );
    }

    #[test]
    fn browser_addresses_distinguish_ports_from_schemes_and_reject_credentials() {
        assert_eq!(
            normalize_url("localhost:3100/page").unwrap().as_str(),
            "https://localhost:3100/page"
        );
        assert_eq!(
            normalize_url("HTTPS://EXAMPLE.COM").unwrap().as_str(),
            "https://example.com/"
        );
        assert_eq!(
            normalize_url("\u{feff}例子.test:8443/path\u{feff}")
                .unwrap()
                .as_str(),
            "https://xn--fsqu00a.test:8443/path"
        );
        assert!(normalize_url("file:///tmp/test").is_err());
        for scheme in ["https", "ftp"] {
            assert_eq!(
                normalize_url(&format!("{scheme}://user:secret@example.com")).unwrap_err(),
                "地址不能包含用户名或密码。"
            );
        }
    }
}
