//! Page observations for a native browser child, scoped to its current request.

use crate::browser::allowed_target;
use block2::Block;
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel},
    sel,
};
use objc2_foundation::{
    NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions,
    NSObjectNSKeyValueObserverRegistration, NSPOSIXErrorDomain, NSString, NSURL,
    NSURLErrorCancelled, NSURLErrorCannotFindHost as CANNOT_FIND_HOST, NSURLErrorDomain,
    NSURLErrorHTTPTooManyRedirects as TOO_MANY_REDIRECTS,
    NSURLErrorNotConnectedToInternet as OFFLINE, NSURLRequest, NSUnderlyingErrorKey, ns_string,
};
use objc2_foundation::{
    NSHTTPURLResponse, NSURLAuthenticationChallenge, NSURLAuthenticationMethodServerTrust,
    NSURLCredential, NSURLSessionAuthChallengeDisposition,
};
use objc2_web_kit::{
    WKBackForwardListItem, WKDownload, WKFrameInfo, WKNavigation, WKNavigationAction,
    WKNavigationActionPolicy, WKNavigationDelegate, WKNavigationResponse,
    WKNavigationResponsePolicy, WKPermissionDecision, WKSecurityOrigin, WKUIDelegate, WKWebView,
    WKWebViewConfiguration, WKWindowFeatures,
};
use serde::Serialize;
use std::{cell::RefCell, ffi::c_void, ptr};

#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Page {
    address: Option<String>,
    title: Option<String>,
    loading: bool,
    can_back: bool,
    can_forward: bool,
    error: Option<String>,
}

#[derive(Default)]
struct State {
    navigation: u64,
    sequence: u64,
    last: Option<Page>,
    previous: Option<PreviousPage>,
    active: Option<Retained<WKNavigation>>,
    awaiting_start: bool,
    error: Option<String>,
    terminated: bool,
}

struct PreviousPage {
    item: Retained<WKBackForwardListItem>,
    address: Option<String>,
    reloading: bool,
}

pub(super) struct ObserverIvars {
    child: Retained<WKWebView>,
    delegate: Option<Retained<ProtocolObject<dyn WKNavigationDelegate>>>,
    ui_delegate: Option<Retained<ProtocolObject<dyn WKUIDelegate>>>,
    application_origin: Option<url::Origin>,
    popup: Box<dyn Fn(&url::Url)>,
    state: RefCell<State>,
    emit: Box<dyn Fn(u64, u64, Page)>,
    terminated: Box<dyn Fn()>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DshGpuiBrowserPageObserver"]
    #[ivars = ObserverIvars]
    pub(super) struct Observer;

    impl Observer {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            _path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            self.refresh();
        }

        #[unsafe(method_id(forwardingTargetForSelector:))]
        fn forwarding_target(&self, selector: Sel) -> Option<Retained<AnyObject>> {
            self.forward_to(selector).map(Message::retain)
        }
    }

    unsafe impl NSObjectProtocol for Observer {
        #[unsafe(method(respondsToSelector:))]
        fn responds_to_selector(&self, selector: Sel) -> bool {
            let implements: bool = unsafe { msg_send![super(self), respondsToSelector: selector] };
            implements || self.forward_to(selector).is_some()
        }
    }

    unsafe impl WKNavigationDelegate for Observer {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn navigation_policy(
            &self,
            view: &WKWebView,
            action: &WKNavigationAction,
            handler: &Block<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let request = unsafe { action.request() };
            let Some(address) = request_address(&request) else {
                handler.call((WKNavigationActionPolicy::Cancel,));
                return;
            };
            let subframe = unsafe { action.targetFrame().is_some_and(|frame| !frame.isMainFrame()) };
            // The desktop guest permits these documents in nested frames.
            if subframe && matches!(address.scheme(), "about" | "blob" | "data")
                && !unsafe { action.shouldPerformDownload() }
            {
                handler.call((WKNavigationActionPolicy::Allow,));
            } else if let Some(delegate) = self.delegate_for(sel!(webView:decidePolicyForNavigationAction:decisionHandler:)) {
                unsafe { delegate.webView_decidePolicyForNavigationAction_decisionHandler(view, action, handler) };
            } else {
                handler.call((WKNavigationActionPolicy::Cancel,));
            }
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn response_policy(
            &self,
            view: &WKWebView,
            response: &WKNavigationResponse,
            handler: &Block<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            let attachment = unsafe { response.response() }
                .downcast::<NSHTTPURLResponse>()
                .ok()
                .and_then(|http| http.valueForHTTPHeaderField(ns_string!("Content-Disposition")))
                .is_some_and(|value| is_attachment(&value.to_string()));
            if attachment || !unsafe { response.canShowMIMEType() } {
                handler.call((WKNavigationResponsePolicy::Cancel,));
            } else if let Some(delegate) = self.delegate_for(sel!(webView:decidePolicyForNavigationResponse:decisionHandler:)) {
                unsafe { delegate.webView_decidePolicyForNavigationResponse_decisionHandler(view, response, handler) };
            } else {
                handler.call((WKNavigationResponsePolicy::Allow,));
            }
        }

        #[unsafe(method(webView:navigationAction:didBecomeDownload:))]
        fn cancel_action_download(&self, _view: &WKWebView, _action: &WKNavigationAction, download: &WKDownload) {
            unsafe { download.cancel(None) };
        }

        #[unsafe(method(webView:navigationResponse:didBecomeDownload:))]
        fn cancel_response_download(&self, _view: &WKWebView, _response: &WKNavigationResponse, download: &WKDownload) {
            unsafe { download.cancel(None) };
        }

        #[unsafe(method(webView:didReceiveAuthenticationChallenge:completionHandler:))]
        fn authentication(
            &self,
            _view: &WKWebView,
            challenge: &NSURLAuthenticationChallenge,
            handler: &Block<dyn Fn(NSURLSessionAuthChallengeDisposition, *mut NSURLCredential)>,
        ) {
            let method = challenge.protectionSpace().authenticationMethod();
            let disposition = if method.isEqualToString(unsafe { NSURLAuthenticationMethodServerTrust }) {
                NSURLSessionAuthChallengeDisposition::PerformDefaultHandling
            } else {
                NSURLSessionAuthChallengeDisposition::RejectProtectionSpace
            };
            handler.call((disposition, ptr::null_mut()));
        }

        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn did_start(&self, view: &WKWebView, navigation: Option<&WKNavigation>) {
            if let Some(delegate) = self.delegate_for(sel!(webView:didStartProvisionalNavigation:)) {
                unsafe { delegate.webView_didStartProvisionalNavigation(view, navigation) };
            }
            {
                let mut state = self.ivars().state.borrow_mut();
                if state.awaiting_start && !same_navigation(state.active.as_deref(), navigation) {
                    return;
                }
                state.awaiting_start = false;
                state.active = navigation.map(Message::retain);
                state.error = None;
            }
            self.refresh();
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(&self, view: &WKWebView, navigation: Option<&WKNavigation>, error: &NSError) {
            if let Some(delegate) = self.delegate_for(sel!(webView:didFailProvisionalNavigation:withError:)) {
                unsafe { delegate.webView_didFailProvisionalNavigation_withError(view, navigation, error) };
            }
            self.fail(navigation, error);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, view: &WKWebView, navigation: Option<&WKNavigation>, error: &NSError) {
            if let Some(delegate) = self.delegate_for(sel!(webView:didFailNavigation:withError:)) {
                unsafe { delegate.webView_didFailNavigation_withError(view, navigation, error) };
            }
            self.fail(navigation, error);
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn process_terminated(&self, view: &WKWebView) {
            if std::mem::replace(&mut self.ivars().state.borrow_mut().terminated, true) {
                return;
            }
            view.setHidden(true);
            // The shell owns retries. Wry's default callback reloads immediately.
            (self.ivars().terminated)();
        }
    }

    unsafe impl WKUIDelegate for Observer {
        #[unsafe(method(webView:requestDeviceOrientationAndMotionPermissionForOrigin:initiatedByFrame:decisionHandler:))]
        fn orientation_permission(
            &self,
            _view: &WKWebView,
            _origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            handler: &Block<dyn Fn(WKPermissionDecision)>,
        ) {
            handler.call((WKPermissionDecision::Deny,));
        }

        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn popup(
            &self,
            _view: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            let active = !self.ivars().state.borrow().terminated;
            let request = unsafe { action.request() };
            // Only an address can be forwarded to another Sidebar tab.
            if active
                && !unsafe { action.shouldPerformDownload() }
                && request.HTTPMethod().is_some_and(|method| method.isEqualToString(ns_string!("GET")))
                && let Some(address) = request_address(&request)
                && allowed_target(&address, self.ivars().application_origin.as_ref())
            {
                (self.ivars().popup)(&address);
            }
            None
        }
    }
);

fn request_address(request: &NSURLRequest) -> Option<url::Url> {
    let url = request.URL()?.absoluteString()?;
    url::Url::parse(&url.to_string()).ok()
}

fn is_attachment(disposition: &str) -> bool {
    disposition
        .split(';')
        .next()
        .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("attachment"))
}

fn same_navigation(active: Option<&WKNavigation>, other: Option<&WKNavigation>) -> bool {
    active.map(ptr::from_ref) == other.map(ptr::from_ref)
}

fn failure_message(error: &NSError) -> Option<String> {
    let domain = error.domain();
    let url_error = domain.isEqualToString(unsafe { NSURLErrorDomain });
    let code = error.code();
    // WebKit also interrupts provisional loads when navigation policy denies or downloads a URL.
    if (url_error && code == NSURLErrorCancelled)
        || (code == 102 && domain.isEqualToString(ns_string!("WebKitErrorDomain")))
    {
        return None;
    }
    let connection_refused = error
        .userInfo()
        .objectForKey(unsafe { NSUnderlyingErrorKey })
        .and_then(|underlying| underlying.downcast::<NSError>().ok())
        .is_some_and(|underlying| {
            underlying
                .domain()
                .isEqualToString(unsafe { NSPOSIXErrorDomain })
                && underlying.code() == libc::ECONNREFUSED as isize
        });
    let detail = match (url_error, code) {
        (true, CANNOT_FIND_HOST) => Some((-105, "ERR_NAME_NOT_RESOLVED")),
        (true, OFFLINE) => Some((-106, "ERR_INTERNET_DISCONNECTED")),
        (true, TOO_MANY_REDIRECTS) => Some((-310, "ERR_TOO_MANY_REDIRECTS")),
        _ if connection_refused => Some((-102, "ERR_CONNECTION_REFUSED")),
        _ => None,
    };
    Some(detail.map_or_else(
        || "页面加载失败；请刷新重试或在系统浏览器中打开。".into(),
        |(code, description)| format!("页面加载失败 ({code}): {description}"),
    ))
}

fn observed_keys() -> [&'static NSString; 5] {
    [
        ns_string!("URL"),
        ns_string!("title"),
        ns_string!("loading"),
        ns_string!("canGoBack"),
        ns_string!("canGoForward"),
    ]
}

impl Observer {
    pub(super) fn attach(
        child: &WKWebView,
        application_origin: Option<url::Origin>,
        popup: impl Fn(&url::Url) + 'static,
        emit: impl Fn(u64, u64, Page) + 'static,
        terminated: impl Fn() + 'static,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let alloc = Self::alloc(mtm).set_ivars(ObserverIvars {
            child: child.retain(),
            // Wry retains its policy and download handling. Forward all unhandled callbacks.
            delegate: unsafe { child.navigationDelegate() },
            ui_delegate: unsafe { child.UIDelegate() },
            application_origin,
            popup: Box::new(popup),
            state: RefCell::default(),
            emit: Box::new(emit),
            terminated: Box::new(terminated),
        });
        // Retain the child for the entire lifetime of these KVO registrations.
        let observer: Retained<Self> = unsafe { msg_send![super(alloc), init] };
        unsafe {
            child.setNavigationDelegate(Some(ProtocolObject::from_ref(&*observer)));
            child.setUIDelegate(Some(ProtocolObject::from_ref(&*observer)));
        }
        for path in observed_keys() {
            unsafe {
                child.addObserver_forKeyPath_options_context(
                    &observer,
                    path,
                    NSKeyValueObservingOptions::New,
                    ptr::null_mut(),
                );
            }
        }
        observer
    }

    fn delegate_for(
        &self,
        selector: Sel,
    ) -> Option<&Retained<ProtocolObject<dyn WKNavigationDelegate>>> {
        self.ivars()
            .delegate
            .as_ref()
            .filter(|delegate| delegate.respondsToSelector(selector))
    }

    fn forward_to(&self, selector: Sel) -> Option<&AnyObject> {
        self.delegate_for(selector)
            .map(|delegate| (**delegate).as_ref())
            .or_else(|| {
                self.ivars()
                    .ui_delegate
                    .as_deref()
                    .filter(|delegate| delegate.respondsToSelector(selector))
                    .map(AsRef::as_ref)
            })
    }

    fn fail(&self, navigation: Option<&WKNavigation>, error: &NSError) {
        {
            let mut state = self.ivars().state.borrow_mut();
            if !same_navigation(state.active.as_deref(), navigation) {
                return;
            }
            state.awaiting_start = false;
            state.error = failure_message(error);
        }
        self.refresh();
    }

    pub(super) fn navigate(&self, navigation: u64, address: &url::Url) {
        if self.ivars().state.borrow().terminated {
            return;
        }
        let child = &self.ivars().child;
        let (current, mut page) = self.read_page();
        let reload = page.address.as_deref() == Some(address.as_str());
        {
            let mut state = self.ivars().state.borrow_mut();
            state.navigation = navigation;
            state.last = None;
            state.active = None;
            state.awaiting_start = false;
            state.error = None;
            state.previous = current.map(|item| PreviousPage {
                item,
                address: page.address.take(),
                reloading: reload,
            });
        }
        page.address = None;
        page.title = None;
        page.loading = true;
        self.publish(page, None);
        // The caller has already validated a canonical HTTP(S) URL.
        let started = unsafe {
            if reload {
                child.reload()
            } else {
                NSURL::URLWithString(&NSString::from_str(address.as_str()))
                    .and_then(|url| child.loadRequest(&NSURLRequest::requestWithURL(&url)))
            }
        };
        if let Some(navigation) = started.as_ref() {
            let mut state = self.ivars().state.borrow_mut();
            // A delegate start can arrive synchronously from the native request.
            state.awaiting_start = !same_navigation(state.active.as_deref(), Some(navigation));
            state.active = Some(navigation.clone());
        }
        // WebKit can complete a same-document request synchronously.
        if started.is_none() {
            self.refresh();
        }
    }

    fn refresh(&self) {
        if self.ivars().state.borrow().terminated {
            return;
        }
        let (current, page) = self.read_page();
        self.publish(page, current.as_deref());
    }

    fn read_page(&self) -> (Option<Retained<WKBackForwardListItem>>, Page) {
        let child = &self.ivars().child;
        // WKWebView.URL can contain a provisional redirect. The current history
        // item describes the displayed document, including same-document history.
        let current = unsafe { child.backForwardList().currentItem() };
        let address = current
            .as_ref()
            .and_then(|item| unsafe { item.URL().absoluteString() })
            .and_then(|value| url::Url::parse(&value.to_string()).ok())
            .filter(|address| allowed_target(address, None));
        let title = address.as_ref().map(|address| {
            current
                .as_ref()
                .and_then(|item| unsafe { item.title() })
                .map(|title| title.to_string())
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| address.host_str().unwrap_or_default().to_owned())
        });
        // All WebKit reads and observation callbacks run on the AppKit thread.
        (current, unsafe {
            Page {
                address: address.map(|address| address.into()),
                title,
                loading: child.isLoading(),
                can_back: child.canGoBack(),
                can_forward: child.canGoForward(),
                error: None,
            }
        })
    }

    fn publish(&self, mut page: Page, current: Option<&WKBackForwardListItem>) {
        let (navigation, sequence) = {
            let mut state = self.ivars().state.borrow_mut();
            page.error.clone_from(&state.error);
            if page.error.is_some() {
                page.loading = false;
            }
            if let Some(previous) = &state.previous
                && page.address.is_some()
            {
                let still_previous = current.is_some_and(|item| ptr::eq(item, &*previous.item))
                    && page.address == previous.address;
                if still_previous && !(previous.reloading && !page.loading) {
                    // The previous document can still change while the new request loads.
                    page.address = None;
                    page.title = None;
                } else {
                    state.previous = None;
                    if !page.loading {
                        state.awaiting_start = false;
                    }
                }
            }
            if state.navigation == 0 || state.last.as_ref() == Some(&page) {
                return;
            }
            state.last = Some(page.clone());
            state.sequence += 1;
            (state.navigation, state.sequence)
        };
        (self.ivars().emit)(navigation, sequence, page);
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        // The child references this delegate weakly. Restore Wry before releasing the proxy.
        unsafe {
            self.ivars()
                .child
                .setNavigationDelegate(self.ivars().delegate.as_deref());
            self.ivars()
                .child
                .setUIDelegate(self.ivars().ui_delegate.as_deref());
        };
        for path in observed_keys() {
            // Drop runs on the AppKit thread, with the observed child still retained.
            unsafe { self.ivars().child.removeObserver_forKeyPath(self, path) };
        }
    }
}
