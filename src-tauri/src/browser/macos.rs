//! An AppKit host that clips both a child WKWebView and its mouse hit area.

use super::{
    BrowserBounds, BrowserClip, BrowserGuest, BrowserId, BrowserLayout, BrowserStorage,
    browser_terminated, current_browser_shell, emit_browser_popup,
};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGPath;
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CAShapeLayer, CATransaction};
use objc2_web_kit::{WKWebView, WKWebViewConfiguration, WKWebsiteDataStore};
use serde_json::json;
use std::{
    cell::RefCell,
    collections::HashMap,
    ptr::{self, NonNull},
    sync::{OnceLock, mpsc},
};
use tauri::{
    Emitter, LogicalPosition, LogicalSize, Manager, Runtime, Webview, Window,
    webview::WebviewBuilder,
};

pub(super) mod input;
mod page;

type Subtract = unsafe extern "C-unwind" fn(*const CGPath, *const CGPath, bool) -> *mut CGPath;

fn subtract_path() -> Option<Subtract> {
    static FUNCTION: OnceLock<Option<Subtract>> = OnceLock::new();
    *FUNCTION.get_or_init(|| {
        // Resolve the macOS 13 API at runtime so earlier macOS can still load the app.
        // SAFETY: `RTLD_DEFAULT` is a valid pseudo-handle that searches the global
        // symbol scope, and the name is a `'static`, NUL-terminated C string literal,
        // which is the one precondition `dlsym` places on it. A missing symbol is a
        // normal answer reported as null and handled by the check just below.
        let symbol = unsafe {
            libc::dlsym(
                libc::RTLD_DEFAULT,
                c"CGPathCreateCopyBySubtractingPath".as_ptr(),
            )
        };
        (!symbol.is_null()).then(|| {
            // CoreGraphics exports this function with the signature in CGPath.h.
            // SAFETY: the non-null `symbol` resolves the `CGPathCreateCopyBySubtractingPath`
            // symbol, whose declaration is `CGPathRef (CGPathRef, CGPathRef, bool)`.
            // `Subtract` has that exact ABI, so calls through this pointer use the
            // real function's signature and calling convention.
            unsafe { std::mem::transmute::<*mut libc::c_void, Subtract>(symbol) }
        })
    })
}

pub(super) fn supports_clipping() -> bool {
    subtract_path().is_some()
}

pub(super) fn add_child<R: Runtime>(
    window: Window<R>,
    builder: WebviewBuilder<R>,
    position: LogicalPosition<f64>,
    size: LogicalSize<f64>,
    storage: BrowserStorage,
    shell: u64,
) -> Result<Webview<R>, String> {
    thread_local! {
        // Retain each account across tab closures for the lifetime of the AppKit thread.
        static STORES: RefCell<HashMap<BrowserStorage, Retained<WKWebsiteDataStore>>> =
            RefCell::default();
    }
    let (send, receive) = mpsc::channel();
    window
        .clone()
        .run_on_main_thread(move || {
            let result = (|| {
                if current_browser_shell(window.app_handle()) != shell {
                    return Err("浏览器界面已重新加载".into());
                }
                let mtm = MainThreadMarker::new().ok_or("浏览器配置必须在主线程创建")?;
                // WebKit configurations and data stores are confined to the AppKit thread.
                // SAFETY: `WKWebViewConfiguration` is `MainThreadOnly`, and `mtm` is a
                // `MainThreadMarker`, which can only be obtained on the AppKit main
                // thread — the thread this closure is running on. `new` is in the `new`
                // method family, so the returned object is +1 and owned by `Retained`.
                let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
                STORES.with_borrow_mut(|stores| {
                    // SAFETY: `WKWebsiteDataStore` is `MainThreadOnly` and `mtm`, still
                    // live here, proves this is the AppKit thread. The class method
                    // returns a fresh store whose +1 reference the `Retained` inside the
                    // map takes ownership of, and `entry(..).or_insert_with` keeps it
                    // alive for as long as the store is reachable from this thread.
                    let store = stores.entry(storage).or_insert_with(|| unsafe {
                        WKWebsiteDataStore::nonPersistentDataStore(mtm)
                    });
                    // SAFETY: `configuration` is a live configuration that `Retained`
                    // keeps alive and that has not been handed to a web view yet, and
                    // `store` is a live data store owned by the thread-local map for the
                    // rest of the AppKit thread's lifetime. The `websiteDataStore`
                    // property is `strong`, so WebKit retains the store, and writing it
                    // here precedes `with_webview_configuration` consuming the
                    // configuration, which is the only ordering WebKit permits.
                    unsafe { configuration.setWebsiteDataStore(store) };
                });
                window
                    .add_child(
                        builder.with_webview_configuration(configuration),
                        position,
                        size,
                    )
                    .map_err(|error| format!("创建内嵌浏览器失败：{error}"))
            })();
            let _ = send.send(result);
        })
        .map_err(|error| format!("无法配置内嵌浏览器：{error}"))?;
    receive.recv().map_err(|_| "无法配置内嵌浏览器")?
}

#[derive(Clone)]
enum Visibility {
    Full,
    Partial(CFRetained<CGPath>),
    Covered,
}

impl Visibility {
    fn contains(&self, point: NSPoint) -> bool {
        match self {
            Self::Full => true,
            Self::Partial(path) => {
                // A null transform tests the same local path used by the layer mask.
                // SAFETY: `path` is a `CFRetained<CGPath>` held by this variant, so the
                // path outlives the call; `contains_point` only reads it. The transform
                // argument is null, which the binding documents as the valid "no
                // transform" form, so the point is tested in the path's own coordinate
                // space — the same space the caller converted `point` into.
                unsafe { CGPath::contains_point(Some(path), ptr::null(), point, false) }
            }
            Self::Covered => false,
        }
    }
}

struct BrowserHostIvars {
    id: BrowserId,
    visibility: RefCell<Visibility>,
    mask: Retained<CAShapeLayer>,
    observer: RefCell<Option<Retained<page::Observer>>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "DshGpuiBrowserHostView"]
    #[ivars = BrowserHostIvars]
    struct BrowserHost;

    impl BrowserHost {
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            self.clipped_hit(point)
        }
    }
);

impl BrowserHost {
    fn clipped_hit(&self, point: NSPoint) -> Option<Retained<NSView>> {
        // AppKit calls hitTest on the main thread while traversing a live parent.
        // SAFETY: `superview` is declared `unsafe_unretained`, so it does not return a
        // +1 reference; the `Retained` wrapper this binding returns assumes the parent
        // outlives the borrow. AppKit guarantees that while a view is attached its
        // superview stays alive, and `self` is a view AppKit is currently traversing
        // for this hit test, so the parent is live for the duration of this call.
        let parent = unsafe { self.superview() }?;
        let local = self.convertPoint_fromView(point, Some(&parent));
        if !self.ivars().visibility.borrow().contains(local) {
            return None;
        }
        // An empty or hidden child must not leave the host blocking the canvas.
        // SAFETY: the receiver is `super(self)`, i.e. the live `NSView` base of this
        // very object, and `hitTest:` is a message `NSView` itself implements, so the
        // selector resolves on that receiver. It is not in a recognized method family,
        // so `msg_send!` treats the returned view as autoreleased and retains it via
        // `Retained::retain_autoreleased`, which is what keeps the result alive after
        // AppKit's autorelease pool drains. `point` is a plain `NSPoint` value.
        let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
        hit.filter(|view| !ptr::eq::<NSView>(&**view, self.as_super()))
    }

    fn attach(
        id: BrowserId,
        child: &NSView,
        parent: &NSView,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let frame = child.frame();
        let alloc = Self::alloc(mtm).set_ivars(BrowserHostIvars {
            id,
            visibility: RefCell::new(Visibility::Covered),
            mask: CAShapeLayer::new(),
            observer: RefCell::default(),
        });
        // Initialize an ordinary NSView with the old child's frame and z order.
        // SAFETY: `alloc` is the uninitialized instance returned by `Self::alloc(mtm)`,
        // so it is a valid, owned receiver of this exact class and the `initWithFrame:`
        // message is the designated initializer the class declares. `frame` is a plain
        // `NSRect` value, and as a method in the `init` family the call consumes `alloc`
        // and hands back a +1 initialized instance that `Retained` owns, so the object
        // is initialized exactly once.
        let host: Retained<Self> = unsafe { msg_send![super(alloc), initWithFrame: frame] };
        host.setWantsLayer(true);
        host.setAutoresizingMask(if parent.isFlipped() {
            NSAutoresizingMaskOptions::ViewMaxYMargin
        } else {
            NSAutoresizingMaskOptions::ViewMinYMargin
        });
        parent.replaceSubview_with(child, &host);
        host.addSubview(child);
        child.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        child.setFrame(host.bounds());
        host
    }

    fn place(&self, child: &NSView, layout: &BrowserLayout, subtract: Subtract) {
        // The host is attached to the live content view by present().
        // SAFETY: `superview` is `unsafe_unretained`, so the returned `Retained` relies
        // on the parent outliving the borrow rather than on a +1 return. `present()`
        // only calls `place` on a host it has already added to a live parent view, and
        // detaching the host takes it out of that hierarchy, so the parent is alive
        // here. The borrow is confined to this function and reads only geometry.
        let Some(parent) = (unsafe { self.superview() }) else {
            return;
        };
        let bounds = parent.bounds();
        let area = layout.bounds;
        let y = if parent.isFlipped() {
            area.y
        } else {
            bounds.size.height - area.y - area.height
        };
        let frame = NSRect::new(
            NSPoint::new(bounds.origin.x + area.x, bounds.origin.y + y),
            NSSize::new(area.width, area.height),
        );
        let visibility = visible_path(layout, subtract, true);
        let hit_area = if layout.covers.iter().any(|cover| cover.visual_only) {
            visible_path(layout, subtract, false)
        } else {
            visibility.clone()
        };

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        if self.frame() != frame {
            self.setFrame(frame);
        }
        let child_frame = self.bounds();
        if child.frame() != child_frame {
            child.setFrame(child_frame);
        }
        let shown = match (&visibility, self.layer()) {
            (Visibility::Covered, _) | (Visibility::Partial(_), None) => false,
            (Visibility::Full, layer) => {
                if let Some(layer) = layer {
                    // This host owns the mask and it was never added as a sublayer.
                    // SAFETY: `setMask:` is `unsafe` because CALayer requires the new
                    // mask to have a nil superlayer; passing `None` installs the null
                    // mask, which carries no such precondition. `layer` is the host's
                    // own live backing layer, returned by `self.layer()`, and clearing
                    // its mask only drops the mask this host had installed.
                    unsafe { layer.setMask(None) };
                }
                true
            }
            (Visibility::Partial(path), Some(layer)) => {
                let mask = &self.ivars().mask;
                mask.setFrame(layer.bounds());
                if let Some(window) = self.window() {
                    mask.setContentsScale(window.backingScaleFactor());
                }
                mask.setPath(Some(path));
                // The shape layer is used exclusively as this host's mask.
                // SAFETY: `layer` is this host's own live backing layer, and `mask` is
                // the `CAShapeLayer` owned by the host's ivars, which is never added to
                // any layer hierarchy — so its superlayer is nil, the precondition
                // `setMask:` requires of a new mask. `mask` outlives `layer`, because
                // the ivar is kept alive by the host that owns the layer, and `path`
                // was written to the mask just above so its shape is current.
                unsafe { layer.setMask(Some(mask)) };
                true
            }
        };
        self.ivars().visibility.replace(hit_area);
        self.setHidden(!shown);
        child.setHidden(!shown);
        CATransaction::commit();
    }
}

pub(super) fn present<R: Runtime>(
    webview: &Webview<R>,
    id: BrowserId,
    layout: BrowserLayout,
    request: Option<(u64, url::Url)>,
    application_origin: Option<url::Origin>,
    guest: BrowserGuest,
) -> Result<(), String> {
    let Some(subtract) = subtract_path() else {
        return Err("当前 macOS 无法裁剪内嵌浏览器".into());
    };
    let app = webview.app_handle().clone();
    webview
        .with_webview(move |platform| {
            if guest.is_terminated() || current_browser_shell(&app) != id.shell {
                return;
            }
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            // Tauri supplies a live WKWebView on the AppKit thread for this callback.
            // SAFETY: `platform.inner()` is the `WKWebView *` Tauri hands to
            // `with_webview` for this live webview, and the callback runs on the main
            // thread, where WebKit requires that object to be touched. The raw pointer
            // is non-null and points at an object Tauri keeps alive for the duration of
            // the callback, so reborrowing it as `&WKWebView` for that same duration
            // cannot dangle; the reference is never used to mutate through a shared
            // alias.
            let child: &WKWebView = unsafe { &*platform.inner().cast() };
            // SAFETY: `child` is the live web view reborrowed just above, and
            // `superview` is `unsafe_unretained`, so the returned `Retained` assumes
            // the parent outlives the borrow. Tauri has already added the web view to
            // its window's view hierarchy by the time it invokes this callback, so the
            // parent exists and stays alive while the callback runs.
            let Some(parent) = (unsafe { child.superview() }) else {
                return;
            };
            let host = match parent.downcast::<BrowserHost>() {
                Ok(host) => host,
                Err(parent) => BrowserHost::attach(id, child, &parent, mtm),
            };
            host.place(child, &layout, subtract);
            if let Some((navigation, address)) = request {
                let observer = host
                    .ivars()
                    .observer
                    .borrow_mut()
                    .get_or_insert_with(|| {
                        let popup_app = app.clone();
                        let terminated_app = app.clone();
                        page::Observer::attach(
                            child,
                            application_origin,
                            move |address| emit_browser_popup(&popup_app, id, address),
                            move |navigation, sequence, page| {
                                let _ = app.emit_to(
                                    "main",
                                    "acp-event",
                                    json!({"kind": "browser_page", "shell": id.shell, "key": id.key, "navigation": navigation,
                                        "sequence": sequence, "page": page}),
                                );
                            },
                            move || browser_terminated(terminated_app.clone(), id, &guest),
                            mtm,
                        )
                    })
                    .clone();
                observer.navigate(navigation, &address);
            }
        })
        .map_err(|error| format!("定位浏览器失败：{error}"))
}

pub(super) fn detach<R: Runtime>(webview: &Webview<R>) {
    let _ = webview.with_webview(|platform| {
        // This callback runs before Wry closes and releases its child view.
        // SAFETY: as in `present`, `platform.inner()` is the live `WKWebView *` Tauri
        // passes to `with_webview`, and this callback runs on the main thread before
        // Wry tears the view down, so the object is still alive and may be reborrowed
        // as `&WKWebView` for the duration of the callback.
        let child: &WKWebView = unsafe { &*platform.inner().cast() };
        // SAFETY: `child` is the same live web view reborrowed above. `superview` is
        // `unsafe_unretained`, so the `Retained` wrapper assumes the parent outlives the
        // borrow; Tauri runs this callback while the web view is still installed in its
        // window, and this callback only removes the host that Tauri attached, so the
        // parent is live for the borrow. The borrow does not outlive the closure, and
        // `downcast` re-checks the concrete class before the parent is treated as a
        // `BrowserHost`.
        if let Some(parent) = unsafe { child.superview() }
            && let Ok(host) = parent.downcast::<BrowserHost>()
        {
            host.ivars().observer.take();
            host.removeFromSuperview();
        }
    });
}

fn local_rect(shape: BrowserBounds, viewport: BrowserBounds) -> NSRect {
    NSRect::new(
        NSPoint::new(
            shape.x - viewport.x,
            viewport.y + viewport.height - shape.y - shape.height,
        ),
        NSSize::new(shape.width, shape.height),
    )
}

fn clip_path(clip: BrowserClip, viewport: BrowserBounds) -> CFRetained<CGPath> {
    let rect = local_rect(clip.bounds, viewport);
    let radius = clip
        .radius
        .min(rect.size.width / 2.)
        .min(rect.size.height / 2.);
    // CoreGraphics takes a null transform for points in the supplied coordinate space.
    if radius > 0. {
        // SAFETY: the transform argument is null, which the binding documents as the
        // valid "no transform" form, so no `CGAffineTransform` is read through it.
        // `rect` is a plain value argument and `radius` was clamped above to at most
        // half the rect's width and height, so the two corner axes stay within the
        // rounded rect CoreGraphics constructs for that rect. The function follows the
        // Create rule, so the returned `CFRetained` takes ownership of its +1 reference.
        unsafe { CGPath::with_rounded_rect(rect, radius, radius, ptr::null()) }
    } else {
        // SAFETY: here too the transform argument is the valid null form, so no
        // `CGAffineTransform` is read through it. `rect` is a plain value argument, and
        // the result follows the Create rule, so the returned `CFRetained` takes
        // ownership of its +1 reference.
        unsafe { CGPath::with_rect(rect, ptr::null()) }
    }
}

fn visible_path(layout: &BrowserLayout, subtract: Subtract, include_visual: bool) -> Visibility {
    let viewport = layout.bounds;
    let resize_grip = layout.frame_clip.map(|frame| BrowserClip {
        bounds: BrowserBounds {
            x: frame.bounds.x + frame.bounds.width - 20.,
            y: frame.bounds.y + frame.bounds.height - 20.,
            width: 20.,
            height: 20.,
        },
        radius: 0.,
        visual_only: false,
    });
    let mut cuts = layout
        .covers
        .iter()
        .copied()
        .chain(resize_grip)
        .filter(|cover| (include_visual || !cover.visual_only) && viewport.intersects(cover.bounds))
        .peekable();
    if layout.frame_clip.is_none() && cuts.peek().is_none() {
        return Visibility::Full;
    }

    let mut path = clip_path(
        layout.frame_clip.unwrap_or(BrowserClip {
            bounds: viewport,
            radius: 0.,
            visual_only: false,
        }),
        viewport,
    );
    for cover in cuts {
        let cut = clip_path(cover, viewport);
        // The Create Rule transfers one reference to the result. Repeated
        // subtraction handles overlapping covers without exposing their overlap.
        // SAFETY: `subtract` resolved to `CGPathCreateCopyBySubtractingPath` and this
        // call only happens on macOS versions where that symbol exists, since
        // `subtract_path` is the sole source of the pointer. Both `path` and `cut` are
        // live `CFRetained<CGPath>` values that outlive the call, matching the
        // function's `CGPathRef` parameters, and the false flag selects the documented
        // non-even-odd fill rule. The function only reads its arguments.
        let result = unsafe { subtract(&*path, &*cut, false) };
        let Some(result) = NonNull::new(result) else {
            return Visibility::Covered;
        };
        // SAFETY: the non-null `result` came from `CGPathCreateCopyBySubtractingPath`,
        // which follows the Create rule, so it already carries the +1 reference that
        // `CFRetained::from_raw` takes ownership of. It was not retained or released
        // in between, and the old `path` is dropped by this assignment while its own
        // reference is still accounted for, so neither path is double-freed.
        path = unsafe { CFRetained::from_raw(result) };
        if CGPath::is_empty(Some(&path)) {
            return Visibility::Covered;
        }
    }
    Visibility::Partial(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clipping() -> Option<Subtract> {
        let subtract = subtract_path();
        if objc2::available!(macos = 13.0) {
            assert!(
                subtract.is_some(),
                "Core Graphics path subtraction must be available"
            );
        }
        subtract
    }

    fn rect(x: f64, y: f64, width: f64, height: f64) -> BrowserBounds {
        BrowserBounds {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn overlapping_covers_remove_the_union_and_leave_uncovered_pixels_interactive() {
        let Some(subtract) = clipping() else {
            return;
        };
        let layout = BrowserLayout {
            bounds: rect(300., 100., 300., 200.),
            frame_clip: None,
            covers: vec![
                BrowserClip {
                    bounds: rect(320., 115., 120., 90.),
                    radius: 0.,
                    visual_only: false,
                },
                BrowserClip {
                    bounds: rect(390., 160., 120., 100.),
                    radius: 16.,
                    visual_only: false,
                },
            ],
        };
        let path = visible_path(&layout, subtract, true);
        assert!(path.contains(NSPoint::new(3., 20.)));
        assert!(!path.contains(NSPoint::new(100., 130.)));
        assert!(path.contains(NSPoint::new(209., 139.)));
        assert!(!path.contains(NSPoint::new(180., 80.)));
        assert!(path.contains(NSPoint::new(290., 50.)));
    }

    #[test]
    fn floating_browser_keeps_the_bottom_edge_apart_from_corners_and_resize_grip() {
        let Some(subtract) = clipping() else {
            return;
        };
        let layout = BrowserLayout {
            bounds: rect(100., 176., 380., 224.),
            frame_clip: Some(BrowserClip {
                bounds: rect(100., 100., 380., 300.),
                radius: 28.,
                visual_only: false,
            }),
            covers: Vec::new(),
        };
        let path = visible_path(&layout, subtract, true);
        assert!(path.contains(NSPoint::new(100., 3.)));
        assert!(!path.contains(NSPoint::new(2., 2.)));
        assert!(!path.contains(NSPoint::new(363., 15.)));
        assert!(path.contains(NSPoint::new(365., 30.)));
    }

    #[test]
    fn complete_coverage_passes_input_back_to_the_canvas() {
        let Some(subtract) = clipping() else {
            return;
        };
        let bounds = rect(300., 100., 300., 200.);
        let layout = BrowserLayout {
            bounds,
            frame_clip: None,
            covers: vec![BrowserClip {
                bounds,
                radius: 0.,
                visual_only: false,
            }],
        };
        assert!(matches!(
            visible_path(&layout, subtract, true),
            Visibility::Covered
        ));
    }

    #[test]
    fn a_tooltip_clips_the_paint_without_consuming_the_webview_hit() {
        let Some(subtract) = clipping() else {
            return;
        };
        let layout = BrowserLayout {
            bounds: rect(300., 76., 310., 250.),
            frame_clip: None,
            covers: vec![BrowserClip {
                bounds: rect(344., 79., 80., 26.),
                radius: 8.,
                visual_only: true,
            }],
        };
        let point = NSPoint::new(84., 234.);
        assert!(!visible_path(&layout, subtract, true).contains(point));
        assert!(visible_path(&layout, subtract, false).contains(point));
    }
}
