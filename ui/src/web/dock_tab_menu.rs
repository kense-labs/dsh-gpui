//! Context menu for an occurrence in the docked tab strip.

use super::menu_surface::{MenuCover, menu_elevation, menu_material};
use super::*;
use crate::anim::{
    CONTEXT_MENU_PREPARE_MS as PREPARE_MS, CONTEXT_MENU_REPLACE_MS as REPLACE_MS,
    context_menu_enter_opacity, context_menu_exit_opacity,
};
use crate::right_dock::{
    FloatRect,
    menu::{TabMenuAction, TabMenuTargets, tab_menu_rect},
};
use gpui::{
    DispatchPhase, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point, canvas,
};

const ENTER_MS: f64 = crate::anim::MENU_ENTER.as_millis() as f64;
const DISMISS_MS: f64 = crate::anim::MENU_EXIT.as_millis() as f64;
const MENU_RADIUS: f32 = 16.;

#[derive(Default)]
pub(super) struct DockTabMenus {
    current: Option<TabMenu>,
    exiting: Option<Exiting>,
}

struct TabMenu {
    pane: DockPaneId,
    tab: RightPanelTab,
    point: Point<Pixels>,
    targets: TabMenuTargets,
    rows: [FocusHandle; 3],
    requested_at: f64,
    entered_at: Option<f64>,
}

struct Exiting {
    menu: TabMenu,
    start: f64,
    opacity: f32,
    duration: f64,
}

impl TabMenu {
    fn rect(&self, w: &Window) -> FloatRect {
        let point = (self.point.x.into(), self.point.y.into());
        let viewport = w.viewport_size();
        let rows = TabMenuAction::ALL
            .iter()
            .filter(|&&action| self.targets.visible(action))
            .count();
        tab_menu_rect(point, (viewport.width.into(), viewport.height.into()), rows)
    }

    fn opacity(&self, now: f64, reduce_motion: bool) -> f32 {
        self.entered_at.map_or(0., |start| {
            if reduce_motion {
                1.
            } else {
                context_menu_enter_opacity(now - start)
            }
        })
    }

    fn enabled_rows(&self) -> impl DoubleEndedIterator<Item = (TabMenuAction, &FocusHandle)> {
        TabMenuAction::ALL
            .into_iter()
            .zip(&self.rows)
            .filter(|(action, _)| self.targets.enabled(*action))
    }
}

impl DockTabMenus {
    pub(super) fn is_open(&self) -> bool {
        self.current.is_some()
    }

    pub(super) fn covers(&self, w: &Window) -> impl Iterator<Item = MenuCover> {
        self.current
            .iter()
            .filter(|menu| menu.entered_at.is_some())
            .map(|menu| (menu, false))
            .chain(self.exiting.iter().map(|exit| (&exit.menu, true)))
            .map(|(menu, visual_only)| MenuCover {
                rect: menu.rect(w),
                radius: MENU_RADIUS,
                visual_only,
            })
    }

    fn exit(
        &mut self,
        now: f64,
        duration: f64,
        reduce_motion: bool,
    ) -> Option<(DockPaneId, RightPanelTab)> {
        let menu = self.current.take()?;
        let anchor = (menu.pane, menu.tab);
        let opacity = menu.opacity(now, reduce_motion);
        if opacity > 0. && !reduce_motion {
            self.exiting = Some(Exiting {
                menu,
                start: now,
                opacity,
                duration,
            });
        }
        Some(anchor)
    }
}

impl DshApp {
    pub(super) fn open_dock_tab_menu(
        &mut self,
        pane: DockPaneId,
        tab: RightPanelTab,
        point: Point<Pixels>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        w.prevent_default();
        cx.stop_propagation();
        if !self.dock_pane_on_screen(pane, cx) {
            return;
        }
        self.dismiss_terminal_shell_menu(false, w, cx);
        let same_anchor = self.dock_tab_menus.current.as_ref().is_some_and(|menu| {
            menu.pane == pane && menu.tab == tab && (menu.point - point).magnitude() <= 4.
        });
        if same_anchor {
            self.dismiss_dock_tab_menu(true, w, cx);
            return;
        }
        let now = bridge::monotonic_ms();
        self.dock_tab_menus
            .exit(now, REPLACE_MS, cx.reduce_motion());
        if let Some(targets) = self.sidebar.surface.layout.tab_menu(pane, tab) {
            self.workspace_context_menu = None;
            self.replace_header_menu(cx);
            self.show_workspace_menu = false;
            self.show_workspace_view_options = false;
            self.show_more_menu = false;
            self.composer_menu = None;
            self.focus_dock_chip(pane, tab, w, cx);
            self.dock_tab_menus.current = Some(TabMenu {
                pane,
                tab,
                point,
                targets,
                rows: std::array::from_fn(|_| cx.focus_handle().tab_stop(false)),
                requested_at: now,
                entered_at: None,
            });
        }
        cx.notify();
    }

    pub(super) fn update_dock_tab_menu(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if !self.floats_on_screen(cx) {
            self.dock_tab_menus = DockTabMenus::default();
            return;
        }
        let now = bridge::monotonic_ms();
        let dock_open = self.right_panel_visible() && self.right_dock_frame.visible;
        let menu = &mut self.dock_tab_menus;
        if menu
            .exiting
            .as_ref()
            .is_some_and(|exit| now - exit.start >= exit.duration || cx.reduce_motion())
        {
            menu.exiting = None;
        }
        if menu.current.as_ref().is_some_and(|current| {
            !dock_open
                || self
                    .sidebar
                    .surface
                    .layout
                    .tab_menu(current.pane, current.tab)
                    .is_none()
        }) {
            menu.exit(now, DISMISS_MS, cx.reduce_motion());
        }
        if let Some(current) = menu.current.as_mut() {
            if current.entered_at.is_none() && now - current.requested_at >= PREPARE_MS {
                current.entered_at = Some(now);
                if let Some((_, focus)) = current.enabled_rows().next() {
                    focus.focus(w, cx);
                }
            }
            if current
                .entered_at
                .is_none_or(|start| !cx.reduce_motion() && now - start < ENTER_MS)
            {
                w.request_animation_frame();
            }
        }
        if menu.exiting.is_some() {
            w.request_animation_frame();
        }
    }

    pub(super) fn dismiss_dock_tab_menu(
        &mut self,
        restore_focus: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((pane, tab)) =
            self.dock_tab_menus
                .exit(bridge::monotonic_ms(), DISMISS_MS, cx.reduce_motion())
        else {
            return false;
        };
        if restore_focus && self.dock_pane_on_screen(pane, cx) {
            self.focus_dock_chip(pane, tab, w, cx);
        }
        cx.notify();
        true
    }

    pub(super) fn dock_menu_pointer_watch(&self, cx: &Context<Self>) -> Option<gpui::Canvas<()>> {
        self.dock_tab_menus.current.as_ref()?;
        let view = cx.weak_entity();
        Some(
            canvas(
                |_, _, _| {},
                move |_, _, w, _| {
                    w.on_mouse_event(move |event: &MouseDownEvent, phase, w, cx| {
                        if phase == DispatchPhase::Capture
                            && let Some(view) = view.upgrade()
                        {
                            view.update(cx, |this, cx| this.dock_menu_pointer_down(event, w, cx));
                        }
                    });
                },
            )
            .absolute()
            .inset_0(),
        )
    }

    fn dock_menu_pointer_down(
        &mut self,
        event: &MouseDownEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.dock_tab_menus.current.as_ref() else {
            return;
        };
        let rect = menu.rect(w);
        let x = f32::from(event.position.x);
        let y = f32::from(event.position.y);
        let outside = menu.entered_at.is_none()
            || !(rect.x..=rect.x + rect.width).contains(&x)
            || !(rect.y..=rect.y + rect.height).contains(&y);
        let context = event.button == MouseButton::Right
            || (event.button == MouseButton::Left && event.modifiers.control && bridge::is_mac());
        if context && outside {
            // The bubble phase can replace this menu from a different tab trigger.
            let request = menu.requested_at;
            cx.defer_in(w, move |this, w, cx| {
                if this
                    .dock_tab_menus
                    .current
                    .as_ref()
                    .is_some_and(|menu| menu.requested_at == request)
                {
                    this.dismiss_dock_tab_menu(false, w, cx);
                }
            });
        } else if !context && outside {
            self.dismiss_dock_tab_menu(false, w, cx);
        }
    }

    fn select_dock_menu_action(
        &mut self,
        action: TabMenuAction,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self
            .dock_tab_menus
            .current
            .as_ref()
            .filter(|menu| menu.entered_at.is_some() && menu.targets.enabled(action))
        else {
            return;
        };
        let pane = menu.pane;
        let tab = menu.tab;
        let close = self
            .sidebar
            .surface
            .layout
            .tab_menu(pane, tab)
            .map(|current| menu.targets.closing(action, &current))
            .unwrap_or_default();
        let anchor_closes = close.contains(&tab);
        self.dismiss_dock_tab_menu(!anchor_closes, w, cx);
        if anchor_closes {
            self.close_right_tab(pane, tab, w, cx);
        } else {
            for tab in close {
                self.remove_right_tab(pane, tab, cx);
            }
        }
    }

    fn dock_menu_key(&mut self, event: &KeyDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.dock_tab_menus.current.as_ref() else {
            return;
        };
        let stroke = &event.keystroke;
        if stroke.modifiers.alt || stroke.modifiers.control || stroke.modifiers.platform {
            return;
        }
        let enabled: Vec<_> = menu.enabled_rows().collect();
        let Some(index) = enabled.iter().position(|(_, focus)| focus.is_focused(w)) else {
            return;
        };
        match stroke.key.as_str() {
            "tab" if stroke.modifiers.shift => {
                self.dismiss_dock_tab_menu(true, w, cx);
            }
            "escape" if !stroke.modifiers.shift => {
                self.dismiss_dock_tab_menu(true, w, cx);
            }
            "tab" | "enter" | "space" => {
                self.select_dock_menu_action(enabled[index].0, w, cx);
            }
            "down" => enabled[(index + 1) % enabled.len()].1.focus(w, cx),
            "up" => enabled[(index + enabled.len() - 1) % enabled.len()]
                .1
                .focus(w, cx),
            "home" => enabled[0].1.focus(w, cx),
            "end" => enabled[enabled.len() - 1].1.focus(w, cx),
            _ => return,
        }
        w.prevent_default();
        cx.stop_propagation();
    }

    pub(super) fn render_dock_tab_menus(
        &self,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let now = bridge::monotonic_ms();
        let mut layers = Vec::with_capacity(2);
        if let Some(exit) = self.dock_tab_menus.exiting.as_ref() {
            let opacity = exit.opacity * context_menu_exit_opacity(now - exit.start, exit.duration);
            layers.push(self.render_dock_tab_menu(&exit.menu, opacity, false, w, cx));
        }
        if let Some(menu) = self
            .dock_tab_menus
            .current
            .as_ref()
            .filter(|menu| menu.entered_at.is_some())
        {
            layers.push(self.render_dock_tab_menu(
                menu,
                menu.opacity(now, cx.reduce_motion()),
                true,
                w,
                cx,
            ));
        }
        layers
    }

    fn render_dock_tab_menu(
        &self,
        menu: &TabMenu,
        opacity: f32,
        interactive: bool,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let rect = menu.rect(w);
        let p = dsh(cx);
        column()
            .id(if interactive {
                "dock-tab-menu"
            } else {
                "dock-tab-menu-exit"
            })
            .absolute()
            .left(px(rect.x))
            .top(px(rect.y))
            .w(px(rect.width))
            .h(px(rect.height))
            .gap_0()
            .p(px(4.))
            .rounded(px(MENU_RADIUS))
            .child(menu_material(p, MENU_RADIUS))
            .shadow(menu_elevation(p))
            .opacity(opacity)
            .overflow_hidden()
            .when(interactive, |card| {
                card.role(gpui::Role::Menu)
                    .occlude()
                    .on_key_down(cx.listener(|this, event, w, cx| this.dock_menu_key(event, w, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _, w, cx| {
                            this.dismiss_dock_tab_menu(true, w, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, w, cx| {
                            if event.modifiers.control && bridge::is_mac() {
                                this.dismiss_dock_tab_menu(true, w, cx);
                                cx.stop_propagation();
                            }
                        }),
                    )
            })
            .child(
                column()
                    .id("dock-tab-menu-rows")
                    .gap_0()
                    .min_h_0()
                    .w_full()
                    .flex_1()
                    .overflow_y_scroll()
                    .children(
                        TabMenuAction::ALL
                            .into_iter()
                            .zip(&menu.rows)
                            .filter(|(action, _)| menu.targets.visible(*action))
                            .map(|(action, focus)| {
                                self.render_dock_menu_row(
                                    action,
                                    focus,
                                    interactive,
                                    menu.targets.enabled(action),
                                    cx,
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_dock_menu_row(
        &self,
        action: TabMenuAction,
        focus: &FocusHandle,
        interactive: bool,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let p = dsh(cx);
        let (id, label) = match action {
            TabMenuAction::Close => ("close", "关闭"),
            TabMenuAction::Others => ("others", "关闭其他标签页"),
            TabMenuAction::Right => ("right", "关闭右侧标签页"),
        };
        row()
            .id(id)
            .h(px(34.))
            .w_full()
            .flex_none()
            .gap(px(6.))
            .px(px(8.))
            .rounded(px(12.))
            .text_size(px(13.))
            .line_height(px(20.))
            .text_color(p.ink)
            .when(interactive, |row| {
                row.role(gpui::Role::MenuItem).aria_label(label)
            })
            .when(!enabled, |row| {
                row.opacity(0.4)
                    .cursor(gpui::CursorStyle::OperationNotAllowed)
            })
            .when(interactive && enabled, |row| {
                row.track_focus(focus)
                    .cursor_pointer()
                    .hover(move |style| style.bg(p.hover))
                    .focus_visible(move |style| style.bg(p.hover))
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, w, cx| {
                        if !(bridge::is_mac() && event.modifiers().control) {
                            this.select_dock_menu_action(action, w, cx);
                        }
                    }))
            })
            .child(
                div()
                    .size(px(14.))
                    .flex_none()
                    .when(action == TabMenuAction::Close, |icon_slot| {
                        icon_slot.child(icon(Glyph::Close, 14., p.secondary))
                    }),
            )
            .child(div().min_w_0().flex_1().truncate().child(label))
            .when(action == TabMenuAction::Close, |row| {
                let (shortcut, aria) = if bridge::is_mac() {
                    ("⌘ W", "Meta+W")
                } else {
                    ("Ctrl W", "Control+W")
                };
                row.aria_keyshortcuts(aria).child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(p.label_caption)
                        .child(shortcut),
                )
            })
    }
}
