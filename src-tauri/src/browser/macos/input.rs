//! Route app shortcuts from focused browser guests and sandboxed HTML previews.

use super::super::{BrowserService, HtmlPreviewFocus};
use super::BrowserHost;
use crate::{
    shortcut_bindings::{self, Command, HeldKeys, Keymap},
    shortcuts::ShortcutService,
};
use block2::RcBlock;
use objc2::{
    DefinedClass, MainThreadMarker, msg_send,
    rc::{Retained, Weak},
    runtime::{AnyObject, NSObjectProtocol},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSView, NSWindow,
};
use serde_json::json;
use std::{
    cell::RefCell,
    collections::HashSet,
    ptr::{self, NonNull},
};
use tauri::{AppHandle, Emitter, Manager};

thread_local! {
    static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static MAIN_VIEW: RefCell<Option<Weak<NSView>>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy)]
enum Target {
    Main,
    Browser(super::BrowserId),
    Html(HtmlPreviewFocus),
}

#[derive(Default)]
struct Keyboard {
    owner: Option<(isize, u8, u64, u32, u64)>,
    held: HeldKeys,
    consumed: HashSet<(isize, u16)>,
}

fn modifiers(flags: NSEventModifierFlags) -> u8 {
    use NSEventModifierFlags as Mods;
    [
        (Mods::Control, shortcut_bindings::CONTROL),
        (Mods::Option, shortcut_bindings::ALT),
        (Mods::Shift, shortcut_bindings::SHIFT),
        (Mods::Command, shortcut_bindings::META),
    ]
    .into_iter()
    .filter(|(flag, _)| flags.contains(*flag))
    .fold(0, |all, (_, flag)| all | flag)
}

fn physical_code(code: u16) -> Option<&'static str> {
    // Carbon virtual key codes, independent of the character typed in the current layout.
    Some(match code {
        0 => "KeyA",
        1 => "KeyS",
        2 => "KeyD",
        3 => "KeyF",
        4 => "KeyH",
        5 => "KeyG",
        6 => "KeyZ",
        7 => "KeyX",
        8 => "KeyC",
        9 => "KeyV",
        11 => "KeyB",
        12 => "KeyQ",
        13 => "KeyW",
        14 => "KeyE",
        15 => "KeyR",
        16 => "KeyY",
        17 => "KeyT",
        18 => "Digit1",
        19 => "Digit2",
        20 => "Digit3",
        21 => "Digit4",
        22 => "Digit6",
        23 => "Digit5",
        24 => "Equal",
        25 => "Digit9",
        26 => "Digit7",
        27 => "Minus",
        28 => "Digit8",
        29 => "Digit0",
        30 => "BracketRight",
        31 => "KeyO",
        32 => "KeyU",
        33 => "BracketLeft",
        34 => "KeyI",
        35 => "KeyP",
        36 => "Enter",
        37 => "KeyL",
        38 => "KeyJ",
        39 => "Quote",
        40 => "KeyK",
        41 => "Semicolon",
        42 => "Backslash",
        43 => "Comma",
        44 => "Slash",
        45 => "KeyN",
        46 => "KeyM",
        47 => "Period",
        48 => "Tab",
        49 => "Space",
        50 => "Backquote",
        51 => "Backspace",
        53 => "Escape",
        54 => "MetaRight",
        55 => "MetaLeft",
        56 => "ShiftLeft",
        58 => "AltLeft",
        59 => "ControlLeft",
        60 => "ShiftRight",
        61 => "AltRight",
        62 => "ControlRight",
        64 => "F17",
        79 => "F18",
        80 => "F19",
        90 => "F20",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        99 => "F3",
        100 => "F8",
        101 => "F9",
        103 => "F11",
        105 => "F13",
        106 => "F16",
        107 => "F14",
        109 => "F10",
        111 => "F12",
        113 => "F15",
        117 => "Delete",
        118 => "F4",
        120 => "F2",
        122 => "F1",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        _ => return None,
    })
}

pub(crate) fn install_guest_input(app: AppHandle) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    MONITOR.with_borrow_mut(|slot| {
        if slot.is_some() {
            return;
        }
        if let Some(webview) = app.get_webview("main") {
            let _ = webview.with_webview(|platform| {
                // SAFETY: `with_webview` runs on the main thread while the webview is
                // alive, and Wry's `inner()` returns the address of that webview's
                // backing `WKWebView`, an `NSView` subclass — so the cast is to the
                // correct type and the referent outlives this borrow.
                let view = unsafe { &*platform.inner().cast::<NSView>() };
                MAIN_VIEW.with_borrow_mut(|main| *main = Some(Weak::new(view)));
            });
        }
        let keyboard = RefCell::new(Keyboard::default());
        let block = RcBlock::new(move |event: NonNull<NSEvent>| {
            // SAFETY: this is a local event monitor, so AppKit calls the handler
            // synchronously with the event it is currently dispatching and keeps that
            // event alive for the whole call; the reference never escapes the closure.
            let event_ref = unsafe { event.as_ref() };
            let Some(mtm) = MainThreadMarker::new() else {
                return event.as_ptr();
            };
            let window = event_ref.window(mtm).or_else(|| {
                if is_pointer_down(event_ref.r#type()) || event_ref.windowNumber() != 0
                {
                    return None;
                }
                NSApplication::sharedApplication(mtm).keyWindow()
            });
            let html_focus = || app.state::<BrowserService>().html_focus.lock().ok().map(|focus| *focus);
            let shortcuts = app.state::<ShortcutService>();
            let shell = super::current_browser_shell(&app);
            let recording = shortcuts.recording(shell);
            let routed = shortcuts.with_keys(|keys, revision| monitor_event(
                event_ref, window.as_deref(), &keyboard, html_focus, keys, revision,
                recording.is_some(),
                |code, modifiers, down, repeat| app.emit_to("main", "acp-event",
                    json!({"kind":"shortcut_record_input", "shell":shell, "recordingRevision":recording, "code":code, "modifiers":modifiers, "down":down, "repeat":repeat})).is_ok(),
                |target, command| {
                    let payload = match target {
                        Target::Browser(id) if id.shell == shell => {
                            if let Some(command) = command {
                                json!({"kind":"browser_input", "shell":shell, "key":id.key, "command":command, "bindingRevision":revision})
                            } else {
                                json!({"kind":"browser_focus", "shell":shell, "key":id.key})
                            }
                        }
                        Target::Html(focus) if focus.shell == shell => {
                            json!({"kind":"html_preview_input", "shell":shell, "key":focus.key,
                                "revision":focus.revision, "command":command, "bindingRevision":revision})
                        }
                        Target::Main => json!({"kind":"main_shortcut_input", "shell":shell, "command":command, "bindingRevision":revision}),
                        _ => return false,
                    };
                    app.emit_to("main", "acp-event", payload).is_ok()
                },
            ));
            if routed {
                ptr::null_mut()
            } else {
                event.as_ptr()
            }
        });
        // Returning the live event preserves native handling. Null consumes a routed shortcut.
        // SAFETY: `addLocalMonitorForEventsMatchingMask` is a main-thread-only API and
        // this function already returned unless a `MainThreadMarker` was available. The
        // mask and handler are plain values, and AppKit returns either a monitor token
        // to remove later or null, both of which are valid for `Option<Retained<_>>`.
        *slot = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown
                    | NSEventMask::KeyDown | NSEventMask::KeyUp | NSEventMask::FlagsChanged,
                &block,
            )
        };
    });
}

pub(crate) fn uninstall_guest_input() {
    MONITOR.with_borrow_mut(|slot| {
        if let Some(monitor) = slot.take() {
            // SAFETY: `monitor` is the token this module stored from
            // `addLocalMonitorForEventsMatchingMask`; `slot.take()` guarantees the
            // matching removal happens at most once, which is what AppKit requires.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
    });
    MAIN_VIEW.with_borrow_mut(|main| *main = None);
}

/// True only when AppKit should suppress a key the app received.
#[allow(clippy::too_many_arguments)]
fn is_pointer_down(event_type: NSEventType) -> bool {
    matches!(
        event_type,
        NSEventType::LeftMouseDown | NSEventType::RightMouseDown | NSEventType::OtherMouseDown
    )
}

fn monitor_event(
    event: &NSEvent,
    window: Option<&NSWindow>,
    keyboard: &RefCell<Keyboard>,
    html_focus: impl FnOnce() -> Option<HtmlPreviewFocus>,
    keys: &Keymap,
    revision: u64,
    recording: bool,
    record: impl FnOnce(&str, u8, bool, bool) -> bool,
    send: impl FnOnce(Target, Option<Command>) -> bool,
) -> bool {
    let event_type = event.r#type();
    if is_pointer_down(event_type) {
        keyboard.borrow_mut().held.clear();
        if let Some(window) = window
            && let Some(host) = pointer_host(window, event)
        {
            send(Target::Browser(host.ivars().id), None);
        }
        return false;
    }
    let Some(window) = window else {
        return false;
    };
    let code = event.keyCode();
    let press = (window.windowNumber(), code);
    let physical = physical_code(code);
    let mut keyboard = keyboard.borrow_mut();
    if event_type == NSEventType::FlagsChanged {
        keyboard.held.clear();
        if !recording {
            return false;
        }
    }
    let released = event_type == NSEventType::KeyUp;
    if released && let Some(code) = physical {
        keyboard.held.release(code);
    }
    if released && keyboard.consumed.remove(&press) {
        return true;
    }
    let Some(physical) = physical else {
        return false;
    };
    let repeat = event_type == NSEventType::KeyDown && event.isARepeat();
    if repeat {
        return keyboard.consumed.contains(&press);
    }
    if !released {
        keyboard.consumed.remove(&press);
    }
    let Some(responder) = window
        .firstResponder()
        .and_then(|responder| responder.downcast::<NSView>().ok())
    else {
        return false;
    };
    if responder.respondsToSelector(sel!(hasMarkedText))
        // SAFETY: the guard proved `responder` implements `hasMarkedText`, and the
        // selector takes no arguments and returns a plain `BOOL`, so the call and its
        // return type match the declared method.
        && unsafe { msg_send![&responder, hasMarkedText] }
    {
        keyboard.held.clear();
        return false;
    }
    let modifiers = modifiers(event.modifierFlags());
    let main = window.isKeyWindow() && is_main_webview_responder(responder.clone());
    if recording {
        keyboard.held.clear();
        if main {
            let flag = shortcut_bindings::modifier_key(physical);
            let down = if event_type == NSEventType::FlagsChanged {
                flag != 0 && modifiers & flag != 0
            } else {
                !released
            };
            let routed = record(physical, modifiers, down, repeat);
            return event_type != NSEventType::FlagsChanged && routed;
        }
        return false;
    }
    if released || event_type != NSEventType::KeyDown {
        return false;
    }
    let (target, identity) = if let Some(host) = browser_host(responder) {
        let id = host.ivars().id;
        (Target::Browser(id), (1, id.key, 0))
    } else if main {
        if let Some(focus) = html_focus().filter(|focus| focus.key.is_some()) {
            (
                Target::Html(focus),
                (2, focus.key.unwrap_or_default(), focus.revision),
            )
        } else {
            (Target::Main, (0, 0, 0))
        }
    } else {
        keyboard.held.clear();
        return false;
    };
    let owner = (press.0, identity.0, identity.1, identity.2, revision);
    if keyboard.owner != Some(owner) {
        keyboard.held.clear();
        keyboard.owner = Some(owner);
    }
    let Some(command) = keyboard.held.press(physical, modifiers, keys, true) else {
        return false;
    };
    let routed = send(target, Some(command));
    if routed {
        keyboard.consumed.insert(press);
    }
    routed
}

fn is_main_webview_responder(mut view: Retained<NSView>) -> bool {
    let Some(main) = MAIN_VIEW.with_borrow(|main| main.as_ref().and_then(Weak::load)) else {
        return false;
    };
    loop {
        if ptr::eq(&*view, &*main) {
            return true;
        }
        // SAFETY: local event monitors run inside AppKit's dispatch, which keeps the
        // whole view hierarchy alive, so each `superview` receiver is valid; the
        // returned parent is unretained and is only walked while that holds.
        let Some(parent) = (unsafe { view.superview() }) else {
            return false;
        };
        view = parent;
    }
}

fn pointer_host(window: &NSWindow, event: &NSEvent) -> Option<Retained<BrowserHost>> {
    let content = window.contentView()?;
    let point = event.locationInWindow();
    // hitTest expects superview coordinates. The NSWindow retains this view hierarchy.
    // SAFETY: `superview` is called on the window's own content view, whose parent is
    // kept alive by the window for the duration of this event dispatch; the returned
    // parent is used only within this expression.
    let point = unsafe { content.superview() }
        .map_or(point, |parent| parent.convertPoint_fromView(point, None));
    browser_host(content.hitTest(point)?)
}

fn browser_host(mut view: Retained<NSView>) -> Option<Retained<BrowserHost>> {
    if view.isHiddenOrHasHiddenAncestor() {
        return None;
    }
    loop {
        view = match view.downcast::<BrowserHost>() {
            Ok(host) => return Some(host),
            // SAFETY: the caller obtained `view` from an event AppKit is dispatching,
            // so the window retains this view hierarchy; the walk stops at the first
            // `BrowserHost` or the root, and each parent stays valid while it runs.
            Err(view) => unsafe { view.superview() }?,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_application_chords_are_consumed() {
        let map = Keymap::new(
            &shortcut_bindings::Document::default(),
            shortcut_bindings::Platform::Mac,
        );
        let command = |code, flags| map.find(physical_code(code).unwrap(), None, modifiers(flags));
        use NSEventModifierFlags as Mods;
        assert_eq!(command(13, Mods::Command), Some(Command::Close));
        assert_eq!(command(15, Mods::Command), Some(Command::Refresh));
        assert_eq!(
            command(11, Mods::Command | Mods::Option),
            Some(Command::ToggleDock)
        );
        assert_eq!(command(13, Mods::Command | Mods::Shift), None);
        for code in [0, 8, 9] {
            assert_eq!(command(code, Mods::Command), None);
        }
        assert_eq!(command(36, Mods::empty()), None);
        assert_eq!(command(48, Mods::Shift), None);
    }
}
