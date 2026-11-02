// SPDX-License-Identifier: GPL-3.0-or-later
//! The session file: the workspace's windows, groups and tabs as TOML.
//!
//! The app owns this file. It lives at
//! `$XDG_STATE_HOME/gimbal-files/session.toml`, or under
//! `~/.local/state` when `XDG_STATE_HOME` is unset or relative. Every save
//! writes the whole document through [`crate::fsutil::save_atomic`].
//!
//! Ids are never written. Groups refer to their tabs by position, windows
//! refer to their groups by position, and a restore gives every window, group
//! and tab a fresh id. A path or name that is not UTF-8 is written as an inline
//! table `{ bytes = [ … ] }`; both forms are read.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use toml_edit::{Array, ArrayOfTables, Document, DocumentMut, InlineTable, Item, Table, Value};

use crate::fsutil;
use crate::workspace::{
    DEFAULT_WIDTH_PERMILLE, FileSelection, Group, GroupId, Heights, RestoreData, RestoreGroup,
    RestoreProblem, RestoreTab, RestoreWindow, Tab, TabId, Window, Workspace,
};

pub const SESSION_DIR: &str = "gimbal-files";
pub const SESSION_FILE: &str = "session.toml";
pub const SESSION_VERSION: i64 = 1;

/// The session path as a pure function of `XDG_STATE_HOME` and the home
/// folder. A relative or missing XDG directory falls back to
/// `~/.local/state`.
pub fn session_path(xdg_state_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    match xdg_state_home {
        Some(directory) if directory.is_absolute() => {
            Some(directory.join(SESSION_DIR).join(SESSION_FILE))
        }
        _ => home.map(|home| {
            home.join(".local")
                .join("state")
                .join(SESSION_DIR)
                .join(SESSION_FILE)
        }),
    }
}

/// The session path from the process environment and `HOME`.
pub fn default_session_path() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    session_path(xdg.as_deref(), home.as_deref())
}

/// What was wrong with a session file, or what `load` had to move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionProblem {
    /// The file could not be read.
    Read { path: PathBuf, message: String },
    /// The file was not valid TOML.
    Parse { path: PathBuf, message: String },
    /// The file's `version` was missing or not 1.
    UnsupportedVersion { path: PathBuf, version: Option<i64> },
    /// A tab's folder no longer exists; it opened its nearest existing parent,
    /// or home.
    MissingFolder { path: PathBuf, fallback: PathBuf },
    /// A repair [`Workspace::restore`] made.
    Restore(RestoreProblem),
}

impl fmt::Display for SessionProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, message } => {
                write!(
                    f,
                    "{}: could not read the session: {message}",
                    path.display()
                )
            }
            Self::Parse { path, message } => {
                write!(
                    f,
                    "{}: could not parse the session: {message}",
                    path.display()
                )
            }
            Self::UnsupportedVersion { path, version } => match version {
                Some(version) => write!(
                    f,
                    "{}: session version {version} is not supported; expected {SESSION_VERSION}",
                    path.display()
                ),
                None => write!(
                    f,
                    "{}: the session has no version; expected {SESSION_VERSION}",
                    path.display()
                ),
            },
            Self::MissingFolder { path, fallback } => write!(
                f,
                "{}: folder no longer exists; opened {}",
                path.display(),
                fallback.display()
            ),
            Self::Restore(problem) => problem.fmt(f),
        }
    }
}

impl std::error::Error for SessionProblem {}

/// Save the workspace, unless it is empty. An empty workspace leaves the
/// previous file alone, so closing the last window does not erase its tabs.
pub fn save(path: &Path, workspace: &Workspace) -> io::Result<()> {
    if workspace.windows.is_empty() {
        return Ok(());
    }

    let document = document_for(workspace);
    fsutil::save_atomic(path, &document.to_string())
}

/// Load the workspace. A missing file gives one window at home with no
/// problem; an unparsable file or another version gives one window at home and
/// one problem. The caller copies such a file with [`preserve_bad`].
pub fn load(
    path: impl AsRef<Path>,
    home: &Path,
    exists: impl Fn(&Path) -> bool,
) -> (Workspace, Vec<SessionProblem>) {
    let path = path.as_ref();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return (
                Workspace::restore(home.to_path_buf(), one_window_at_home(home)).0,
                Vec::new(),
            );
        }
        Err(error) => {
            return (
                fresh_workspace(home),
                vec![SessionProblem::Read {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                }],
            );
        }
    };

    let document = match Document::parse(text.as_str()) {
        Ok(document) => document,
        Err(error) => {
            return (
                fresh_workspace(home),
                vec![SessionProblem::Parse {
                    path: path.to_path_buf(),
                    message: error.message().to_string(),
                }],
            );
        }
    };

    let root = document.as_table();
    let version = root.get("version").and_then(Item::as_integer);
    if version != Some(SESSION_VERSION) {
        return (
            fresh_workspace(home),
            vec![SessionProblem::UnsupportedVersion {
                path: path.to_path_buf(),
                version,
            }],
        );
    }

    let mut data = RestoreData {
        focused_window: root
            .get("focused_window")
            .and_then(Item::as_integer)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0),
        windows: Vec::new(),
    };

    if let Some(windows) = root.get("window").and_then(Item::as_array_of_tables) {
        for window in windows.iter() {
            data.windows.push(RestoreWindow {
                pills: read_indices(window.get("pills")),
                focused_tab: window
                    .get("focused_tab")
                    .and_then(Item::as_integer)
                    .map(|value| usize::try_from(value).unwrap_or(usize::MAX))
                    .unwrap_or(0),
                groups: read_groups(window, home),
            });
        }
    }

    let mut problems = Vec::new();
    for window in &mut data.windows {
        for group in &mut window.groups {
            for tab in &mut group.tabs {
                if exists(&tab.folder) {
                    continue;
                }
                let fallback = tab
                    .folder
                    .ancestors()
                    .skip(1)
                    .find(|parent| exists(parent))
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| home.to_path_buf());
                problems.push(SessionProblem::MissingFolder {
                    path: tab.folder.clone(),
                    fallback: fallback.clone(),
                });
                tab.folder = fallback;
            }
        }
    }

    let (workspace, restore_problems) = Workspace::restore(home.to_path_buf(), data);
    problems.extend(restore_problems.into_iter().map(SessionProblem::Restore));
    (workspace, problems)
}

/// Copy a bad session file to `session.toml.bad`, replacing an older copy, and
/// return the copy's path.
pub fn preserve_bad(path: &Path) -> io::Result<PathBuf> {
    let mut name = path
        .file_name()
        .unwrap_or_else(|| OsStr::new(SESSION_FILE))
        .to_os_string();
    name.push(".bad");
    let bad = path.with_file_name(name);
    fs::copy(path, &bad)?;
    Ok(bad)
}

fn fresh_workspace(home: &Path) -> Workspace {
    Workspace::restore(home.to_path_buf(), one_window_at_home(home)).0
}

fn one_window_at_home(home: &Path) -> RestoreData {
    RestoreData {
        focused_window: 0,
        windows: vec![RestoreWindow {
            pills: vec![0],
            focused_tab: 0,
            groups: vec![RestoreGroup {
                name: None,
                number: 1,
                width_permille: DEFAULT_WIDTH_PERMILLE,
                heights: [None, None],
                columns: [vec![0], Vec::new()],
                tabs: vec![RestoreTab {
                    folder: home.to_path_buf(),
                    ..RestoreTab::default()
                }],
            }],
        }],
    }
}

// -------------------------------------------------------------------------
// Writing
// -------------------------------------------------------------------------

fn document_for(workspace: &Workspace) -> DocumentMut {
    let mut document = DocumentMut::new();
    let root = document.as_table_mut();
    root.insert("version", Item::Value(Value::from(SESSION_VERSION)));
    let focused = workspace
        .focused_window
        .and_then(|id| workspace.windows.iter().position(|window| window.id == id))
        .unwrap_or(0);
    root.insert("focused_window", Item::Value(Value::from(focused as i64)));

    let mut windows = ArrayOfTables::new();
    for window in &workspace.windows {
        windows.push(window_table(window));
    }
    root.insert("window", Item::ArrayOfTables(windows));
    document
}

fn window_table(window: &Window) -> Table {
    let mut table = Table::new();

    let group_positions: BTreeMap<GroupId, usize> = window
        .groups
        .keys()
        .enumerate()
        .map(|(index, id)| (*id, index))
        .collect();
    let mut pills = Array::new();
    for group_id in &window.pills {
        pills.push(Value::from(
            group_positions.get(group_id).copied().unwrap_or(0) as i64,
        ));
    }
    table.insert("pills", Item::Value(Value::Array(pills)));

    let flat: Vec<TabId> = window
        .pills
        .iter()
        .filter_map(|group_id| window.groups.get(group_id))
        .flat_map(|group| group.tabs.iter().map(|tab| tab.id))
        .collect();
    let focused = flat
        .iter()
        .position(|tab| *tab == window.focused_tab)
        .unwrap_or(0);
    table.insert("focused_tab", Item::Value(Value::from(focused as i64)));

    let mut groups = ArrayOfTables::new();
    for group in window.groups.values() {
        groups.push(group_table(group));
    }
    table.insert("group", Item::ArrayOfTables(groups));
    table
}

fn group_table(group: &Group) -> Table {
    let mut table = Table::new();
    if let Some(name) = &group.name {
        table.insert("name", Item::Value(Value::from(name.clone())));
    }
    table.insert("number", Item::Value(Value::from(i64::from(group.number))));
    table.insert(
        "width_permille",
        Item::Value(Value::from(i64::from(group.layout.width_permille))),
    );

    let mut heights = Array::new();
    for column in &group.layout.heights {
        heights.push(heights_value(column));
    }
    table.insert("heights", Item::Value(Value::Array(heights)));

    let tab_positions: BTreeMap<TabId, usize> = group
        .tabs
        .iter()
        .enumerate()
        .map(|(index, tab)| (tab.id, index))
        .collect();
    let mut columns = Array::new();
    for column in &group.layout.columns {
        let mut indices = Array::new();
        for tab_id in column {
            indices.push(Value::from(
                tab_positions.get(tab_id).copied().unwrap_or(0) as i64
            ));
        }
        columns.push(Value::Array(indices));
    }
    table.insert("columns", Item::Value(Value::Array(columns)));

    let mut tabs = ArrayOfTables::new();
    for tab in &group.tabs {
        tabs.push(tab_table(tab));
    }
    table.insert("tab", Item::ArrayOfTables(tabs));
    table
}

fn tab_table(tab: &Tab) -> Table {
    let mut table = Table::new();
    table.insert("folder", Item::Value(path_value(&tab.folder)));

    let mut back = Array::new();
    for folder in &tab.back {
        back.push(path_value(folder));
    }
    table.insert("back", Item::Value(Value::Array(back)));

    let mut forward = Array::new();
    for folder in &tab.forward {
        forward.push(path_value(folder));
    }
    table.insert("forward", Item::Value(Value::Array(forward)));

    let mut names = Array::new();
    for name in &tab.selection.names {
        names.push(os_value(name));
    }
    table.insert("selection_names", Item::Value(Value::Array(names)));
    if let Some(focused) = &tab.selection.focused {
        table.insert("selection_focused", Item::Value(os_value(focused)));
    }
    if let Some(anchor) = &tab.selection.anchor {
        table.insert("selection_anchor", Item::Value(os_value(anchor)));
    }
    if let Some(anchor) = &tab.scroll_anchor {
        table.insert("scroll_anchor", Item::Value(os_value(anchor)));
    }

    table
}

fn heights_value(heights: &Heights) -> Value {
    match heights {
        Heights::Default => Value::from("default"),
        Heights::Hand(values) => {
            let mut array = Array::new();
            for value in values {
                array.push(Value::from(i64::from(*value)));
            }
            Value::Array(array)
        }
    }
}

fn path_value(path: &Path) -> Value {
    os_value(path.as_os_str())
}

fn os_value(value: &OsStr) -> Value {
    match value.to_str() {
        Some(text) => Value::from(text.to_string()),
        None => bytes_value(value.as_bytes()),
    }
}

fn bytes_value(bytes: &[u8]) -> Value {
    let mut table = InlineTable::new();
    let mut array = Array::new();
    for byte in bytes {
        array.push(Value::from(i64::from(*byte)));
    }
    table.insert("bytes", Value::Array(array));
    Value::InlineTable(table)
}

// -------------------------------------------------------------------------
// Reading
// -------------------------------------------------------------------------

fn read_groups(table: &Table, home: &Path) -> Vec<RestoreGroup> {
    let Some(groups) = table.get("group").and_then(Item::as_array_of_tables) else {
        return Vec::new();
    };

    groups
        .iter()
        .enumerate()
        .map(|(index, group)| RestoreGroup {
            name: group.get("name").and_then(Item::as_str).map(str::to_string),
            number: group
                .get("number")
                .and_then(Item::as_integer)
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(index as u32 + 1),
            width_permille: group
                .get("width_permille")
                .and_then(Item::as_integer)
                .map(|value| value.clamp(0, i64::from(u16::MAX)) as u16)
                .unwrap_or(DEFAULT_WIDTH_PERMILLE),
            heights: read_heights(group.get("heights")),
            columns: read_columns(group.get("columns")),
            tabs: group
                .get("tab")
                .and_then(Item::as_array_of_tables)
                .map(|tabs| tabs.iter().map(|tab| read_tab(tab, home)).collect())
                .unwrap_or_default(),
        })
        .collect()
}

fn read_tab(table: &Table, home: &Path) -> RestoreTab {
    let folder = table
        .get("folder")
        .and_then(Item::as_value)
        .and_then(read_os)
        .map(PathBuf::from)
        .unwrap_or_else(|| home.to_path_buf());

    let back = read_paths(table.get("back"));
    let forward = read_paths(table.get("forward"));
    let names = table
        .get("selection_names")
        .and_then(Item::as_array)
        .map(|array| array.iter().filter_map(read_os).collect())
        .unwrap_or_default();

    RestoreTab {
        folder,
        back,
        forward,
        selection: FileSelection {
            names,
            focused: table
                .get("selection_focused")
                .and_then(Item::as_value)
                .and_then(read_os),
            anchor: table
                .get("selection_anchor")
                .and_then(Item::as_value)
                .and_then(read_os),
        },
        scroll_anchor: table
            .get("scroll_anchor")
            .and_then(Item::as_value)
            .and_then(read_os),
    }
}

fn read_paths(item: Option<&Item>) -> Vec<PathBuf> {
    item.and_then(Item::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(read_os)
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

fn read_heights(item: Option<&Item>) -> [Option<Vec<i32>>; 2] {
    let mut heights = [None, None];
    let Some(array) = item.and_then(Item::as_array) else {
        return heights;
    };
    for (column, value) in array.iter().take(2).enumerate() {
        if let Some(values) = value.as_array() {
            heights[column] = Some(
                values
                    .iter()
                    .filter_map(Value::as_integer)
                    .map(|value| value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
                    .collect(),
            );
        }
    }
    heights
}

fn read_columns(item: Option<&Item>) -> [Vec<usize>; 2] {
    let mut columns = [Vec::new(), Vec::new()];
    let Some(array) = item.and_then(Item::as_array) else {
        return columns;
    };
    for (column, value) in array.iter().take(2).enumerate() {
        if let Some(values) = value.as_array() {
            columns[column] = values
                .iter()
                .map(|value| {
                    value
                        .as_integer()
                        .and_then(|value| usize::try_from(value).ok())
                        .unwrap_or(usize::MAX)
                })
                .collect();
        }
    }
    columns
}

fn read_indices(item: Option<&Item>) -> Vec<usize> {
    item.and_then(Item::as_array)
        .map(|array| {
            array
                .iter()
                .map(|value| {
                    value
                        .as_integer()
                        .and_then(|value| usize::try_from(value).ok())
                        .unwrap_or(usize::MAX)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn read_os(value: &Value) -> Option<OsString> {
    if let Some(text) = value.as_str() {
        return Some(OsString::from(text));
    }

    let table = value.as_inline_table()?;
    let array = table.get("bytes")?.as_array()?;
    let mut bytes = Vec::with_capacity(array.len());
    for byte in array.iter() {
        bytes.push(u8::try_from(byte.as_integer()?).ok()?);
    }
    Some(OsString::from_vec(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = Path::new("/tmp").join(format!(
                "gimbal-p2e-{tag}-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.path.join(name);
            fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct TabView {
        folder: PathBuf,
        back: Vec<PathBuf>,
        forward: Vec<PathBuf>,
        names: BTreeSet<OsString>,
        focused: Option<OsString>,
        anchor: Option<OsString>,
        scroll: Option<OsString>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct GroupView {
        name: Option<String>,
        number: u32,
        width: u16,
        heights: [Heights; 2],
        columns: [Vec<usize>; 2],
        tabs: Vec<TabView>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct WindowView {
        groups: Vec<GroupView>,
        focused: usize,
    }

    fn tab_view(tab: &Tab) -> TabView {
        TabView {
            folder: tab.folder.clone(),
            back: tab.back.clone(),
            forward: tab.forward.clone(),
            names: tab.selection.names.clone(),
            focused: tab.selection.focused.clone(),
            anchor: tab.selection.anchor.clone(),
            scroll: tab.scroll_anchor.clone(),
        }
    }

    fn group_view(group: &Group) -> GroupView {
        let positions = |ids: &[TabId]| {
            ids.iter()
                .map(|id| group.tabs.iter().position(|tab| tab.id == *id).unwrap())
                .collect()
        };
        GroupView {
            name: group.name.clone(),
            number: group.number,
            width: group.layout.width_permille,
            heights: group.layout.heights.clone(),
            columns: [
                positions(&group.layout.columns[0]),
                positions(&group.layout.columns[1]),
            ],
            tabs: group.tabs.iter().map(tab_view).collect(),
        }
    }

    fn view(workspace: &Workspace) -> (Vec<WindowView>, usize) {
        let windows = workspace
            .windows
            .iter()
            .map(|window| {
                let groups = window
                    .pills
                    .iter()
                    .map(|group_id| group_view(&window.groups[group_id]))
                    .collect();
                let focused = window
                    .pills
                    .iter()
                    .flat_map(|group_id| window.groups[group_id].tabs.iter())
                    .position(|tab| tab.id == window.focused_tab)
                    .unwrap_or(0);
                WindowView { groups, focused }
            })
            .collect();
        let focused_window = workspace
            .focused_window
            .and_then(|id| workspace.windows.iter().position(|window| window.id == id))
            .unwrap_or(0);
        (windows, focused_window)
    }

    fn rich_workspace(home: &Path) -> Workspace {
        let mut workspace = Workspace::with_home(home.to_path_buf());
        workspace
            .window_new(Some(home.join("work").join("one")))
            .unwrap();
        for name in ["two", "three", "four"] {
            workspace
                .tab_new(Some(home.join("work").join(name)))
                .unwrap();
        }
        for index in 0..4 {
            workspace.pill_ctrl_click(index).unwrap();
        }
        workspace.group_split().unwrap();
        workspace.group_rename("Build & test").unwrap();
        workspace
            .tab_new(Some(home.join("work").join("notes")))
            .unwrap();

        let history_tab = workspace.focused().unwrap().focused_tab;
        workspace.nav_go(home.join("work").join("a"), None).unwrap();
        workspace.nav_go(home.join("work").join("b"), None).unwrap();
        workspace.nav_back().unwrap();
        workspace.file_click(OsString::from("alpha")).unwrap();
        workspace.file_ctrl_click(OsString::from("beta")).unwrap();
        workspace
            .record_scroll_anchor(Some(OsString::from("beta")))
            .unwrap();

        workspace.pill_click(0).unwrap();
        workspace
            .set_column_heights(0, Heights::Hand(vec![420, 260]))
            .unwrap();
        workspace
            .set_column_heights(1, Heights::Hand(vec![310, 290]))
            .unwrap();
        workspace.set_width_share(600).unwrap();

        workspace.pill_ctrl_click(0).unwrap();
        workspace.focus_tab(history_tab).unwrap();

        workspace
            .window_new(Some(home.join("work").join("other")))
            .unwrap();
        workspace
    }

    #[test]
    fn session_path_is_a_pure_function_of_xdg_state_home_and_home() {
        let home = Path::new("/home/kyle");
        assert_eq!(
            session_path(Some(Path::new("/tmp/state")), Some(home)),
            Some(PathBuf::from("/tmp/state/gimbal-files/session.toml"))
        );
        assert_eq!(
            session_path(Some(Path::new("relative")), Some(home)),
            Some(PathBuf::from(
                "/home/kyle/.local/state/gimbal-files/session.toml"
            ))
        );
        assert_eq!(
            session_path(None, Some(home)),
            Some(PathBuf::from(
                "/home/kyle/.local/state/gimbal-files/session.toml"
            ))
        );
        assert_eq!(session_path(Some(Path::new("relative")), None), None);
        assert_eq!(session_path(None, None), None);
    }

    #[test]
    fn a_rich_workspace_round_trips() {
        let temp = TempDir::new("rich");
        let home = Path::new("/home/kyle");
        let workspace = rich_workspace(home);
        let path = temp.path().join("session.toml");

        save(&path, &workspace).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("version = 1"));
        assert!(text.contains("[[window]]"));
        assert!(text.contains("[[window.group]]"));
        assert!(text.contains("[[window.group.tab]]"));
        assert!(text.contains("name = \"Build & test\""));
        assert!(text.contains("width_permille = 600"));

        if std::env::var_os("GIMBAL_P2E_PRINT").is_some() {
            println!("{text}");
        }

        let (loaded, problems) = load(&path, home, |_| true);
        assert!(problems.is_empty(), "{problems:?}");
        loaded.check_invariants().unwrap();
        assert_eq!(view(&workspace), view(&loaded));

        for window in &loaded.windows {
            assert!(window.selection.selected.iter().all(|selected| !selected));
        }
    }

    #[test]
    fn a_non_utf8_folder_and_file_name_round_trip() {
        let temp = TempDir::new("non-utf8");
        let home = temp.path().to_path_buf();
        let folder = home.join(OsString::from_vec(b"gimbal-\xff".to_vec()));
        let subfolder = folder.join(OsString::from_vec(b"sub-\xfd".to_vec()));
        let name = OsString::from_vec(vec![b'n', 0xfe, b'a']);

        let mut workspace = Workspace::with_home(home.clone());
        workspace.window_new(Some(folder.clone())).unwrap();
        workspace.nav_go(subfolder.clone(), None).unwrap();
        workspace.file_click(name.clone()).unwrap();
        workspace.record_scroll_anchor(Some(name.clone())).unwrap();

        let path = temp.path().join("session.toml");
        save(&path, &workspace).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("bytes = ["));

        let (loaded, problems) = load(&path, &home, |_| true);
        assert!(problems.is_empty(), "{problems:?}");
        loaded.check_invariants().unwrap();

        let window = &loaded.windows[0];
        let tab = window.tab(window.focused_tab).unwrap();
        assert_eq!(tab.folder, subfolder);
        assert_eq!(tab.back, vec![folder]);
        assert_eq!(tab.selection.names, BTreeSet::from([name.clone()]));
        assert_eq!(tab.selection.focused, Some(name.clone()));
        assert_eq!(tab.selection.anchor, Some(name.clone()));
        assert_eq!(tab.scroll_anchor, Some(name));
    }

    #[test]
    fn a_missing_file_gives_one_window_at_home_without_a_problem() {
        let temp = TempDir::new("missing");
        let home = temp.path().to_path_buf();
        let path = temp.path().join("session.toml");

        let (workspace, problems) = load(&path, &home, |_| true);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(workspace.windows.len(), 1);
        let window = &workspace.windows[0];
        assert_eq!(window.pills.len(), 1);
        assert_eq!(window.groups[&window.pills[0]].tabs[0].folder, home);
        workspace.check_invariants().unwrap();
    }

    #[test]
    fn an_unparsable_file_gives_one_window_at_home_and_a_problem() {
        let temp = TempDir::new("parse");
        let home = temp.path().to_path_buf();
        let path = temp.write("session.toml", "[window\n");

        let (workspace, problems) = load(&path, &home, |_| true);
        assert!(matches!(
            problems.as_slice(),
            [SessionProblem::Parse { .. }]
        ));
        assert_eq!(workspace.windows.len(), 1);
        let window = &workspace.windows[0];
        assert_eq!(window.groups[&window.pills[0]].tabs[0].folder, home);
        workspace.check_invariants().unwrap();
    }

    #[test]
    fn a_version_2_file_gives_one_window_at_home_and_a_problem() {
        let temp = TempDir::new("version");
        let home = temp.path().to_path_buf();
        let path = temp.write("session.toml", "version = 2\n");

        let (workspace, problems) = load(&path, &home, |_| true);
        assert!(matches!(
            problems.as_slice(),
            [SessionProblem::UnsupportedVersion {
                version: Some(2),
                ..
            }]
        ));
        assert_eq!(workspace.windows.len(), 1);
        let window = &workspace.windows[0];
        assert_eq!(window.groups[&window.pills[0]].tabs[0].folder, home);
        workspace.check_invariants().unwrap();
    }

    #[test]
    fn preserve_bad_replaces_an_older_copy() {
        let temp = TempDir::new("bad");
        let path = temp.write("session.toml", "the bad session");
        let older = temp.write("session.toml.bad", "an older session");

        let bad = preserve_bad(&path).unwrap();
        assert_eq!(bad, path.with_file_name("session.toml.bad"));
        assert_eq!(bad, older);
        assert_eq!(fs::read_to_string(&bad).unwrap(), "the bad session");
    }

    #[test]
    fn a_missing_folder_falls_back_to_its_parent_then_to_home() {
        let temp = TempDir::new("fallback");
        let home = temp.path().to_path_buf();
        let parent = home.join("parent");
        fs::create_dir_all(&parent).unwrap();

        let first = parent.join("gone").join("deeper");
        let second = PathBuf::from(format!(
            "/nonexistent/gimbal-p2e-{}-missing/child",
            std::process::id()
        ));

        let mut workspace = Workspace::with_home(home.clone());
        workspace.window_new(Some(first.clone())).unwrap();
        workspace.tab_new(Some(second.clone())).unwrap();
        {
            let window = &mut workspace.windows[0];
            let group_id = window.pills[0];
            let group = window.groups.get_mut(&group_id).unwrap();
            group.tabs[0].back = vec![home.join("older")];
        }

        let path = temp.path().join("session.toml");
        save(&path, &workspace).unwrap();

        let exists = |path: &Path| path == parent || path == home;
        let (loaded, problems) = load(&path, &home, exists);
        assert_eq!(
            problems,
            vec![
                SessionProblem::MissingFolder {
                    path: first,
                    fallback: parent.clone()
                },
                SessionProblem::MissingFolder {
                    path: second,
                    fallback: home.clone()
                }
            ]
        );
        loaded.check_invariants().unwrap();

        let window = &loaded.windows[0];
        let first_tab = &window.groups[&window.pills[0]].tabs[0];
        let second_tab = &window.groups[&window.pills[1]].tabs[0];
        assert_eq!(first_tab.folder, parent);
        assert_eq!(first_tab.back, vec![home.join("older")]);
        assert_eq!(second_tab.folder, home);
    }

    #[test]
    fn an_empty_workspace_is_never_saved() {
        let temp = TempDir::new("empty");
        let path = temp.write("session.toml", "the previous session");
        let workspace = Workspace::with_home(temp.path().to_path_buf());

        save(&path, &workspace).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "the previous session");

        let absent = temp.path().join("new-session.toml");
        save(&absent, &workspace).unwrap();
        assert!(!absent.exists());
    }
}
