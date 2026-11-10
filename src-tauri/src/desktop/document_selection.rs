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
                // AppKit provides a static pasteboard type for a plain string.
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
            // The macOS webview's native handle is a WKWebView, an NSView subclass.
            let Some(view) = (unsafe { webview.inner().cast::<NSView>().as_ref() }) else {
                return;
            };
            let target = CopyTarget::alloc(mtm).set_ivars(text);
            // NSObject initializes the target after the Rust ivars have been set.
            let target: Retained<CopyTarget> = unsafe { msg_send![super(target), init] };
            let item = NSMenuItem::new(mtm);
            item.setTitle(&NSString::from_str("复制"));
            // The target stays retained until the synchronous menu tracking ends.
            unsafe {
                item.setTarget(Some(&target));
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
