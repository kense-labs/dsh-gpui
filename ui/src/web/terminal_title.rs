//! Inline naming for docked and floating terminal titles.

use super::*;
use gpui::{AnyElement, ClickEvent, EntityInputHandler, MouseDownEvent};

const TITLE_LIMIT: u32 = 120;

pub(super) struct TerminalTitleEdit {
    pane: DockPaneId,
    key: u64,
    editor: gpui::Entity<Editor>,
    restore_input: js_sys::Function,
    _reconcile_input: Closure<dyn FnMut(u32, u32, String)>,
    _subscription: Subscription,
}

impl Drop for TerminalTitleEdit {
    fn drop(&mut self) {
        let _ = self.restore_input.call0(&JsValue::UNDEFINED);
    }
}

impl DshApp {
    pub(super) fn terminal_title_has_focus(&self, w: &Window, cx: &App) -> bool {
        self.terminal_title_edit
            .as_ref()
            .is_some_and(|edit| edit.editor.read(cx).focus.is_focused(w))
    }

    pub(super) fn begin_terminal_title(
        &mut self,
        pane: DockPaneId,
        tab: RightPanelTab,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let RightPanelTab::Terminal(key) = tab else {
            return false;
        };
        if !self
            .sidebar
            .surface
            .layout
            .pane(pane)
            .is_some_and(|pane| pane.tabs().contains(&tab))
        {
            return false;
        }
        if self
            .terminal_title_edit
            .as_ref()
            .is_some_and(|edit| (edit.pane, edit.key) == (pane, key))
        {
            return true;
        }
        self.finish_terminal_title(true, w, cx);
        self.dismiss_dock_tab_menu(false, w, cx);
        self.dismiss_terminal_shell_menu(false, w, cx);
        self.focus_dock_pane(pane, w, cx);
        let title = self.dock_tab_label(tab);
        let input = editor("终端名称".into(), InputMode::Single, w, cx);
        let subscription = cx.subscribe_in(&input, w, |this, sender, event, w, cx| {
            if !this
                .terminal_title_edit
                .as_ref()
                .is_some_and(|edit| edit.editor == *sender)
            {
                return;
            }
            match event {
                EditEvent::Escape => this.finish_terminal_title(false, w, cx),
                EditEvent::Submitted | EditEvent::Blurred => {
                    this.finish_terminal_title(true, w, cx)
                }
                _ => {}
            }
        });
        input.update(cx, |editor, cx| {
            editor.set_placeholder("");
            editor.set_value(title.to_string(), w, cx);
            editor.focus(w, cx);
            editor.select_all(w, cx);
        });
        let target = input.downgrade();
        let mut window = w.to_async(cx);
        let reconcile_input = Closure::<dyn FnMut(u32, u32, String)>::new(
            move |start: u32, end: u32, text: String| {
                let _ = target.update_in(&mut window, |editor, w, cx| {
                    if editor.focus.is_focused(w)
                        && !editor.is_composing()
                        && editor.value() != text
                    {
                        let end_of_value = editor.value().encode_utf16().count();
                        editor.replace_text_in_range(Some(0..end_of_value), &text, w, cx);
                        editor.set_selected_text_range(start as usize..end as usize, w, cx);
                    }
                });
            },
        );
        let restore_input =
            bridge::limit_terminal_title(TITLE_LIMIT, reconcile_input.as_ref().unchecked_ref());
        self.terminal_title_edit = Some(TerminalTitleEdit {
            pane,
            key,
            editor: input,
            restore_input,
            _reconcile_input: reconcile_input,
            _subscription: subscription,
        });
        cx.notify();
        true
    }

    pub(super) fn finish_terminal_title(
        &mut self,
        commit: bool,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(edit) = self.terminal_title_edit.take() else {
            return;
        };
        if edit.editor.read(cx).focus.is_focused(w) {
            w.blur(cx);
        }
        cx.notify();
        if !commit {
            return;
        }
        let value = edit.editor.read(cx).value();
        // The browser enforces maxlength for normal edits. Bound any text an IME
        // committed directly to the GPUI input handler before comparing names.
        let mut units = 0;
        let limit = value
            .char_indices()
            .find_map(|(index, ch)| {
                units += ch.len_utf16() as u32;
                (units > TITLE_LIMIT).then_some(index)
            })
            .unwrap_or(value.len());
        let title = js_sys::JsString::from(&value[..limit])
            .trim()
            .as_string()
            .unwrap_or_default();
        let tab = RightPanelTab::Terminal(edit.key);
        if title.is_empty() || self.dock_tab_label(tab).as_ref() == title {
            return;
        }
        let (Some(owner), Some(checkpoint)) = (
            self.active_session.clone(),
            self.sidebar.surface.terminals.get(&edit.key),
        ) else {
            return;
        };
        let id = checkpoint.id.clone();
        let key = edit.key;
        cx.spawn_in(w, async move |this, cx| {
            let result = bridge::invoke(
                "rename_terminal",
                json!({
                    "terminalId": id, "session": owner, "title": title,
                }),
            )
            .await;
            if let Err(error) = result {
                let _ = this.update_in(cx, |app, _, cx| {
                    let Some(tab) = app
                        .sidebar_for_session(&owner)
                        .and_then(|sidebar| sidebar.terminals.get_mut(&key))
                        .filter(|tab| tab.id == id)
                    else {
                        return;
                    };
                    tab.screen.update(cx, |screen, cx| screen.unbind(cx));
                    tab.connection = if error == "终端不存在" {
                        TerminalConnection::Missing
                    } else {
                        TerminalConnection::Failed {
                            error,
                            retry_create: false,
                            has_screen: true,
                        }
                    };
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn cancel_closed_terminal_title(&mut self, key: u64) {
        if self
            .terminal_title_edit
            .as_ref()
            .is_some_and(|edit| edit.key == key)
        {
            self.terminal_title_edit = None;
        }
    }

    pub(super) fn update_terminal_title(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if let Some(edit) = &self.terminal_title_edit
            && (!self.dock_pane_on_screen(edit.pane, cx)
                || !self
                    .sidebar
                    .surface
                    .layout
                    .pane(edit.pane)
                    .is_some_and(|pane| pane.tabs().contains(&RightPanelTab::Terminal(edit.key))))
        {
            self.finish_terminal_title(true, w, cx);
        }
    }

    pub(super) fn render_terminal_title_editor(
        &self,
        pane: DockPaneId,
        tab: RightPanelTab,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let edit = self.terminal_title_edit.as_ref()?;
        if edit.pane != pane || tab != RightPanelTab::Terminal(edit.key) {
            return None;
        }
        let p = dsh(cx);
        Some(
            div()
                .id("terminal-title-edit")
                .w(px(120.))
                .min_w(px(48.))
                .max_w_full()
                .h(px(19.))
                .px(px(4.))
                .rounded(px(8.))
                .border(px(0.5))
                .border_color(p.border_l3)
                .bg(p.bg_layer_1)
                .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, w, cx| {
                    this.finish_terminal_title(true, w, cx);
                }))
                .on_click(|_: &ClickEvent, _, cx| cx.stop_propagation())
                .on_key_down(|_, _, cx| cx.stop_propagation())
                .child(edit.editor.clone())
                .into_any_element(),
        )
    }
}
