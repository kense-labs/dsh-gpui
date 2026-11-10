//! Captured tab menu actions and cursor placement.

use super::{DockPaneId, FloatRect, RightDock, RightPanelTab};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabMenuAction {
    Close,
    Others,
    Right,
}

impl TabMenuAction {
    pub(crate) const ALL: [Self; 3] = [Self::Close, Self::Others, Self::Right];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabMenuTargets {
    close: Option<RightPanelTab>,
    others: Vec<RightPanelTab>,
    right: Vec<RightPanelTab>,
}

impl TabMenuTargets {
    fn targets(&self, action: TabMenuAction) -> &[RightPanelTab] {
        match action {
            TabMenuAction::Close => self.close.as_slice(),
            TabMenuAction::Others => &self.others,
            TabMenuAction::Right => &self.right,
        }
    }

    pub(crate) fn visible(&self, action: TabMenuAction) -> bool {
        action != TabMenuAction::Right || self.enabled(action)
    }

    pub(crate) fn enabled(&self, action: TabMenuAction) -> bool {
        !self.targets(action).is_empty()
    }

    pub(crate) fn closing(&self, action: TabMenuAction, current: &Self) -> Vec<RightPanelTab> {
        let eligible = match action {
            TabMenuAction::Close => current.close.as_slice(),
            TabMenuAction::Others | TabMenuAction::Right => &current.others,
        };
        self.targets(action)
            .iter()
            .copied()
            .filter(|tab| eligible.contains(tab))
            .collect()
    }
}

impl RightDock {
    pub(crate) fn tab_menu(
        &self,
        pane: DockPaneId,
        anchor: RightPanelTab,
    ) -> Option<TabMenuTargets> {
        let tabs = self.pane(pane)?.tabs();
        let index = tabs.iter().position(|tab| *tab == anchor)?;
        let closeable = |tab: &RightPanelTab| self.can_close(pane, *tab);
        let close = closeable(&anchor).then_some(anchor);
        let others: Vec<_> = tabs
            .iter()
            .copied()
            .filter(|tab| *tab != anchor && closeable(tab))
            .collect();
        if close.is_none() && others.is_empty() {
            return None;
        }
        Some(TabMenuTargets {
            close,
            others,
            right: tabs[index + 1..]
                .iter()
                .copied()
                .filter(closeable)
                .collect(),
        })
    }
}

pub(crate) fn tab_menu_rect(point: (f32, f32), viewport: (f32, f32), rows: usize) -> FloatRect {
    const MARGIN: f32 = 12.;
    const GAP: f32 = 4.;
    let (viewport_width, viewport_height) = viewport;
    let width = 144_f32.min((viewport_width - MARGIN * 2.).max(0.));
    let height = (8. + 34. * rows as f32).min((viewport_height - MARGIN * 2.).max(0.));
    let max_x = (viewport_width - MARGIN - width).max(MARGIN);
    let max_y = (viewport_height - MARGIN - height).max(MARGIN);
    let below = (point.1 + GAP).max(MARGIN);
    let above = point.1 - GAP - height;
    let y = if below <= max_y {
        below
    } else if above >= MARGIN {
        above.min(max_y)
    } else {
        MARGIN
    };
    FloatRect {
        x: point.0.clamp(MARGIN, max_x),
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::right_dock::{DockDrop, DockPaneId::*, RightPanelTab::*};
    use TabMenuAction::*;

    #[test]
    fn only_the_clicked_pane_contributes_menu_actions() {
        let mut dock = RightDock::default();
        assert_eq!(dock.tab_menu(Primary, Guide), None);
        dock.open(Primary, Files);
        let lone = dock.tab_menu(Primary, Files).unwrap();
        assert!(lone.enabled(Close));
        assert!(lone.visible(Others));
        assert!(!lone.enabled(Others));
        assert_eq!(
            TabMenuAction::ALL
                .into_iter()
                .filter(|action| lone.visible(*action))
                .collect::<Vec<_>>(),
            vec![Close, Others]
        );

        dock.open(Primary, Browser(1));
        dock.open(Primary, Guide);
        dock.split(Primary);
        dock.open(Secondary, Terminal(9));
        let menu = dock.tab_menu(Primary, Browser(1)).unwrap();
        assert_eq!(menu.targets(Others), &[Files, Guide]);
        assert_eq!(menu.targets(Right), &[Guide]);
        assert_eq!(menu.targets(Close), &[Browser(1)]);
    }

    #[test]
    fn close_right_uses_captured_ids_and_checks_that_they_still_belong_to_the_pane() {
        let mut dock = RightDock::default();
        dock.open(Primary, Files);
        dock.open(Primary, Browser(1));
        dock.open(Primary, Browser(2));
        let menu = dock.tab_menu(Primary, Files).unwrap();
        dock.split(Primary);
        dock.drop_tab(Primary, Browser(1), DockDrop::Center(Secondary));
        dock.drop_tab(
            Primary,
            Browser(2),
            DockDrop::Strip {
                pane: Primary,
                slot: 0,
            },
        );
        dock.open(Primary, Terminal(3));
        let current = dock.tab_menu(Primary, Files).unwrap();
        assert_eq!(menu.closing(Right, &current), vec![Browser(2)]);
        dock.close(Primary, Files);
        assert_eq!(dock.tab_menu(Primary, Files), None);
    }

    #[test]
    fn menu_stays_in_the_viewport_and_flips_above_the_pointer() {
        assert_eq!(
            tab_menu_rect((100., 40.), (700., 500.), 2),
            FloatRect {
                x: 100.,
                y: 44.,
                width: 144.,
                height: 76.
            }
        );
        assert_eq!(
            tab_menu_rect((690., 480.), (700., 500.), 3),
            FloatRect {
                x: 544.,
                y: 366.,
                width: 144.,
                height: 110.
            }
        );
    }
}
