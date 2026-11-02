// SPDX-License-Identifier: GPL-3.0-or-later
//! The workspace model: the plain data the UI shows and agents read.
//!
//! A [`Workspace`] holds its [`Window`]s and the most recently focused one. A
//! window holds its strip [`Pill`]s in order, each naming a [`Group`]; a group
//! holds one or more [`Tab`]s and, when it has two or more, the split view's
//! two columns and remembered heights. A tab holds its folder, history, file
//! selection and scroll anchor.
//!
//! Every operation returns the [`Event`]s it caused, in order. A failed
//! operation changes nothing and returns an error, never events. Nothing here
//! draws anything or knows about pixels: the split view's rules are pure
//! functions in [`self::split`], ported from the spike.

mod split;

#[cfg(test)]
mod tests;

pub use split::{
    Columns, Drop, Heights, Selection, add_to_shorter, apply_drop, column_counts,
    columns_after_insert, columns_after_swap, columns_without, heights_after_change,
    initial_columns, selection_after,
};

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// The most folders a tab remembers in each of its back and forward stacks.
pub const HISTORY_LIMIT: usize = 100;

/// The default width share: half of the width left after the gutter.
pub const DEFAULT_WIDTH_PERMILLE: u16 = 500;

macro_rules! stable_id {
    ($name:ident) => {
        /// A stable id from the workspace's one counter. Numbers are never
        /// reused, so a closed entity's id never names a new one.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u64);

        impl $name {
            /// Build an id from a stored number. Session restore uses this;
            /// the live model always takes ids from its counter.
            pub const fn from_raw(raw: u64) -> Self {
                Self(raw)
            }

            pub const fn raw(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

stable_id!(WindowId);
stable_id!(GroupId);
stable_id!(TabId);

/// One counter for every id in a workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct IdSource {
    next: u64,
}

impl IdSource {
    fn take(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn window(&mut self) -> WindowId {
        WindowId::from_raw(self.take())
    }

    fn group(&mut self) -> GroupId {
        GroupId::from_raw(self.take())
    }

    fn tab(&mut self) -> TabId {
        TabId::from_raw(self.take())
    }
}

/// A tab's file selection: names in its folder, plus the focused name and the
/// Shift anchor. Names are `OsString`, so no file name has to be UTF-8.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileSelection {
    pub names: BTreeSet<OsString>,
    pub focused: Option<OsString>,
    pub anchor: Option<OsString>,
}

impl FileSelection {
    fn clear(&mut self) -> bool {
        let had = !self.names.is_empty() || self.focused.is_some() || self.anchor.is_some();
        self.names.clear();
        self.focused = None;
        self.anchor = None;
        had
    }
}

/// One tab: its folder, history, file selection and scroll anchor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: TabId,
    /// An absolute folder.
    pub folder: PathBuf,
    /// Oldest first, at most [`HISTORY_LIMIT`] entries.
    pub back: Vec<PathBuf>,
    /// Oldest first, at most [`HISTORY_LIMIT`] entries.
    pub forward: Vec<PathBuf>,
    pub selection: FileSelection,
    /// The name of the first visible item. A restore or resize keeps the
    /// place without pixels.
    pub scroll_anchor: Option<OsString>,
}

impl Tab {
    fn new(id: TabId, folder: PathBuf) -> Self {
        Self {
            id,
            folder,
            back: Vec::new(),
            forward: Vec::new(),
            selection: FileSelection::default(),
            scroll_anchor: None,
        }
    }
}

/// The split view's data for one group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitLayout {
    /// The two columns, left first, each top to bottom.
    pub columns: Columns,
    /// Each column's remembered heights.
    pub heights: [Heights; 2],
    /// The left column's share of the width left after the gutter, in
    /// permille (0–1000), never a float.
    pub width_permille: u16,
}

/// One group of tabs, shown as a normal pill with one tab and as the split
/// view with two or more.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub id: GroupId,
    /// The group's tabs, in column order (left column top to bottom, then the
    /// right). The split layout's column lists hold exactly these ids.
    pub tabs: Vec<Tab>,
    pub layout: SplitLayout,
    /// A user-chosen name, if any.
    pub name: Option<String>,
    /// The number in the default "Group N" label: the lowest number not used
    /// by another group in its window when the group was made.
    pub number: u32,
}

impl Group {
    fn new(id: GroupId, number: u32, tabs: Vec<Tab>, columns: Columns) -> Self {
        let mut group = Self {
            id,
            tabs,
            layout: SplitLayout {
                columns,
                heights: [Heights::Default, Heights::Default],
                width_permille: DEFAULT_WIDTH_PERMILLE,
            },
            name: None,
            number,
        };
        reorder_tabs(&mut group);
        group
    }

    /// Whether this group shows as the split view.
    pub fn is_split(&self) -> bool {
        self.tabs.len() >= 2
    }

    /// The default label "Group N", or the chosen name.
    pub fn title(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("Group {}", self.number))
    }

    /// The header pill's label: a split group shows its title and tab count;
    /// a one-tab group is just its tab.
    pub fn label(&self) -> String {
        if self.is_split() {
            format!("{} · {} tabs", self.title(), self.tabs.len())
        } else {
            self.title()
        }
    }

    pub fn first_tab(&self) -> Option<TabId> {
        self.tabs.first().map(|tab| tab.id)
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }
}

/// One window: its strip, its groups, its focused tab and its pill selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: WindowId,
    /// The header's pills in strip order; each names a group.
    pub pills: Vec<GroupId>,
    pub groups: BTreeMap<GroupId, Group>,
    /// The focused tab always belongs to one of this window's groups.
    pub focused_tab: TabId,
    /// The pill selection: one flag per pill, and the Shift anchor (a pill
    /// index), as the spike's strip keeps them.
    pub selection: Selection,
}

impl Window {
    fn new(id: WindowId, group_id: GroupId, group: Group, focused_tab: TabId) -> Self {
        Self {
            id,
            pills: vec![group_id],
            groups: BTreeMap::from([(group_id, group)]),
            focused_tab,
            selection: Selection::new(1),
        }
    }

    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.get(&id)
    }

    pub fn pill_index(&self, id: GroupId) -> Option<usize> {
        self.pills.iter().position(|pill| *pill == id)
    }

    pub fn group_of(&self, tab: TabId) -> Option<&Group> {
        self.groups
            .values()
            .find(|group| group.tabs.iter().any(|candidate| candidate.id == tab))
    }

    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.groups
            .values()
            .find_map(|group| group.tabs.iter().find(|tab| tab.id == id))
    }

    pub fn focused_group(&self) -> Option<&Group> {
        self.group_of(self.focused_tab)
    }

    pub fn focused_pill_index(&self) -> Option<usize> {
        self.focused_group()
            .and_then(|group| self.pill_index(group.id))
    }

    pub fn tab_count(&self) -> usize {
        self.groups.values().map(|group| group.tabs.len()).sum()
    }
}

/// One operation's cause for the UI to animate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    WindowOpened(WindowId),
    WindowClosed(WindowId),
    /// Another window is now the most recently focused one.
    WindowFocused(WindowId),
    /// The last window closed; the app decides to quit.
    LastWindowClosed,
    TabAdded {
        window: WindowId,
        group: GroupId,
        tab: TabId,
        /// The new pill's strip position.
        index: usize,
    },
    TabClosed {
        window: WindowId,
        group: GroupId,
        tab: TabId,
    },
    /// A tab moved between groups, without being closed.
    TabMoved {
        window: WindowId,
        tab: TabId,
        from: GroupId,
        to: GroupId,
    },
    /// A group now shows as the split view.
    GroupFormed {
        window: WindowId,
        group: GroupId,
    },
    /// A group stopped showing as a split view or stopped existing.
    GroupDissolved {
        window: WindowId,
        group: GroupId,
    },
    GroupRenamed {
        window: WindowId,
        group: GroupId,
    },
    FocusChanged {
        window: WindowId,
        tab: TabId,
    },
    /// A pill changed strip position.
    PillMoved {
        window: WindowId,
        from: usize,
        to: usize,
    },
    /// A group's columns, heights or width share changed.
    LayoutChanged {
        window: WindowId,
        group: GroupId,
    },
    SelectionChanged {
        window: WindowId,
    },
    Navigated {
        window: WindowId,
        tab: TabId,
        folder: PathBuf,
    },
    ScrollAnchorChanged {
        window: WindowId,
        tab: TabId,
    },
}

/// Why an operation was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    NoWindow,
    NoFocusedTab,
    NoSuchWindow(WindowId),
    NoSuchGroup(GroupId),
    NoSuchTab(TabId),
    IndexOutOfRange { index: usize, len: usize },
    NotEnoughTabs { selected: usize },
    AlreadyInGroup,
    EmptyGroupName,
    ColumnOutOfRange(usize),
    HeightsLength { expected: usize, got: usize },
    WidthShareOutOfRange(u16),
    DropRefused,
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkspaceError::NoWindow => f.write_str("no window is open"),
            WorkspaceError::NoFocusedTab => f.write_str("no tab is focused"),
            WorkspaceError::NoSuchWindow(id) => write!(f, "window {id} does not exist"),
            WorkspaceError::NoSuchGroup(id) => write!(f, "group {id} does not exist"),
            WorkspaceError::NoSuchTab(id) => write!(f, "tab {id} does not exist"),
            WorkspaceError::IndexOutOfRange { index, len } => {
                write!(f, "index {index} is past the end of {len} pills")
            }
            WorkspaceError::NotEnoughTabs { selected } => {
                write!(f, "a split needs two or more tabs; {selected} selected")
            }
            WorkspaceError::AlreadyInGroup => f.write_str("the tab is already in that group"),
            WorkspaceError::EmptyGroupName => f.write_str("a group name may not be empty"),
            WorkspaceError::ColumnOutOfRange(column) => {
                write!(f, "column {column} does not exist")
            }
            WorkspaceError::HeightsLength { expected, got } => {
                write!(f, "heights need {expected} entries, got {got}")
            }
            WorkspaceError::WidthShareOutOfRange(value) => {
                write!(f, "width share {value} is outside 0–1000 permille")
            }
            WorkspaceError::DropRefused => f.write_str("the drop would empty a column"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

/// A broken model invariant. Only tests (and a future debug build) ask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvariantError {
    pub message: String,
}

impl fmt::Display for InvariantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for InvariantError {}

fn broken(message: impl Into<String>) -> InvariantError {
    InvariantError {
        message: message.into(),
    }
}

/// The whole workspace: its windows and the most recently focused one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub windows: Vec<Window>,
    pub focused_window: Option<WindowId>,
    home: PathBuf,
    ids: IdSource,
}

/// The parsed contents of a session file, before fresh ids are assigned.
///
/// Positions are zero-based: a window's `pills` index its `groups`, a group's
/// `columns` index its `tabs`, and a window's `focused_tab` is a flat index
/// over its tabs in pill order, each group's tabs in column order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreData {
    pub focused_window: usize,
    pub windows: Vec<RestoreWindow>,
}

/// One stored window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreWindow {
    pub pills: Vec<usize>,
    pub groups: Vec<RestoreGroup>,
    pub focused_tab: usize,
}

/// One stored group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreGroup {
    pub name: Option<String>,
    pub number: u32,
    pub width_permille: u16,
    /// `None` is the default, equal-share heights. `Some` is a hand-set list
    /// in design px, which restore checks against the column's tile count.
    pub heights: [Option<Vec<i32>>; 2],
    pub columns: [Vec<usize>; 2],
    pub tabs: Vec<RestoreTab>,
}

/// One stored tab.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreTab {
    pub folder: PathBuf,
    pub back: Vec<PathBuf>,
    pub forward: Vec<PathBuf>,
    pub selection: FileSelection,
    pub scroll_anchor: Option<OsString>,
}

/// A repair the restore made so the result holds the model invariants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreProblem {
    /// The stored columns did not hold exactly the group's tabs; the initial
    /// distribution replaced them.
    Columns { window: usize, group: usize },
    /// A hand heights list did not have one entry per tile; the column became
    /// `Heights::Default`.
    Heights {
        window: usize,
        group: usize,
        column: usize,
    },
    /// A width share above 1000 permille became 500.
    WidthShare {
        window: usize,
        group: usize,
        value: u16,
    },
    /// A focused tab index past the window's tabs became the first tab.
    FocusedTab { window: usize, index: usize },
    /// A history stack over the cap lost its oldest entries.
    HistoryTrimmed {
        window: usize,
        group: usize,
        tab: usize,
        dropped: usize,
    },
    /// A group with no tabs was dropped.
    EmptyGroup { window: usize, group: usize },
    /// A window with no groups left was dropped.
    EmptyWindow { window: usize },
    /// A pill index did not name a stored group; the pill was dropped.
    PillOutOfRange { window: usize, index: usize },
    /// A pill named a group twice; the repeat was dropped.
    DuplicatePill { window: usize, index: usize },
    /// Two groups in one window had the same number; the later one took the
    /// lowest free number.
    DuplicateNumber {
        window: usize,
        group: usize,
        number: u32,
    },
}

impl fmt::Display for RestoreProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Columns { window, group } => write!(
                f,
                "window {} group {}: columns did not match its tabs; used the initial columns",
                window + 1,
                group + 1
            ),
            Self::Heights {
                window,
                group,
                column,
            } => write!(
                f,
                "window {} group {} column {}: heights did not match its tiles; used the default",
                window + 1,
                group + 1,
                column + 1
            ),
            Self::WidthShare {
                window,
                group,
                value,
            } => write!(
                f,
                "window {} group {}: width share {value} is over 1000 permille; used 500",
                window + 1,
                group + 1
            ),
            Self::FocusedTab { window, index } => write!(
                f,
                "window {}: focused tab {index} is out of range; used the first tab",
                window + 1
            ),
            Self::HistoryTrimmed {
                window,
                group,
                tab,
                dropped,
            } => write!(
                f,
                "window {} group {} tab {}: dropped {dropped} oldest history entries",
                window + 1,
                group + 1,
                tab + 1
            ),
            Self::EmptyGroup { window, group } => write!(
                f,
                "window {} group {}: empty group dropped",
                window + 1,
                group + 1
            ),
            Self::EmptyWindow { window } => {
                write!(f, "window {}: empty window dropped", window + 1)
            }
            Self::PillOutOfRange { window, index } => write!(
                f,
                "window {}: pill {index} names no stored group; dropped",
                window + 1
            ),
            Self::DuplicatePill { window, index } => write!(
                f,
                "window {}: pill {index} names a group twice; dropped",
                window + 1
            ),
            Self::DuplicateNumber {
                window,
                group,
                number,
            } => write!(
                f,
                "window {} group {}: number {number} is already used; took the lowest free number",
                window + 1,
                group + 1
            ),
        }
    }
}

impl std::error::Error for RestoreProblem {}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

impl Workspace {
    /// An empty workspace whose fallback folder is `$HOME`.
    pub fn new() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        Self::with_home(home)
    }

    /// An empty workspace with an explicit home, for tests and for a caller
    /// that knows the user's home another way.
    pub fn with_home(home: PathBuf) -> Self {
        Self {
            windows: Vec::new(),
            focused_window: None,
            home,
            ids: IdSource::default(),
        }
    }

    /// Rebuild a workspace from stored data with fresh ids. Broken stored
    /// values are repaired, one [`RestoreProblem`] each, so the result always
    /// passes [`Workspace::check_invariants`].
    pub fn restore(home: PathBuf, data: RestoreData) -> (Self, Vec<RestoreProblem>) {
        let mut workspace = Self::with_home(home);
        let mut problems = Vec::new();
        let mut focused_id = None;

        for (window_index, source) in data.windows.iter().enumerate() {
            let mut pill_indices = Vec::new();
            let mut seen_pills = BTreeSet::new();
            for (index, group_index) in source.pills.iter().enumerate() {
                if *group_index >= source.groups.len() {
                    problems.push(RestoreProblem::PillOutOfRange {
                        window: window_index,
                        index,
                    });
                    continue;
                }
                if !seen_pills.insert(*group_index) {
                    problems.push(RestoreProblem::DuplicatePill {
                        window: window_index,
                        index,
                    });
                    continue;
                }
                pill_indices.push(*group_index);
            }
            if pill_indices.is_empty() {
                problems.push(RestoreProblem::EmptyWindow {
                    window: window_index,
                });
                continue;
            }

            let window_id = workspace.ids.window();
            let mut groups = BTreeMap::new();
            let mut pills = Vec::new();
            let mut flat_tabs = Vec::new();
            let mut numbers = BTreeSet::new();
            let mut kept_group = false;

            for group_index in pill_indices {
                let source_group = &source.groups[group_index];
                if source_group.tabs.is_empty() {
                    problems.push(RestoreProblem::EmptyGroup {
                        window: window_index,
                        group: group_index,
                    });
                    continue;
                }
                kept_group = true;

                let mut tab_ids = Vec::with_capacity(source_group.tabs.len());
                let mut tabs = Vec::with_capacity(source_group.tabs.len());
                for (tab_index, source_tab) in source_group.tabs.iter().enumerate() {
                    let tab_id = workspace.ids.tab();

                    let mut back = source_tab.back.clone();
                    if back.len() > HISTORY_LIMIT {
                        let dropped = back.len() - HISTORY_LIMIT;
                        back.drain(..dropped);
                        problems.push(RestoreProblem::HistoryTrimmed {
                            window: window_index,
                            group: group_index,
                            tab: tab_index,
                            dropped,
                        });
                    }
                    let mut forward = source_tab.forward.clone();
                    if forward.len() > HISTORY_LIMIT {
                        let dropped = forward.len() - HISTORY_LIMIT;
                        forward.drain(..dropped);
                        problems.push(RestoreProblem::HistoryTrimmed {
                            window: window_index,
                            group: group_index,
                            tab: tab_index,
                            dropped,
                        });
                    }

                    tab_ids.push(tab_id);
                    tabs.push(Tab {
                        id: tab_id,
                        folder: source_tab.folder.clone(),
                        back,
                        forward,
                        selection: source_tab.selection.clone(),
                        scroll_anchor: source_tab.scroll_anchor.clone(),
                    });
                }

                let columns = if columns_match(&source_group.columns, tab_ids.len()) {
                    [
                        source_group.columns[0]
                            .iter()
                            .map(|index| tab_ids[*index])
                            .collect(),
                        source_group.columns[1]
                            .iter()
                            .map(|index| tab_ids[*index])
                            .collect(),
                    ]
                } else {
                    problems.push(RestoreProblem::Columns {
                        window: window_index,
                        group: group_index,
                    });
                    initial_columns(&tab_ids)
                };

                let mut heights = [Heights::Default, Heights::Default];
                for column in 0..2 {
                    let Some(values) = &source_group.heights[column] else {
                        continue;
                    };
                    if values.len() == columns[column].len() {
                        heights[column] = Heights::Hand(values.clone());
                    } else {
                        problems.push(RestoreProblem::Heights {
                            window: window_index,
                            group: group_index,
                            column,
                        });
                    }
                }

                let width_permille = if source_group.width_permille > 1000 {
                    problems.push(RestoreProblem::WidthShare {
                        window: window_index,
                        group: group_index,
                        value: source_group.width_permille,
                    });
                    DEFAULT_WIDTH_PERMILLE
                } else {
                    source_group.width_permille
                };

                let mut number = source_group.number;
                if !numbers.insert(number) {
                    let duplicate = number;
                    number = first_free_number(&numbers);
                    numbers.insert(number);
                    problems.push(RestoreProblem::DuplicateNumber {
                        window: window_index,
                        group: group_index,
                        number: duplicate,
                    });
                }

                let group_id = workspace.ids.group();
                let mut group = Group {
                    id: group_id,
                    tabs,
                    layout: SplitLayout {
                        columns,
                        heights,
                        width_permille,
                    },
                    name: source_group.name.clone(),
                    number,
                };
                reorder_tabs(&mut group);
                flat_tabs.extend(
                    group.layout.columns[0]
                        .iter()
                        .chain(group.layout.columns[1].iter())
                        .copied(),
                );
                groups.insert(group_id, group);
                pills.push(group_id);
            }

            if !kept_group {
                problems.push(RestoreProblem::EmptyWindow {
                    window: window_index,
                });
                continue;
            }

            let focused_tab = if source.focused_tab < flat_tabs.len() {
                flat_tabs[source.focused_tab]
            } else {
                problems.push(RestoreProblem::FocusedTab {
                    window: window_index,
                    index: source.focused_tab,
                });
                flat_tabs[0]
            };

            let window = Window {
                id: window_id,
                selection: Selection::new(pills.len()),
                pills,
                groups,
                focused_tab,
            };
            if window_index == data.focused_window {
                focused_id = Some(window_id);
            }
            workspace.windows.push(window);
        }

        workspace.focused_window = if workspace.windows.is_empty() {
            None
        } else {
            focused_id.or_else(|| workspace.windows.first().map(|window| window.id))
        };

        (workspace, problems)
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn focused(&self) -> Option<&Window> {
        let id = self.focused_window?;
        self.windows.iter().find(|window| window.id == id)
    }

    fn focused_index(&self) -> Option<usize> {
        let id = self.focused_window?;
        self.windows.iter().position(|window| window.id == id)
    }

    fn focused_index_or_error(&self) -> Result<usize, WorkspaceError> {
        self.focused_index().ok_or(WorkspaceError::NoWindow)
    }

    /// A new window at `path`, or home. It starts with one tab, focused.
    pub fn window_new(&mut self, path: Option<PathBuf>) -> Result<Vec<Event>, WorkspaceError> {
        let folder = path.unwrap_or_else(|| self.home.clone());
        let window_id = self.ids.window();
        let group_id = self.ids.group();
        let tab_id = self.ids.tab();

        let tab = Tab::new(tab_id, folder);
        let group = Group::new(group_id, 1, vec![tab], initial_columns(&[tab_id]));
        self.windows
            .push(Window::new(window_id, group_id, group, tab_id));
        self.focused_window = Some(window_id);

        Ok(vec![
            Event::WindowOpened(window_id),
            Event::TabAdded {
                window: window_id,
                group: group_id,
                tab: tab_id,
                index: 0,
            },
            Event::FocusChanged {
                window: window_id,
                tab: tab_id,
            },
        ])
    }

    /// Close the focused window. Closing the last one says so, and the app
    /// decides to quit.
    pub fn window_close(&mut self) -> Result<Vec<Event>, WorkspaceError> {
        let index = self.focused_index_or_error()?;
        let window_id = self.windows[index].id;
        self.windows.remove(index);

        let mut events = vec![Event::WindowClosed(window_id)];
        if self.windows.is_empty() {
            self.focused_window = None;
            events.push(Event::LastWindowClosed);
        } else {
            let next = index.min(self.windows.len() - 1);
            let window = &self.windows[next];
            self.focused_window = Some(window.id);
            events.push(Event::WindowFocused(window.id));
            events.push(Event::FocusChanged {
                window: window.id,
                tab: window.focused_tab,
            });
        }
        Ok(events)
    }

    /// Open a tab after the focused pill. With no path it opens the focused
    /// tab's folder, or home when no window is open.
    pub fn tab_new(&mut self, path: Option<PathBuf>) -> Result<Vec<Event>, WorkspaceError> {
        let Some(window_index) = self.focused_index() else {
            return self.window_new(path);
        };

        let (window_id, folder, position) = {
            let window = &self.windows[window_index];
            let position = window
                .focused_pill_index()
                .ok_or(WorkspaceError::NoFocusedTab)?
                + 1;
            let folder = match path {
                Some(path) => path,
                None => window
                    .focused_tab_ref()
                    .ok_or(WorkspaceError::NoFocusedTab)?
                    .folder
                    .clone(),
            };
            (window.id, folder, position)
        };

        let tab_id = self.ids.tab();
        let group_id = self.ids.group();
        let number = next_group_number(&self.windows[window_index]);
        let tab = Tab::new(tab_id, folder);
        let group = Group::new(group_id, number, vec![tab], initial_columns(&[tab_id]));

        let window = &mut self.windows[window_index];
        window.groups.insert(group_id, group);
        window.pills.insert(position, group_id);
        window.selection.selected.insert(position, false);
        let had_selection = window.selection.selected.iter().any(|selected| *selected);
        window.selection.selected.fill(false);
        window.selection.anchor = position;
        window.focused_tab = tab_id;

        let mut events = vec![
            Event::TabAdded {
                window: window_id,
                group: group_id,
                tab: tab_id,
                index: position,
            },
            Event::FocusChanged {
                window: window_id,
                tab: tab_id,
            },
        ];
        if had_selection {
            events.push(Event::SelectionChanged { window: window_id });
        }
        Ok(events)
    }

    /// Close the focused tab. A tab that is the last one in its pill closes
    /// the pill, and a window with no pills closes.
    pub fn tab_close(&mut self) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let (group_id, pill, tab_id, count) = {
            let window = &self.windows[window_index];
            let group = window.focused_group().ok_or(WorkspaceError::NoFocusedTab)?;
            let pill = window
                .pill_index(group.id)
                .ok_or(WorkspaceError::NoSuchGroup(group.id))?;
            (group.id, pill, window.focused_tab, group.tabs.len())
        };

        if count == 1 {
            self.close_pills(window_index, &[pill])
        } else {
            self.close_tab_in_group(window_index, group_id, tab_id)
        }
    }

    /// Close every tab in the selected pills.
    pub fn tab_close_selected(&mut self) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let indices: Vec<usize> = self.windows[window_index]
            .selection
            .selected
            .iter()
            .enumerate()
            .filter_map(|(index, selected)| selected.then_some(index))
            .collect();
        if indices.is_empty() {
            return Ok(Vec::new());
        }
        self.close_pills(window_index, &indices)
    }

    /// Focus the pill at a strip position.
    pub fn tab_focus(&mut self, index: usize) -> Result<Vec<Event>, WorkspaceError> {
        self.pill_click(index)
    }

    /// Move the pill at `from` to strip position `to`.
    pub fn tab_move(&mut self, from: usize, to: usize) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let len = self.windows[window_index].pills.len();
        if from >= len {
            return Err(WorkspaceError::IndexOutOfRange { index: from, len });
        }
        if to >= len {
            return Err(WorkspaceError::IndexOutOfRange { index: to, len });
        }
        if from == to {
            return Ok(Vec::new());
        }

        let window_id = self.windows[window_index].id;
        let window = &mut self.windows[window_index];
        let group_id = window.pills.remove(from);
        window.pills.insert(to, group_id);
        let selected = window.selection.selected.remove(from);
        window.selection.selected.insert(to, selected);
        let had_selection = window.selection.selected.iter().any(|value| *value);
        window.selection.selected.fill(false);
        window.selection.anchor = to;

        let tab_id = window
            .groups
            .get(&group_id)
            .ok_or(WorkspaceError::NoSuchGroup(group_id))?
            .first_tab()
            .ok_or(WorkspaceError::NoFocusedTab)?;
        let old_focus = window.focused_tab;
        window.focused_tab = tab_id;

        let mut events = vec![Event::PillMoved {
            window: window_id,
            from,
            to,
        }];
        if old_focus != tab_id {
            events.push(Event::FocusChanged {
                window: window_id,
                tab: tab_id,
            });
        }
        if had_selection {
            events.push(Event::SelectionChanged { window: window_id });
        }
        Ok(events)
    }

    /// Move the focused tab out of its group and into a new focused window.
    pub fn tab_open_in_new_window(&mut self) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let window_id = self.windows[window_index].id;
        let (group_id, pill, tab_id, source_count) = {
            let window = &self.windows[window_index];
            let group = window.focused_group().ok_or(WorkspaceError::NoFocusedTab)?;
            (
                group.id,
                window
                    .pill_index(group.id)
                    .ok_or(WorkspaceError::NoSuchGroup(group.id))?,
                window.focused_tab,
                group.tabs.len(),
            )
        };

        let new_window_id = self.ids.window();
        let new_group_id = self.ids.group();
        let mut events = Vec::new();
        let moved_tab;

        if source_count == 1 {
            self.windows[window_index].pills.remove(pill);
            self.windows[window_index].selection.selected.remove(pill);
            let group = self.windows[window_index]
                .groups
                .remove(&group_id)
                .ok_or(WorkspaceError::NoSuchGroup(group_id))?;
            moved_tab = group
                .tabs
                .into_iter()
                .next()
                .ok_or(WorkspaceError::NoFocusedTab)?;

            events.push(Event::TabMoved {
                window: window_id,
                tab: tab_id,
                from: group_id,
                to: new_group_id,
            });

            if self.windows[window_index].pills.is_empty() {
                self.windows.remove(window_index);
                events.push(Event::WindowClosed(window_id));
            } else {
                let window = &mut self.windows[window_index];
                let target = pill.min(window.pills.len() - 1);
                let next_group = window.pills[target];
                let next_tab = window
                    .groups
                    .get(&next_group)
                    .and_then(Group::first_tab)
                    .ok_or(WorkspaceError::NoFocusedTab)?;
                window.focused_tab = next_tab;
                window.selection.selected.fill(false);
                window.selection.anchor = target;
                events.push(Event::FocusChanged {
                    window: window_id,
                    tab: next_tab,
                });
            }
        } else {
            let old_columns = self.windows[window_index].groups[&group_id]
                .layout
                .columns
                .clone();
            let old_heights = self.windows[window_index].groups[&group_id]
                .layout
                .heights
                .clone();
            let old_flat = flatten(&old_columns);
            let removed_at = old_flat
                .iter()
                .position(|id| *id == tab_id)
                .ok_or(WorkspaceError::NoSuchTab(tab_id))?;
            let new_columns = columns_without(&old_columns, tab_id);
            let new_heights = heights_after_change(&old_columns, &old_heights, &new_columns);
            let new_flat = flatten(&new_columns);
            let next_focus = new_flat
                .get(removed_at)
                .copied()
                .or_else(|| new_flat.last().copied())
                .ok_or(WorkspaceError::NoFocusedTab)?;

            let group = self.windows[window_index]
                .groups
                .get_mut(&group_id)
                .ok_or(WorkspaceError::NoSuchGroup(group_id))?;
            let position = group
                .tabs
                .iter()
                .position(|tab| tab.id == tab_id)
                .ok_or(WorkspaceError::NoSuchTab(tab_id))?;
            let tab = group.tabs.remove(position);
            moved_tab = tab;
            group.layout.columns = new_columns;
            group.layout.heights = new_heights;
            if group.tabs.len() == 1 {
                let only = group.tabs[0].id;
                group.layout.columns = [vec![only], vec![]];
                group.layout.heights = [Heights::Default, Heights::Default];
            }
            reorder_tabs(group);

            self.windows[window_index].focused_tab = next_focus;
            self.windows[window_index].selection.selected.fill(false);

            events.push(Event::TabMoved {
                window: window_id,
                tab: tab_id,
                from: group_id,
                to: new_group_id,
            });
            if self.windows[window_index].groups[&group_id].tabs.len() == 1 {
                events.push(Event::GroupDissolved {
                    window: window_id,
                    group: group_id,
                });
            }
            events.push(Event::LayoutChanged {
                window: window_id,
                group: group_id,
            });
            events.push(Event::FocusChanged {
                window: window_id,
                tab: next_focus,
            });
        }

        let new_group = Group::new(new_group_id, 1, vec![moved_tab], initial_columns(&[tab_id]));
        self.windows
            .push(Window::new(new_window_id, new_group_id, new_group, tab_id));
        self.focused_window = Some(new_window_id);

        // The new window takes over at once, so this is never
        // `LastWindowClosed`.
        events.push(Event::WindowOpened(new_window_id));
        events.push(Event::FocusChanged {
            window: new_window_id,
            tab: tab_id,
        });
        Ok(events)
    }

    /// Make one split group from the selected pills, at the first selected
    /// pill's position.
    pub fn group_split(&mut self) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let window_id = self.windows[window_index].id;
        let (selected, total) = {
            let window = &self.windows[window_index];
            let selected: Vec<usize> = window
                .selection
                .selected
                .iter()
                .enumerate()
                .filter_map(|(index, value)| value.then_some(index))
                .collect();
            let total = selected
                .iter()
                .map(|index| window.groups[&window.pills[*index]].tabs.len())
                .sum();
            (selected, total)
        };
        if total < 2 {
            return Err(WorkspaceError::NotEnoughTabs { selected: total });
        }

        let first = selected[0];
        let old: Vec<(GroupId, Vec<Tab>)> = selected
            .iter()
            .map(|index| {
                let window = &self.windows[window_index];
                let group_id = window.pills[*index];
                let group = &window.groups[&group_id];
                (group_id, group.tabs.clone())
            })
            .collect();

        let mut tabs = Vec::new();
        for (_, group_tabs) in &old {
            tabs.extend(group_tabs.iter().cloned());
        }

        for index in selected.iter().rev() {
            let group_id = self.windows[window_index].pills.remove(*index);
            self.windows[window_index].groups.remove(&group_id);
            self.windows[window_index].selection.selected.remove(*index);
        }

        let group_id = self.ids.group();
        let number = next_group_number(&self.windows[window_index]);
        let ids: Vec<TabId> = tabs.iter().map(|tab| tab.id).collect();
        let columns = initial_columns(&ids);
        let group = Group::new(group_id, number, tabs, columns);

        let window = &mut self.windows[window_index];
        window.pills.insert(first, group_id);
        window.groups.insert(group_id, group);
        window.selection.selected = vec![false; window.pills.len()];
        window.selection.anchor = first;
        let first_tab = window.groups[&group_id]
            .first_tab()
            .ok_or(WorkspaceError::NoFocusedTab)?;
        window.focused_tab = first_tab;

        let mut events = Vec::new();
        for (old_group, _) in &old {
            events.push(Event::GroupDissolved {
                window: window_id,
                group: *old_group,
            });
        }
        events.push(Event::GroupFormed {
            window: window_id,
            group: group_id,
        });
        events.push(Event::FocusChanged {
            window: window_id,
            tab: first_tab,
        });
        Ok(events)
    }

    /// Close every tab in the focused group. The UI asks for confirmation
    /// first; the model does not.
    pub fn group_close(&mut self) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let window = &self.windows[window_index];
        let group = window.focused_group().ok_or(WorkspaceError::NoFocusedTab)?;
        let pill = window
            .pill_index(group.id)
            .ok_or(WorkspaceError::NoSuchGroup(group.id))?;
        self.close_pills(window_index, &[pill])
    }

    /// Rename the focused group.
    pub fn group_rename(&mut self, name: &str) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let name = name.trim();
        if name.is_empty() {
            return Err(WorkspaceError::EmptyGroupName);
        }
        let window_id = self.windows[window_index].id;
        let group_id = self.windows[window_index]
            .focused_group()
            .ok_or(WorkspaceError::NoFocusedTab)?
            .id;
        let group = self.windows[window_index]
            .groups
            .get_mut(&group_id)
            .ok_or(WorkspaceError::NoSuchGroup(group_id))?;
        group.name = Some(name.to_string());
        Ok(vec![Event::GroupRenamed {
            window: window_id,
            group: group_id,
        }])
    }

    /// Add the focused tab to the pill at `group_index`. A single-tab target
    /// and the tab form a split; a split target takes the tab in its shorter
    /// column and resets that column's heights.
    pub fn group_add_tab(&mut self, group_index: usize) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let window_id = self.windows[window_index].id;
        let (target_group, source_group, tab_id, source_len) = {
            let window = &self.windows[window_index];
            if group_index >= window.pills.len() {
                return Err(WorkspaceError::IndexOutOfRange {
                    index: group_index,
                    len: window.pills.len(),
                });
            }
            let target_group = window.pills[group_index];
            let tab_id = window.focused_tab;
            let source_group = window
                .group_of(tab_id)
                .ok_or(WorkspaceError::NoFocusedTab)?
                .id;
            let source_len = window.groups[&source_group].tabs.len();
            (target_group, source_group, tab_id, source_len)
        };
        if target_group == source_group {
            return Err(WorkspaceError::AlreadyInGroup);
        }

        let mut events = Vec::new();
        let tab;
        let source_removed;
        let source_split_before;
        let source_split_after;

        if source_len == 1 {
            let source_pill = self.windows[window_index]
                .pill_index(source_group)
                .ok_or(WorkspaceError::NoSuchGroup(source_group))?;
            self.windows[window_index].pills.remove(source_pill);
            self.windows[window_index]
                .selection
                .selected
                .remove(source_pill);
            source_removed = true;
            source_split_before = false;
            source_split_after = false;
            let group = self.windows[window_index]
                .groups
                .remove(&source_group)
                .ok_or(WorkspaceError::NoSuchGroup(source_group))?;
            tab = group
                .tabs
                .into_iter()
                .next()
                .ok_or(WorkspaceError::NoSuchTab(tab_id))?;
        } else {
            source_removed = false;
            let group = self.windows[window_index]
                .groups
                .get_mut(&source_group)
                .ok_or(WorkspaceError::NoSuchGroup(source_group))?;
            source_split_before = group.is_split();
            let position = group
                .tabs
                .iter()
                .position(|candidate| candidate.id == tab_id)
                .ok_or(WorkspaceError::NoSuchTab(tab_id))?;
            tab = group.tabs.remove(position);
            let old_columns = group.layout.columns.clone();
            let old_heights = group.layout.heights.clone();
            let new_columns = columns_without(&old_columns, tab_id);
            let new_heights = heights_after_change(&old_columns, &old_heights, &new_columns);
            group.layout.columns = new_columns;
            group.layout.heights = new_heights;
            if group.tabs.len() == 1 {
                let only = group.tabs[0].id;
                group.layout.columns = [vec![only], vec![]];
                group.layout.heights = [Heights::Default, Heights::Default];
            }
            reorder_tabs(group);
            source_split_after = group.is_split();
        }

        let target_pill = self.windows[window_index]
            .pill_index(target_group)
            .ok_or(WorkspaceError::NoSuchGroup(target_group))?;
        let target_split_before;
        let target_split_after;
        {
            let group = self.windows[window_index]
                .groups
                .get_mut(&target_group)
                .ok_or(WorkspaceError::NoSuchGroup(target_group))?;
            target_split_before = group.is_split();
            let old_columns = group.layout.columns.clone();
            let old_heights = group.layout.heights.clone();
            add_to_shorter(&mut group.layout.columns, tab_id);
            group.layout.heights =
                heights_after_change(&old_columns, &old_heights, &group.layout.columns);
            group.tabs.push(tab);
            reorder_tabs(group);
            target_split_after = group.is_split();
        }

        let window = &mut self.windows[window_index];
        window.focused_tab = tab_id;
        let had_selection = window.selection.selected.iter().any(|value| *value);
        window.selection.selected.fill(false);
        window.selection.anchor = target_pill;

        events.push(Event::TabMoved {
            window: window_id,
            tab: tab_id,
            from: source_group,
            to: target_group,
        });
        if source_removed || (source_split_before && !source_split_after) {
            events.push(Event::GroupDissolved {
                window: window_id,
                group: source_group,
            });
        }
        if !source_removed {
            events.push(Event::LayoutChanged {
                window: window_id,
                group: source_group,
            });
        }
        if !target_split_before && target_split_after {
            events.push(Event::GroupFormed {
                window: window_id,
                group: target_group,
            });
        }
        events.push(Event::LayoutChanged {
            window: window_id,
            group: target_group,
        });
        if had_selection {
            events.push(Event::SelectionChanged { window: window_id });
        }

        Ok(events)
    }

    /// Go to a folder, optionally selecting one name in it. The caller decides
    /// whether a path is a file and passes its parent folder plus the name.
    pub fn nav_go(
        &mut self,
        folder: PathBuf,
        select: Option<OsString>,
    ) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let window_id = self.windows[window_index].id;
        let tab_id = self.windows[window_index].focused_tab;
        let group_id = self.windows[window_index]
            .group_of(tab_id)
            .ok_or(WorkspaceError::NoFocusedTab)?
            .id;
        let tab = self.windows[window_index]
            .groups
            .get_mut(&group_id)
            .and_then(|group| group.tab_mut(tab_id))
            .ok_or(WorkspaceError::NoFocusedTab)?;

        if tab.folder == folder {
            return Ok(Vec::new());
        }

        push_history(&mut tab.back, tab.folder.clone());
        tab.forward.clear();
        tab.folder = folder.clone();
        tab.selection.clear();
        tab.scroll_anchor = None;
        if let Some(name) = select {
            tab.selection.names.insert(name.clone());
            tab.selection.focused = Some(name.clone());
            tab.selection.anchor = Some(name);
        }

        Ok(vec![Event::Navigated {
            window: window_id,
            tab: tab_id,
            folder,
        }])
    }

    /// Go back in the focused tab's history. An empty stack does nothing.
    pub fn nav_back(&mut self) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let window_id = self.windows[window_index].id;
        let tab_id = self.windows[window_index].focused_tab;
        let group_id = self.windows[window_index]
            .group_of(tab_id)
            .ok_or(WorkspaceError::NoFocusedTab)?
            .id;
        let tab = self.windows[window_index]
            .groups
            .get_mut(&group_id)
            .and_then(|group| group.tab_mut(tab_id))
            .ok_or(WorkspaceError::NoFocusedTab)?;

        let Some(folder) = tab.back.pop() else {
            return Ok(Vec::new());
        };
        push_history(&mut tab.forward, tab.folder.clone());
        tab.folder = folder.clone();
        tab.selection.clear();
        tab.scroll_anchor = None;

        Ok(vec![Event::Navigated {
            window: window_id,
            tab: tab_id,
            folder,
        }])
    }

    /// Go forward in the focused tab's history. An empty stack does nothing.
    pub fn nav_forward(&mut self) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let window_id = self.windows[window_index].id;
        let tab_id = self.windows[window_index].focused_tab;
        let group_id = self.windows[window_index]
            .group_of(tab_id)
            .ok_or(WorkspaceError::NoFocusedTab)?
            .id;
        let tab = self.windows[window_index]
            .groups
            .get_mut(&group_id)
            .and_then(|group| group.tab_mut(tab_id))
            .ok_or(WorkspaceError::NoFocusedTab)?;

        let Some(folder) = tab.forward.pop() else {
            return Ok(Vec::new());
        };
        push_history(&mut tab.back, tab.folder.clone());
        tab.folder = folder.clone();
        tab.selection.clear();
        tab.scroll_anchor = None;

        Ok(vec![Event::Navigated {
            window: window_id,
            tab: tab_id,
            folder,
        }])
    }

    /// Go to the parent folder. At `/` this does nothing.
    pub fn nav_up(&mut self) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let current = self.windows[window_index]
            .focused_tab_ref()
            .ok_or(WorkspaceError::NoFocusedTab)?
            .folder
            .clone();
        if current == Path::new("/") {
            return Ok(Vec::new());
        }
        let Some(parent) = current.parent() else {
            return Ok(Vec::new());
        };
        let parent = parent.to_path_buf();
        if parent == current {
            return Ok(Vec::new());
        }
        self.nav_go(parent, None)
    }

    /// Select every item named by the handler; the model has no folder
    /// listing of its own.
    pub fn selection_all(&mut self, names: &[OsString]) -> Result<Vec<Event>, WorkspaceError> {
        let (window_id, _, tab) = self.focused_tab_mut()?;
        tab.selection.names = names.iter().cloned().collect();
        tab.selection.focused = None;
        tab.selection.anchor = None;
        Ok(vec![Event::SelectionChanged { window: window_id }])
    }

    /// Clear the focused tab's file selection.
    pub fn selection_none(&mut self) -> Result<Vec<Event>, WorkspaceError> {
        let (window_id, _, tab) = self.focused_tab_mut()?;
        tab.selection.clear();
        Ok(vec![Event::SelectionChanged { window: window_id }])
    }

    /// A plain click on a pill: focus its first tab, clear the pill selection,
    /// and move the Shift anchor.
    pub fn pill_click(&mut self, index: usize) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        if index >= self.windows[window_index].pills.len() {
            return Err(WorkspaceError::IndexOutOfRange {
                index,
                len: self.windows[window_index].pills.len(),
            });
        }
        let window_id = self.windows[window_index].id;
        let window = &mut self.windows[window_index];
        let group_id = window.pills[index];
        let tab_id = window.groups[&group_id]
            .first_tab()
            .ok_or(WorkspaceError::NoFocusedTab)?;
        let old_focus = window.focused_tab;
        let had_selection = window.selection.selected.iter().any(|value| *value);
        window.focused_tab = tab_id;
        window.selection.selected.fill(false);
        window.selection.anchor = index;

        let mut events = Vec::new();
        if old_focus != tab_id {
            events.push(Event::FocusChanged {
                window: window_id,
                tab: tab_id,
            });
        }
        if had_selection {
            events.push(Event::SelectionChanged { window: window_id });
        }
        Ok(events)
    }

    /// A Ctrl-click on a pill: toggle it without moving the Shift anchor.
    pub fn pill_ctrl_click(&mut self, index: usize) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        if index >= self.windows[window_index].pills.len() {
            return Err(WorkspaceError::IndexOutOfRange {
                index,
                len: self.windows[window_index].pills.len(),
            });
        }
        let window_id = self.windows[window_index].id;
        let window = &mut self.windows[window_index];
        let group_id = window.pills[index];
        let tab_id = window.groups[&group_id]
            .first_tab()
            .ok_or(WorkspaceError::NoFocusedTab)?;
        let old_focus = window.focused_tab;
        window.selection.selected[index] = !window.selection.selected[index];
        window.focused_tab = tab_id;

        let mut events = vec![Event::SelectionChanged { window: window_id }];
        if old_focus != tab_id {
            events.push(Event::FocusChanged {
                window: window_id,
                tab: tab_id,
            });
        }
        Ok(events)
    }

    /// A Shift-click on a pill: select from the anchor to it.
    pub fn pill_shift_click(&mut self, index: usize) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        if index >= self.windows[window_index].pills.len() {
            return Err(WorkspaceError::IndexOutOfRange {
                index,
                len: self.windows[window_index].pills.len(),
            });
        }
        let window_id = self.windows[window_index].id;
        let window = &mut self.windows[window_index];
        let group_id = window.pills[index];
        let tab_id = window.groups[&group_id]
            .first_tab()
            .ok_or(WorkspaceError::NoFocusedTab)?;
        let old_focus = window.focused_tab;
        window.selection = selection_after(&window.selection, index, false, true);
        window.focused_tab = tab_id;

        let mut events = vec![Event::SelectionChanged { window: window_id }];
        if old_focus != tab_id {
            events.push(Event::FocusChanged {
                window: window_id,
                tab: tab_id,
            });
        }
        Ok(events)
    }

    /// Focus a tab, used when a split tile's own pill is clicked.
    pub fn focus_tab(&mut self, tab: TabId) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let window = &self.windows[window_index];
        if window.tab(tab).is_none() {
            return Err(WorkspaceError::NoSuchTab(tab));
        }
        if window.focused_tab == tab {
            return Ok(Vec::new());
        }
        let window_id = window.id;
        self.windows[window_index].focused_tab = tab;
        Ok(vec![Event::FocusChanged {
            window: window_id,
            tab,
        }])
    }

    /// A plain click on a file: focus it, select only it, and move the anchor.
    pub fn file_click(&mut self, name: OsString) -> Result<Vec<Event>, WorkspaceError> {
        let (window_id, _, tab) = self.focused_tab_mut()?;
        tab.selection.names.clear();
        tab.selection.names.insert(name.clone());
        tab.selection.focused = Some(name.clone());
        tab.selection.anchor = Some(name);
        Ok(vec![Event::SelectionChanged { window: window_id }])
    }

    /// A Ctrl-click on a file: toggle it without moving the anchor.
    pub fn file_ctrl_click(&mut self, name: OsString) -> Result<Vec<Event>, WorkspaceError> {
        let (window_id, _, tab) = self.focused_tab_mut()?;
        if !tab.selection.names.remove(&name) {
            tab.selection.names.insert(name.clone());
        }
        tab.selection.focused = Some(name);
        Ok(vec![Event::SelectionChanged { window: window_id }])
    }

    /// A Shift-click on a file. `order` is the visible order the handler sees,
    /// because the model has no folder listing; the range runs from the anchor
    /// to `name` in that order.
    pub fn file_shift_click(
        &mut self,
        name: OsString,
        order: &[OsString],
    ) -> Result<Vec<Event>, WorkspaceError> {
        let (window_id, _, tab) = self.focused_tab_mut()?;
        let clicked = order.iter().position(|candidate| *candidate == name);
        let anchor = tab
            .selection
            .anchor
            .as_ref()
            .and_then(|anchor| order.iter().position(|candidate| candidate == anchor));
        if let (Some(anchor), Some(clicked)) = (anchor, clicked) {
            let (from, to) = (anchor.min(clicked), anchor.max(clicked));
            tab.selection.names = order[from..=to].iter().cloned().collect();
        } else {
            tab.selection.names.clear();
            tab.selection.names.insert(name.clone());
            tab.selection.anchor = Some(name.clone());
        }
        tab.selection.focused = Some(name);
        Ok(vec![Event::SelectionChanged { window: window_id }])
    }

    /// Record the first visible item's name.
    pub fn record_scroll_anchor(
        &mut self,
        name: Option<OsString>,
    ) -> Result<Vec<Event>, WorkspaceError> {
        let (window_id, tab_id, tab) = self.focused_tab_mut()?;
        if tab.scroll_anchor == name {
            return Ok(Vec::new());
        }
        tab.scroll_anchor = name;
        Ok(vec![Event::ScrollAnchorChanged {
            window: window_id,
            tab: tab_id,
        }])
    }

    /// Apply a tile drop to the group that holds `from`.
    pub fn apply_tile_drop(
        &mut self,
        from: TabId,
        drop: Drop,
    ) -> Result<Vec<Event>, WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let window_id = self.windows[window_index].id;
        let group_id = self.windows[window_index]
            .groups
            .values()
            .find(|group| group.tabs.iter().any(|tab| tab.id == from))
            .map(|group| group.id)
            .ok_or(WorkspaceError::NoSuchTab(from))?;

        let new_columns = {
            let group = &self.windows[window_index].groups[&group_id];
            apply_drop(&group.layout.columns, from, drop).ok_or(WorkspaceError::DropRefused)?
        };
        let group = self.windows[window_index]
            .groups
            .get_mut(&group_id)
            .ok_or(WorkspaceError::NoSuchGroup(group_id))?;
        let new_heights =
            heights_after_change(&group.layout.columns, &group.layout.heights, &new_columns);
        group.layout.columns = new_columns;
        group.layout.heights = new_heights;
        reorder_tabs(group);

        Ok(vec![Event::LayoutChanged {
            window: window_id,
            group: group_id,
        }])
    }

    /// Set one column's remembered heights after a border drag.
    pub fn set_column_heights(
        &mut self,
        column: usize,
        heights: Heights,
    ) -> Result<Vec<Event>, WorkspaceError> {
        if column > 1 {
            return Err(WorkspaceError::ColumnOutOfRange(column));
        }
        let window_index = self.focused_index_or_error()?;
        let window_id = self.windows[window_index].id;
        let group_id = self.windows[window_index]
            .focused_group()
            .ok_or(WorkspaceError::NoFocusedTab)?
            .id;
        let group = self.windows[window_index]
            .groups
            .get_mut(&group_id)
            .ok_or(WorkspaceError::NoSuchGroup(group_id))?;
        if let Heights::Hand(values) = &heights {
            let expected = group.layout.columns[column].len();
            if values.len() != expected {
                return Err(WorkspaceError::HeightsLength {
                    expected,
                    got: values.len(),
                });
            }
        }
        group.layout.heights[column] = heights;
        Ok(vec![Event::LayoutChanged {
            window: window_id,
            group: group_id,
        }])
    }

    /// Set the left column's width share after the column border is dragged.
    pub fn set_width_share(&mut self, permille: u16) -> Result<Vec<Event>, WorkspaceError> {
        if permille > 1000 {
            return Err(WorkspaceError::WidthShareOutOfRange(permille));
        }
        let window_index = self.focused_index_or_error()?;
        let window_id = self.windows[window_index].id;
        let group_id = self.windows[window_index]
            .focused_group()
            .ok_or(WorkspaceError::NoFocusedTab)?
            .id;
        let group = self.windows[window_index]
            .groups
            .get_mut(&group_id)
            .ok_or(WorkspaceError::NoSuchGroup(group_id))?;
        group.layout.width_permille = permille;
        Ok(vec![Event::LayoutChanged {
            window: window_id,
            group: group_id,
        }])
    }

    /// The second-launch policy. With no paths, a new window at home; with one
    /// or more folders, one new tab per folder in the most recently focused
    /// window, the last one focused.
    pub fn open_request(&mut self, paths: &[PathBuf]) -> Result<Vec<Event>, WorkspaceError> {
        if paths.is_empty() {
            return self.window_new(None);
        }

        let mut events = Vec::new();
        if self.focused_window.is_none() {
            events.extend(self.window_new(Some(paths[0].clone()))?);
            for path in &paths[1..] {
                events.extend(self.tab_new(Some(path.clone()))?);
            }
        } else {
            for path in paths {
                events.extend(self.tab_new(Some(path.clone()))?);
            }
        }
        Ok(events)
    }

    /// Check the model's invariants after an operation.
    pub fn check_invariants(&self) -> Result<(), InvariantError> {
        let mut ids: BTreeSet<u64> = BTreeSet::new();

        for window in &self.windows {
            if !ids.insert(window.id.raw()) {
                return Err(broken(format!("window id {} appears twice", window.id)));
            }
            if window.pills.len() != window.selection.selected.len() {
                return Err(broken(format!(
                    "window {} has {} pills but {} selected flags",
                    window.id,
                    window.pills.len(),
                    window.selection.selected.len()
                )));
            }
            if !window.pills.is_empty() && window.selection.anchor >= window.pills.len() {
                return Err(broken(format!(
                    "window {} has anchor {} past {} pills",
                    window.id,
                    window.selection.anchor,
                    window.pills.len()
                )));
            }

            let mut seen_groups: BTreeSet<GroupId> = BTreeSet::new();
            let mut numbers: BTreeSet<u32> = BTreeSet::new();
            for group_id in &window.pills {
                if !seen_groups.insert(*group_id) {
                    return Err(broken(format!(
                        "group id {group_id} appears twice in window {}",
                        window.id
                    )));
                }
                if !ids.insert(group_id.raw()) {
                    return Err(broken(format!("group id {group_id} appears twice")));
                }
                let group = window.groups.get(group_id).ok_or_else(|| {
                    broken(format!(
                        "window {} lists missing group {group_id}",
                        window.id
                    ))
                })?;
                if group.id != *group_id {
                    return Err(broken(format!(
                        "group {group_id} is stored under another id"
                    )));
                }
                if !numbers.insert(group.number) {
                    return Err(broken(format!(
                        "window {} uses group number {} twice",
                        window.id, group.number
                    )));
                }
                if group.tabs.is_empty() {
                    return Err(broken(format!("group {group_id} has no tabs")));
                }
                if group.layout.width_permille > 1000 {
                    return Err(broken(format!(
                        "group {group_id} width share {} is outside 0–1000",
                        group.layout.width_permille
                    )));
                }

                let mut tab_ids: BTreeSet<TabId> = BTreeSet::new();
                for tab in &group.tabs {
                    if !tab_ids.insert(tab.id) {
                        return Err(broken(format!(
                            "tab id {} appears twice in group {group_id}",
                            tab.id
                        )));
                    }
                    if !ids.insert(tab.id.raw()) {
                        return Err(broken(format!("tab id {} appears twice", tab.id)));
                    }
                    if tab.back.len() > HISTORY_LIMIT {
                        return Err(broken(format!(
                            "tab {} has {} back entries",
                            tab.id,
                            tab.back.len()
                        )));
                    }
                    if tab.forward.len() > HISTORY_LIMIT {
                        return Err(broken(format!(
                            "tab {} has {} forward entries",
                            tab.id,
                            tab.forward.len()
                        )));
                    }
                }

                let mut column_ids: BTreeSet<TabId> = BTreeSet::new();
                for column in 0..2 {
                    if let Heights::Hand(values) = &group.layout.heights[column]
                        && values.len() != group.layout.columns[column].len()
                    {
                        return Err(broken(format!(
                            "group {group_id} column {column} has {} heights for {} tiles",
                            values.len(),
                            group.layout.columns[column].len()
                        )));
                    }
                    for tab_id in &group.layout.columns[column] {
                        if !column_ids.insert(*tab_id) {
                            return Err(broken(format!(
                                "tab id {tab_id} is in group {group_id}'s columns twice"
                            )));
                        }
                        if !tab_ids.contains(tab_id) {
                            return Err(broken(format!(
                                "group {group_id}'s columns hold unknown tab {tab_id}"
                            )));
                        }
                    }
                }
                if column_ids != tab_ids {
                    return Err(broken(format!(
                        "group {group_id}'s columns do not hold exactly its tabs"
                    )));
                }
                if group.tabs.len() >= 2
                    && (group.layout.columns[0].is_empty() || group.layout.columns[1].is_empty())
                {
                    return Err(broken(format!(
                        "group {group_id} has {} tabs but an empty column",
                        group.tabs.len()
                    )));
                }
            }

            if seen_groups.len() != window.groups.len() {
                return Err(broken(format!(
                    "window {} has groups that its pills do not list",
                    window.id
                )));
            }
            if window.group_of(window.focused_tab).is_none() {
                return Err(broken(format!(
                    "window {} focuses tab {} outside the window",
                    window.id, window.focused_tab
                )));
            }
        }

        if self.windows.is_empty() {
            if self.focused_window.is_some() {
                return Err(broken("an empty workspace still has a focused window"));
            }
        } else {
            let focused = self
                .focused_window
                .ok_or_else(|| broken("a non-empty workspace has no focused window"))?;
            if !self.windows.iter().any(|window| window.id == focused) {
                return Err(broken(format!(
                    "the focused window {focused} does not exist"
                )));
            }
        }

        Ok(())
    }

    fn focused_tab_mut(&mut self) -> Result<(WindowId, TabId, &mut Tab), WorkspaceError> {
        let window_index = self.focused_index_or_error()?;
        let window_id = self.windows[window_index].id;
        let tab_id = self.windows[window_index].focused_tab;
        let group_id = self.windows[window_index]
            .group_of(tab_id)
            .ok_or(WorkspaceError::NoFocusedTab)?
            .id;
        let tab = self.windows[window_index]
            .groups
            .get_mut(&group_id)
            .and_then(|group| group.tab_mut(tab_id))
            .ok_or(WorkspaceError::NoFocusedTab)?;
        Ok((window_id, tab_id, tab))
    }

    fn close_tab_in_group(
        &mut self,
        window_index: usize,
        group_id: GroupId,
        tab_id: TabId,
    ) -> Result<Vec<Event>, WorkspaceError> {
        let window_id = self.windows[window_index].id;
        let window = &mut self.windows[window_index];
        let group = window
            .groups
            .get_mut(&group_id)
            .ok_or(WorkspaceError::NoSuchGroup(group_id))?;
        let old_focus = window.focused_tab;
        let old_count = group.tabs.len();
        let old_columns = group.layout.columns.clone();
        let old_heights = group.layout.heights.clone();
        let old_flat = flatten(&old_columns);
        let removed_at = old_flat
            .iter()
            .position(|id| *id == tab_id)
            .ok_or(WorkspaceError::NoSuchTab(tab_id))?;
        let new_columns = columns_without(&old_columns, tab_id);
        let new_heights = heights_after_change(&old_columns, &old_heights, &new_columns);
        let new_flat = flatten(&new_columns);
        let next_focus = new_flat
            .get(removed_at)
            .copied()
            .or_else(|| new_flat.last().copied())
            .ok_or(WorkspaceError::NoFocusedTab)?;

        group.tabs.retain(|tab| tab.id != tab_id);
        group.layout.columns = new_columns;
        group.layout.heights = new_heights;
        if group.tabs.len() == 1 {
            let only = group.tabs[0].id;
            group.layout.columns = [vec![only], vec![]];
            group.layout.heights = [Heights::Default, Heights::Default];
        }
        reorder_tabs(group);

        window.focused_tab = next_focus;
        window.selection.selected.fill(false);

        let mut events = vec![Event::TabClosed {
            window: window_id,
            group: group_id,
            tab: tab_id,
        }];
        if old_count >= 2 && window.groups[&group_id].tabs.len() == 1 {
            events.push(Event::GroupDissolved {
                window: window_id,
                group: group_id,
            });
        }
        events.push(Event::LayoutChanged {
            window: window_id,
            group: group_id,
        });
        if old_focus != next_focus {
            events.push(Event::FocusChanged {
                window: window_id,
                tab: next_focus,
            });
        }
        Ok(events)
    }

    fn close_pills(
        &mut self,
        window_index: usize,
        indices: &[usize],
    ) -> Result<Vec<Event>, WorkspaceError> {
        let window_id = self.windows[window_index].id;
        let mut indices = indices.to_vec();
        indices.sort_unstable();
        indices.dedup();
        if indices.is_empty() {
            return Ok(Vec::new());
        }
        let len = self.windows[window_index].pills.len();
        if let Some(index) = indices.iter().find(|index| **index >= len) {
            return Err(WorkspaceError::IndexOutOfRange { index: *index, len });
        }

        let first = indices[0];
        let removed: Vec<(GroupId, Vec<TabId>, bool)> = indices
            .iter()
            .map(|index| {
                let group_id = self.windows[window_index].pills[*index];
                let group = &self.windows[window_index].groups[&group_id];
                (
                    group_id,
                    group.tabs.iter().map(|tab| tab.id).collect(),
                    group.is_split(),
                )
            })
            .collect();
        let had_selection = self.windows[window_index]
            .selection
            .selected
            .iter()
            .any(|value| *value);

        for index in indices.iter().rev() {
            let group_id = self.windows[window_index].pills.remove(*index);
            self.windows[window_index].groups.remove(&group_id);
            self.windows[window_index].selection.selected.remove(*index);
        }

        let mut events = Vec::new();
        for (group_id, tabs, split) in &removed {
            for tab_id in tabs {
                events.push(Event::TabClosed {
                    window: window_id,
                    group: *group_id,
                    tab: *tab_id,
                });
            }
            if *split {
                events.push(Event::GroupDissolved {
                    window: window_id,
                    group: *group_id,
                });
            }
        }

        if self.windows[window_index].pills.is_empty() {
            self.windows.remove(window_index);
            events.push(Event::WindowClosed(window_id));
            if self.windows.is_empty() {
                self.focused_window = None;
                events.push(Event::LastWindowClosed);
            } else {
                let next = window_index.min(self.windows.len() - 1);
                let window = &self.windows[next];
                self.focused_window = Some(window.id);
                events.push(Event::WindowFocused(window.id));
                events.push(Event::FocusChanged {
                    window: window.id,
                    tab: window.focused_tab,
                });
            }
        } else {
            let window = &mut self.windows[window_index];
            let target = first.min(window.pills.len() - 1);
            let group_id = window.pills[target];
            let tab_id = window.groups[&group_id]
                .first_tab()
                .ok_or(WorkspaceError::NoFocusedTab)?;
            window.focused_tab = tab_id;
            window.selection.selected.fill(false);
            window.selection.anchor = target;
            events.push(Event::FocusChanged {
                window: window_id,
                tab: tab_id,
            });
            if had_selection {
                events.push(Event::SelectionChanged { window: window_id });
            }
        }

        Ok(events)
    }
}

impl Window {
    fn focused_tab_ref(&self) -> Option<&Tab> {
        self.tab(self.focused_tab)
    }
}

/// Whether a stored column list holds every tab exactly once and leaves no
/// column empty when there are two or more tabs.
fn columns_match(columns: &[Vec<usize>; 2], tab_count: usize) -> bool {
    if tab_count == 0 {
        return false;
    }

    let mut seen = BTreeSet::new();
    for column in columns {
        for index in column {
            if *index >= tab_count || !seen.insert(*index) {
                return false;
            }
        }
    }

    if seen.len() != tab_count {
        return false;
    }
    if tab_count >= 2 && (columns[0].is_empty() || columns[1].is_empty()) {
        return false;
    }
    true
}

/// The lowest positive group number not already used in a window.
fn first_free_number(used: &BTreeSet<u32>) -> u32 {
    let mut number = 1;
    while used.contains(&number) {
        number += 1;
    }
    number
}

fn flatten(columns: &Columns) -> Vec<TabId> {
    columns[0]
        .iter()
        .chain(columns[1].iter())
        .copied()
        .collect()
}

fn reorder_tabs(group: &mut Group) {
    let order = flatten(&group.layout.columns);
    let mut by_id: BTreeMap<TabId, Tab> = std::mem::take(&mut group.tabs)
        .into_iter()
        .map(|tab| (tab.id, tab))
        .collect();
    group.tabs = order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .collect();
    // A tab missing from the columns would violate an invariant; keep it so
    // the check reports it rather than silently dropping a tab.
    group.tabs.extend(by_id.into_values());
}

fn next_group_number(window: &Window) -> u32 {
    let mut number = 1;
    while window.groups.values().any(|group| group.number == number) {
        number += 1;
    }
    number
}

fn push_history(stack: &mut Vec<PathBuf>, folder: PathBuf) {
    if stack.len() >= HISTORY_LIMIT {
        stack.remove(0);
    }
    stack.push(folder);
}
