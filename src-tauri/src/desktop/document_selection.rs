//! The native Copy item for an active GPUI document selection.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSMenu, NSMenuItem, NSPasteboard, NSPasteboardTypeString, NSView};
use objc2_foundation::{NSObject, NSPoint, NSString};

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "DshGpuiDocumentCopyTarget"]
    #[ivars = String]
    struct CopyTarget;

    impl CopyTarget {
        #[unsafe(method(copySelection:))]
        fn copy_selection(&self, _sender: &AnyObject) {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            pasteboard.setString_forType(
                &NSString::from_str(self.ivars()),
                // SAFETY: `NSPasteboardTypeString` is an AppKit static whose address
                // is valid for the process lifetime, which is what the accessor's
                // contract requires; the reference is used only for this call.
                unsafe { NSPasteboardTypeString },
            );
        }
    }
);

pub(super) fn show(webview: tauri::Webview, text: String, x: f64, y: f64) -> Result<(), String> {
    webview
        .with_webview(move |webview| {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            // SAFETY: `with_webview` runs on the main thread while the webview is
            // alive, and Wry's `inner()` hands back the address of that webview's
            // backing `WKWebView`, which is an `NSView` subclass — so the cast is
            // to the correct type. `as_ref` rejects a null handle.
            let Some(view) = (unsafe { webview.inner().cast::<NSView>().as_ref() }) else {
                return;
            };
            let target = CopyTarget::alloc(mtm).set_ivars(text);
            // SAFETY: `alloc` returned an uninitialized `CopyTarget`, and `init` is
            // `NSObject`'s initializer, which returns the same instance now fully
            // initialized; the ivars were set above, before any other use.
            let target: Retained<CopyTarget> = unsafe { msg_send![super(target), init] };
            let item = NSMenuItem::new(mtm);
            item.setTitle(&NSString::from_str("复制"));
            // SAFETY: `target` is a live initialised object and `item` retains it for
            // as long as the menu tracks; `sel!(copySelection:)` names the method
            // declared on `CopyTarget` above, so the action's signature matches.
            unsafe {
                item.setTarget(Some(&target));
            }
            // SAFETY: as above — the selector is the one `CopyTarget` implements.
            unsafe {
                item.setAction(Some(sel!(copySelection:)));
            }
            item.setEnabled(true);
            let menu = NSMenu::new(mtm);
            menu.setAutoenablesItems(false);
            menu.addItem(&item);
            let bounds = view.bounds();
            let y = if view.isFlipped() {
                y
            } else {
                bounds.size.height - y
            };
            menu.popUpMenuPositioningItem_atLocation_inView(
                None,
                NSPoint::new(bounds.origin.x + x, bounds.origin.y + y),
                Some(view),
            );
        })
        .map_err(|error| error.to_string())
}
