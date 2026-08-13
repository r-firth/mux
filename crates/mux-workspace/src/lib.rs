//! Product-level workspace state and actions.
//!
//! This crate intentionally knows nothing about PTYs, rendering, IPC, or
//! platform keyboard events.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

macro_rules! entity_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
        )]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

entity_id!(SessionId);
entity_id!(TabId);
entity_id!(PaneId);
entity_id!(AgentSessionId);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SplitAxis {
    /// Children appear left-to-right.
    Horizontal,
    /// Children appear top-to-bottom.
    Vertical,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SplitRatio(u16);

impl SplitRatio {
    pub const HALF: Self = Self(500);

    pub fn new(thousandths: u16) -> Result<Self, WorkspaceError> {
        if (1..1_000).contains(&thousandths) {
            Ok(Self(thousandths))
        } else {
            Err(WorkspaceError::InvalidSplitRatio(thousandths))
        }
    }

    #[must_use]
    pub const fn thousandths(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PaneLayout {
    Leaf(PaneId),
    Split {
        axis: SplitAxis,
        ratio: SplitRatio,
        first: Box<Self>,
        second: Box<Self>,
    },
}

impl PaneLayout {
    const MIN_SPLIT_THOUSANDTHS: u16 = 100;
    const MAX_SPLIT_THOUSANDTHS: u16 = 900;
    const RESIZE_STEP_THOUSANDTHS: u16 = 50;

    pub fn pane_ids(&self, output: &mut Vec<PaneId>) {
        match self {
            Self::Leaf(pane_id) => output.push(*pane_id),
            Self::Split { first, second, .. } => {
                first.pane_ids(output);
                second.pane_ids(output);
            }
        }
    }

    #[must_use]
    pub fn contains(&self, pane_id: PaneId) -> bool {
        match self {
            Self::Leaf(candidate) => *candidate == pane_id,
            Self::Split { first, second, .. } => {
                first.contains(pane_id) || second.contains(pane_id)
            }
        }
    }

    pub fn split(
        &mut self,
        target: PaneId,
        new_pane: PaneId,
        axis: SplitAxis,
    ) -> Result<(), WorkspaceError> {
        match self {
            Self::Leaf(pane_id) if *pane_id == target => {
                *self = Self::Split {
                    axis,
                    ratio: SplitRatio::HALF,
                    first: Box::new(Self::Leaf(target)),
                    second: Box::new(Self::Leaf(new_pane)),
                };
                Ok(())
            }
            Self::Leaf(_) => Err(WorkspaceError::UnknownPane(target)),
            Self::Split { first, second, .. } => {
                if first.contains(target) {
                    first.split(target, new_pane, axis)
                } else {
                    second.split(target, new_pane, axis)
                }
            }
        }
    }

    pub fn remove(&mut self, target: PaneId) -> Result<(), WorkspaceError> {
        match self {
            Self::Leaf(_) => Err(WorkspaceError::CannotCloseLastPane),
            Self::Split { first, second, .. } if first.contains(target) => {
                if matches!(first.as_ref(), Self::Leaf(id) if *id == target) {
                    *self = (**second).clone();
                    Ok(())
                } else {
                    first.remove(target)
                }
            }
            Self::Split { first, second, .. } if second.contains(target) => {
                if matches!(second.as_ref(), Self::Leaf(id) if *id == target) {
                    *self = (**first).clone();
                    Ok(())
                } else {
                    second.remove(target)
                }
            }
            Self::Split { .. } => Err(WorkspaceError::UnknownPane(target)),
        }
    }

    /// Grow the target pane toward `direction` by moving the nearest matching
    /// split boundary. Ratios move in five-percent increments, like Zellij's
    /// default resize behavior, while keeping both children usable.
    pub fn resize_toward(
        &mut self,
        target: PaneId,
        direction: Direction,
    ) -> Result<bool, WorkspaceError> {
        if !self.contains(target) {
            return Err(WorkspaceError::UnknownPane(target));
        }
        Ok(self.resize_toward_inner(target, direction))
    }

    fn resize_toward_inner(&mut self, target: PaneId, direction: Direction) -> bool {
        let Self::Split {
            axis,
            ratio,
            first,
            second,
        } = self
        else {
            return false;
        };

        let target_in_first = first.contains(target);
        let child_changed = if target_in_first {
            first.resize_toward_inner(target, direction)
        } else {
            second.resize_toward_inner(target, direction)
        };
        if child_changed {
            return true;
        }

        let delta = match (*axis, target_in_first, direction) {
            (SplitAxis::Horizontal, true, Direction::Right)
            | (SplitAxis::Vertical, true, Direction::Down) => {
                i32::from(Self::RESIZE_STEP_THOUSANDTHS)
            }
            (SplitAxis::Horizontal, false, Direction::Left)
            | (SplitAxis::Vertical, false, Direction::Up) => {
                -i32::from(Self::RESIZE_STEP_THOUSANDTHS)
            }
            _ => return false,
        };
        let current = i32::from(ratio.thousandths());
        let next = (current + delta).clamp(
            i32::from(Self::MIN_SPLIT_THOUSANDTHS),
            i32::from(Self::MAX_SPLIT_THOUSANDTHS),
        );
        if next == current {
            return false;
        }
        *ratio = SplitRatio::new(u16::try_from(next).expect("clamped split ratio"))
            .expect("clamped split ratio is valid");
        true
    }

    fn regions(&self, bounds: Region, output: &mut Vec<(PaneId, Region)>) {
        match self {
            Self::Leaf(pane_id) => output.push((*pane_id, bounds)),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let first_fraction = f64::from(ratio.thousandths()) / 1_000.0;
                let (first_bounds, second_bounds) = match axis {
                    SplitAxis::Horizontal => {
                        let split = bounds.left + bounds.width() * first_fraction;
                        (
                            Region {
                                right: split,
                                ..bounds
                            },
                            Region {
                                left: split,
                                ..bounds
                            },
                        )
                    }
                    SplitAxis::Vertical => {
                        let split = bounds.top + bounds.height() * first_fraction;
                        (
                            Region {
                                bottom: split,
                                ..bounds
                            },
                            Region {
                                top: split,
                                ..bounds
                            },
                        )
                    }
                };
                first.regions(first_bounds, output);
                second.regions(second_bounds, output);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Region {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Region {
    fn width(self) -> f64 {
        self.right - self.left
    }

    fn height(self) -> f64 {
        self.bottom - self.top
    }

    fn center(self) -> (f64, f64) {
        (
            self.left.midpoint(self.right),
            self.top.midpoint(self.bottom),
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub layout: PaneLayout,
    pub focused_pane: PaneId,
    pub zoomed_pane: Option<PaneId>,
}

impl Tab {
    pub fn with_panes(
        title: impl Into<String>,
        pane_ids: &[PaneId],
    ) -> Result<Self, WorkspaceError> {
        let Some((&first, remaining)) = pane_ids.split_first() else {
            return Err(WorkspaceError::EmptyTab);
        };

        let layout = remaining
            .iter()
            .copied()
            .fold(PaneLayout::Leaf(first), |layout, pane_id| {
                PaneLayout::Split {
                    axis: SplitAxis::Horizontal,
                    ratio: SplitRatio::HALF,
                    first: Box::new(layout),
                    second: Box::new(PaneLayout::Leaf(pane_id)),
                }
            });

        Ok(Self {
            id: TabId::new(),
            title: title.into(),
            layout,
            focused_pane: first,
            zoomed_pane: None,
        })
    }

    pub fn focus(&mut self, pane_id: PaneId) -> Result<(), WorkspaceError> {
        if self.layout.contains(pane_id) {
            self.focused_pane = pane_id;
            Ok(())
        } else {
            Err(WorkspaceError::UnknownPane(pane_id))
        }
    }

    pub fn split_focused(
        &mut self,
        new_pane: PaneId,
        axis: SplitAxis,
    ) -> Result<(), WorkspaceError> {
        self.layout.split(self.focused_pane, new_pane, axis)?;
        self.focused_pane = new_pane;
        self.zoomed_pane = None;
        Ok(())
    }

    pub fn close_focused(&mut self) -> Result<PaneId, WorkspaceError> {
        let removed = self.focused_pane;
        self.layout.remove(removed)?;
        let mut remaining = Vec::new();
        self.layout.pane_ids(&mut remaining);
        self.focused_pane = remaining[0];
        self.zoomed_pane = None;
        Ok(removed)
    }

    pub fn toggle_zoom(&mut self) {
        self.zoomed_pane = if self.zoomed_pane == Some(self.focused_pane) {
            None
        } else {
            Some(self.focused_pane)
        };
    }

    pub fn focus_neighbor(&mut self, direction: Direction) -> Result<bool, WorkspaceError> {
        let mut regions = Vec::new();
        self.layout.regions(
            Region {
                left: 0.0,
                top: 0.0,
                right: 1.0,
                bottom: 1.0,
            },
            &mut regions,
        );
        let current = regions
            .iter()
            .find(|(pane, _)| *pane == self.focused_pane)
            .map(|(_, region)| *region)
            .ok_or(WorkspaceError::UnknownPane(self.focused_pane))?;
        let (current_x, current_y) = current.center();
        let candidate = regions
            .into_iter()
            .filter(|(pane, _)| *pane != self.focused_pane)
            .filter_map(|(pane, region)| {
                let (x, y) = region.center();
                let (primary, secondary) = match direction {
                    Direction::Left if x < current_x => (current_x - x, (current_y - y).abs()),
                    Direction::Right if x > current_x => (x - current_x, (current_y - y).abs()),
                    Direction::Up if y < current_y => (current_y - y, (current_x - x).abs()),
                    Direction::Down if y > current_y => (y - current_y, (current_x - x).abs()),
                    _ => return None,
                };
                Some((pane, primary + secondary * 2.0))
            })
            .min_by(|left, right| left.1.total_cmp(&right.1));
        if let Some((pane, _)) = candidate {
            self.focused_pane = pane;
            return Ok(true);
        }
        Ok(false)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub tabs: Vec<Tab>,
    pub active_tab: TabId,
}

impl Session {
    pub fn with_panes(
        name: impl Into<String>,
        pane_ids: &[PaneId],
    ) -> Result<Self, WorkspaceError> {
        let tab = Tab::with_panes("1", pane_ids)?;
        let active_tab = tab.id;
        Ok(Self {
            id: SessionId::new(),
            name: name.into(),
            tabs: vec![tab],
            active_tab,
        })
    }

    #[must_use]
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == self.active_tab)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == self.active_tab)
    }

    pub fn add_tab(&mut self, pane_id: PaneId) -> Result<TabId, WorkspaceError> {
        let title = (self.tabs.len() + 1).to_string();
        let tab = Tab::with_panes(title, &[pane_id])?;
        let id = tab.id;
        self.tabs.push(tab);
        self.active_tab = id;
        Ok(id)
    }

    pub fn close_active_tab(&mut self) -> Result<Vec<PaneId>, WorkspaceError> {
        if self.tabs.len() == 1 {
            return Err(WorkspaceError::CannotCloseLastTab);
        }
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.id == self.active_tab)
            .ok_or(WorkspaceError::UnknownTab(self.active_tab))?;
        let tab = self.tabs.remove(index);
        let mut panes = Vec::new();
        tab.layout.pane_ids(&mut panes);
        self.active_tab = self.tabs[index.saturating_sub(1)].id;
        Ok(panes)
    }

    pub fn select_tab(&mut self, tab_id: TabId) -> Result<(), WorkspaceError> {
        if self.tabs.iter().any(|tab| tab.id == tab_id) {
            self.active_tab = tab_id;
            Ok(())
        } else {
            Err(WorkspaceError::UnknownTab(tab_id))
        }
    }

    pub fn cycle_tab(&mut self, offset: isize) -> Result<(), WorkspaceError> {
        let current = self
            .tabs
            .iter()
            .position(|tab| tab.id == self.active_tab)
            .ok_or(WorkspaceError::UnknownTab(self.active_tab))?;
        let len = isize::try_from(self.tabs.len()).expect("tab count fits isize");
        let next = (isize::try_from(current).expect("index fits isize") + offset).rem_euclid(len);
        self.active_tab = self.tabs[usize::try_from(next).expect("non-negative index")].id;
        Ok(())
    }

    /// Move within the active tab, falling through to the neighboring tab at
    /// horizontal pane edges. This is Zellij's `MoveFocusOrTab` behavior.
    pub fn move_focus_or_tab(&mut self, direction: Direction) -> Result<(), WorkspaceError> {
        let active_tab = self.active_tab;
        let moved = self
            .active_tab_mut()
            .ok_or(WorkspaceError::UnknownTab(active_tab))?
            .focus_neighbor(direction)?;
        if !moved {
            match direction {
                Direction::Left => self.cycle_tab(-1)?,
                Direction::Right => self.cycle_tab(1)?,
                Direction::Up | Direction::Down => {}
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum InputMode {
    Normal,
    Pane,
    Tab,
    Session,
    Resize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Action {
    Sequence(Vec<Action>),
    EnterMode(InputMode),
    WriteTerminal(Vec<u8>),
    SplitPane(SplitAxis),
    FocusPane(Direction),
    FocusPaneOrTab(Direction),
    ResizePane(Direction),
    ClosePane,
    TogglePaneZoom,
    NewTab,
    CloseTab,
    RenameTab,
    SelectTab(u8),
    NextTab,
    PreviousTab,
    OpenSessionSwitcher,
    DetachSession,
    OpenCommandPalette,
    OpenAgentSurface,
    OpenSettings,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum WorkspaceCommand {
    SplitPane(SplitAxis),
    FocusPane(Direction),
    SetFocusedPane(PaneId),
    ClosePane,
    TogglePaneZoom,
    NewTab,
    CloseTab,
    RenameTab(String),
    SelectTab(TabId),
    NextTab,
    PreviousTab,
    RenameSession(String),
    // Keep new protocol variants appended so existing discriminants remain
    // stable for live development daemons that still own terminal processes.
    FocusPaneOrTab(Direction),
    ResizePane(Direction),
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const EMPTY: Self = Self(0);
    pub const CONTROL: Self = Self(1 << 0);
    pub const ALT: Self = Self(1 << 1);
    pub const SHIFT: Self = Self(1 << 2);
    pub const SUPER: Self = Self(1 << 3);

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum Key {
    Character(char),
    Escape,
    Enter,
    Tab,
    Backspace,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct KeyChord {
    pub key: Key,
    pub modifiers: Modifiers,
}

impl KeyChord {
    #[must_use]
    pub const fn plain(key: Key) -> Self {
        Self {
            key,
            modifiers: Modifiers::EMPTY,
        }
    }

    #[must_use]
    pub const fn control(character: char) -> Self {
        Self {
            key: Key::Character(character),
            modifiers: Modifiers::CONTROL,
        }
    }

    #[must_use]
    pub const fn alt(key: Key) -> Self {
        Self {
            key,
            modifiers: Modifiers::ALT,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Keymap {
    bindings: HashMap<(InputMode, KeyChord), Action>,
}

impl Keymap {
    pub fn bind(&mut self, mode: InputMode, chord: KeyChord, action: Action) {
        self.bindings.insert((mode, chord), action);
    }

    #[must_use]
    pub fn resolve(&self, mode: InputMode, chord: KeyChord) -> Option<&Action> {
        self.bindings.get(&(mode, chord))
    }

    /// The Zellij default preset for the pane, resize, tab, and session subset
    /// that Mux currently exposes.
    #[must_use]
    pub fn zellij_default() -> Self {
        let mut keymap = Self::default();
        keymap.bind_zellij_mode_entry();
        keymap.bind_zellij_pane_mode();
        keymap.bind_zellij_resize_mode();
        keymap.bind_zellij_tab_mode();
        keymap.bind_zellij_session_mode();
        keymap.bind_zellij_shared_navigation();
        keymap.bind(
            InputMode::Normal,
            KeyChord {
                key: Key::Character('a'),
                modifiers: Modifiers::SUPER.union(Modifiers::SHIFT),
            },
            Action::OpenAgentSurface,
        );
        keymap.bind(
            InputMode::Normal,
            KeyChord {
                key: Key::Character('s'),
                modifiers: Modifiers::SUPER.union(Modifiers::SHIFT),
            },
            Action::OpenSessionSwitcher,
        );
        keymap.bind(
            InputMode::Normal,
            KeyChord {
                key: Key::Character(','),
                modifiers: Modifiers::SUPER,
            },
            Action::OpenSettings,
        );
        keymap
    }

    fn bind_zellij_mode_entry(&mut self) {
        let modes = [
            InputMode::Normal,
            InputMode::Pane,
            InputMode::Tab,
            InputMode::Session,
            InputMode::Resize,
        ];
        // Normal terminal input deliberately reserves only the two explicit
        // Zellij mode prefixes. In particular, Ctrl+n/Ctrl+o must reach apps
        // such as Vim instead of silently putting Mux into another mode.
        for (character, target) in [('p', InputMode::Pane), ('t', InputMode::Tab)] {
            for source in modes {
                let destination = if source == target {
                    InputMode::Normal
                } else {
                    target
                };
                self.bind(
                    source,
                    KeyChord::control(character),
                    Action::EnterMode(destination),
                );
            }
        }
        for mode in modes.into_iter().filter(|mode| *mode != InputMode::Normal) {
            for key in [Key::Escape, Key::Enter] {
                self.bind(
                    mode,
                    KeyChord::plain(key),
                    Action::EnterMode(InputMode::Normal),
                );
            }
        }
    }

    fn bind_zellij_pane_mode(&mut self) {
        // Keep Ctrl+n available to terminal applications in Normal mode, but
        // preserve the familiar Zellij resize prefix once the user has
        // explicitly entered Mux's Pane mode with Ctrl+p.
        self.bind(
            InputMode::Pane,
            KeyChord::control('n'),
            Action::EnterMode(InputMode::Resize),
        );
        for (key, arrow, direction) in [
            ('h', Key::ArrowLeft, Direction::Left),
            ('j', Key::ArrowDown, Direction::Down),
            ('k', Key::ArrowUp, Direction::Up),
            ('l', Key::ArrowRight, Direction::Right),
        ] {
            self.bind(
                InputMode::Pane,
                KeyChord::plain(Key::Character(key)),
                Action::FocusPane(direction),
            );
            self.bind(
                InputMode::Pane,
                KeyChord::plain(arrow),
                Action::FocusPane(direction),
            );
        }
        self.bind(
            InputMode::Pane,
            KeyChord::plain(Key::Character('n')),
            Action::Sequence(vec![
                Action::SplitPane(SplitAxis::Horizontal),
                Action::EnterMode(InputMode::Normal),
            ]),
        );
        self.bind(
            InputMode::Pane,
            KeyChord::plain(Key::Character('d')),
            Action::Sequence(vec![
                Action::SplitPane(SplitAxis::Vertical),
                Action::EnterMode(InputMode::Normal),
            ]),
        );
        self.bind(
            InputMode::Pane,
            KeyChord::plain(Key::Character('r')),
            Action::Sequence(vec![
                Action::SplitPane(SplitAxis::Horizontal),
                Action::EnterMode(InputMode::Normal),
            ]),
        );
        self.bind(
            InputMode::Pane,
            KeyChord::plain(Key::Character('x')),
            Action::Sequence(vec![
                Action::ClosePane,
                Action::EnterMode(InputMode::Normal),
            ]),
        );
        self.bind(
            InputMode::Pane,
            KeyChord::plain(Key::Character('f')),
            Action::Sequence(vec![
                Action::TogglePaneZoom,
                Action::EnterMode(InputMode::Normal),
            ]),
        );
    }

    fn bind_zellij_resize_mode(&mut self) {
        for (key, arrow, direction) in [
            ('h', Key::ArrowLeft, Direction::Left),
            ('j', Key::ArrowDown, Direction::Down),
            ('k', Key::ArrowUp, Direction::Up),
            ('l', Key::ArrowRight, Direction::Right),
        ] {
            self.bind(
                InputMode::Resize,
                KeyChord::plain(Key::Character(key)),
                Action::ResizePane(direction),
            );
            self.bind(
                InputMode::Resize,
                KeyChord::plain(arrow),
                Action::ResizePane(direction),
            );
        }
    }

    fn bind_zellij_tab_mode(&mut self) {
        self.bind(
            InputMode::Tab,
            KeyChord::plain(Key::Character('r')),
            Action::RenameTab,
        );
        self.bind(
            InputMode::Tab,
            KeyChord::plain(Key::Character('n')),
            Action::Sequence(vec![Action::NewTab, Action::EnterMode(InputMode::Normal)]),
        );
        self.bind(
            InputMode::Tab,
            KeyChord::plain(Key::Character('x')),
            Action::Sequence(vec![Action::CloseTab, Action::EnterMode(InputMode::Normal)]),
        );
        self.bind(
            InputMode::Tab,
            KeyChord::plain(Key::Character('h')),
            Action::PreviousTab,
        );
        self.bind(
            InputMode::Tab,
            KeyChord::plain(Key::Character('l')),
            Action::NextTab,
        );
        for key in [Key::ArrowLeft, Key::ArrowUp] {
            self.bind(InputMode::Tab, KeyChord::plain(key), Action::PreviousTab);
        }
        for key in [Key::ArrowRight, Key::ArrowDown] {
            self.bind(InputMode::Tab, KeyChord::plain(key), Action::NextTab);
        }
        self.bind(
            InputMode::Tab,
            KeyChord::plain(Key::Character('j')),
            Action::NextTab,
        );
        self.bind(
            InputMode::Tab,
            KeyChord::plain(Key::Character('k')),
            Action::PreviousTab,
        );
        for number in 1..=9 {
            self.bind(
                InputMode::Tab,
                KeyChord::plain(Key::Character(char::from(b'0' + number))),
                Action::Sequence(vec![
                    Action::SelectTab(number),
                    Action::EnterMode(InputMode::Normal),
                ]),
            );
        }
    }

    fn bind_zellij_session_mode(&mut self) {
        self.bind(
            InputMode::Session,
            KeyChord::plain(Key::Character('d')),
            Action::DetachSession,
        );
        self.bind(
            InputMode::Session,
            KeyChord::plain(Key::Character('w')),
            Action::OpenSessionSwitcher,
        );
    }

    fn bind_zellij_shared_navigation(&mut self) {
        // Zellij exposes these as shared bindings in every mode except Locked.
        for mode in [
            InputMode::Normal,
            InputMode::Pane,
            InputMode::Tab,
            InputMode::Session,
            InputMode::Resize,
        ] {
            for (character, arrow, direction) in [
                ('h', Key::ArrowLeft, Direction::Left),
                ('j', Key::ArrowDown, Direction::Down),
                ('k', Key::ArrowUp, Direction::Up),
                ('l', Key::ArrowRight, Direction::Right),
            ] {
                let action = if matches!(direction, Direction::Left | Direction::Right) {
                    Action::FocusPaneOrTab(direction)
                } else {
                    Action::FocusPane(direction)
                };
                self.bind(
                    mode,
                    KeyChord::alt(Key::Character(character)),
                    action.clone(),
                );
                self.bind(mode, KeyChord::alt(arrow), action);
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("a tab must contain at least one pane")]
    EmptyTab,
    #[error("split ratio must be between 1 and 999 thousandths, got {0}")]
    InvalidSplitRatio(u16),
    #[error("unknown pane {0}")]
    UnknownPane(PaneId),
    #[error("unknown tab {0}")]
    UnknownTab(TabId),
    #[error("the final pane in a tab cannot be closed")]
    CannotCloseLastPane,
    #[error("the final tab in a session cannot be closed")]
    CannotCloseLastTab,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_panes_have_stable_order_and_focus() {
        let panes = [PaneId::new(), PaneId::new()];
        let session = Session::with_panes("daily", &panes).expect("valid session");
        let tab = session.active_tab().expect("active tab");

        let mut actual = Vec::new();
        tab.layout.pane_ids(&mut actual);
        assert_eq!(actual, panes);
        assert_eq!(tab.focused_pane, panes[0]);
    }

    #[test]
    fn zellij_pane_mode_uses_navigation_muscle_memory() {
        let keymap = Keymap::zellij_default();
        assert_eq!(
            keymap.resolve(InputMode::Normal, KeyChord::control('p'),),
            Some(&Action::EnterMode(InputMode::Pane)),
        );
        assert_eq!(
            keymap.resolve(InputMode::Pane, KeyChord::plain(Key::Character('h')),),
            Some(&Action::FocusPane(Direction::Left)),
        );
        assert_eq!(
            keymap.resolve(InputMode::Pane, KeyChord::plain(Key::Character('d'))),
            Some(&Action::Sequence(vec![
                Action::SplitPane(SplitAxis::Vertical),
                Action::EnterMode(InputMode::Normal),
            ])),
        );
        assert_eq!(
            keymap.resolve(InputMode::Normal, KeyChord::alt(Key::ArrowLeft)),
            Some(&Action::FocusPaneOrTab(Direction::Left)),
        );
        assert_eq!(
            keymap.resolve(InputMode::Tab, KeyChord::control('p')),
            Some(&Action::EnterMode(InputMode::Pane)),
        );
        assert_eq!(
            keymap.resolve(InputMode::Pane, KeyChord::plain(Key::Enter)),
            Some(&Action::EnterMode(InputMode::Normal)),
        );
        assert_eq!(
            keymap.resolve(InputMode::Tab, KeyChord::plain(Key::Character('r'))),
            Some(&Action::RenameTab),
        );
        assert_eq!(
            keymap.resolve(InputMode::Normal, KeyChord::control('n')),
            None
        );
        assert_eq!(
            keymap.resolve(
                InputMode::Normal,
                KeyChord {
                    key: Key::Character(','),
                    modifiers: Modifiers::SUPER,
                },
            ),
            Some(&Action::OpenSettings),
        );
        assert_eq!(
            keymap.resolve(InputMode::Pane, KeyChord::control('n')),
            Some(&Action::EnterMode(InputMode::Resize))
        );
        assert_eq!(
            keymap.resolve(InputMode::Normal, KeyChord::control('o')),
            None
        );
        assert_eq!(
            keymap.resolve(
                InputMode::Normal,
                KeyChord {
                    key: Key::Character('s'),
                    modifiers: Modifiers::SUPER.union(Modifiers::SHIFT),
                },
            ),
            Some(&Action::OpenSessionSwitcher),
        );
        assert_eq!(
            keymap.resolve(InputMode::Normal, KeyChord::plain(Key::Character(':'))),
            None,
        );
        assert_eq!(
            keymap.resolve(InputMode::Normal, KeyChord::alt(Key::Character('n'))),
            None,
        );
        assert_eq!(
            keymap.resolve(InputMode::Normal, KeyChord::plain(Key::Tab)),
            None,
        );
    }

    #[test]
    fn focus_or_tab_prefers_a_pane_then_falls_through_at_horizontal_edges() {
        let left = PaneId::new();
        let right = PaneId::new();
        let next_tab_pane = PaneId::new();
        let mut session = Session::with_panes("daily", &[left, right]).expect("session");
        let first_tab = session.active_tab;
        let second_tab = session.add_tab(next_tab_pane).expect("second tab");
        session.select_tab(first_tab).expect("select first tab");

        session
            .move_focus_or_tab(Direction::Right)
            .expect("focus right pane");
        assert_eq!(session.active_tab, first_tab);
        assert_eq!(session.active_tab().expect("tab").focused_pane, right);

        session
            .move_focus_or_tab(Direction::Right)
            .expect("fall through to next tab");
        assert_eq!(session.active_tab, second_tab);
    }

    #[test]
    fn resize_moves_the_nearest_boundary_by_five_percent() {
        let top = PaneId::new();
        let bottom = PaneId::new();
        let mut layout = PaneLayout::Split {
            axis: SplitAxis::Vertical,
            ratio: SplitRatio::HALF,
            first: Box::new(PaneLayout::Leaf(top)),
            second: Box::new(PaneLayout::Leaf(bottom)),
        };

        assert!(layout.resize_toward(top, Direction::Down).expect("resize"));
        let PaneLayout::Split { ratio, .. } = layout else {
            panic!("split layout");
        };
        assert_eq!(ratio.thousandths(), 550);
    }

    #[test]
    fn split_ratio_excludes_degenerate_layouts() {
        assert!(SplitRatio::new(500).is_ok());
        assert!(SplitRatio::new(0).is_err());
        assert!(SplitRatio::new(1_000).is_err());
    }
}
