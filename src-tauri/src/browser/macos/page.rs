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

    // Stands in for the `NSObject` protocol conformance WebKit requires of any
    // object it stores as an observer or delegate.
    unsafe impl NSObjectProtocol for Observer {
        #[unsafe(method(respondsToSelector:))]
        fn responds_to_selector(&self, selector: Sel) -> bool {
            // SAFETY: `super(self)` is the `NSObject` superclass instance of the
            // same live receiver, and `respondsToSelector:` is implemented by
            // `NSObject` itself, so the selector exists on that receiver; `selector`
            // is a valid `Sel` and the method returns a `BOOL` that maps to `bool`.
            let implements: bool = unsafe { msg_send![super(self), respondsToSelector: selector] };
            implements || self.forward_to(selector).is_some()
        }
    }

    // Declares that this observer is the child web view's navigation delegate; the
    // callbacks below answer the ones this module handles and forward every other
    // one to the delegate Wry had installed.
    unsafe impl WKNavigationDelegate for Observer {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn navigation_policy(
            &self,
            view: &WKWebView,
            action: &WKNavigationAction,
            handler: &Block<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // SAFETY: `action` is the live `WKNavigationAction` WebKit passes to
            // this delegate callback, so the receiver is valid and the class
            // implements `request`; the returned `NSURLRequest` is owned by the
            // `Retained` wrapper, which keeps it alive while `request` is used.
            let request = unsafe { action.request() };
            let Some(address) = request_address(&request) else {
                handler.call((WKNavigationActionPolicy::Cancel,));
                return;
            };
            // SAFETY: `action` is the live `WKNavigationAction` WebKit passes to
            // this delegate callback, so the receiver is valid and the class
            // implements `targetFrame`; the result is either null (a new-window
            // navigation, handled by `is_some_and`) or an owned `WKFrameInfo`.
            let target_frame = unsafe { action.targetFrame() };
            // SAFETY: `target_frame` is the frame returned by the call just above
            // and this binding keeps it alive, so the closure runs `isMainFrame` on
            // a live `WKFrameInfo`, where it only reads a `BOOL` property.
            let subframe = target_frame.is_some_and(|frame| unsafe { !frame.isMainFrame() });
            // The desktop guest permits these documents in nested frames.
            if subframe && matches!(address.scheme(), "about" | "blob" | "data")
                // SAFETY: `action` is the live `WKNavigationAction` from this
                // callback, and `shouldPerformDownload` only reads the `BOOL` that
                // this very navigation carries.
                && !unsafe { action.shouldPerformDownload() }
            {
                handler.call((WKNavigationActionPolicy::Allow,));
            } else if let Some(delegate) = self.delegate_for(sel!(webView:decidePolicyForNavigationAction:decisionHandler:)) {
                // SAFETY: `delegate_for` returned this delegate only after it
                // answered `respondsToSelector:` for this exact selector, so the
                // method exists. `ProtocolObject` guarantees the receiver is an
                // object of the declared protocol type, and `view`, `action` and
                // `handler` are the callback arguments WebKit supplied for exactly
                // this method, so they satisfy its preconditions for the call.
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
            // SAFETY: `response` is the live `WKNavigationResponse` WebKit passes to
            // this delegate callback, so the receiver is valid and `response()` —
            // which every `WKNavigationResponse` implements — returns the owned
            // `NSURLResponse` held by the `Retained` wrapper. `downcast` re-checks
            // the concrete class before the value is used as an HTTP response.
            let attachment = unsafe { response.response() }
                .downcast::<NSHTTPURLResponse>()
                .ok()
                .and_then(|http| http.valueForHTTPHeaderField(ns_string!("Content-Disposition")))
                .is_some_and(|value| is_attachment(&value.to_string()));
            // SAFETY: `response` is the same live callback argument, so
            // `canShowMIMEType` is implemented; it only reads the `BOOL` WebKit
            // already computed for this response.
            if attachment || !unsafe { response.canShowMIMEType() } {
                handler.call((WKNavigationResponsePolicy::Cancel,));
            } else if let Some(delegate) = self.delegate_for(sel!(webView:decidePolicyForNavigationResponse:decisionHandler:)) {
                // SAFETY: `delegate_for` returned this delegate only after it
                // answered `respondsToSelector:` for this exact selector, so the
                // method exists. `ProtocolObject` guarantees the receiver is an
                // object of the declared protocol type, and `view`, `response` and
                // `handler` are the callback arguments WebKit supplied for exactly
                // this method, so they satisfy its preconditions for the call.
                unsafe { delegate.webView_decidePolicyForNavigationResponse_decisionHandler(view, response, handler) };
            } else {
                handler.call((WKNavigationResponsePolicy::Allow,));
            }
        }

        #[unsafe(method(webView:navigationAction:didBecomeDownload:))]
        fn cancel_action_download(&self, _view: &WKWebView, _action: &WKNavigationAction, download: &WKDownload) {
            // SAFETY: `download` is the `WKDownload` WebKit created for this
            // navigation and passed to the callback, so it is live and the class
            // implements `cancel:`; `None` is the null completion handler that
            // parameter accepts when the caller wants no notification.
            unsafe { download.cancel(None) };
        }

        #[unsafe(method(webView:navigationResponse:didBecomeDownload:))]
        fn cancel_response_download(&self, _view: &WKWebView, _response: &WKNavigationResponse, download: &WKDownload) {
            // SAFETY: `download` is the `WKDownload` WebKit created for this
            // response and passed to the callback, so it is live and the class
            // implements `cancel:`; `None` is the null completion handler that
            // parameter accepts when the caller wants no notification.
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
            // SAFETY: `NSURLAuthenticationMethodServerTrust` is a Foundation static
            // that the framework initializes and keeps alive for the whole process,
            // so the borrow points at live constant storage; only its address is
            // read, for the comparison below.
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
                // SAFETY: `delegate_for` returned this delegate only after it
                // answered `respondsToSelector:` for this exact selector, so the
                // method exists. `ProtocolObject` guarantees the receiver is an
                // object of the declared protocol type, and `view` together with
                // the `navigation` WebKit handed to this callback are the arguments
                // the method expects, including the null case for a nil navigation.
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
                // SAFETY: `delegate_for` returned this delegate only after it
                // answered `respondsToSelector:` for this exact selector, so the
                // method exists. `ProtocolObject` guarantees the receiver is an
                // object of the declared protocol type, and `view`, the `navigation`
                // and the `error` are the callback arguments WebKit supplied for
                // exactly this method, so they satisfy its preconditions.
                unsafe { delegate.webView_didFailProvisionalNavigation_withError(view, navigation, error) };
            }
            self.fail(navigation, error);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, view: &WKWebView, navigation: Option<&WKNavigation>, error: &NSError) {
            if let Some(delegate) = self.delegate_for(sel!(webView:didFailNavigation:withError:)) {
                // SAFETY: `delegate_for` returned this delegate only after it
                // answered `respondsToSelector:` for this exact selector, so the
                // method exists. `ProtocolObject` guarantees the receiver is an
                // object of the declared protocol type, and `view`, the `navigation`
                // and the `error` are the callback arguments WebKit supplied for
                // exactly this method, so they satisfy its preconditions.
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

    // Declares that this observer is the child web view's UI delegate; it denies
    // device-orientation permission prompts and re-routes popup requests instead
    // of letting WebKit open a window of its own.
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
            // SAFETY: `action` is the live `WKNavigationAction` WebKit passes to
            // this delegate callback, so the receiver is valid and the class
            // implements `request`; the returned `NSURLRequest` is owned by the
            // `Retained` wrapper, which keeps it alive while `request` is used.
            let request = unsafe { action.request() };
            // Only an address can be forwarded to another Sidebar tab.
            if active
                // SAFETY: `action` is the same live callback argument, and
                // `shouldPerformDownload` only reads the `BOOL` that this very
                // navigation carries.
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
    // SAFETY: `NSURLErrorDomain` is a Foundation static that the framework
    // initializes and keeps alive for the whole process, so the borrow points at
    // live constant storage; only its address is read, for this comparison.
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
        // SAFETY: `NSUnderlyingErrorKey` is a Foundation static that the framework
        // initializes and keeps alive for the whole process, and it is the key
        // `NSError` itself uses for the underlying error, so it is a valid key for
        // this error's `userInfo` dictionary; the returned value is owned by the
        // `Retained` wrapper, and a missing entry yields `None`.
        .objectForKey(unsafe { NSUnderlyingErrorKey })
        .and_then(|underlying| underlying.downcast::<NSError>().ok())
        .is_some_and(|underlying| {
            underlying
                .domain()
                // SAFETY: `NSPOSIXErrorDomain` is a Foundation static that the
                // framework initializes and keeps alive for the whole process, so
                // the borrow points at live constant storage; only its address is
                // read, for this comparison on the underlying error.
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
            // SAFETY: `child` is the live `WKWebView` this observer is being attached
            // to and the caller's borrow keeps it valid for the call; the class
            // implements `navigationDelegate`, and the getter's result is owned by
            // the returned `Option<Retained<...>>`. It is read before the delegate
            // is replaced just below, so it is still Wry's delegate.
            delegate: unsafe { child.navigationDelegate() },
            // SAFETY: same live `WKWebView` as above, so `UIDelegate` is
            // implemented; its result is likewise owned by the returned
            // `Option<Retained<...>>`, and it is read before the UI delegate is
            // replaced below, so it is still Wry's delegate.
            ui_delegate: unsafe { child.UIDelegate() },
            application_origin,
            popup: Box::new(popup),
            state: RefCell::default(),
            emit: Box::new(emit),
            terminated: Box::new(terminated),
        });
        // Retain the child for the entire lifetime of these KVO registrations.
        // SAFETY: `alloc` is the `Allocated<Self>` from `Self::alloc` and had its
        // ivars initialized by `set_ivars`, which is precisely the
        // allocated-but-uninitialized state `NSObject`'s `init` expects; the sent
        // receiver is `Observer`'s `NSObject` superclass, and the single
        // `Retained<Self>` returned takes ownership of the new instance.
        let observer: Retained<Self> = unsafe { msg_send![super(alloc), init] };
        // SAFETY: `ProtocolObject::from_ref` only re-tags this same live observer
        // pointer with the protocol type `Observer` implements, which is what
        // `setNavigationDelegate:` expects; `observer` outlives the call and stays
        // retained for as long as the web view holds the weakly-referenced delegate.
        unsafe { child.setNavigationDelegate(Some(ProtocolObject::from_ref(&*observer))) };
        // SAFETY: as for `setNavigationDelegate:` above — `Observer` also conforms
        // to `WKUIDelegate`, and `observer` is alive and retained for the whole
        // time the web view can call back into this delegate.
        unsafe { child.setUIDelegate(Some(ProtocolObject::from_ref(&*observer))) };
        for path in observed_keys() {
            // SAFETY: `observer` is an `NSObject` subclass, so it is a valid
            // observer for the registration API; `path` is one of the static
            // `NSString` key paths the web view exposes, `New` is a valid options
            // mask, and the null context pointer is the same one passed when this
            // registration is removed in `Drop`, so the pair stays balanced.
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
        let started = if reload {
            // SAFETY: `child` is the live `WKWebView` this observer is attached to
            // and keeps retained in its ivars, so the receiver is valid and the
            // class implements `reload`; the returned optional `WKNavigation` is
            // owned by the `Retained` wrapper.
            unsafe { child.reload() }
        } else {
            NSURL::URLWithString(&NSString::from_str(address.as_str())).and_then(|url| {
                // SAFETY: `child` is the same live web view, and `url` was built on
                // the line above from the caller-validated address, so it is a
                // non-null `NSURL`; `NSURLRequest::requestWithURL` borrows `url`
                // only for this call, and the returned `Option<Retained<...>>` owns
                // the navigation.
                unsafe { child.loadRequest(&NSURLRequest::requestWithURL(&url)) }
            })
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
        // SAFETY: `child` is the live `WKWebView` this observer is attached to and
        // keeps retained in its ivars, so the receiver is valid and the class
        // implements `backForwardList`; the list is owned by the `Retained`
        // wrapper and lives for the calls chained onto it.
        let list = unsafe { child.backForwardList() };
        // SAFETY: `list` is the retained back/forward list obtained on the line
        // above, so it is live and implements `currentItem`; the result is either
        // null (no history item yet, handled by `as_ref`) or an owned item.
        let current = unsafe { list.currentItem() };
        let address = current
            .as_ref()
            // SAFETY: `item` is the current history item and `as_ref` above keeps a
            // borrow of it alive for this chain, so the receiver is valid and
            // `URL` — a non-optional accessor of `WKBackForwardListItem` — returns
            // an owned `NSURL` that the temporary retains for the call.
            .and_then(|item| unsafe { item.URL().absoluteString() })
            .and_then(|value| url::Url::parse(&value.to_string()).ok())
            .filter(|address| allowed_target(address, None));
        let title = address.as_ref().map(|address| {
            current
                .as_ref()
                // SAFETY: `item` is the same current history item kept alive by the
                // `as_ref` borrow, so the receiver is valid; `title` is an optional
                // accessor of `WKBackForwardListItem` and its `NSString` result is
                // owned by the `Retained` wrapper.
                .and_then(|item| unsafe { item.title() })
                .map(|title| title.to_string())
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| address.host_str().unwrap_or_default().to_owned())
        });
        // All WebKit reads and observation callbacks run on the AppKit thread.
        // SAFETY: `child` is the live web view owned by this observer and the call
        // runs on the AppKit thread, which is the only thread allowed to touch it
        // (the observer is `MainThreadOnly`); `isLoading` only reads a `BOOL`
        // property of the receiver.
        let loading = unsafe { child.isLoading() };
        // SAFETY: same live web view on the same AppKit thread as above;
        // `canGoBack` only reads a `BOOL` from the receiver's history state.
        let can_back = unsafe { child.canGoBack() };
        // SAFETY: same live web view on the same AppKit thread as above;
        // `canGoForward` only reads a `BOOL` from the receiver's history state.
        let can_forward = unsafe { child.canGoForward() };
        (
            current,
            Page {
                address: address.map(|address| address.into()),
                title,
                loading,
                can_back,
                can_forward,
                error: None,
            },
        )
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
        // SAFETY: the ivars still retain `child`, so the web view is live for the
        // call; `delegate` is the value read back from that web view in `attach`
        // and is borrowed only for the duration of the setter, which accepts exactly
        // this optional protocol-object type. `Observer` is `MainThreadOnly`, so
        // `drop` runs on the AppKit thread that owns the web view.
        unsafe {
            self.ivars()
                .child
                .setNavigationDelegate(self.ivars().delegate.as_deref())
        };
        // SAFETY: same live, still-retained `child` as above; `ui_delegate` is the
        // value read back from it in `attach`, borrowed only for this call, and the
        // setter accepts exactly this optional protocol-object type.
        unsafe {
            self.ivars()
                .child
                .setUIDelegate(self.ivars().ui_delegate.as_deref())
        };
        for path in observed_keys() {
            // Drop runs on the AppKit thread, with the observed child still retained.
            // SAFETY: every key path in `observed_keys` was registered by `attach`
            // with this same observer and none has been removed since, so each
            // removal here is balanced. `self` is a live `NSObject` observer for the
            // whole call and the child is still retained in the ivars, so it cannot
            // be observing a deallocated object.
            unsafe { self.ivars().child.removeObserver_forKeyPath(self, path) };
        }
    }
}
