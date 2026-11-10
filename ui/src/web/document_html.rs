//! Layout of isolated HTML content inside GPUI document panes.

use super::{menu_surface::MenuCover, *};
use crate::right_dock::FloatRect;
use gpui::{Bounds, Canvas, Pixels, canvas};
use serde::Serialize;

#[wasm_bindgen(raw_module = "../document-html.js")]
extern "C" {
    #[wasm_bindgen(js_name = prepareStaticHtml)]
    pub(super) fn prepare_static_html(source: &str) -> js_sys::Promise;

    #[wasm_bindgen(js_name = htmlAssetReferences)]
    pub(super) fn asset_references(source: &str) -> JsValue;

    #[wasm_bindgen(js_name = beginHtmlFrames)]
    pub(super) fn begin_frames();

    #[wasm_bindgen(js_name = listenHtmlFocus)]
    pub(super) fn listen_focus(listener: &js_sys::Function);

    #[wasm_bindgen(js_name = republishHtmlFocus)]
    pub(super) fn republish_focus();

    #[wasm_bindgen(js_name = htmlFocusMatches)]
    fn focus_matches(key: &str, revision: u32) -> bool;

    #[wasm_bindgen(js_name = htmlOwnsFocus)]
    pub(super) fn owns_focus() -> bool;

    #[wasm_bindgen(js_name = restoreHtmlShellFocus)]
    pub(super) fn restore_shell_focus();

    #[wasm_bindgen(js_name = stageHtmlFrame)]
    fn stage_frame(key: &str, content: &JsValue, layout: JsValue);

    #[wasm_bindgen(js_name = coverHtmlFrame)]
    fn cover_frame(key: &str, bounds: JsValue);

    #[wasm_bindgen(js_name = coverAllHtmlFrames)]
    fn cover_all_frames(cover: JsValue);
}

type Rect = [f32; 4];

#[derive(Serialize)]
struct FrameCover {
    bounds: Rect,
    radius: f32,
    shadow: bool,
    visual_only: bool,
}

impl FrameCover {
    fn floating(rect: FloatRect, radius: f32, shadow: bool) -> Self {
        Self {
            bounds: [rect.x, rect.y, rect.x + rect.width, rect.y + rect.height],
            radius,
            shadow,
            visual_only: false,
        }
    }
}

impl From<MenuCover> for FrameCover {
    fn from(cover: MenuCover) -> Self {
        Self {
            visual_only: cover.visual_only,
            ..Self::floating(cover.rect, cover.radius, false)
        }
    }
}

pub(super) fn cover_all(cover: MenuCover) {
    if let Ok(cover) = serde_wasm_bindgen::to_value(&FrameCover::from(cover)) {
        cover_all_frames(cover);
    }
}

#[derive(Serialize)]
struct FrameLayout<'a> {
    bounds: Rect,
    viewport: Rect,
    covers: &'a [FrameCover],
    radius: f32,
    background: &'static str,
    hidden: bool,
    inert: bool,
}

fn bounds_rect(bounds: Bounds<Pixels>) -> Rect {
    [
        bounds.left().into(),
        bounds.top().into(),
        bounds.right().into(),
        bounds.bottom().into(),
    ]
}

pub(super) fn cover(key: u64, radius: f32) -> Canvas<()> {
    let key = key.to_string();
    canvas(
        move |bounds, _, _| {
            if let Ok(rect) = serde_wasm_bindgen::to_value(&FrameCover {
                bounds: bounds_rect(bounds),
                radius,
                shadow: true,
                visual_only: false,
            }) {
                cover_frame(&key, rect);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

impl DshApp {
    fn html_preview_pane(&self, key: u64, cx: &App) -> Option<DockPaneId> {
        if self.sidebar_drawer_covers_window()
            || !self
                .sidebar
                .documents
                .get(&key)
                .is_some_and(|tab| tab.shows_html())
        {
            return None;
        }
        self.sidebar
            .surface
            .layout
            .all_panes()
            .find(|pane| {
                pane.active() == RightPanelTab::Document(key)
                    && self.dock_pane_on_screen(pane.id, cx)
            })
            .map(|pane| pane.id)
    }

    pub(super) fn html_preview_focus_changed(
        &mut self,
        key: Option<u64>,
        revision: u32,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = key.and_then(|key| self.html_preview_pane(key, cx).map(|pane| (key, pane)));
        if let Some(shell) = self.browser_shell
            && bridge::is_mac()
        {
            let key = target.map(|(key, _)| key);
            wasm_bindgen_futures::spawn_local(async move {
                let _ = bridge::invoke(
                    "set_html_preview_focus",
                    json!({"shell":shell,"revision":revision,"key":key}),
                )
                .await;
            });
        }
        let Some((_, pane)) = target else {
            return;
        };
        self.dismiss_header_menu_for_guest(cx);
        self.dismiss_stats_dialog(cx);
        self.dismiss_dock_tab_menu(false, w, cx);
        if self.sidebar.surface.layout.focused() != pane {
            // The sandbox keeps the browser's own focus and selection.
            self.sidebar.surface.layout.focus(pane);
            cx.notify();
        }
    }

    pub(super) fn handle_html_preview_input(
        &mut self,
        event: &Value,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.browser_shell.is_none()
            || self.browser_shell != event["shell"].as_u64()
            || !self.shortcuts.accepted_event(event)
        {
            return;
        }
        let Some(key) = event["key"].as_u64() else {
            return;
        };
        let Some(revision) = event["revision"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
        else {
            return;
        };
        if !focus_matches(&key.to_string(), revision) {
            return;
        }
        let Some(pane) = self.html_preview_pane(key, cx) else {
            return;
        };
        if let Ok(command) = serde_json::from_value(event["command"].clone()) {
            self.run_application_shortcut(
                command,
                Some((pane, RightPanelTab::Document(key))),
                w,
                cx,
            );
        }
    }

    pub(super) fn html_document_frame(
        &self,
        pane: DockPaneId,
        key: u64,
        content: JsValue,
        viewer_open: bool,
        w: &Window,
        cx: &App,
    ) -> Canvas<()> {
        let floats = self.float_layers(w);
        let first_above = floats
            .iter()
            .position(|(float, _)| float.pane.id == pane)
            .map_or(0, |index| index + 1);
        let covers: Vec<_> = floats[first_above..]
            .iter()
            .map(|(_, rect)| FrameCover::floating(*rect, super::floating_panes::FLOAT_RADIUS, true))
            .chain(self.dock_tab_menus.covers(w).map(|cover| FrameCover {
                shadow: true,
                ..cover.into()
            }))
            .chain(self.terminal_shell_menu_rect(w, cx).map(|rect| {
                FrameCover::floating(rect, super::terminal_shell_menu::MENU_RADIUS, true)
            }))
            .chain(
                self.dock_controls
                    .hints(&self.shortcuts, w, cx)
                    .into_iter()
                    .filter(|hint| hint.opacity > 0.)
                    .map(|hint| {
                        FrameCover::floating(hint.bounds, super::dock_controls::HINT_RADIUS, false)
                    }),
            )
            .collect();
        let hidden = self.sidebar_drawer_covers_window()
            || self.session_downloads.visible()
            || self.session_feedback.visible()
            || self.show_settings
            || self.shortcuts.open
            || self.confirm_permission.is_some()
            || self.drop_active
            || self.workspace_rename.is_some()
            || self.workspace_delete.is_some()
            || self.session_rename.is_some()
            || self.dock_drop_hint_pane() == Some(pane);
        let inert = viewer_open || self.terminal_shell_menu_rect(w, cx).is_some();
        let radius = if pane.is_floating() {
            super::floating_panes::FLOAT_RADIUS
        } else {
            0.
        };
        let background = if super::workspace::dark(cx) {
            "#151517"
        } else {
            "#ffffff"
        };
        let key = key.to_string();
        canvas(
            move |bounds, w, _| {
                let layout = FrameLayout {
                    bounds: bounds_rect(bounds),
                    viewport: bounds_rect(w.content_mask().bounds),
                    covers: &covers,
                    radius,
                    background,
                    hidden,
                    inert,
                };
                if let Ok(layout) = serde_wasm_bindgen::to_value(&layout) {
                    stage_frame(&key, &content, layout);
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }
}
