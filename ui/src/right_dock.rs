use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

mod floating;
pub(crate) mod menu;
pub(crate) mod strip;
pub(crate) use floating::FloatRect;
#[cfg(target_family = "wasm")]
pub(crate) use floating::{FloatViewport, SpringAxis};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub(crate) enum RightPanelTab {
    Guide,
    Files,
    Terminal(u64),
    Browser(u64),
    Document(u64),
    Plan(u64),
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum DockPaneId {
    Primary,
    Secondary,
    Floating(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DockSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DockDrop {
    Strip { pane: DockPaneId, slot: usize },
    Center(DockPaneId),
    Split { pane: DockPaneId, side: DockSide },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DockDropResult {
    Reordered,
    Focus(DockPaneId),
}

pub(crate) fn drag_threshold(delta_x: f32, delta_y: f32) -> bool {
    delta_x.abs() >= 4. || delta_y.abs() >= 4.
}

pub(crate) fn strip_drop_slot(
    midpoints: impl Iterator<Item = f32>,
    x: f32,
    source_index: Option<usize>,
) -> Option<usize> {
    let slot = midpoints.take_while(|midpoint| x >= *midpoint).count();
    (!source_index.is_some_and(|from| slot == from || slot == from + 1)).then_some(slot)
}

impl DockPaneId {
    pub(crate) fn is_floating(self) -> bool {
        matches!(self, Self::Floating(_))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DockPane {
    pub(crate) id: DockPaneId,
    tabs: Vec<RightPanelTab>,
    active: RightPanelTab,
}

impl DockPane {
    fn guide(id: DockPaneId) -> Self {
        Self {
            id,
            tabs: vec![RightPanelTab::Guide],
            active: RightPanelTab::Guide,
        }
    }

    pub(crate) fn tabs(&self) -> &[RightPanelTab] {
        &self.tabs
    }

    pub(crate) fn active(&self) -> RightPanelTab {
        self.active
    }

    fn remove(&mut self, tab: RightPanelTab) {
        let Some(index) = self.tabs.iter().position(|candidate| *candidate == tab) else {
            return;
        };
        self.tabs.remove(index);
        if self.active == tab {
            self.active = self
                .tabs
                .get(index.saturating_sub(1))
                .copied()
                .unwrap_or(RightPanelTab::Guide);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct RightDock {
    panes: Vec<DockPane>,
    #[serde(default)]
    floats: Vec<FloatPane>,
    focused: DockPaneId,
    split_fraction: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct FloatPane {
    pub(crate) pane: DockPane,
    pub(crate) rect: FloatRect,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DockSurface {
    pub(crate) layout: RightDock,
    pub(crate) open: bool,
    pub(crate) fullscreen: bool,
    pub(crate) browsers: BTreeMap<u64, BrowserCheckpoint>,
    pub(crate) documents: BTreeMap<u64, String>,
    pub(crate) plans: BTreeMap<u64, String>,
    pub(crate) terminals: BTreeMap<u64, TerminalCheckpoint>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TerminalCheckpoint {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) title: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct BrowserCheckpoint {
    pub(crate) address: String,
    pub(crate) title: String,
}

#[cfg(any(target_family = "wasm", test))]
#[derive(Default)]
pub(crate) struct BrowserObservations {
    pub(crate) navigation: Option<u64>,
    pub(crate) terminated: bool,
    sequence: u64,
}

#[cfg(any(target_family = "wasm", test))]
impl BrowserObservations {
    pub(crate) fn page(&mut self, navigation: u64, sequence: Option<u64>) -> bool {
        if let Some(sequence) = sequence
            && (self.navigation.is_some_and(|last| navigation < last)
                || (self.navigation == Some(navigation)
                    && (self.terminated || sequence <= self.sequence)))
        {
            return false;
        }
        self.navigation = Some(navigation);
        self.sequence = sequence.unwrap_or(0);
        self.terminated = false;
        true
    }

    pub(crate) fn terminate(&mut self, navigation: u64) -> bool {
        if self.navigation.is_some_and(|last| navigation < last)
            || (self.navigation == Some(navigation) && self.terminated)
        {
            return false;
        }
        self.navigation = Some(navigation);
        self.terminated = true;
        true
    }
}

#[cfg(test)]
#[test]
fn a_recreated_browser_accepts_its_first_observation_and_ignores_the_old_guest() {
    let mut state = BrowserObservations::default();
    assert!(state.page(4, Some(83)));
    assert!(state.terminate(4));
    assert!(!state.page(4, Some(84)));
    assert!(!state.terminate(4));
    assert!(state.page(5, Some(1)));
    assert!(!state.terminated);
    assert!(!state.terminate(4));
    assert!(!state.page(5, Some(1)));
    assert!(state.page(5, Some(2)));
}

impl DockSurface {
    pub(crate) fn restore(json: &str) -> Self {
        serde_json::from_str::<Value>(json)
            .ok()
            .and_then(|mut data| {
                migrate_single_browser(&mut data);
                serde_json::from_value::<Self>(data).ok()
            })
            .filter(|surface| surface.layout.valid())
            .map(|mut surface| {
                let invalid: Vec<_> = surface
                    .layout
                    .all_panes()
                    .flat_map(|pane| {
                        pane.tabs.iter().filter_map(|tab| match tab {
                            RightPanelTab::Plan(key)
                                if !surface
                                    .plans
                                    .get(key)
                                    .is_some_and(|call_id| !call_id.is_empty()) =>
                            {
                                Some((pane.id, *tab))
                            }
                            RightPanelTab::Document(key)
                                if !surface
                                    .documents
                                    .get(key)
                                    .is_some_and(|path| workspace_document_path(path)) =>
                            {
                                Some((pane.id, *tab))
                            }
                            _ => None,
                        })
                    })
                    .collect();
                for (pane, tab) in invalid {
                    if surface.layout.close(pane, tab) {
                        surface.open = false;
                        surface.fullscreen = false;
                    }
                }
                surface.documents.retain(|key, _| {
                    surface
                        .layout
                        .all_panes()
                        .any(|pane| pane.tabs.contains(&RightPanelTab::Document(*key)))
                });
                surface.plans.retain(|key, _| {
                    surface
                        .layout
                        .all_panes()
                        .any(|pane| pane.tabs.contains(&RightPanelTab::Plan(*key)))
                });
                surface
            })
            .unwrap_or_default()
    }
}

pub(crate) fn workspace_document_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\0')
        && !std::path::Path::new(path).is_absolute()
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn migrate_single_browser(data: &mut Value) {
    let Some(panes) = data
        .pointer_mut("/layout/panes")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let mut migrated = false;
    for pane in panes {
        if let Some(tabs) = pane.get_mut("tabs").and_then(Value::as_array_mut) {
            for tab in tabs {
                if tab.as_str() == Some("Browser") {
                    *tab = json!({"Browser": 1});
                    migrated = true;
                }
            }
        }
        if pane.get("active").and_then(Value::as_str) == Some("Browser") {
            pane["active"] = json!({"Browser": 1});
        }
    }
    if migrated
        && let Some(address) = data
            .get("browser_address")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
    {
        data["browsers"] = json!({"1": {"address": address}});
    }
}

impl Default for RightDock {
    fn default() -> Self {
        Self {
            panes: vec![DockPane::guide(DockPaneId::Primary)],
            floats: Vec::new(),
            focused: DockPaneId::Primary,
            split_fraction: 0.5,
        }
    }
}

impl RightDock {
    pub(crate) fn float_tab(
        &mut self,
        source: DockPaneId,
        tab: RightPanelTab,
        rect: FloatRect,
    ) -> Option<DockPaneId> {
        let index = self
            .panes
            .iter()
            .position(|pane| pane.id == source && pane.tabs.contains(&tab))?;
        if !rect.valid() {
            return None;
        }
        let next = self
            .floats
            .iter()
            .filter_map(|float| match float.pane.id {
                DockPaneId::Floating(id) => Some(id),
                _ => None,
            })
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .filter(|next| *next < u64::MAX)?;
        let id = DockPaneId::Floating(next);
        self.panes[index].remove(tab);
        self.fill_or_remove_dock(index);
        self.floats.push(FloatPane {
            pane: DockPane {
                id,
                tabs: vec![tab],
                active: tab,
            },
            rect,
        });
        self.focused = id;
        Some(id)
    }

    pub(crate) fn return_float(&mut self, id: DockPaneId) -> Option<(DockPaneId, RightPanelTab)> {
        let index = self.floats.iter().position(|float| float.pane.id == id)?;
        let tab = self.floats.remove(index).pane.active;
        let target = self.active_dock_pane();
        let pane = self.pane_mut(target)?;
        if !pane.tabs.contains(&tab) {
            pane.tabs.push(tab);
        }
        self.select(target, tab);
        Some((target, tab))
    }

    pub(crate) fn set_float_rect(&mut self, id: DockPaneId, rect: FloatRect) {
        if rect.valid()
            && let Some(float) = self.floats.iter_mut().find(|float| float.pane.id == id)
        {
            float.rect = rect;
            self.focus(id);
        }
    }

    fn fill_or_remove_dock(&mut self, index: usize) {
        if self.panes[index].tabs.is_empty() {
            if self.panes.len() == 1 {
                self.panes[index] = DockPane::guide(self.panes[index].id);
            } else {
                self.panes.remove(index);
            }
        }
    }

    pub(crate) fn drop_tab(
        &mut self,
        source: DockPaneId,
        tab: RightPanelTab,
        drop: DockDrop,
    ) -> Option<DockDropResult> {
        let source_index = self.panes.iter().position(|pane| pane.id == source)?;
        let tab_index = self.panes[source_index]
            .tabs
            .iter()
            .position(|item| *item == tab)?;
        match drop {
            DockDrop::Strip { pane, slot } if pane == source => {
                let tabs = &mut self.panes[source_index].tabs;
                let slot = slot.min(tabs.len());
                let destination = slot - usize::from(slot > tab_index);
                if destination == tab_index {
                    return None;
                }
                tabs.remove(tab_index);
                tabs.insert(destination, tab);
                Some(DockDropResult::Reordered)
            }
            DockDrop::Strip { pane, slot } => self.move_tab(source_index, tab, pane, slot),
            DockDrop::Center(pane) if pane != source => {
                let end = self.pane(pane)?.tabs.len();
                self.move_tab(source_index, tab, pane, end)
            }
            DockDrop::Split { pane, side } if pane == source && self.panes.len() == 1 => {
                let target = self.split(source)?;
                self.panes[0].remove(tab);
                if self.panes[0].tabs.is_empty() {
                    self.panes[0] = DockPane::guide(source);
                }
                self.panes[1] = DockPane {
                    id: target,
                    tabs: vec![tab],
                    active: tab,
                };
                if side == DockSide::Left {
                    self.panes.swap(0, 1);
                }
                Some(DockDropResult::Focus(target))
            }
            _ => None,
        }
    }

    fn move_tab(
        &mut self,
        from: usize,
        tab: RightPanelTab,
        pane: DockPaneId,
        slot: usize,
    ) -> Option<DockDropResult> {
        let to = self.panes.iter().position(|target| target.id == pane)?;
        self.panes[from].remove(tab);
        let target = &mut self.panes[to];
        // A page can only occupy one tab in the destination pane.
        if !target.tabs.contains(&tab) {
            target.tabs.insert(slot.min(target.tabs.len()), tab);
        }
        target.active = tab;
        self.focused = pane;
        if self.panes[from].tabs.is_empty() {
            self.panes.remove(from);
        }
        Some(DockDropResult::Focus(pane))
    }

    fn valid(&self) -> bool {
        (1..=2).contains(&self.panes.len())
            && self.pane(self.focused).is_some()
            && (0.2..=0.8).contains(&self.split_fraction)
            && (self.panes.len() == 1 || self.panes[0].id != self.panes[1].id)
            && self.panes.iter().all(|pane| !pane.id.is_floating())
            && self.floats.iter().enumerate().all(|(index, float)| {
                matches!(float.pane.id, DockPaneId::Floating(id) if id > 0 && id < u64::MAX)
                    && float.pane.tabs.len() == 1
                    && float.rect.valid()
                    && !self.floats[..index]
                        .iter()
                        .any(|other| other.pane.id == float.pane.id)
            })
            && self.all_panes().all(|pane| {
                pane.tabs.contains(&pane.active)
                    && pane.tabs.iter().enumerate().all(|(index, tab)| {
                        !pane.tabs[..index].contains(tab)
                            && match tab {
                                RightPanelTab::Guide | RightPanelTab::Files => true,
                                RightPanelTab::Terminal(key)
                                | RightPanelTab::Browser(key)
                                | RightPanelTab::Document(key)
                                | RightPanelTab::Plan(key)
                                    if *key == 0 || *key >= u64::MAX - 1 =>
                                {
                                    false
                                }
                                _ => self
                                    .all_panes()
                                    .all(|other| other.id == pane.id || !other.tabs.contains(tab)),
                            }
                    })
            })
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn last_terminal_key(&self) -> u64 {
        self.terminal_keys().max().unwrap_or(0)
    }

    #[cfg(target_family = "wasm")]
    pub(crate) fn terminal_keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.all_panes()
            .flat_map(|pane| pane.tabs())
            .filter_map(|tab| match tab {
                RightPanelTab::Terminal(key) => Some(*key),
                _ => None,
            })
    }

    pub(crate) fn browser_keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.all_panes()
            .flat_map(|pane| pane.tabs())
            .filter_map(|tab| match tab {
                RightPanelTab::Browser(key) => Some(*key),
                _ => None,
            })
    }
    pub(crate) fn panes(&self) -> &[DockPane] {
        &self.panes
    }

    pub(crate) fn floats(&self) -> &[FloatPane] {
        &self.floats
    }

    pub(crate) fn all_panes(&self) -> impl Iterator<Item = &DockPane> {
        self.panes
            .iter()
            .chain(self.floats.iter().map(|float| &float.pane))
    }

    pub(crate) fn pane(&self, id: DockPaneId) -> Option<&DockPane> {
        self.all_panes().find(|pane| pane.id == id)
    }

    fn pane_mut(&mut self, id: DockPaneId) -> Option<&mut DockPane> {
        self.panes
            .iter_mut()
            .chain(self.floats.iter_mut().map(|float| &mut float.pane))
            .find(|pane| pane.id == id)
    }

    pub(crate) fn focused(&self) -> DockPaneId {
        self.focused
    }

    pub(crate) fn active_dock_pane(&self) -> DockPaneId {
        if self.focused.is_floating() {
            self.panes[0].id
        } else {
            self.focused
        }
    }

    pub(crate) fn focus(&mut self, pane: DockPaneId) {
        if let Some(index) = self.floats.iter().position(|float| float.pane.id == pane) {
            if index + 1 != self.floats.len() {
                let raised = self.floats.remove(index);
                self.floats.push(raised);
            }
        } else if !self.panes.iter().any(|candidate| candidate.id == pane) {
            return;
        }
        self.focused = pane;
    }

    pub(crate) fn active(&self) -> RightPanelTab {
        self.pane(self.focused).expect("focused dock pane").active
    }

    pub(crate) fn select(&mut self, pane: DockPaneId, tab: RightPanelTab) {
        if let Some(target) = self.pane_mut(pane)
            && target.tabs.contains(&tab)
        {
            target.active = tab;
            self.focus(pane);
        }
    }

    pub(crate) fn split(&mut self, pane: DockPaneId) -> Option<DockPaneId> {
        if self.panes.len() != 1 || self.panes[0].id != pane {
            return None;
        }
        let id = match pane {
            DockPaneId::Primary => DockPaneId::Secondary,
            DockPaneId::Secondary => DockPaneId::Primary,
            DockPaneId::Floating(_) => return None,
        };
        self.panes.push(DockPane::guide(id));
        self.focused = id;
        self.split_fraction = 0.5;
        Some(id)
    }

    pub(crate) fn set_split_fraction(&mut self, fraction: f32) {
        self.split_fraction = fraction.clamp(0.2, 0.8);
    }

    pub(crate) fn pane_bounds(&self, index: usize, width: f32) -> (f32, f32) {
        if self.panes.len() == 1 {
            (0., width)
        } else {
            let left = width * self.split_fraction;
            if index == 0 {
                (0., left)
            } else {
                (left, width - left)
            }
        }
    }

    /// Reveal an open tool or replace the destination pane's selected guide.
    pub(crate) fn open(&mut self, pane: DockPaneId, tab: RightPanelTab) -> bool {
        let Some(target) = self.pane(pane) else {
            return false;
        };
        let pane = if pane.is_floating() && target.active != RightPanelTab::Guide {
            self.active_dock_pane()
        } else {
            pane
        };
        let target = self.pane(pane).expect("known pane");
        let replace_guide = target.active == RightPanelTab::Guide && tab != RightPanelTab::Guide;
        let existing = if matches!(tab, RightPanelTab::Guide | RightPanelTab::Files) {
            target.tabs.contains(&tab).then_some(pane)
        } else {
            self.all_panes()
                .find(|candidate| candidate.tabs.contains(&tab))
                .map(|candidate| candidate.id)
        };
        if let Some(existing) = existing {
            self.select(existing, tab);
            if replace_guide {
                self.close(pane, RightPanelTab::Guide);
            }
            return false;
        }
        let target = self.pane_mut(pane).expect("known pane");
        if replace_guide {
            let slot = target
                .tabs
                .iter_mut()
                .find(|slot| **slot == RightPanelTab::Guide)
                .expect("selected guide");
            *slot = tab;
        } else {
            target.tabs.push(tab);
        }
        target.active = tab;
        self.focus(pane);
        true
    }

    /// A new resource takes the active resource's slot, including in a floating pane.
    #[cfg(any(target_family = "wasm", test))]
    pub(crate) fn replace_document(&mut self, pane: DockPaneId, document: u64) -> Option<u64> {
        let target = self.pane_mut(pane)?;
        let RightPanelTab::Document(previous) = target.active else {
            return None;
        };
        let slot = target.tabs.iter_mut().find(|tab| **tab == target.active)?;
        let next = RightPanelTab::Document(document);
        *slot = next;
        target.active = next;
        self.focus(pane);
        Some(previous)
    }

    pub(crate) fn can_close(&self, pane: DockPaneId, tab: RightPanelTab) -> bool {
        pane.is_floating()
            || self.panes.len() > 1
            || self.panes[0].tabs != [RightPanelTab::Guide]
            || tab != RightPanelTab::Guide
    }

    /// Collapse the dock only when its final pane has lost its final tool.
    pub(crate) fn close(&mut self, pane: DockPaneId, tab: RightPanelTab) -> bool {
        if !self.pane(pane).is_some_and(|pane| pane.tabs.contains(&tab))
            || !self.can_close(pane, tab)
        {
            return false;
        }
        if let Some(index) = self.floats.iter().position(|float| float.pane.id == pane) {
            self.floats.remove(index);
            if self.focused == pane {
                self.focused = self.panes[0].id;
            }
            return false;
        }
        let Some(index) = self.panes.iter().position(|candidate| candidate.id == pane) else {
            return false;
        };
        self.panes[index].remove(tab);
        if !self.panes[index].tabs.is_empty() {
            return false;
        }
        let collapse = self.panes.len() == 1;
        self.fill_or_remove_dock(index);
        if self.focused == pane {
            self.focused = self.panes[0].id;
        }
        collapse
    }
}

pub(crate) fn auto_fullscreen(viewport: f32) -> bool {
    viewport < 768.
}

pub(crate) fn default_dock_width(viewport: f32) -> f32 {
    (viewport * 0.45).round().max(300.)
}

pub(crate) fn clamp_dock_preference(viewport: f32, width: f32) -> f32 {
    width.round().clamp(300., (viewport * 0.7).max(300.))
}

pub(crate) fn dock_width(viewport: f32, sidebar: f32, preference: f32) -> f32 {
    let available = viewport - sidebar - 400.;
    if available < 300. {
        0.
    } else {
        clamp_dock_preference(viewport, preference).min(available)
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct DockFrame {
    pub(crate) left: f32,
    pub(crate) width: f32,
    pub(crate) track: f32,
    pub(crate) fullscreen: bool,
    pub(crate) visible: bool,
}

impl DockFrame {
    pub(crate) fn new(viewport: f32, width: f32, fullscreen: bool, progress: f32) -> Self {
        let panel_width = if fullscreen { viewport } else { width };
        let track = if fullscreen && progress < 1. {
            0.
        } else {
            width * progress
        };
        Self {
            left: viewport - panel_width * progress,
            width: panel_width,
            track,
            fullscreen,
            visible: panel_width > 0. && progress > 0.,
        }
    }
}

const SLIDE_MS: f64 = 300.;

/// One clock for the moving panel edge and the conversation's layout track.
pub(crate) struct DockSlide {
    start: f64,
    from: f64,
    target: f64,
    duration_factor: f64,
    duration: f64,
    easing: [f64; 4],
}

impl Default for DockSlide {
    fn default() -> Self {
        Self::with_timing(SLIDE_MS, (0.4, 0.2))
    }
}

impl DockSlide {
    pub(crate) fn with_timing(duration: f64, (x1, x2): (f64, f64)) -> Self {
        Self::with_curve(duration, [x1, 0., x2, 1.])
    }

    pub(crate) fn with_curve(duration: f64, easing: [f64; 4]) -> Self {
        Self {
            start: 0.,
            from: 0.,
            target: 0.,
            duration_factor: 0.,
            duration,
            easing,
        }
    }

    pub(crate) fn update(&mut self, open: bool, now: f64, animate: bool) -> (f32, bool) {
        let target = if open { 1. } else { 0. };
        if !animate {
            self.from = target;
            self.target = target;
            self.duration_factor = 0.;
        } else if self.target != target {
            let (from, running) = self.sample(now);
            // CSS transitions shorten a reversal by the progress already traveled.
            self.duration_factor = if running {
                1. - self.duration_factor * (1. - self.eased_time(now))
            } else {
                1.
            };
            self.from = from;
            self.target = target;
            self.start = now;
        }
        let (progress, running) = self.sample(now);
        (progress as f32, running)
    }

    pub(crate) fn sample(&self, now: f64) -> (f64, bool) {
        if self.from == self.target || now - self.start >= self.duration * self.duration_factor {
            (self.target, false)
        } else {
            let progress = self.eased_time(now);
            (self.from + (self.target - self.from) * progress, true)
        }
    }

    fn eased_time(&self, now: f64) -> f64 {
        let elapsed = ((now - self.start) / (self.duration * self.duration_factor)).clamp(0., 1.);
        cubic_bezier_easing(elapsed, self.easing)
    }
}

/// CSS cubic-bezier(x1, 0, x2, 1) used by the dock transition and its drop scrim.
pub(crate) fn dock_easing(elapsed: f64, x1: f64, x2: f64) -> f64 {
    cubic_bezier_easing(elapsed, [x1, 0., x2, 1.])
}

pub(crate) fn cubic_bezier_easing(elapsed: f64, [x1, y1, x2, y2]: [f64; 4]) -> f64 {
    if elapsed == 0. || elapsed == 1. {
        return elapsed;
    }
    let (mut low, mut high) = (0., 1.);
    for _ in 0..24 {
        let t = (low + high) * 0.5;
        let u = 1. - t;
        let x = 3. * (x1 * u * u * t + x2 * u * t * t) + t * t * t;
        if x < elapsed {
            low = t;
        } else {
            high = t;
        }
    }
    let t = (low + high) * 0.5;
    let u = 1. - t;
    3. * (y1 * u * u * t + y2 * u * t * t) + t * t * t
}

#[cfg(test)]
mod tests {
    use super::{
        BrowserCheckpoint, DockDrop, DockDropResult, DockFrame, DockPaneId::*, DockSide, DockSlide,
        DockSurface, FloatRect, RightDock, RightPanelTab::*, default_dock_width, dock_width,
        drag_threshold, strip_drop_slot,
    };

    #[test]
    fn floating_and_returning_a_browser_keeps_the_instance_and_reseeds_the_dock() {
        let mut surface = DockSurface {
            open: true,
            ..Default::default()
        };
        surface.layout.open(Primary, Browser(9));
        surface.browsers.insert(
            9,
            BrowserCheckpoint {
                address: "https://example.com/".into(),
                title: "Example".into(),
            },
        );
        let rect = FloatRect::at_drop(380., 260.);
        let id = surface.layout.float_tab(Primary, Browser(9), rect).unwrap();
        assert_eq!(surface.layout.panes()[0].tabs(), &[Guide]);
        assert_eq!(surface.layout.active(), Browser(9));
        assert_eq!(surface.layout.floats()[0].rect, rect);

        surface.open = false;
        let saved = serde_json::to_string(&surface).unwrap();
        let mut restored = DockSurface::restore(&saved);
        assert_eq!(restored, surface);
        assert_eq!(restored.layout.browser_keys().collect::<Vec<_>>(), vec![9]);
        assert_eq!(
            restored.layout.return_float(id),
            Some((Primary, Browser(9)))
        );
        assert_eq!(restored.layout.panes()[0].tabs(), &[Guide, Browser(9)]);
        assert_eq!(restored.browsers[&9].address, "https://example.com/");
        assert!(restored.layout.valid());
    }

    #[test]
    fn floating_a_split_panes_last_tab_merges_it_and_closing_the_dock_keeps_floats() {
        let mut dock = RightDock::default();
        dock.open(Primary, Files);
        dock.split(Primary);
        dock.open(Secondary, Terminal(3));
        let floating = dock
            .float_tab(Secondary, Terminal(3), FloatRect::at_drop(300., 200.))
            .unwrap();
        assert_eq!(dock.panes().len(), 1);
        assert!(dock.close(Primary, Files));
        assert_eq!(dock.panes()[0].tabs(), &[Guide]);
        assert_eq!(dock.active(), Terminal(3));
        assert!(!dock.close(floating, Terminal(3)));
        assert_eq!(dock.active(), Guide);
        assert!(dock.valid());
    }

    #[test]
    fn floating_panels_raise_without_consuming_the_two_docked_pane_slots() {
        let mut dock = RightDock::default();
        dock.open(Primary, Terminal(1));
        dock.open(Primary, Browser(2));
        let rect = FloatRect::at_drop(300., 220.);
        let terminal = dock.float_tab(Primary, Terminal(1), rect).unwrap();
        let browser = dock.float_tab(Primary, Browser(2), rect).unwrap();
        assert_eq!(dock.floats()[1].pane.id, browser);
        dock.focus(terminal);
        assert_eq!(dock.floats()[1].pane.id, terminal);
        let moved = FloatRect { x: 200., ..rect };
        dock.set_float_rect(browser, moved);
        assert_eq!(dock.floats()[1].rect, moved);
        assert_eq!(dock.focused(), browser);
        assert_eq!(dock.split(Primary), Some(Secondary));
        assert_eq!(dock.return_float(terminal), Some((Secondary, Terminal(1))));
        assert!(dock.valid());
    }

    #[test]
    fn a_floating_guide_can_open_a_tool_in_place_and_returns_without_duplication() {
        let mut dock = RightDock::default();
        let float = dock
            .float_tab(Primary, Guide, FloatRect::at_drop(200., 200.))
            .unwrap();
        assert_eq!(dock.return_float(float), Some((Primary, Guide)));
        assert_eq!(dock.panes()[0].tabs(), &[Guide]);
        let float = dock
            .float_tab(Primary, Guide, FloatRect::at_drop(200., 200.))
            .unwrap();
        assert!(dock.open(float, Files));
        assert_eq!(dock.focused(), float);
        assert_eq!(dock.floats()[0].pane.tabs(), &[Files]);
        assert_eq!(dock.panes()[0].tabs(), &[Guide]);
        assert!(dock.valid());
    }

    #[test]
    fn drag_threshold_and_caret_follow_the_pointer_axes_and_chip_midpoints() {
        assert!(!drag_threshold(3., 3.));
        assert!(drag_threshold(-4., 0.));
        assert!(drag_threshold(0., 4.));

        let slot = |x, source| strip_drop_slot([60., 170., 280.].into_iter(), x, source);
        assert_eq!(slot(59., Some(1)), Some(0));
        assert_eq!(slot(60., Some(1)), None);
        assert_eq!(slot(170., Some(1)), None);
        assert_eq!(slot(280., Some(1)), Some(3));
        assert_eq!(slot(170., None), Some(2));
    }

    #[test]
    fn dragging_within_a_strip_reorders_without_selecting_the_dragged_tab() {
        let mut dock = RightDock::default();
        dock.open(Primary, Files);
        dock.open(Primary, Terminal(7));
        dock.open(Primary, Browser(8));
        dock.split(Primary);
        let slot = |slot| DockDrop::Strip {
            pane: Primary,
            slot,
        };

        assert_eq!(dock.drop_tab(Primary, Files, slot(1)), None);
        assert_eq!(
            dock.drop_tab(Primary, Files, slot(3)),
            Some(DockDropResult::Reordered)
        );
        assert_eq!(dock.panes()[0].tabs(), &[Terminal(7), Browser(8), Files]);
        assert_eq!(dock.panes()[0].active(), Browser(8));
        assert_eq!(dock.focused(), Secondary);
        assert_eq!(
            dock.drop_tab(Primary, Files, DockDrop::Center(Primary)),
            None
        );
        assert!(dock.valid());
    }

    #[test]
    fn dragging_to_another_strip_places_the_same_tool_and_focuses_its_new_pane() {
        let mut dock = RightDock::default();
        dock.open(Primary, Files);
        dock.open(Primary, Browser(2));
        dock.split(Primary);
        dock.open(Secondary, Terminal(4));

        assert_eq!(
            dock.drop_tab(
                Primary,
                Browser(2),
                DockDrop::Strip {
                    pane: Secondary,
                    slot: 0
                }
            ),
            Some(DockDropResult::Focus(Secondary))
        );
        assert_eq!(dock.panes()[0].active(), Files);
        assert_eq!(dock.panes()[1].tabs(), &[Browser(2), Terminal(4)]);
        assert_eq!(dock.active(), Browser(2));
        assert_eq!(
            dock.drop_tab(Primary, Files, DockDrop::Center(Secondary)),
            Some(DockDropResult::Focus(Secondary))
        );
        assert_eq!(dock.panes().len(), 1);
        assert_eq!(dock.panes()[0].tabs(), &[Browser(2), Terminal(4), Files]);
        assert_eq!(dock.active(), Files);
        assert!(dock.valid());
    }

    #[test]
    fn dragging_a_guide_onto_a_pane_with_a_guide_reuses_the_destination_page() {
        let mut dock = RightDock::default();
        dock.split(Primary);
        assert_eq!(
            dock.drop_tab(Primary, Guide, DockDrop::Center(Secondary)),
            Some(DockDropResult::Focus(Secondary))
        );
        assert_eq!(dock.panes().len(), 1);
        assert_eq!(dock.panes()[0].tabs(), &[Guide]);
        assert_eq!(dock.focused(), Secondary);
        assert!(dock.valid());
    }

    #[test]
    fn dragging_to_an_edge_splits_in_that_direction_and_backfills_an_empty_source() {
        for (side, index) in [(DockSide::Left, 0), (DockSide::Right, 1)] {
            let mut dock = RightDock::default();
            dock.open(Primary, Browser(3));
            let edge = |pane| DockDrop::Split { pane, side };

            assert_eq!(
                dock.drop_tab(Primary, Browser(3), edge(Primary)),
                Some(DockDropResult::Focus(Secondary))
            );
            assert_eq!(dock.panes()[index].tabs(), &[Browser(3)]);
            assert_eq!(dock.panes()[1 - index].tabs(), &[Guide]);
            assert_eq!(dock.pane_bounds(index, 600.), (300. * index as f32, 300.));
            assert_eq!(dock.drop_tab(Secondary, Browser(3), edge(Secondary)), None);
            assert_eq!(dock.active(), Browser(3));
            assert!(dock.valid());
        }
    }

    #[test]
    fn sidebar_snapshot_preserves_the_session_layout_and_rejects_invalid_panes() {
        let mut first = DockSurface {
            open: true,
            fullscreen: true,
            ..Default::default()
        };
        first.layout.open(Primary, Files);
        first.layout.split(Primary);
        first.layout.open(Secondary, Terminal(7));
        first.terminals.insert(
            7,
            super::TerminalCheckpoint {
                id: "11383fb58d4e48f7ac1394014d570c27".into(),
                title: Some("zsh".into()),
            },
        );
        first.layout.set_split_fraction(0.63);
        let mut second = DockSurface::default();
        second.layout.open(Primary, Browser(1));
        second.browsers.insert(
            1,
            BrowserCheckpoint {
                address: "https://example.com/".into(),
                title: "Example".into(),
            },
        );
        let data = [
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap(),
        ];
        assert_eq!(DockSurface::restore(&data[1]), second);
        assert_eq!(DockSurface::restore(&data[0]), first);
        assert_eq!(DockSurface::restore("{broken"), DockSurface::default());
        let mut damaged = serde_json::to_value(first).unwrap();
        damaged["layout"]["panes"][1]["active"] = serde_json::json!({"Browser": 1});
        assert_eq!(
            DockSurface::restore(&damaged.to_string()),
            DockSurface::default()
        );
    }

    #[test]
    fn restores_only_the_plan_tabs_with_logged_call_addresses() {
        let mut surface = DockSurface {
            open: true,
            ..Default::default()
        };
        surface.layout.open(Primary, Plan(1));
        surface.layout.open(Primary, Plan(2));
        surface.plans.insert(1, "parent:ptc:1".into());
        surface.plans.insert(9, "unused".into());
        let saved = serde_json::to_string(&surface).unwrap();
        let restored = DockSurface::restore(&saved);
        assert!(restored.open);
        assert_eq!(restored.layout.active(), Plan(1));
        assert_eq!(restored.layout.panes()[0].tabs(), &[Plan(1)]);
        assert_eq!(restored.plans, [(1, "parent:ptc:1".to_owned())].into());
    }

    #[test]
    fn legacy_browser_migrates_and_browser_occurrences_are_independent() {
        let saved = r#"{"open":true,"browser_address":"https://example.com/","layout":{"panes":[{"id":"Primary","tabs":["Browser"],"active":"Browser"}],"focused":"Primary","split_fraction":0.5}}"#;
        let mut surface = DockSurface::restore(saved);
        assert_eq!(surface.layout.active(), Browser(1));
        assert_eq!(surface.browsers[&1].address, "https://example.com/");
        assert!(surface.layout.open(Primary, Browser(2)));
        surface.layout.split(Primary);
        assert!(surface.layout.open(Secondary, Browser(3)));
        assert!(!surface.layout.close(Primary, Browser(2)));
        assert_eq!(
            surface.layout.browser_keys().collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(surface.layout.active(), Browser(3));
    }

    #[test]
    fn guide_is_replaced_and_closing_the_last_tool_collapses_the_dock() {
        let mut dock = RightDock::default();
        assert!(!dock.close(Primary, Guide));
        assert!(dock.open(Primary, Files));
        assert_eq!(dock.panes()[0].tabs(), &[Files]);
        assert!(dock.close(Primary, Files));
        assert_eq!(dock.panes()[0].tabs(), &[Guide]);
        assert_eq!(dock.active(), Guide);
    }

    #[test]
    fn opening_from_a_new_guide_reuses_tools_and_selects_the_previous_tab_on_close() {
        let mut dock = RightDock::default();
        dock.open(Primary, Files);
        dock.open(Primary, Guide);
        assert!(dock.open(Primary, Browser(1)));
        assert_eq!(dock.panes()[0].tabs(), &[Files, Browser(1)]);
        dock.select(Primary, Files);
        assert_eq!(dock.active(), Files);
        dock.open(Primary, Guide);
        assert!(!dock.open(Primary, Browser(1)));
        assert_eq!(dock.panes()[0].tabs(), &[Files, Browser(1)]);
        assert!(!dock.close(Primary, Browser(1)));
        assert_eq!(dock.active(), Files);
        assert!(dock.open(Primary, Terminal(1)));
        assert_eq!(dock.panes()[0].tabs(), &[Files, Terminal(1)]);
    }

    #[test]
    fn split_keeps_the_existing_tools_and_focuses_a_new_guide() {
        let mut dock = RightDock::default();
        dock.open(Primary, Files);
        assert_eq!(dock.split(Primary), Some(Secondary));
        assert_eq!(dock.focused(), Secondary);
        assert_eq!(dock.panes()[0].active(), Files);
        assert_eq!(dock.active(), Guide);
        assert_eq!(dock.split(Secondary), None);
        dock.open(Secondary, Terminal(1));
        assert_eq!(dock.panes()[0].tabs(), &[Files]);
        assert_eq!(dock.panes()[1].tabs(), &[Terminal(1)]);
        assert!(!dock.close(Secondary, Terminal(1)));
        assert_eq!(dock.focused(), Primary);
        assert_eq!(dock.panes().len(), 1);
    }

    #[test]
    fn revealing_an_existing_tool_retires_the_guide_that_led_to_it() {
        let mut dock = RightDock::default();
        dock.open(Primary, Browser(1));
        dock.split(Primary);
        assert!(!dock.open(Secondary, Browser(1)));
        assert_eq!(dock.focused(), Primary);
        assert_eq!(dock.panes().len(), 1);
    }

    #[test]
    fn each_pane_keeps_its_files_page_and_a_floating_preview_replaces_in_place() {
        let mut surface = DockSurface::default();
        let layout = &mut surface.layout;
        layout.open(Primary, Files);
        layout.split(Primary);
        assert!(layout.open(Secondary, Files));
        assert_eq!(layout.pane(Primary).unwrap().tabs(), &[Files]);
        assert_eq!(layout.pane(Secondary).unwrap().tabs(), &[Files]);

        layout.open(Primary, Document(1));
        let rect = FloatRect {
            x: 100.,
            y: 100.,
            width: 400.,
            height: 500.,
        };
        let floating = layout.float_tab(Primary, Document(1), rect).unwrap();
        assert_eq!(layout.replace_document(floating, 2), Some(1));
        assert_eq!(layout.floats()[0].rect, rect);
        assert!(!layout.open(Secondary, Document(2)));
        assert_eq!(layout.focused(), floating);
        surface.documents.insert(2, "sub/notes.txt".into());
        assert_eq!(
            DockSurface::restore(&serde_json::to_string(&surface).unwrap()),
            surface
        );

        surface.documents.insert(2, "../outside.txt".into());
        let restored = DockSurface::restore(&serde_json::to_string(&surface).unwrap());
        assert!(restored.documents.is_empty());
        assert!(restored.layout.floats().is_empty());
        assert_eq!(restored.layout.pane(Primary).unwrap().tabs(), &[Files]);
        assert_eq!(restored.layout.pane(Secondary).unwrap().tabs(), &[Files]);
    }

    #[test]
    fn terminal_occurrences_live_and_close_independently() {
        let mut dock = RightDock::default();
        assert!(dock.open(Primary, Terminal(1)));
        assert!(dock.open(Primary, Terminal(2)));
        assert_eq!(dock.panes()[0].tabs(), &[Terminal(1), Terminal(2)]);
        dock.split(Primary);
        assert!(dock.open(Secondary, Terminal(3)));
        assert!(!dock.close(Primary, Terminal(2)));
        assert_eq!(dock.panes()[0].active(), Terminal(1));
        assert_eq!(dock.panes()[1].tabs(), &[Terminal(3)]);
        assert_eq!(dock.focused(), Secondary);
    }

    #[test]
    fn divider_preserves_twenty_percent_for_each_pane() {
        let mut dock = RightDock::default();
        dock.split(Primary);
        assert_eq!(dock.pane_bounds(0, 500.), (0., 250.));
        dock.set_split_fraction(0.95);
        assert_eq!(dock.pane_bounds(1, 500.), (400., 100.));
        dock.focus(Primary);
        assert_eq!(dock.focused(), Primary);
    }

    #[test]
    fn right_dock_concedes_space_to_the_conversation_column() {
        let preference = default_dock_width(1280.);
        assert_eq!(dock_width(1280., 280., preference), 576.);
        assert_eq!(dock_width(1440., 280., preference), 576.);
        assert_eq!(dock_width(1000., 280., preference), 320.);
        assert_eq!(dock_width(950., 280., preference), 0.);
    }

    #[test]
    fn panel_keeps_its_width_and_tracks_the_conversation_throughout_the_slide() {
        let mut slide = DockSlide::default();
        slide.update(true, 0., true);
        let (progress, running) = slide.update(true, 150., true);
        assert!(running);
        assert!((progress - 0.77556).abs() < 0.00001);
        let frame = DockFrame::new(1280., 576., false, progress);
        assert!(frame.visible);
        assert_eq!(frame.width, 576.);
        assert_eq!(frame.left, 1280. - frame.track);
        assert_eq!(slide.update(true, 300., true), (1., false));
        slide.update(false, 300., true);
        assert_eq!(slide.update(false, 600., true), (0., false));
    }

    #[test]
    fn reversal_is_continuous_and_reduced_motion_settles_immediately() {
        let mut slide = DockSlide::default();
        slide.update(true, 0., true);
        let (progress, _) = slide.update(true, 150., true);
        let (reversed, running) = slide.update(false, 150., true);
        assert!(running);
        assert!((progress - reversed).abs() < 0.00001);
        assert_eq!(slide.update(false, 383., true), (0., false));
        assert_eq!(slide.update(true, 400., false), (1., false));
    }

    #[test]
    fn resize_grip_eases_out_and_reverses_from_its_current_opacity() {
        let mut fade = DockSlide::with_timing(120., (0., 0.58));
        assert_eq!(fade.update(true, 0., true), (0., true));
        let (opacity, running) = fade.update(true, 60., true);
        assert!(running && (0.68..0.69).contains(&opacity));
        assert_eq!(fade.update(false, 60., true), (opacity, true));
        assert!(fade.update(false, 100., true).0 < opacity);
        assert_eq!(fade.update(false, 143., true), (0., false));
        assert_eq!(fade.update(true, 150., true), (0., true));
        assert_eq!(fade.update(true, 270., true), (1., false));
    }

    #[test]
    fn fullscreen_slides_over_the_entire_window_without_squeezing_the_center() {
        assert!(super::auto_fullscreen(767.));
        assert!(!super::auto_fullscreen(768.));
        let frame = DockFrame::new(1280., 576., true, 0.75);
        assert!(frame.visible && frame.fullscreen);
        assert_eq!(frame.width, 1280.);
        assert_eq!(frame.left, 320.);
        assert_eq!(frame.track, 0.);
        let covered = DockFrame::new(1280., 576., true, 1.);
        assert_eq!(covered.left, 0.);
        assert_eq!(covered.track, 576.);
    }
}
