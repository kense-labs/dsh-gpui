//! Typed Rust-to-JavaScript bridge for Tauri IPC in the WASM host.

use serde_json::Value;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
// Each save contains the entire preference document. Wait for the previous
// write so a slower save cannot overwrite a more recent choice.
let workspacePreferencesWrite = Promise.resolve();

export function dshInvoke(command, args) {
    const api = window.__TAURI__;
    if (!api?.core?.invoke) {
        return Promise.reject(new Error('Tauri IPC 不可用，请通过 dsh-gpui 桌面客户端打开。'));
    }
    if (command === 'save_workspace_preferences') {
        const write = workspacePreferencesWrite.then(() => api.core.invoke(command, args));
        workspacePreferencesWrite = write.catch(() => {});
        return write;
    }
    return api.core.invoke(command, args);
}

export function dshListen(callback) {
    const api = window.__TAURI__;
    if (!api?.event?.listen) {
        return Promise.reject(new Error('Tauri 事件通道不可用。'));
    }
    return api.event.listen('acp-event', event => callback(JSON.stringify(event.payload)));
}

let keyboardFocusVisible = false;

export function dshKeyboardFocusVisible() {
    return keyboardFocusVisible;
}

export function dshListenKeys(callback) {
    window.addEventListener('pointerdown', () => { keyboardFocusVisible = false; }, true);
    const dispatch = (event, phase) => {
        const composing = event.isComposing || event.key === 'Dead' || event.getModifierState('AltGraph');
        if (phase === 0 && !composing && (['Tab', 'Home', 'End', 'PageUp', 'PageDown'].includes(event.key)
            || event.key.startsWith('Arrow'))) keyboardFocusVisible = true;
        const combo = (event.metaKey ? 'meta+' : '') + (event.ctrlKey ? 'ctrl+' : '')
            + (event.altKey ? 'alt+' : '') + (event.shiftKey ? 'shift+' : '') + event.key;
        const modifiers = Number(event.ctrlKey) | (Number(event.altKey) << 1)
            | (Number(event.shiftKey) << 2) | (Number(event.metaKey) << 3);
        if (callback(combo, event.code, modifiers, phase, event.repeat, composing)) {
            event.preventDefault();
            event.stopImmediatePropagation();
        }
    };
    window.addEventListener('keydown', event => dispatch(event, 0), true);
    window.addEventListener('keyup', event => dispatch(event, 1), true);
    const reset = () => callback('', '', 0, 2, false, false);
    window.addEventListener('blur', reset);
    window.addEventListener('compositionstart', reset, true);
    window.addEventListener('pointerdown', reset, true);

}

export function dshNow() {
    return Date.now();
}

// Follow-the-system appearance: read the platform colour scheme. The caller
// polls this while the preference is "system"; matchMedia is cheap.
export function dshSystemPrefersDark() {
    return window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? false;
}

export function dshListenDockCancel(callback) {
    window.addEventListener('pointercancel', callback, true);
    window.addEventListener('lostpointercapture', callback, true);
    window.addEventListener('blur', callback);
}

// JavaScript-only helpers (terminal emulation, document rendering) report
// failures through this event so the GPUI layer can surface a toast instead of
// leaving them in the browser console only.
export function dshListenRuntimeError(callback) {
    window.addEventListener('kwork-runtime-error', event => {
        callback(String(event.detail ?? '操作失败'));
    });
}

export function dshReadSidebar(session) {
    try { return localStorage.getItem(`dsh.gpui.sidebar-right.v1.${session}`) ?? ''; }
    catch { return ''; }
}

export function dshRandomId() {
    return Array.from(crypto.getRandomValues(new Uint8Array(16)),
        byte => byte.toString(16).padStart(2, '0')).join('');
}

export function dshPreferredTerminalShell() {
    try { return localStorage.getItem('dsh.terminal.shell') ?? ''; }
    catch { return ''; }
}

export function dshRememberTerminalShell(path) {
    try { localStorage.setItem('dsh.terminal.shell', path); }
    catch { /* Storage is optional for this launch. */ }
}

export function dshLimitTerminalTitle(limit, reconcile) {
    const controller = new AbortController();
    const options = { capture: true, signal: controller.signal };
    let input;
    const attach = target => {
        if (!(target instanceof HTMLTextAreaElement) || input === target) return;
        input = target;
        const previous = target.getAttribute('maxlength');
        target.maxLength = limit;
        controller.signal.addEventListener('abort', () => {
            if (previous === null) target.removeAttribute('maxlength');
            else target.setAttribute('maxlength', previous);
        }, { once: true });
    };
    attach(document.activeElement);
    document.addEventListener('focusin', event => attach(event.target), options);
    document.addEventListener('paste', event => {
        if (event.target !== input || input.readOnly) return;
        event.preventDefault();
        event.stopImmediatePropagation();
        const text = event.clipboardData?.getData('text/plain')?.replace(/[\r\n]/g, '') ?? '';
        const start = input.selectionStart;
        const end = input.selectionEnd;
        const available = Math.max(0, limit - input.value.length + end - start);
        let inserted = text.slice(0, available);
        // A UTF-16 limit must not split a supplementary character.
        if (inserted.length < text.length) inserted = inserted.replace(/[\uD800-\uDBFF]$/, '');
        if (!inserted) return;
        input.setRangeText(inserted, start, end, 'end');
        input.dispatchEvent(new InputEvent('input', {
            bubbles: true, inputType: 'insertFromPaste', data: inserted,
        }));
    }, options);
    document.addEventListener('input', event => {
        if (event.target !== input || event.isComposing) return;
        const { value, selectionStart, selectionEnd } = input;
        // The mirror's value diff can drop shared prefixes inside the
        // replaced selection. The bubble phase follows GPUI's input listener.
        reconcile(selectionStart, selectionEnd, value);
    }, { signal: controller.signal });
    return () => controller.abort();
}

export function dshWriteSidebar(session, state) {
    try { localStorage.setItem(`dsh.gpui.sidebar-right.v1.${session}`, state); }
    catch (error) { console.warn('Unable to persist the sidebar', error); }
}

export function dshMonotonicNow() {
    return performance.now();
}

export function dshIsWindows() { return navigator.platform.startsWith('Win'); }

export function dshIsMac() {
    return navigator.platform.startsWith('Mac');
}

export function dshHasMacWindowMaterial() {
    return document.documentElement.hasAttribute('data-native-mac-material');
}

export function dshPrefersReducedMotion() {
    return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

export function dshWatchReducedMotion(update) {
    window.matchMedia('(prefers-reduced-motion: reduce)').addEventListener('change', event => update(event.matches));
}

export function dshReducesTransparency() {
    return window.matchMedia('(prefers-reduced-transparency: reduce)').matches;
}

export function dshWatchTransparency(refresh) {
    window.matchMedia('(prefers-reduced-transparency: reduce)').addEventListener('change', refresh);
}

export function dshHasPointerHover() {
    return window.matchMedia('(hover: hover)').matches;
}

export function dshHasAnyPointerHover() {
    return window.matchMedia('(any-hover: hover)').matches;
}

export function dshBrowserAddress(value) {
    const input = value.trim();
    if (!input) throw '请输入地址。';
    if (input.length > 16 * 1024) throw '这个地址无效或过长。';

    // A host followed by a numeric port has no explicit scheme.
    const explicitScheme = /^[A-Za-z][A-Za-z\d+.-]*:(?!\d+(?:[/?#]|$))/u.test(input);
    let url;
    try { url = new URL(explicitScheme ? input : 'https://' + input); }
    catch { throw '这个地址无效或过长。'; }
    if (url.username || url.password) throw '地址不能包含用户名或密码。';
    if (url.protocol !== 'http:' && url.protocol !== 'https:') {
        throw '只支持 HTTP 和 HTTPS 地址；本地文件请使用文档预览。';
    }
    if (url.origin === window.location.origin) {
        throw '不能在嵌入浏览器中打开 DSH 应用自身。';
    }
    return { address: url.href, title: url.hostname };
}

export function dshListenBrowserSelection(advance) {
    document.addEventListener('beforeinput', event => {
        const input = event.target;
        if (!(input instanceof HTMLTextAreaElement) || input.readOnly
            || event.isComposing || event.inputType !== 'insertText' || !event.data) return;
        const start = input.selectionStart;
        const end = input.selectionEnd;
        if (start === end) return;
        let prefix = 0;
        for (const point of event.data) {
            if (start + prefix + point.length > end || !input.value.startsWith(point, start + prefix)) break;
            prefix += point.length;
        }
        // The GPUI mirror diffs away this common prefix, then applies the
        // shortened insertion to the editor's current selection.
        if (prefix) advance(event.data.slice(0, prefix));
    }, true);
}

export async function dshCopyText(text) {
    if (navigator.clipboard?.writeText) {
        try {
            await navigator.clipboard.writeText(text);
            return true;
        } catch {
            return false;
        }
    } else {
        const area = document.createElement('textarea');
        area.value = text;
        area.readOnly = true;
        area.style.position = 'fixed';
        area.style.left = '-9999px';
        document.body.appendChild(area);
        area.select();
        try { return document.execCommand('copy'); }
        catch { return false; }
        finally { area.remove(); }
    }
}

// Start the deferred clipboard write in the menu gesture, before the host
// reads the selected Session. WebKit can then preserve the user activation.
export async function dshCopySessionMarkdown(sessionId, cwd) {
    const prepared = dshInvoke('session_markdown', { sessionId, cwd }).then(markdown => {
        if (typeof markdown !== 'string') throw new TypeError('Invalid Session Markdown');
        return markdown;
    });
    if (typeof globalThis.ClipboardItem === 'function' && navigator.clipboard?.write) {
        const data = prepared.then(markdown => new Blob([markdown], { type: 'text/plain' }));
        let write;
        try {
            write = navigator.clipboard.write([new ClipboardItem({ 'text/plain': data })]);
        } catch (error) {
            write = Promise.reject(error);
        }
        const [source, blob, clipboard] = await Promise.allSettled([prepared, data, write]);
        if (source.status === 'rejected' || blob.status === 'rejected') return 'preparationFailed';
        return clipboard.status === 'fulfilled' ? 'success' : 'clipboardFailed';
    }
    let markdown;
    try {
        markdown = await prepared;
    } catch {
        return 'preparationFailed';
    }
    return await dshCopyText(markdown) ? 'success' : 'clipboardFailed';
}

export function dshOpenUrl(url) {
    if (typeof url !== 'string') return;
    const trimmed = url.trim();
    if (!/^https?:\/\//i.test(trimmed)) return;
    try {
        window.open(trimmed, '_blank', 'noopener,noreferrer');
    } catch (error) {}
}

async function fileToDraft(file) {
    const item = {
        id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
        name: file.name || 'pasted-image.png',
        size: file.size || 0,
        mime: file.type || '',
        kind: (file.type || '').startsWith('image/') ? 'image' : 'file',
        preview: null,
    };
    if (item.kind === 'image' && file.size <= 4 * 1024 * 1024) {
        try {
            item.preview = await new Promise((ok, err) => {
                const reader = new FileReader();
                reader.onload = () => ok(reader.result);
                reader.onerror = err;
                reader.readAsDataURL(file);
            });
        } catch (error) {}
    }
    return item;
}

export function dshPickFiles(accept) {
    return new Promise(resolve => {
        const input = document.createElement('input');
        input.type = 'file';
        input.multiple = true;
        if (accept) input.accept = accept;
        input.style.display = 'none';
        document.body.appendChild(input);
        input.addEventListener('change', async () => {
            const files = Array.from(input.files || []);
            const items = [];
            for (const file of files) {
                items.push(await fileToDraft(file));
            }
            input.remove();
            resolve(JSON.stringify(items));
        }, { once: true });
        input.addEventListener('cancel', () => {
            input.remove();
            resolve('[]');
        }, { once: true });
        input.click();
    });
}

export function dshListenFiles(callback) {
    let dragDepth = 0;
    const emit = (kind, payload) => {
        try { callback(JSON.stringify({ kind, ...payload })); } catch (error) {}
    };
    window.addEventListener('dragenter', event => {
        if (![...event.dataTransfer.types].includes('Files')) return;
        event.preventDefault();
        dragDepth += 1;
        emit('drag', { active: true });
    });
    window.addEventListener('dragover', event => {
        if (![...event.dataTransfer.types].includes('Files')) return;
        event.preventDefault();
        emit('drag', { active: true });
    });
    window.addEventListener('dragleave', event => {
        if (![...event.dataTransfer.types].includes('Files')) return;
        dragDepth = Math.max(0, dragDepth - 1);
        if (dragDepth === 0) emit('drag', { active: false });
    });
    window.addEventListener('drop', async event => {
        if (![...event.dataTransfer.types].includes('Files')) return;
        event.preventDefault();
        dragDepth = 0;
        emit('drag', { active: false });
        const files = Array.from(event.dataTransfer.files || []);
        if (!files.length) return;
        const items = [];
        for (const file of files) items.push(await fileToDraft(file));
        emit('files', { items });
    });
    window.addEventListener('paste', async event => {
        const items = Array.from(event.clipboardData?.items || []);
        const files = [];
        for (const item of items) {
            if (item.kind === 'file') {
                const file = item.getAsFile();
                if (file) files.push(file);
            }
        }
        if (!files.length) return;
        event.preventDefault();
        const drafts = [];
        for (const file of files) drafts.push(await fileToDraft(file));
        emit('files', { items: drafts });
    });
}

export function dshTimeLabel(ms) {
    const date = new Date(ms);
    const now = new Date();
    const pad = n => String(n).padStart(2, '0');
    const clock = pad(date.getHours()) + ':' + pad(date.getMinutes());
    const sameDay = date.getFullYear() === now.getFullYear()
        && date.getMonth() === now.getMonth()
        && date.getDate() === now.getDate();
    if (sameDay) return clock;
    if (date.getFullYear() === now.getFullYear()) {
        return `${date.getMonth() + 1}月${date.getDate()}日 ${clock}`;
    }
    return `${date.getFullYear()}年${date.getMonth() + 1}月${date.getDate()}日 ${clock}`;
}
"#)]
extern "C" {
    #[wasm_bindgen(js_name = dshInvoke)]
    fn invoke_js(command: &str, args: JsValue) -> js_sys::Promise;

    #[wasm_bindgen(js_name = dshListen)]
    fn listen_js(callback: &js_sys::Function) -> js_sys::Promise;

    #[wasm_bindgen(js_name = dshListenKeys)]
    fn listen_keys_js(callback: &js_sys::Function);

    #[wasm_bindgen(js_name = dshKeyboardFocusVisible)]
    pub(crate) fn keyboard_focus_visible() -> bool;

    #[wasm_bindgen(js_name = dshListenFiles)]
    fn listen_files_js(callback: &js_sys::Function);

    #[wasm_bindgen(js_name = dshListenDockCancel)]
    pub(crate) fn listen_dock_cancel(callback: &js_sys::Function);

    #[wasm_bindgen(js_name = dshListenRuntimeError)]
    pub(crate) fn listen_runtime_error(callback: &js_sys::Function);

    #[wasm_bindgen(js_name = dshNow)]
    fn now_js() -> f64;

    #[wasm_bindgen(js_name = dshSystemPrefersDark)]
    fn system_prefers_dark_js() -> bool;

    #[wasm_bindgen(js_name = dshReadSidebar)]
    pub(crate) fn read_sidebar(session: &str) -> String;

    #[wasm_bindgen(js_name = dshRandomId)]
    pub(crate) fn random_id() -> String;

    #[wasm_bindgen(js_name = dshPreferredTerminalShell)]
    pub(crate) fn preferred_terminal_shell() -> String;

    #[wasm_bindgen(js_name = dshRememberTerminalShell)]
    pub(crate) fn remember_terminal_shell(path: &str);

    #[wasm_bindgen(js_name = dshLimitTerminalTitle)]
    pub(crate) fn limit_terminal_title(
        limit: u32,
        reconcile: &js_sys::Function,
    ) -> js_sys::Function;

    #[wasm_bindgen(js_name = dshWriteSidebar)]
    pub(crate) fn write_sidebar(session: &str, state: &str);

    #[wasm_bindgen(js_name = dshMonotonicNow)]
    pub(crate) fn monotonic_ms() -> f64;

    #[wasm_bindgen(js_name = dshIsWindows)]
    pub(crate) fn is_windows() -> bool;

    #[wasm_bindgen(js_name = dshIsMac)]
    pub(crate) fn is_mac() -> bool;

    #[wasm_bindgen(js_name = dshHasMacWindowMaterial)]
    pub(crate) fn has_mac_window_material() -> bool;

    #[wasm_bindgen(js_name = dshPrefersReducedMotion)]
    pub(crate) fn prefers_reduced_motion() -> bool;

    #[wasm_bindgen(js_name = dshWatchReducedMotion)]
    pub(crate) fn watch_reduced_motion(update: &js_sys::Function);

    #[wasm_bindgen(js_name = dshReducesTransparency)]
    pub(crate) fn reduces_transparency() -> bool;

    #[wasm_bindgen(js_name = dshWatchTransparency)]
    pub(crate) fn watch_transparency(refresh: &js_sys::Function);

    #[wasm_bindgen(js_name = dshHasPointerHover)]
    pub(crate) fn has_pointer_hover() -> bool;

    #[wasm_bindgen(js_name = dshHasAnyPointerHover)]
    pub(crate) fn has_any_pointer_hover() -> bool;

    #[wasm_bindgen(catch, js_name = dshBrowserAddress)]
    pub(crate) fn browser_address(input: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(js_name = dshListenBrowserSelection)]
    pub(crate) fn listen_browser_selection(advance: &js_sys::Function);

    #[wasm_bindgen(js_name = dshCopySessionMarkdown)]
    pub(crate) fn copy_session_markdown(session_id: &str, cwd: &str) -> js_sys::Promise;

    #[wasm_bindgen(js_name = dshCopyText)]
    pub(crate) fn copy_text_checked(text: &str) -> js_sys::Promise;

    #[wasm_bindgen(js_name = dshOpenUrl)]
    fn open_url_js(url: &str);

    #[wasm_bindgen(js_name = dshPickFiles)]
    fn pick_files_js(accept: &str) -> js_sys::Promise;

    #[wasm_bindgen(js_name = dshTimeLabel)]
    fn time_label_js(ms: f64) -> String;
}

/// Whether the platform currently prefers a dark appearance.
pub(crate) fn system_prefers_dark() -> bool {
    system_prefers_dark_js()
}

/// Wall-clock milliseconds since the Unix epoch (local browser clock).
pub fn now_ms() -> f64 {
    now_js()
}

/// Copy text to the system clipboard, ignoring failures.
pub fn copy_text(text: &str) {
    let _ = copy_text_checked(text);
}

/// Open an http(s) URL in a new browser tab/window.
pub fn open_url(url: &str) {
    open_url_js(url);
}

/// Open a native-looking file picker in the webview and return selected drafts as JSON.
pub async fn pick_files(accept: &str) -> Result<String, String> {
    let result = wasm_bindgen_futures::JsFuture::from(pick_files_js(accept)).await;
    match result {
        Ok(value) => value
            .as_string()
            .ok_or_else(|| "文件选择结果无效".to_string()),
        Err(error) => Err(js_error(error)),
    }
}

/// Format epoch milliseconds as a local "HH:MM" label.
pub fn time_label(ms: f64) -> String {
    time_label_js(ms)
}

/// Invoke one Tauri command and normalize its result to JSON.
pub async fn invoke(command: &str, args: Value) -> Result<Value, String> {
    let args = serde_wasm_bindgen::to_value(&args).map_err(|error| error.to_string())?;
    let result = wasm_bindgen_futures::JsFuture::from(invoke_js(command, args)).await;
    match result {
        Ok(value) => serde_wasm_bindgen::from_value(value).map_err(|error| error.to_string()),
        Err(error) => Err(js_error(error)),
    }
}

pub async fn terminal_font() -> Option<Vec<u8>> {
    font_data("terminal_font").await.map(|bytes| bytes.to_vec())
}

pub(crate) async fn font_data(command: &str) -> Option<js_sys::Uint8Array> {
    let args = js_sys::Object::new().into();
    let data = wasm_bindgen_futures::JsFuture::from(invoke_js(command, args))
        .await
        .ok()?;
    let bytes = js_sys::Uint8Array::new(&data);
    (bytes.byte_length() > 0).then_some(bytes)
}

/// Subscribe to backend ACP events. The returned Promise resolves to an unlisten function.
pub fn listen(callback: &js_sys::Function) -> js_sys::Promise {
    listen_js(callback)
}

/// Intercept composer navigation before the editor consumes those keys.
pub fn listen_keys(callback: &js_sys::Function) {
    listen_keys_js(callback);
}

/// Listen for document-level file drag/drop and paste of files/images.
#[cfg(target_family = "wasm")]
#[wasm_bindgen(inline_js = r#"
export function dshReportPanic(message) {
  try {
    if (window.__TAURI_INTERNALS__) window.__TAURI_INTERNALS__.invoke('note_panic', { message });
  } catch (error) {}
}
"#)]
extern "C" {
    fn dshReportPanic(message: &str);
}

/// Send one wasm panic message to the native log before the instance aborts.
pub(crate) fn report_panic(message: &str) {
    dshReportPanic(message);
}

pub fn listen_files(callback: &js_sys::Function) {
    listen_files_js(callback);
}

fn js_error(value: JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            value
                .dyn_ref::<js_sys::Error>()
                .map(|error| String::from(error.message()))
        })
        .or_else(|| {
            js_sys::JSON::stringify(&value)
                .ok()
                .and_then(|text| text.as_string())
        })
        .unwrap_or_else(|| format!("{value:?}"))
}
