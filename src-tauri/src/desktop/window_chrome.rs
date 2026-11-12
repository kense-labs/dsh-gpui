//! AppKit material and traffic lights for the two shell presentations.

use std::{
    cell::{Cell, RefCell},
    ptr::NonNull,
};

use block2::RcBlock;
use objc2::{
    rc::Retained,
    runtime::{NSObjectProtocol, ProtocolObject},
};
use objc2_app_kit::{
    NSWindow, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowDidChangeOcclusionStateNotification, NSWindowDidDeminiaturizeNotification,
    NSWindowDidEndLiveResizeNotification, NSWindowDidEnterFullScreenNotification,
    NSWindowDidExitFullScreenNotification, NSWindowDidMiniaturizeNotification,
    NSWindowDidResizeNotification, NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSOperationQueue, NSPoint, NSRect};
use tauri::{
    Emitter, Manager, Webview, Window, WindowEvent,
    window::{Color, Effect, EffectState, EffectsBuilder},
};

use super::ShellWindow;

thread_local! {
    static OBSERVERS: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>> = const { RefCell::new(Vec::new()) };
    static BACKDROP_VISIBLE: Cell<Option<bool>> = const { Cell::new(None) };
    static FULLSCREEN: Cell<Option<bool>> = const { Cell::new(None) };
    static WELCOME: Cell<bool> = const { Cell::new(true) };
}

pub(super) fn install(webview: &Webview) -> tauri::Result<()> {
    webview.set_background_color(Some(Color(0, 0, 0, 0)))?;
    let app = webview.app_handle().clone();
    webview.with_webview(move |platform| {
        // SAFETY: Tauri lends us the attached NSWindow on the AppKit thread.
        // `with_webview` runs this closure on that thread while the webview is
        // still alive, and Wry only ever hands out the address of the NSWindow
        // backing that webview, which AppKit keeps alive for as long as the
        // window exists. `as_ref()` yields `None` for a null pointer, so no
        // reference is created unless the pointer is non-null, and the borrow
        // does not outlive this callback.
        if let Some(native) = unsafe { platform.ns_window().cast::<NSWindow>().as_ref() } {
            native.setOpaque(false);
            let observer = RcBlock::new(move |_| {
                refresh_later(app.clone());
            });
            let center = NSNotificationCenter::defaultCenter();
            // Stay in AppKit's resize pass. Dispatching the button frames through
            // Tauri's queued resize events lets the default layout flash first.
            let resize = RcBlock::new(move |notification: NonNull<NSNotification>| {
                // SAFETY: AppKit invokes this observer block with a non-null
                // `NSNotification` and keeps it alive for the duration of the
                // call, so the pointer is dereferenceable here; the borrow
                // ends inside this block and the notification is never stored.
                let notification = unsafe { notification.as_ref() };
                if let Some(window) = notification
                    .object()
                    .and_then(|object| object.downcast::<NSWindow>().ok())
                {
                    position_buttons(&window, WELCOME.get());
                }
            });
            // AppKit posts these notifications on its main thread for this NSWindow.
            // SAFETY: `native` is the live NSWindow this webview belongs to, which is
            // the object AppKit uses to post the `NSWindowDid*` notifications, so it
            // is the correct type for the `object:` argument; passing `None` for the
            // queue keeps delivery on the main thread the closure already runs on,
            // so AppKit is never touched off-thread. The notification names are
            // immutable `&'static` statics vended by the linked AppKit framework,
            // valid for the whole process. `observer` is a heap-allocated `RcBlock`
            // that the center retains for as long as the observation lives, and the
            // returned tokens are handed back to the caller below to be kept in
            // `OBSERVERS`, so nothing dangles.
            let tokens = unsafe {
                [
                    NSWindowDidMiniaturizeNotification,
                    NSWindowDidDeminiaturizeNotification,
                    NSWindowDidChangeOcclusionStateNotification,
                    NSWindowDidEnterFullScreenNotification,
                    NSWindowDidExitFullScreenNotification,
                ]
                .map(|notification| {
                    center.addObserverForName_object_queue_usingBlock(
                        Some(notification),
                        Some(native),
                        None,
                        &observer,
                    )
                })
            };
            // SAFETY: same contract as the registrations above: `native` is the
            // window object AppKit posts `NSWindowDidResizeNotification` and
            // `NSWindowDidEndLiveResizeNotification` with, the `None` queue keeps
            // the block on the posting (main) thread, and the `&'static` names are
            // immutable framework statics. The center retains `resize` for the
            // lifetime of the observation and the returned tokens are stored in
            // `OBSERVERS`, so the block outlives this callback.
            let resize_tokens = unsafe {
                [
                    NSWindowDidResizeNotification,
                    NSWindowDidEndLiveResizeNotification,
                ]
                .map(|notification| {
                    center.addObserverForName_object_queue_usingBlock(
                        Some(notification),
                        Some(native),
                        None,
                        &resize,
                    )
                })
            };
            OBSERVERS.with_borrow_mut(|observers| {
                observers.extend(tokens);
                observers.extend(resize_tokens);
            });
        }
    })?;
    configure(webview, true)
}

pub(super) fn configure(webview: &Webview, welcome: bool) -> tauri::Result<()> {
    webview.with_webview(move |platform| {
        // SAFETY: identical to `install`: `with_webview` runs the closure on the
        // AppKit thread while the webview is alive, so the NSWindow Tauri returns
        // is the live window for this webview; `as_ref()` checks for null before
        // forming the reference and the borrow ends with this callback.
        if let Some(native) = unsafe { platform.ns_window().cast::<NSWindow>().as_ref() } {
            WELCOME.set(welcome);
            let mut behavior = native.collectionBehavior();
            behavior.set(NSWindowCollectionBehavior::FullScreenNone, welcome);
            behavior.set(NSWindowCollectionBehavior::FullScreenPrimary, !welcome);
            native.setCollectionBehavior(behavior);
        }
    })?;
    refresh(webview, welcome, true)
}

pub(super) fn refresh(webview: &Webview, welcome: bool, force: bool) -> tauri::Result<()> {
    let window = webview.window();
    webview.with_webview(move |platform| {
        // SAFETY: as in `install`/`configure`, the closure runs on the AppKit
        // thread with the webview alive, so the returned pointer is the live
        // NSWindow backing this webview; `as_ref()` yields `None` when the
        // pointer is null, and the reference is confined to this callback.
        let Some(native) = (unsafe { platform.ns_window().cast::<NSWindow>().as_ref() }) else {
            return;
        };
        if !native.inLiveResize() {
            position_buttons(native, welcome);
        }
        let visible = welcome || (native.isVisible() && !native.isMiniaturized());
        if BACKDROP_VISIBLE.replace(Some(visible)) != Some(visible) || force {
            update_backdrop(&window, welcome, visible);
        }
        let fullscreen = native.styleMask().contains(NSWindowStyleMask::FullScreen);
        if FULLSCREEN.replace(Some(fullscreen)) != Some(fullscreen) || force {
            // with_webview holds Wry's window ID lock while invoking this
            // callback. Emitting to its webview must wait for that lock.
            on_next_main_turn(move || {
                if FULLSCREEN.get() == Some(fullscreen) {
                    let _ = window.emit_to(
                        "main",
                        "acp-event",
                        serde_json::json!({"kind": "shell_fullscreen", "value": fullscreen}),
                    );
                }
            });
        }
    })
}

fn update_backdrop(window: &Window, welcome: bool, visible: bool) {
    // A solid window backs the macOS minimize animation while the material
    // view is detached. Reattach it as soon as the workspace is visible.
    let (effects, background) = if visible {
        let material = if welcome {
            Effect::Menu
        } else {
            Effect::Sidebar
        };
        (
            Some(
                EffectsBuilder::new()
                    .effect(material)
                    .state(EffectState::Active)
                    .build(),
            ),
            Color(0, 0, 0, 0),
        )
    } else {
        let color = match window.theme() {
            Ok(tauri::Theme::Light) => Color(249, 250, 251, 255),
            _ => Color(27, 27, 28, 255),
        };
        (None, color)
    };
    let _ = window.set_background_color(Some(background));
    let _ = window.set_effects(effects);
}

pub(crate) fn handle_event(window: &Window, event: &WindowEvent) {
    if window.label() != "main" {
        return;
    }
    match event {
        WindowEvent::Destroyed => {
            OBSERVERS.with_borrow_mut(|observers| {
                let center = NSNotificationCenter::defaultCenter();
                for token in observers.drain(..) {
                    // These observers were installed on this center for the main window.
                    // SAFETY: `defaultCenter()` returns the process-wide center that
                    // `install` registered against, and each drained `token` is a
                    // token previously returned by that registration, so it is the
                    // correct observer type for `removeObserver:`. `Retained` derefs
                    // to the observer and keeps it alive across the call, and it is
                    // dropped only after removal, so the center never holds a
                    // dangling observer.
                    unsafe { center.removeObserver((*token).as_ref()) };
                }
            });
            BACKDROP_VISIBLE.set(None);
            FULLSCREEN.set(None);
            WELCOME.set(true);
        }
        WindowEvent::Focused(_) | WindowEvent::ScaleFactorChanged { .. } => {
            refresh_later(window.app_handle().clone());
        }
        _ => {}
    }
}

fn refresh_later(app: tauri::AppHandle) {
    on_next_main_turn(move || {
        if let Some(webview) = app.get_webview("main")
            && let Some(welcome) = current_mode(&webview.window())
        {
            let _ = refresh(&webview, welcome, false);
        }
    });
}

fn on_next_main_turn(work: impl Fn() + Send + Sync + 'static) {
    let operation = RcBlock::new(work);
    // SAFETY: `mainQueue()` is the process-wide main operation queue and is never
    // null; adding the block is safe from any thread, and the block is a
    // heap-allocated `RcBlock` that the queue retains until it has run, so it
    // outlives this call. The closure captures only sendable Tauri handles and
    // scalar values, which is what `addOperationWithBlock:` requires of a block
    // that may execute on another thread.
    unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&operation) };
}

fn current_mode(window: &Window) -> Option<bool> {
    let state = window.try_state::<ShellWindow>()?;
    state.0.try_lock().ok().map(|mode| *mode)
}

fn position_buttons(window: &NSWindow, welcome: bool) {
    if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return;
    }
    let (Some(close), Some(minimize), Some(zoom)) = (
        window.standardWindowButton(NSWindowButton::CloseButton),
        window.standardWindowButton(NSWindowButton::MiniaturizeButton),
        window.standardWindowButton(NSWindowButton::ZoomButton),
    ) else {
        return;
    };
    // The live standard button and its ancestors belong to this window.
    // SAFETY: `superview` returns an unretained back-pointer, so it requires only
    // that the receiver still be alive. Both receivers are alive here: `close` is
    // the standard close button owned by the window that `position_buttons` was
    // called with, and the outer `superview()` result is kept in the `buttons`
    // binding for the duration of the call, so the receiver of the second
    // `superview()` cannot be deallocated in between.
    let superview = unsafe { close.superview() };
    // SAFETY: same contract as above: `superview` only requires a live receiver,
    // and the outer result is held in the local `superview` binding established on
    // the previous line, so that receiver is still alive and its own superview is
    // therefore still alive as well. Nothing is dropped before this call returns.
    let titlebar = unsafe { superview.and_then(|buttons| buttons.superview()) };
    let Some(titlebar) = titlebar else {
        return;
    };

    // AppKit keeps the three buttons in a nested titlebar view. Resize its
    // enclosing view to move them down while preserving the system spacing.
    let (left, top) = if welcome { (21., 21.) } else { (16., 18.) };
    let mut titlebar_frame = titlebar.frame();
    titlebar_frame.size.height = close.frame().size.height + top;
    titlebar_frame.origin.y = window.frame().size.height - titlebar_frame.size.height;
    if titlebar.frame() != titlebar_frame {
        titlebar.setFrame(titlebar_frame);
    }
    let spacing = minimize.frame().origin.x - close.frame().origin.x;
    for (index, button) in [close, minimize, zoom].into_iter().enumerate() {
        // The offsets are measured from the window's top-left corner. The
        // titlebar's inner view can introduce its own vertical inset.
        // SAFETY: `superview` requires only that the receiver is still alive.
        // `button` was taken from `window.standardWindowButton(..)` above and is
        // still owned by that live window, so AppKit keeps it — and hence its
        // superview — alive while this loop body runs.
        let Some(parent) = (unsafe { button.superview() }) else {
            continue;
        };
        let size = button.frame().size;
        let frame = NSRect::new(
            NSPoint::new(
                left + index as f64 * spacing,
                window.frame().size.height - top - size.height,
            ),
            size,
        );
        let origin = parent.convertRect_fromView(frame, None).origin;
        if button.frame().origin != origin {
            button.setFrameOrigin(origin);
        }
    }
}
