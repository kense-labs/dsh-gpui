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
        // Tauri lends us the attached NSWindow on the AppKit thread.
        if let Some(native) = unsafe { platform.ns_window().cast::<NSWindow>().as_ref() } {
            native.setOpaque(false);
            let observer = RcBlock::new(move |_| {
                refresh_later(app.clone());
            });
            let center = NSNotificationCenter::defaultCenter();
            // Stay in AppKit's resize pass. Dispatching the button frames through
            // Tauri's queued resize events lets the default layout flash first.
            let resize = RcBlock::new(move |notification: NonNull<NSNotification>| {
                let notification = unsafe { notification.as_ref() };
                if let Some(window) = notification
                    .object()
                    .and_then(|object| object.downcast::<NSWindow>().ok())
                {
                    position_buttons(&window, WELCOME.get());
                }
            });
            // AppKit posts these notifications on its main thread for this NSWindow.
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
                    unsafe { center.removeObserver((&*token).as_ref()) };
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
    // The closure captures only sendable Tauri handles and scalar values.
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
    let titlebar = unsafe { close.superview().and_then(|buttons| buttons.superview()) };
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
