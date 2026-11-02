// SPDX-License-Identifier: GPL-3.0-or-later
//! The TOML config: typed settings, loading with line and column problems,
//! and one-key saves that leave the rest of the file untouched.
//!
//! The file lives at `$XDG_CONFIG_HOME/gimbal-files/config.toml`, or under
//! `~/.config` when `XDG_CONFIG_HOME` is unset or relative. `toml_edit` keeps
//! comments, key order and formatting, so a save rewrites only the changed
//! key. A file that does not parse is never written.
//!
//! `home` on [`Config`] is runtime context, not a setting: it is used to
//! expand `~`. It is never written to the file.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use toml_edit::{
    Array, ArrayOfTables, Document, DocumentMut, InlineTable, Item, Table, TableLike, Value,
};

use crate::command::{Caller, CheckedCommand, Invocation, Registry, Value as CommandValue};

pub const CONFIG_DIR: &str = "gimbal-files";
pub const CONFIG_FILE: &str = "config.toml";

pub const DEFAULT_SCALE: f64 = 1.0;
pub const DEFAULT_BASE: f64 = 1.0;
pub const DEFAULT_OPACITY: f64 = 0.85;
pub const DEFAULT_FONT: &str = "";

pub const MIN_SCALE: f64 = 0.50;
pub const MAX_SCALE: f64 = 1.00;
pub const SCALE_STEP: f64 = 0.05;
pub const MIN_BASE: f64 = 0.50;
pub const MAX_BASE: f64 = 2.00;
pub const MIN_OPACITY: f64 = 0.0;
pub const MAX_OPACITY: f64 = 1.0;

pub const DEFAULT_LEFT_WIDTH: u32 = 348;
pub const DEFAULT_RIGHT_WIDTH: u32 = 648;
pub const MIN_LEFT_WIDTH: u32 = 300;
pub const MIN_RIGHT_WIDTH: u32 = 360;

const KNOWN_TOP_LEVEL: &[&str] = &["look", "sidebars", "sections", "pins", "sort", "hotkeys"];

/// The config path as a pure function of `XDG_CONFIG_HOME` and the home
/// folder. A relative or missing XDG directory falls back to `~/.config`.
pub fn config_path(xdg_config_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    match xdg_config_home {
        Some(directory) if directory.is_absolute() => {
            Some(directory.join(CONFIG_DIR).join(CONFIG_FILE))
        }
        _ => home.map(|home| home.join(".config").join(CONFIG_DIR).join(CONFIG_FILE)),
    }
}

/// The config path from the process environment and `HOME`.
pub fn default_config_path() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let home = home_dir();
    config_path(xdg.as_deref(), Some(home.as_path()))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// The XDG folders used for the default pins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpecialFolders {
    pub desktop: Option<PathBuf>,
    pub documents: Option<PathBuf>,
    pub downloads: Option<PathBuf>,
    pub music: Option<PathBuf>,
    pub pictures: Option<PathBuf>,
    pub videos: Option<PathBuf>,
}

/// The one thin wrapper around `glib::user_special_dir`. Everything below this
/// point is a pure function of the paths it receives.
pub fn special_folders() -> SpecialFolders {
    SpecialFolders {
        desktop: glib::user_special_dir(glib::UserDirectory::Desktop),
        documents: glib::user_special_dir(glib::UserDirectory::Documents),
        downloads: glib::user_special_dir(glib::UserDirectory::Downloads),
        music: glib::user_special_dir(glib::UserDirectory::Music),
        pictures: glib::user_special_dir(glib::UserDirectory::Pictures),
        videos: glib::user_special_dir(glib::UserDirectory::Videos),
    }
}

/// The default pins: home, then the special folders that exist, in the
/// order of the brief. Duplicates are dropped.
pub fn default_pins(home: &Path, special: &SpecialFolders) -> Vec<PathBuf> {
    let mut pins = vec![normalize_path(home)];

    for folder in [
        &special.desktop,
        &special.documents,
        &special.downloads,
        &special.music,
        &special.pictures,
        &special.videos,
    ]
    .into_iter()
    .flatten()
    {
        if !folder.is_dir() {
            continue;
        }
        let path = normalize_path(folder);
        if !pins.contains(&path) {
            pins.push(path);
        }
    }

    pins
}

/// Expand a leading `~` or `~/…` against home. `~user` is not expanded.
pub fn expand_home(path: &Path, home: &Path) -> PathBuf {
    if path == Path::new("~") {
        home.to_path_buf()
    } else if let Ok(rest) = path.strip_prefix("~") {
        home.join(rest)
    } else {
        path.to_path_buf()
    }
}

/// Remove `.`, `..` and a trailing slash lexically, without touching the
/// filesystem. A `..` at the root stays at the root; in a relative path it is
/// kept.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(std::path::MAIN_SEPARATOR_STR),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                } else if !path.is_absolute() {
                    normalized.push("..");
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    if normalized.as_os_str().is_empty() {
        normalized.push(".");
    }

    normalized
}

/// The scale and base size, opacity and font family.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub scale: f64,
    pub base: f64,
    pub opacity: f64,
    pub font: String,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            scale: DEFAULT_SCALE,
            base: DEFAULT_BASE,
            opacity: DEFAULT_OPACITY,
            font: DEFAULT_FONT.to_string(),
        }
    }
}

/// A sidebar's collapsed state and width in design px.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sidebar {
    pub collapsed: bool,
    pub width: u32,
}

/// The left and right sidebars.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sidebars {
    pub left: Sidebar,
    pub right: Sidebar,
}

impl Default for Sidebars {
    fn default() -> Self {
        Self {
            left: Sidebar {
                collapsed: false,
                width: DEFAULT_LEFT_WIDTH,
            },
            right: Sidebar {
                collapsed: false,
                width: DEFAULT_RIGHT_WIDTH,
            },
        }
    }
}

impl Sidebars {
    pub fn get(&self, side: Side) -> Sidebar {
        match side {
            Side::Left => self.left,
            Side::Right => self.right,
        }
    }

    pub fn get_mut(&mut self, side: Side) -> &mut Sidebar {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        }
    }
}

/// A sidebar side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

/// A collapsible sidebar section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Recent,
    Devices,
    Remote,
}

impl Section {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "recent" => Some(Self::Recent),
            "devices" => Some(Self::Devices),
            "remote" => Some(Self::Remote),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recent => "recent",
            Self::Devices => "devices",
            Self::Remote => "remote",
        }
    }
}

/// Whether a section is expanded or collapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionState {
    Expanded,
    Collapsed,
}

impl SectionState {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "expanded" => Some(Self::Expanded),
            "collapsed" => Some(Self::Collapsed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expanded => "expanded",
            Self::Collapsed => "collapsed",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            Self::Expanded => Self::Collapsed,
            Self::Collapsed => Self::Expanded,
        }
    }
}

/// The three sections of the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sections {
    pub recent: SectionState,
    pub devices: SectionState,
    pub remote: SectionState,
}

impl Default for Sections {
    fn default() -> Self {
        Self {
            recent: SectionState::Expanded,
            devices: SectionState::Expanded,
            remote: SectionState::Expanded,
        }
    }
}

impl Sections {
    pub fn get(&self, section: Section) -> SectionState {
        match section {
            Section::Recent => self.recent,
            Section::Devices => self.devices,
            Section::Remote => self.remote,
        }
    }

    pub fn set(&mut self, section: Section, state: SectionState) {
        match section {
            Section::Recent => self.recent = state,
            Section::Devices => self.devices = state,
            Section::Remote => self.remote = state,
        }
    }
}

/// What a sort sorts by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Modified,
    Size,
    Type,
}

impl SortBy {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "name" => Some(Self::Name),
            "modified" => Some(Self::Modified),
            "size" => Some(Self::Size),
            "type" => Some(Self::Type),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Modified => "modified",
            Self::Size => "size",
            Self::Type => "type",
        }
    }
}

/// The direction of a sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

impl SortOrder {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ascending" => Some(Self::Ascending),
            "descending" => Some(Self::Descending),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        }
    }
}

/// The effective sort rule for one folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortRule {
    pub by: SortBy,
    pub order: SortOrder,
    pub folders_first: bool,
}

/// One folder's sort exception.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderSort {
    pub folder: PathBuf,
    pub by: SortBy,
    pub order: SortOrder,
}

/// The global sort and the folder exceptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sort {
    pub by: SortBy,
    pub order: SortOrder,
    pub folders_first: bool,
    pub folders: Vec<FolderSort>,
}

impl Default for Sort {
    fn default() -> Self {
        Self {
            by: SortBy::Name,
            order: SortOrder::Ascending,
            folders_first: true,
            folders: Vec::new(),
        }
    }
}

impl Sort {
    pub fn global(&self) -> SortRule {
        SortRule {
            by: self.by,
            order: self.order,
            folders_first: self.folders_first,
        }
    }
}

/// One hotkey: the command and its already-split `key=value` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    pub command: String,
    pub arguments: Vec<(String, String)>,
}

impl Hotkey {
    pub fn invocation(&self) -> Invocation {
        Invocation::new(self.command.clone(), self.arguments.clone())
    }
}

/// Every setting in the file, plus the runtime context used to expand paths.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub look: Look,
    pub sidebars: Sidebars,
    pub sections: Sections,
    pub pins: Vec<PathBuf>,
    pub sort: Sort,
    pub hotkeys: BTreeMap<String, Hotkey>,
    /// Runtime context: home for `~` expansion. Not written to the file.
    pub home: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        let home = home_dir();
        Self::with_defaults(&home, &special_folders())
    }
}

impl Config {
    /// The default settings for the given home and special folders.
    pub fn with_defaults(home: &Path, special: &SpecialFolders) -> Self {
        let home = normalize_path(home);
        Self {
            look: Look::default(),
            sidebars: Sidebars::default(),
            sections: Sections::default(),
            pins: default_pins(&home, special),
            sort: Sort::default(),
            hotkeys: default_hotkeys(),
            home,
        }
    }

    /// The sort rule for one exact folder. Folder exceptions match that exact
    /// folder, never its subfolders; paths are compared after `~` expansion
    /// and lexical normalization.
    pub fn sort_for(&self, folder: &Path) -> SortRule {
        let target = normalize_path(&expand_home(folder, &self.home));

        if let Some(rule) = self
            .sort
            .folders
            .iter()
            .rev()
            .find(|rule| normalize_path(&expand_home(&rule.folder, &self.home)) == target)
        {
            SortRule {
                by: rule.by,
                order: rule.order,
                folders_first: self.sort.folders_first,
            }
        } else {
            self.sort.global()
        }
    }
}

/// The default hotkeys from the brief. Commands that need typed input get no
/// default yet.
pub fn default_hotkeys() -> BTreeMap<String, Hotkey> {
    let defaults = [
        ("<Control>t", "tab.new"),
        ("<Control>w", "tab.close"),
        ("<Control>n", "window.new"),
        ("<Control>q", "app.quit"),
        ("<Control>l", "nav.focus-path"),
        ("<Control>f", "search.open"),
        ("<Control>a", "selection.all"),
        ("<Control>z", "file.undo"),
        ("<Control>x", "file.cut"),
        ("<Control>c", "file.copy-to-clipboard"),
        ("<Control>v", "file.paste"),
        ("<Control>equal", "view.zoom-in"),
        ("<Control>minus", "view.zoom-out"),
        ("<Alt>Left", "nav.back"),
        ("<Alt>Right", "nav.forward"),
        ("<Alt>Up", "nav.up"),
        ("F5", "nav.refresh"),
        ("Delete", "file.trash"),
    ];

    let mut hotkeys = BTreeMap::new();
    for (accelerator, command) in defaults {
        hotkeys.insert(
            accelerator.to_string(),
            Hotkey {
                command: command.to_string(),
                arguments: Vec::new(),
            },
        );
    }
    hotkeys
}

/// A setting the app writes. Each variant maps to one key or one list entry.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    SetScale(f64),
    SetBase(f64),
    SetOpacity(f64),
    SetFont(String),
    SetSidebarCollapsed {
        side: Side,
        collapsed: bool,
    },
    SetSection {
        section: Section,
        state: SectionState,
    },
    SetSidebarWidth {
        side: Side,
        width: u32,
    },
    AddPin(PathBuf),
    RemovePin(PathBuf),
    MovePin {
        from: usize,
        to: usize,
    },
    SetSort {
        folder: Option<PathBuf>,
        by: SortBy,
        order: SortOrder,
    },
}

/// What went wrong while loading the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigProblemKind {
    Parse,
    Read,
    WrongType,
    OutOfRange,
    OffStep,
    BadChoice,
    UnknownKey,
    UnknownTable,
    MissingValue,
    UnknownCommand,
    InvalidHotkeyArguments,
    EmptyAccelerator,
    RelativePin,
}

impl ConfigProblemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Parse => "parse error",
            Self::Read => "read error",
            Self::WrongType => "wrong type",
            Self::OutOfRange => "out of range",
            Self::OffStep => "off step",
            Self::BadChoice => "bad choice",
            Self::UnknownKey => "unknown key",
            Self::UnknownTable => "unknown table",
            Self::MissingValue => "missing value",
            Self::UnknownCommand => "unknown command",
            Self::InvalidHotkeyArguments => "invalid hotkey arguments",
            Self::EmptyAccelerator => "empty accelerator",
            Self::RelativePin => "relative pin path",
        }
    }

    pub fn is_warning(self) -> bool {
        matches!(self, Self::UnknownKey | Self::UnknownTable)
    }
}

/// One problem in the config file: where it is, which key, and what is wrong.
/// Unknown keys and tables are warnings and stay in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProblem {
    pub file: PathBuf,
    pub line: usize,
    pub column: usize,
    pub key: String,
    pub kind: ConfigProblemKind,
    pub message: String,
}

impl ConfigProblem {
    fn new(
        file: &Path,
        line: usize,
        column: usize,
        key: impl Into<String>,
        kind: ConfigProblemKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            file: file.to_path_buf(),
            line,
            column,
            key: key.into(),
            kind,
            message: message.into(),
        }
    }

    pub fn is_warning(&self) -> bool {
        self.kind.is_warning()
    }
}

impl fmt::Display for ConfigProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = if self.key.is_empty() {
            "<file>"
        } else {
            &self.key
        };
        let severity = if self.is_warning() {
            "warning"
        } else {
            "error"
        };
        write!(
            f,
            "{}:{}:{}: {}: {}: {}",
            self.file.display(),
            self.line,
            self.column,
            key,
            severity,
            self.message
        )
    }
}

/// The config plus the parsed document that saves edit.
#[derive(Debug)]
pub struct ConfigFile {
    path: PathBuf,
    config: Config,
    /// `None` when the file exists but did not parse. Such a file is never
    /// written.
    document: Option<DocumentMut>,
}

impl ConfigFile {
    /// Load a config using `HOME` and GLib's special folders.
    pub fn load(path: impl AsRef<Path>) -> (Self, Vec<ConfigProblem>) {
        let home = home_dir();
        let special = special_folders();
        Self::load_with(path.as_ref(), &home, &special)
    }

    /// Load a config with explicit home and special folders. Tests use this so
    /// they never read the real desktop settings.
    pub fn load_with(
        path: &Path,
        home: &Path,
        special: &SpecialFolders,
    ) -> (Self, Vec<ConfigProblem>) {
        let path = path.to_path_buf();
        let mut config = Config::with_defaults(home, special);

        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let file = Self {
                    path,
                    config,
                    document: Some(DocumentMut::new()),
                };
                return (file, Vec::new());
            }
            Err(error) => {
                let problem = ConfigProblem::new(
                    &path,
                    1,
                    1,
                    "",
                    ConfigProblemKind::Read,
                    format!("could not read the file: {error}"),
                );
                let file = Self {
                    path,
                    config,
                    document: None,
                };
                return (file, vec![problem]);
            }
        };

        let document = match Document::parse(text.as_str()) {
            Ok(document) => document,
            Err(error) => {
                let (line, column) = error
                    .span()
                    .map(|span| line_column(&text, span.start))
                    .unwrap_or((1, 1));
                let problem = ConfigProblem::new(
                    &path,
                    line,
                    column,
                    "",
                    ConfigProblemKind::Parse,
                    error.message(),
                );
                let file = Self {
                    path,
                    config,
                    document: None,
                };
                return (file, vec![problem]);
            }
        };

        let mut loader = Loader {
            path: path.clone(),
            text: &text,
            home: config.home.clone(),
            problems: Vec::new(),
        };
        loader.read_document(&document, &mut config);
        loader
            .problems
            .sort_by_key(|problem| (problem.line, problem.column));

        let file = Self {
            path,
            config,
            document: Some(document.into_mut()),
        };
        (file, loader.problems)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn config_mut(&mut self) -> &mut Config {
        &mut self.config
    }

    /// False after a file that did not parse; changes are kept in memory but
    /// never saved.
    pub fn is_writable(&self) -> bool {
        self.document.is_some()
    }

    /// Update the in-memory config and the parsed document, then save. A file
    /// that did not parse is never written.
    pub fn apply(&mut self, change: Change) -> Result<(), ApplyError> {
        apply_to_config(&mut self.config, &change);

        let Self {
            path,
            config,
            document,
        } = self;

        let Some(document) = document.as_mut() else {
            return Err(ApplyError::Unparsable { path: path.clone() });
        };

        apply_to_document(document, &change, config);
        let text = document.to_string();
        crate::fsutil::save_atomic(path, &text).map_err(ApplyError::Io)
    }
}

/// Load just the typed settings and the problems, without keeping the document.
pub fn load(path: impl AsRef<Path>) -> (Config, Vec<ConfigProblem>) {
    let (file, problems) = ConfigFile::load(path);
    (file.config, problems)
}

/// A save could not happen.
#[derive(Debug)]
pub enum ApplyError {
    /// The file exists but did not parse, so nothing was written.
    Unparsable {
        path: PathBuf,
    },
    Io(io::Error),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unparsable { path } => write!(
                f,
                "not saving {}: the file does not parse as TOML",
                path.display()
            ),
            Self::Io(error) => write!(f, "could not save the config: {error}"),
        }
    }
}

impl std::error::Error for ApplyError {}

/// Turn every `Settings` command into the change it writes. Commands that need
/// the focused folder use `current_folder`.
pub fn change_for(
    command: &CheckedCommand,
    config: &Config,
    current_folder: &Path,
) -> Option<Change> {
    match command.name() {
        "view.set-scale" => number_argument(command, "scale").map(Change::SetScale),
        "view.zoom-in" => stepped_scale(config.look.scale, 5).map(Change::SetScale),
        "view.zoom-out" => stepped_scale(config.look.scale, -5).map(Change::SetScale),
        "view.set-opacity" => number_argument(command, "opacity").map(Change::SetOpacity),
        "view.toggle-sidebar" => {
            let side = Side::parse(choice_argument(command, "side")?)?;
            Some(Change::SetSidebarCollapsed {
                side,
                collapsed: !config.sidebars.get(side).collapsed,
            })
        }
        "view.toggle-section" => {
            let section = Section::parse(choice_argument(command, "section")?)?;
            Some(Change::SetSection {
                section,
                state: config.sections.get(section).toggled(),
            })
        }
        "view.set-sort" => {
            let by = SortBy::parse(choice_argument(command, "by")?)?;
            let order = SortOrder::parse(choice_argument(command, "order")?)?;
            let scope = choice_argument(command, "scope").unwrap_or("folder");
            match scope {
                "global" => Some(Change::SetSort {
                    folder: None,
                    by,
                    order,
                }),
                "folder" => Some(Change::SetSort {
                    folder: Some(current_folder.to_path_buf()),
                    by,
                    order,
                }),
                _ => None,
            }
        }
        "pins.add" => command
            .get("path")
            .and_then(path_argument)
            .or_else(|| Some(current_folder.to_path_buf()))
            .map(Change::AddPin),
        "pins.remove" => command
            .get("path")
            .and_then(path_argument)
            .map(Change::RemovePin),
        "pins.move" => Some(Change::MovePin {
            from: integer_argument(command, "from")? as usize,
            to: integer_argument(command, "to")? as usize,
        }),
        _ => None,
    }
}

fn stepped_scale(current: f64, step: i32) -> Option<f64> {
    let hundredths = (current * 100.0).round() as i32;
    let next = hundredths + step;
    if (50..=100).contains(&next) {
        Some(f64::from(next) / 100.0)
    } else {
        None
    }
}

fn number_argument(command: &CheckedCommand, name: &str) -> Option<f64> {
    match command.get(name) {
        Some(CommandValue::Number(number)) => Some(number.as_f64()),
        _ => None,
    }
}

fn integer_argument(command: &CheckedCommand, name: &str) -> Option<i64> {
    match command.get(name) {
        Some(CommandValue::Int(value)) => Some(*value),
        _ => None,
    }
}

fn choice_argument<'a>(command: &'a CheckedCommand, name: &str) -> Option<&'a str> {
    match command.get(name) {
        Some(CommandValue::Choice(value)) => Some(value),
        _ => None,
    }
}

fn path_argument(value: &CommandValue) -> Option<PathBuf> {
    match value {
        CommandValue::Path(path) => Some(path.clone()),
        _ => None,
    }
}

// -------------------------------------------------------------------------
// Loading
// -------------------------------------------------------------------------

struct Loader<'a> {
    path: PathBuf,
    text: &'a str,
    home: PathBuf,
    problems: Vec<ConfigProblem>,
}

impl Loader<'_> {
    fn report(
        &mut self,
        kind: ConfigProblemKind,
        key: impl Into<String>,
        span: Option<Range<usize>>,
        message: impl Into<String>,
    ) {
        let (line, column) = span
            .map(|span| line_column(self.text, span.start))
            .unwrap_or((1, 1));
        self.problems.push(ConfigProblem::new(
            &self.path, line, column, key, kind, message,
        ));
    }

    fn report_item(
        &mut self,
        kind: ConfigProblemKind,
        key: impl Into<String>,
        item: &Item,
        message: impl Into<String>,
    ) {
        self.report(kind, key, item.span(), message);
    }

    fn wrong_type(&mut self, key: impl Into<String>, item: &Item, expected: &str) {
        let found = item.type_name();
        self.report_item(
            ConfigProblemKind::WrongType,
            key,
            item,
            format!("expected {expected}, found {found}"),
        );
    }

    fn read_document(&mut self, document: &Document<&str>, config: &mut Config) {
        let root = document.as_table();

        for (key, item) in root.iter() {
            if !KNOWN_TOP_LEVEL.contains(&key) {
                self.unknown_entry(root, "", key, item);
            }
        }

        self.read_look(root, config);
        self.read_sidebars(root, config);
        self.read_sections(root, config);
        self.read_pins(root, config);
        self.read_sort(root, config);
        self.read_hotkeys(root, config);
    }

    fn unknown_entry(&mut self, table: &dyn TableLike, prefix: &str, key: &str, item: &Item) {
        let kind = if is_table_like(item) || matches!(item, Item::ArrayOfTables(_)) {
            ConfigProblemKind::UnknownTable
        } else {
            ConfigProblemKind::UnknownKey
        };
        let span = table
            .key(key)
            .and_then(toml_edit::Key::span)
            .or_else(|| item.span());
        let key_path = join_key(prefix, key);
        let message = if kind == ConfigProblemKind::UnknownTable {
            "unknown table; kept in the file"
        } else {
            "unknown key; kept in the file"
        };
        self.report(kind, key_path, span, message);
    }

    fn read_look(&mut self, root: &Table, config: &mut Config) {
        let Some(item) = root.get("look") else {
            return;
        };
        let Some(table) = table_like(item) else {
            self.wrong_type("look", item, "a table");
            return;
        };

        for (key, value) in table.iter() {
            if !matches!(key, "scale" | "base" | "opacity" | "font") {
                self.unknown_entry(table, "look", key, value);
            }
        }

        if let Some(item) = table.get("scale")
            && let Some(value) = self.read_scale(item, "look.scale")
        {
            config.look.scale = value;
        }
        if let Some(item) = table.get("base")
            && let Some(value) = self.read_number_in_range(item, "look.base", MIN_BASE, MAX_BASE)
        {
            config.look.base = value;
        }
        if let Some(item) = table.get("opacity")
            && let Some(value) =
                self.read_number_in_range(item, "look.opacity", MIN_OPACITY, MAX_OPACITY)
        {
            config.look.opacity = value;
        }
        if let Some(item) = table.get("font")
            && let Some(value) = self.read_string(item, "look.font")
        {
            config.look.font = value;
        }
    }

    fn read_sidebars(&mut self, root: &Table, config: &mut Config) {
        let Some(item) = root.get("sidebars") else {
            return;
        };
        let Some(table) = table_like(item) else {
            self.wrong_type("sidebars", item, "a table");
            return;
        };

        for (key, value) in table.iter() {
            if !matches!(key, "left" | "right") {
                self.unknown_entry(table, "sidebars", key, value);
            }
        }

        if let Some(item) = table.get("left") {
            self.read_sidebar(
                item,
                "sidebars.left",
                MIN_LEFT_WIDTH,
                &mut config.sidebars.left,
            );
        }
        if let Some(item) = table.get("right") {
            self.read_sidebar(
                item,
                "sidebars.right",
                MIN_RIGHT_WIDTH,
                &mut config.sidebars.right,
            );
        }
    }

    fn read_sidebar(&mut self, item: &Item, key: &str, minimum: u32, sidebar: &mut Sidebar) {
        let Some(table) = table_like(item) else {
            self.wrong_type(key, item, "a table");
            return;
        };

        for (child, value) in table.iter() {
            if !matches!(child, "collapsed" | "width") {
                self.unknown_entry(table, key, child, value);
            }
        }

        if let Some(item) = table.get("collapsed")
            && let Some(value) = self.read_bool(item, &format!("{key}.collapsed"))
        {
            sidebar.collapsed = value;
        }
        if let Some(item) = table.get("width")
            && let Some(value) = self.read_width(item, &format!("{key}.width"), minimum)
        {
            sidebar.width = value;
        }
    }

    fn read_sections(&mut self, root: &Table, config: &mut Config) {
        let Some(item) = root.get("sections") else {
            return;
        };
        let Some(table) = table_like(item) else {
            self.wrong_type("sections", item, "a table");
            return;
        };

        for (key, value) in table.iter() {
            if !matches!(key, "recent" | "devices" | "remote") {
                self.unknown_entry(table, "sections", key, value);
            }
        }

        for section in [Section::Recent, Section::Devices, Section::Remote] {
            let key = section.as_str();
            if let Some(item) = table.get(key)
                && let Some(state) = self.read_section_state(item, &join_key("sections", key))
            {
                config.sections.set(section, state);
            }
        }
    }

    fn read_pins(&mut self, root: &Table, config: &mut Config) {
        let Some(item) = root.get("pins") else {
            return;
        };
        let Some(table) = table_like(item) else {
            self.wrong_type("pins", item, "a table");
            return;
        };

        for (key, value) in table.iter() {
            if key != "folders" {
                self.unknown_entry(table, "pins", key, value);
            }
        }

        let Some(item) = table.get("folders") else {
            return;
        };
        let Some(array) = item.as_value().and_then(toml_edit::Value::as_array) else {
            self.wrong_type("pins.folders", item, "an array of paths");
            return;
        };

        let mut pins = Vec::new();
        for (index, value) in array.iter().enumerate() {
            let key = format!("pins.folders[{index}]");
            let Some(text) = value.as_str() else {
                self.report(
                    ConfigProblemKind::WrongType,
                    key,
                    value.span(),
                    "expected a path string",
                );
                continue;
            };
            let expanded = expand_home(Path::new(text), &self.home);
            if !expanded.is_absolute() {
                self.report(
                    ConfigProblemKind::RelativePin,
                    key,
                    value.span(),
                    format!("path `{text}` is not absolute after `~` expansion"),
                );
                continue;
            }
            pins.push(normalize_path(&expanded));
        }
        config.pins = pins;
    }

    fn read_sort(&mut self, root: &Table, config: &mut Config) {
        let Some(item) = root.get("sort") else {
            return;
        };
        let Some(table) = table_like(item) else {
            self.wrong_type("sort", item, "a table");
            return;
        };

        for (key, value) in table.iter() {
            if !matches!(key, "by" | "order" | "folders-first" | "folder") {
                self.unknown_entry(table, "sort", key, value);
            }
        }

        if let Some(item) = table.get("by")
            && let Some(by) = self.read_sort_by(item, "sort.by")
        {
            config.sort.by = by;
        }
        if let Some(item) = table.get("order")
            && let Some(order) = self.read_sort_order(item, "sort.order")
        {
            config.sort.order = order;
        }
        if let Some(item) = table.get("folders-first")
            && let Some(value) = self.read_bool(item, "sort.folders-first")
        {
            config.sort.folders_first = value;
        }

        let Some(item) = table.get("folder") else {
            return;
        };

        match item {
            Item::ArrayOfTables(tables) => {
                for (index, rule) in tables.iter().enumerate() {
                    self.read_folder_sort(rule, rule.span(), index, config);
                }
            }
            Item::Value(value) if value.is_array() => {
                for (index, rule) in value.as_array().unwrap().iter().enumerate() {
                    let Some(table) = rule.as_inline_table() else {
                        self.report(
                            ConfigProblemKind::WrongType,
                            format!("sort.folder[{index}]"),
                            rule.span(),
                            "expected a table",
                        );
                        continue;
                    };
                    self.read_folder_sort(table, rule.span(), index, config);
                }
            }
            _ => self.wrong_type("sort.folder", item, "an array of tables"),
        }
    }

    fn read_folder_sort(
        &mut self,
        table: &dyn TableLike,
        fallback: Option<Range<usize>>,
        index: usize,
        config: &mut Config,
    ) {
        let prefix = format!("sort.folder[{index}]");

        for (key, value) in table.iter() {
            if !matches!(key, "path" | "by" | "order") {
                self.unknown_entry(table, &prefix, key, value);
            }
        }

        let Some(path_item) = table.get("path") else {
            self.report(
                ConfigProblemKind::MissingValue,
                join_key(&prefix, "path"),
                fallback,
                "missing the folder path",
            );
            return;
        };
        let Some(text) = self.read_string(path_item, &join_key(&prefix, "path")) else {
            return;
        };
        let expanded = expand_home(Path::new(&text), &self.home);
        if !expanded.is_absolute() {
            self.report_item(
                ConfigProblemKind::RelativePin,
                join_key(&prefix, "path"),
                path_item,
                format!("path `{text}` is not absolute after `~` expansion"),
            );
            return;
        }
        let folder = normalize_path(&expanded);

        let by = table
            .get("by")
            .and_then(|item| self.read_sort_by(item, &join_key(&prefix, "by")))
            .unwrap_or(config.sort.by);
        let order = table
            .get("order")
            .and_then(|item| self.read_sort_order(item, &join_key(&prefix, "order")))
            .unwrap_or(config.sort.order);

        config.sort.folders.push(FolderSort { folder, by, order });
    }

    fn read_hotkeys(&mut self, root: &Table, config: &mut Config) {
        let Some(item) = root.get("hotkeys") else {
            return;
        };
        let Some(table) = table_like(item) else {
            self.wrong_type("hotkeys", item, "a table");
            return;
        };

        for (accelerator, item) in table.iter() {
            let key = format!("hotkeys.\"{accelerator}\"");
            let key_span = table.key(accelerator).and_then(toml_edit::Key::span);

            if accelerator.is_empty() {
                self.report(
                    ConfigProblemKind::EmptyAccelerator,
                    key,
                    key_span,
                    "a hotkey accelerator must not be empty",
                );
                continue;
            }

            let Some(value) = item.as_value() else {
                self.wrong_type(key, item, "a command name or an inline table");
                continue;
            };

            match value {
                toml_edit::Value::String(command) => {
                    if command.value().is_empty() {
                        config.hotkeys.remove(accelerator);
                        continue;
                    }
                    let hotkey = Hotkey {
                        command: command.value().clone(),
                        arguments: Vec::new(),
                    };
                    self.insert_hotkey(config, accelerator, hotkey, item, &key);
                }
                toml_edit::Value::InlineTable(inline) => {
                    let Some(hotkey) = self.read_inline_hotkey(inline, &key) else {
                        continue;
                    };
                    self.insert_hotkey(config, accelerator, hotkey, item, &key);
                }
                _ => self.wrong_type(key, item, "a command name or an inline table"),
            }
        }
    }

    fn read_inline_hotkey(&mut self, inline: &InlineTable, key: &str) -> Option<Hotkey> {
        let Some(command_value) = inline.get("command") else {
            self.report(
                ConfigProblemKind::MissingValue,
                join_key(key, "command"),
                inline.span(),
                "missing the command name",
            );
            return None;
        };

        let Some(command) = command_value.as_str() else {
            self.report(
                ConfigProblemKind::WrongType,
                join_key(key, "command"),
                inline.key("command").and_then(toml_edit::Key::span),
                "expected a command name string",
            );
            return None;
        };

        let mut arguments = Vec::new();
        for (name, value) in inline.iter() {
            if name == "command" {
                continue;
            }
            match hotkey_tokens(value) {
                Ok(tokens) => {
                    for token in tokens {
                        arguments.push((name.to_string(), token));
                    }
                }
                Err(message) => {
                    self.report(
                        ConfigProblemKind::InvalidHotkeyArguments,
                        join_key(key, name),
                        value.span(),
                        message,
                    );
                    return None;
                }
            }
        }

        Some(Hotkey {
            command: command.to_string(),
            arguments,
        })
    }

    fn insert_hotkey(
        &mut self,
        config: &mut Config,
        accelerator: &str,
        hotkey: Hotkey,
        item: &Item,
        key: &str,
    ) {
        let invocation = hotkey.invocation();
        match Registry::new().validate(Caller::Hotkey, &invocation) {
            Ok(_) => {
                config.hotkeys.insert(accelerator.to_string(), hotkey);
            }
            Err(error) => {
                let kind = match error.kind() {
                    crate::command::CommandErrorKind::UnknownCommand { .. } => {
                        ConfigProblemKind::UnknownCommand
                    }
                    _ => ConfigProblemKind::InvalidHotkeyArguments,
                };
                self.report_item(kind, key, item, error.to_string());
            }
        }
    }

    fn read_scale(&mut self, item: &Item, key: &str) -> Option<f64> {
        let value = self.read_number(item, key)?;
        let hundredths = value * 100.0;
        let rounded = hundredths.round();

        if (hundredths - rounded).abs() > 1e-6 {
            self.report_item(
                ConfigProblemKind::OffStep,
                key,
                item,
                format!("value `{value}` is off the {} step", SCALE_STEP),
            );
            return None;
        }

        let hundredths = rounded as i32;
        if !(50..=100).contains(&hundredths) {
            self.report_item(
                ConfigProblemKind::OutOfRange,
                key,
                item,
                format!("value `{value}` is outside {MIN_SCALE:.2}..{MAX_SCALE:.2}"),
            );
            return None;
        }
        if (hundredths - 50) % 5 != 0 {
            self.report_item(
                ConfigProblemKind::OffStep,
                key,
                item,
                format!("value `{value}` is off the {} step", SCALE_STEP),
            );
            return None;
        }

        Some(f64::from(hundredths) / 100.0)
    }

    fn read_number_in_range(
        &mut self,
        item: &Item,
        key: &str,
        minimum: f64,
        maximum: f64,
    ) -> Option<f64> {
        let value = self.read_number(item, key)?;
        if (minimum..=maximum).contains(&value) {
            Some(value)
        } else {
            self.report_item(
                ConfigProblemKind::OutOfRange,
                key,
                item,
                format!("value `{value}` is outside {minimum:.2}..{maximum:.2}"),
            );
            None
        }
    }

    fn read_number(&mut self, item: &Item, key: &str) -> Option<f64> {
        let Some(value) = item.as_value() else {
            self.wrong_type(key, item, "a number");
            return None;
        };
        match value {
            toml_edit::Value::Float(number) if number.value().is_finite() => Some(*number.value()),
            toml_edit::Value::Integer(number) => Some(*number.value() as f64),
            _ => {
                self.wrong_type(key, item, "a finite number");
                None
            }
        }
    }

    fn read_bool(&mut self, item: &Item, key: &str) -> Option<bool> {
        match item.as_value().and_then(toml_edit::Value::as_bool) {
            Some(value) => Some(value),
            None => {
                self.wrong_type(key, item, "true or false");
                None
            }
        }
    }

    fn read_string(&mut self, item: &Item, key: &str) -> Option<String> {
        match item.as_value().and_then(toml_edit::Value::as_str) {
            Some(value) => Some(value.to_string()),
            None => {
                self.wrong_type(key, item, "a string");
                None
            }
        }
    }

    fn read_width(&mut self, item: &Item, key: &str, minimum: u32) -> Option<u32> {
        let value = match item.as_value().and_then(toml_edit::Value::as_integer) {
            Some(value) => value,
            None => {
                self.wrong_type(key, item, "an integer width");
                return None;
            }
        };

        if value < i64::from(minimum) {
            self.report_item(
                ConfigProblemKind::OutOfRange,
                key,
                item,
                format!("width `{value}` is below the minimum {minimum}"),
            );
            return None;
        }
        if value > i64::from(u32::MAX) {
            self.report_item(
                ConfigProblemKind::OutOfRange,
                key,
                item,
                format!("width `{value}` is above {}", u32::MAX),
            );
            return None;
        }

        Some(value as u32)
    }

    fn read_section_state(&mut self, item: &Item, key: &str) -> Option<SectionState> {
        let text = self.read_string(item, key)?;
        match SectionState::parse(&text) {
            Some(state) => Some(state),
            None => {
                self.report_item(
                    ConfigProblemKind::BadChoice,
                    key,
                    item,
                    format!("`{text}` is not `expanded` or `collapsed`"),
                );
                None
            }
        }
    }

    fn read_sort_by(&mut self, item: &Item, key: &str) -> Option<SortBy> {
        let text = self.read_string(item, key)?;
        match SortBy::parse(&text) {
            Some(by) => Some(by),
            None => {
                self.report_item(
                    ConfigProblemKind::BadChoice,
                    key,
                    item,
                    format!("`{text}` is not `name`, `modified`, `size` or `type`"),
                );
                None
            }
        }
    }

    fn read_sort_order(&mut self, item: &Item, key: &str) -> Option<SortOrder> {
        let text = self.read_string(item, key)?;
        match SortOrder::parse(&text) {
            Some(order) => Some(order),
            None => {
                self.report_item(
                    ConfigProblemKind::BadChoice,
                    key,
                    item,
                    format!("`{text}` is not `ascending` or `descending`"),
                );
                None
            }
        }
    }
}

fn is_table_like(item: &Item) -> bool {
    match item {
        Item::Table(_) => true,
        Item::Value(value) => value.is_inline_table(),
        _ => false,
    }
}

fn table_like(item: &Item) -> Option<&dyn TableLike> {
    match item {
        Item::Table(table) => Some(table),
        Item::Value(value) => value.as_inline_table().map(|table| table as &dyn TableLike),
        _ => None,
    }
}

fn join_key(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

fn hotkey_tokens(value: &toml_edit::Value) -> Result<Vec<String>, String> {
    match value {
        toml_edit::Value::String(text) => Ok(vec![text.value().clone()]),
        toml_edit::Value::Integer(number) => Ok(vec![number.value().to_string()]),
        toml_edit::Value::Boolean(flag) => Ok(vec![flag.value().to_string()]),
        toml_edit::Value::Float(number) => Ok(vec![number.value().to_string()]),
        toml_edit::Value::Array(array) => {
            let mut tokens = Vec::new();
            for value in array.iter() {
                match value {
                    toml_edit::Value::String(text) => tokens.push(text.value().clone()),
                    toml_edit::Value::Integer(number) => tokens.push(number.value().to_string()),
                    toml_edit::Value::Boolean(flag) => tokens.push(flag.value().to_string()),
                    toml_edit::Value::Float(number) => tokens.push(number.value().to_string()),
                    _ => {
                        return Err(
                            "expected text, integer, boolean or an array of those".to_string()
                        );
                    }
                }
            }
            Ok(tokens)
        }
        _ => Err("expected text, integer, boolean or an array of those".to_string()),
    }
}

/// The one-based line and column of a byte offset.
fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = before.rfind('\n').map(|index| index + 1).unwrap_or(0);
    let column = text[line_start..offset].chars().count() + 1;
    (line, column)
}

// -------------------------------------------------------------------------
// Applying changes
// -------------------------------------------------------------------------

fn apply_to_config(config: &mut Config, change: &Change) {
    match change {
        Change::SetScale(value) => {
            config.look.scale = round_hundredths(*value)
                .map(|hundredths| f64::from(hundredths) / 100.0)
                .unwrap_or(*value);
        }
        Change::SetBase(value) => config.look.base = *value,
        Change::SetOpacity(value) => config.look.opacity = *value,
        Change::SetFont(font) => config.look.font = font.clone(),
        Change::SetSidebarCollapsed { side, collapsed } => {
            config.sidebars.get_mut(*side).collapsed = *collapsed;
        }
        Change::SetSection { section, state } => config.sections.set(*section, *state),
        Change::SetSidebarWidth { side, width } => {
            config.sidebars.get_mut(*side).width = *width;
        }
        Change::AddPin(path) => {
            let path = normalize_path(&expand_home(path, &config.home));
            if !config.pins.contains(&path) {
                config.pins.push(path);
            }
        }
        Change::RemovePin(path) => {
            let path = normalize_path(&expand_home(path, &config.home));
            if let Some(index) = config.pins.iter().position(|pin| pin == &path) {
                config.pins.remove(index);
            }
        }
        Change::MovePin { from, to } => {
            if *from < config.pins.len() && *to < config.pins.len() {
                let pin = config.pins.remove(*from);
                config.pins.insert(*to, pin);
            }
        }
        Change::SetSort { folder, by, order } => {
            match folder {
                None => {
                    config.sort.by = *by;
                    config.sort.order = *order;
                }
                Some(folder) => {
                    let folder = normalize_path(&expand_home(folder, &config.home));
                    if let Some(rule) = config.sort.folders.iter_mut().rev().find(|rule| {
                        normalize_path(&expand_home(&rule.folder, &config.home)) == folder
                    }) {
                        rule.by = *by;
                        rule.order = *order;
                    } else {
                        config.sort.folders.push(FolderSort {
                            folder,
                            by: *by,
                            order: *order,
                        });
                    }
                }
            }
        }
    }
}

fn round_hundredths(value: f64) -> Option<i32> {
    if !value.is_finite() {
        return None;
    }
    let hundredths = value * 100.0;
    let rounded = hundredths.round();
    if (hundredths - rounded).abs() <= 1e-6 {
        Some(rounded as i32)
    } else {
        None
    }
}

fn apply_to_document(document: &mut DocumentMut, change: &Change, config: &Config) {
    match change {
        Change::SetScale(value) => {
            let value = round_hundredths(*value)
                .map(|hundredths| f64::from(hundredths) / 100.0)
                .unwrap_or(*value);
            set_root_value(document, "look", "scale", Value::from(value));
        }
        Change::SetBase(value) => set_root_value(document, "look", "base", Value::from(*value)),
        Change::SetOpacity(value) => {
            set_root_value(document, "look", "opacity", Value::from(*value));
        }
        Change::SetFont(font) => {
            set_root_value(document, "look", "font", Value::from(font.clone()));
        }
        Change::SetSidebarCollapsed { side, collapsed } => {
            set_side_value(document, *side, "collapsed", Value::from(*collapsed));
        }
        Change::SetSidebarWidth { side, width } => {
            set_side_value(document, *side, "width", Value::from(i64::from(*width)));
        }
        Change::SetSection { section, state } => {
            set_root_value(
                document,
                "sections",
                section.as_str(),
                Value::from(state.as_str()),
            );
        }
        // With no pin list in the file yet, the in-memory list (the defaults
        // plus this change, already applied) is written whole. Writing only
        // the one change would replace the defaults with a single pin on the
        // next load (found in P2i).
        Change::AddPin(_) | Change::RemovePin(_) | Change::MovePin { .. }
            if !has_pins_array(document) =>
        {
            let array = pins_array(document);
            for pin in &config.pins {
                array.push(Value::from(path_string(pin)));
            }
        }
        Change::AddPin(path) => {
            let target = normalize_path(&expand_home(path, &config.home));
            let array = pins_array(document);
            if !array_has_pin(array, &config.home, &target) {
                array.push(Value::from(path_string(path)));
            }
        }
        Change::RemovePin(path) => {
            let target = normalize_path(&expand_home(path, &config.home));
            let array = pins_array(document);
            if let Some(index) = pin_index(array, &config.home, &target) {
                array.remove(index);
            }
        }
        Change::MovePin { from, to } => {
            let array = pins_array(document);
            if *from < array.len() && *to < array.len() {
                let pin = array.remove(*from);
                array.insert_formatted(*to, pin);
            }
        }
        Change::SetSort { folder, by, order } => {
            let sort = ensure_table(document, "sort");
            // A folder rule leaves the global sort alone.
            let Some(folder) = folder else {
                set_table_value(sort, "by", Value::from(by.as_str()));
                set_table_value(sort, "order", Value::from(order.as_str()));
                return;
            };
            let target = normalize_path(&expand_home(folder, &config.home));
            let name = path_string(&target);

            let item = sort
                .get_mut("folder")
                .filter(|item| matches!(item, Item::ArrayOfTables(_)) || is_table_array(item));
            let Some(item) = item else {
                let mut tables = ArrayOfTables::new();
                tables.push(folder_sort_table(&name, *by, *order));
                sort.insert("folder", Item::ArrayOfTables(tables));
                return;
            };

            match item {
                Item::ArrayOfTables(tables) => {
                    let existing = tables.iter_mut().find(|table| {
                        table
                            .get("path")
                            .and_then(Item::as_value)
                            .and_then(toml_edit::Value::as_str)
                            .map(|text| {
                                normalize_path(&expand_home(Path::new(text), &config.home))
                                    == target
                            })
                            .unwrap_or(false)
                    });
                    if let Some(table) = existing {
                        set_table_value(table, "by", Value::from(by.as_str()));
                        set_table_value(table, "order", Value::from(order.as_str()));
                    } else {
                        tables.push(folder_sort_table(&name, *by, *order));
                    }
                }
                Item::Value(value) => {
                    let array = value.as_array_mut().expect("filtered to an array");
                    let existing = array.iter_mut().find_map(|value| {
                        let table = value.as_inline_table_mut()?;
                        let text = table.get("path")?.as_str()?;
                        (normalize_path(&expand_home(Path::new(text), &config.home)) == target)
                            .then_some(table)
                    });
                    if let Some(table) = existing {
                        set_inline_value(table, "by", Value::from(by.as_str()));
                        set_inline_value(table, "order", Value::from(order.as_str()));
                    } else {
                        array.push(Value::InlineTable(folder_sort_inline(&name, *by, *order)));
                    }
                }
                _ => {}
            }
        }
    }
}

fn is_table_array(item: &Item) -> bool {
    item.as_value().is_some_and(toml_edit::Value::is_array)
}

fn folder_sort_table(path: &str, by: SortBy, order: SortOrder) -> Table {
    let mut table = Table::new();
    table.insert("path", Item::Value(Value::from(path)));
    table.insert("by", Item::Value(Value::from(by.as_str())));
    table.insert("order", Item::Value(Value::from(order.as_str())));
    table
}

fn folder_sort_inline(path: &str, by: SortBy, order: SortOrder) -> InlineTable {
    let mut table = InlineTable::new();
    table.insert("path", Value::from(path));
    table.insert("by", Value::from(by.as_str()));
    table.insert("order", Value::from(order.as_str()));
    table
}

fn set_root_value(document: &mut DocumentMut, table: &str, key: &str, value: Value) {
    let root = document.as_table_mut();
    match root.get_mut(table) {
        Some(Item::Table(table)) => set_table_value(table, key, value),
        Some(Item::Value(existing)) if existing.is_inline_table() => {
            set_inline_value(existing.as_inline_table_mut().unwrap(), key, value);
        }
        _ => {
            let mut table_value = Table::new();
            set_table_value(&mut table_value, key, value);
            root.insert(table, Item::Table(table_value));
        }
    }
}

fn ensure_table<'a>(document: &'a mut DocumentMut, name: &str) -> &'a mut Table {
    let root = document.as_table_mut();

    let replace = match root.get(name) {
        Some(Item::Table(_)) => false,
        Some(Item::Value(value)) if value.is_inline_table() => true,
        _ => true,
    };

    if replace {
        let table = match root.remove(name) {
            Some(Item::Value(mut value)) if value.is_inline_table() => {
                std::mem::take(value.as_inline_table_mut().unwrap()).into_table()
            }
            _ => Table::new(),
        };
        root.insert(name, Item::Table(table));
    }

    root.get_mut(name).unwrap().as_table_mut().unwrap()
}

fn set_table_value(table: &mut Table, key: &str, mut value: Value) {
    if let Some(item) = table.get_mut(key) {
        if let Some(old) = item.as_value() {
            *value.decor_mut() = old.decor().clone();
        }
        *item = Item::Value(value);
    } else {
        table.insert(key, Item::Value(value));
    }
}

fn set_inline_value(table: &mut InlineTable, key: &str, mut value: Value) {
    let decor = table.get(key).map(|old| old.decor().clone());
    let key_format = table.key(key).cloned();

    if let Some(decor) = decor {
        *value.decor_mut() = decor;
    }

    match key_format {
        Some(key) => {
            table.insert_formatted(&key, value);
        }
        None => {
            table.insert(key, value);
        }
    }
}

fn set_side_value(document: &mut DocumentMut, side: Side, key: &str, value: Value) {
    let sidebars = ensure_table(document, "sidebars");
    let name = side.as_str();

    let valid = sidebars.get(name).is_some_and(is_table_like);
    if !valid {
        sidebars.insert(name, Item::Value(Value::InlineTable(InlineTable::new())));
    }

    match sidebars.get_mut(name).unwrap() {
        Item::Table(table) => set_table_value(table, key, value),
        Item::Value(existing) => {
            if let Some(table) = existing.as_inline_table_mut() {
                set_inline_value(table, key, value);
            }
        }
        _ => {}
    }
}

fn has_pins_array(document: &DocumentMut) -> bool {
    document
        .get("pins")
        .and_then(|pins| pins.get("folders"))
        .and_then(Item::as_value)
        .is_some_and(toml_edit::Value::is_array)
}

fn pins_array(document: &mut DocumentMut) -> &mut Array {
    let pins = ensure_table(document, "pins");
    let valid = pins
        .get("folders")
        .and_then(Item::as_value)
        .is_some_and(toml_edit::Value::is_array);

    if !valid {
        pins.insert("folders", Item::Value(Value::Array(Array::new())));
    }

    pins.get_mut("folders")
        .unwrap()
        .as_value_mut()
        .unwrap()
        .as_array_mut()
        .unwrap()
}

fn array_has_pin(array: &Array, home: &Path, target: &Path) -> bool {
    pin_index(array, home, target).is_some()
}

fn pin_index(array: &Array, home: &Path, target: &Path) -> Option<usize> {
    array.iter().position(|value| {
        value
            .as_str()
            .map(|text| normalize_path(&expand_home(Path::new(text), home)) == target)
            .unwrap_or(false)
    })
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = Path::new("/tmp").join(format!(
                "gimbal-p2c-{tag}-{}-{sequence}",
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

    fn default_config(home: &Path) -> Config {
        Config::with_defaults(home, &SpecialFolders::default())
    }

    fn check(_name: &str, tokens: &[&str]) -> CheckedCommand {
        let tokens: Vec<String> = tokens.iter().map(|token| token.to_string()).collect();
        let invocation = Invocation::parse(&tokens).unwrap();
        Registry::new()
            .validate(Caller::Ui, &invocation)
            .expect("the test's command should validate")
    }

    // ---------------------------------------------------------------------
    // Path and defaults
    // ---------------------------------------------------------------------

    #[test]
    fn config_path_is_a_pure_function_of_xdg_config_home_and_home() {
        let home = Path::new("/home/kyle");
        assert_eq!(
            config_path(Some(Path::new("/tmp/xdg")), Some(home)),
            Some(PathBuf::from("/tmp/xdg/gimbal-files/config.toml"))
        );
        assert_eq!(
            config_path(Some(Path::new("relative")), Some(home)),
            Some(PathBuf::from("/home/kyle/.config/gimbal-files/config.toml"))
        );
        assert_eq!(
            config_path(None, Some(home)),
            Some(PathBuf::from("/home/kyle/.config/gimbal-files/config.toml"))
        );
        assert_eq!(config_path(Some(Path::new("relative")), None), None);
        assert_eq!(config_path(None, None), None);
    }

    #[test]
    fn every_default_matches_the_brief() {
        let home = Path::new("/home/kyle");
        let config = default_config(home);

        assert_eq!(config.look.scale, 1.0);
        assert_eq!(config.look.base, 1.0);
        assert_eq!(config.look.opacity, 0.85);
        assert_eq!(config.look.font, "");
        assert_eq!(
            config.sidebars,
            Sidebars {
                left: Sidebar {
                    collapsed: false,
                    width: 348
                },
                right: Sidebar {
                    collapsed: false,
                    width: 648
                }
            }
        );
        assert_eq!(
            config.sections,
            Sections {
                recent: SectionState::Expanded,
                devices: SectionState::Expanded,
                remote: SectionState::Expanded
            }
        );
        assert_eq!(config.pins, vec![PathBuf::from("/home/kyle")]);
        assert_eq!(config.sort.by, SortBy::Name);
        assert_eq!(config.sort.order, SortOrder::Ascending);
        assert!(config.sort.folders_first);
        assert!(config.sort.folders.is_empty());
        assert_eq!(config.hotkeys.len(), 18);
        assert_eq!(config.home, PathBuf::from("/home/kyle"));

        let expected = [
            ("<Control>t", "tab.new"),
            ("<Control>w", "tab.close"),
            ("<Control>n", "window.new"),
            ("<Control>q", "app.quit"),
            ("<Control>l", "nav.focus-path"),
            ("<Control>f", "search.open"),
            ("<Control>a", "selection.all"),
            ("<Control>z", "file.undo"),
            ("<Control>equal", "view.zoom-in"),
            ("<Control>minus", "view.zoom-out"),
            ("<Alt>Left", "nav.back"),
            ("<Alt>Right", "nav.forward"),
            ("<Alt>Up", "nav.up"),
            ("F5", "nav.refresh"),
            ("Delete", "file.trash"),
        ];
        for (accelerator, command) in expected {
            assert_eq!(config.hotkeys[accelerator].command, command);
            assert!(config.hotkeys[accelerator].arguments.is_empty());
        }
    }

    #[test]
    fn default_pins_include_home_then_only_existing_special_folders() {
        let temp = TempDir::new("default-pins");
        let home = temp.path();
        for folder in ["Desktop", "Documents", "Music", "Videos"] {
            fs::create_dir_all(home.join(folder)).unwrap();
        }

        let special = SpecialFolders {
            desktop: Some(home.join("Desktop")),
            documents: Some(home.join("Documents")),
            downloads: Some(home.join("Downloads")),
            music: Some(home.join("Music")),
            pictures: None,
            videos: Some(home.join("Videos")),
        };

        assert_eq!(
            default_pins(home, &special),
            vec![
                normalize_path(home),
                normalize_path(&home.join("Desktop")),
                normalize_path(&home.join("Documents")),
                normalize_path(&home.join("Music")),
                normalize_path(&home.join("Videos")),
            ]
        );

        let mut duplicate = special;
        duplicate.documents = Some(home.to_path_buf());
        let pins = default_pins(home, &duplicate);
        assert_eq!(pins.iter().filter(|pin| *pin == home).count(), 1);
    }

    #[test]
    fn sort_for_expands_home_and_normalizes_dot_dot_and_trailing_slashes() {
        let temp = TempDir::new("sort-for");
        let home = temp.path();
        let config_text = r#"
[sort]
by = "name"
order = "ascending"
folders-first = true

[[sort.folder]]
path = "~/Documents//../Downloads/"
by = "modified"
order = "descending"
"#;
        let path = temp.write("config.toml", config_text);
        let (file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert!(problems.is_empty(), "{problems:?}");

        let rule = file.config().sort_for(&home.join("Downloads"));
        assert_eq!(
            rule,
            SortRule {
                by: SortBy::Modified,
                order: SortOrder::Descending,
                folders_first: true
            }
        );

        let trailing = file
            .config()
            .sort_for(Path::new(&format!("{}/Downloads/", home.display())));
        assert_eq!(trailing, rule);

        let subfolder = file.config().sort_for(&home.join("Downloads/inner"));
        assert_eq!(subfolder.by, SortBy::Name);
        assert_eq!(subfolder.order, SortOrder::Ascending);
    }

    // ---------------------------------------------------------------------
    // Problems
    // ---------------------------------------------------------------------

    #[test]
    fn every_problem_kind_reports_its_line_and_column() {
        let temp = TempDir::new("problems");
        let home = temp.path();
        let path = temp.write(
            "config.toml",
            r#"[look]
scale = 0.57
base = 3.0
font = 12

[sections]
recent = "open"

[pins]
folders = ["relative"]

[sort]
by = "sideways"

[[sort.folder]]
path = "~/Downloads"
by = "biggest"

[hotkeys]
"" = "tab.new"
"<Control>x" = "no.such-command"
"<Control>y" = { command = "tab.focus", index = -1 }

[mystery]
answer = 42

[look.extra]
value = 1
"#,
        );

        let (_file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());

        let expected = [
            ("look.scale", 2, 9, ConfigProblemKind::OffStep),
            ("look.base", 3, 8, ConfigProblemKind::OutOfRange),
            ("look.font", 4, 8, ConfigProblemKind::WrongType),
            ("sections.recent", 7, 10, ConfigProblemKind::BadChoice),
            ("pins.folders[0]", 10, 12, ConfigProblemKind::RelativePin),
            ("sort.by", 13, 6, ConfigProblemKind::BadChoice),
            ("sort.folder[0].by", 17, 6, ConfigProblemKind::BadChoice),
            ("hotkeys.\"\"", 20, 1, ConfigProblemKind::EmptyAccelerator),
            (
                "hotkeys.\"<Control>x\"",
                21,
                16,
                ConfigProblemKind::UnknownCommand,
            ),
            (
                "hotkeys.\"<Control>y\"",
                22,
                16,
                ConfigProblemKind::InvalidHotkeyArguments,
            ),
            ("mystery", 24, 2, ConfigProblemKind::UnknownTable),
            ("look.extra", 27, 7, ConfigProblemKind::UnknownTable),
        ];

        for (key, line, column, kind) in expected {
            let problem = problems
                .iter()
                .find(|problem| problem.key == key && problem.kind == kind)
                .unwrap_or_else(|| panic!("missing problem {key} {kind:?}: {problems:#?}"));
            assert_eq!(
                (problem.line, problem.column),
                (line, column),
                "wrong position for {key}: {problem}"
            );
        }

        assert!(problems.iter().all(|problem| {
            problem.is_warning()
                == matches!(
                    problem.kind,
                    ConfigProblemKind::UnknownKey | ConfigProblemKind::UnknownTable
                )
        }));
    }

    #[test]
    fn an_unparsable_file_reports_one_problem_with_a_position() {
        let temp = TempDir::new("parse");
        let home = temp.path();
        let path = temp.write("config.toml", "[look\nscale = 1.0\n");

        let (file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].kind, ConfigProblemKind::Parse);
        assert_eq!((problems[0].line, problems[0].column), (1, 6));
        assert!(!file.is_writable());
    }

    #[test]
    fn an_unknown_key_is_a_warning_with_its_position() {
        let temp = TempDir::new("unknown-key");
        let home = temp.path();
        let path = temp.write("config.toml", "[look]\nscale = 1.0\nmystery = 1\n");

        let (_file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        let problem = problems
            .iter()
            .find(|problem| problem.key == "look.mystery")
            .expect("the unknown key should be reported");
        assert_eq!(problem.kind, ConfigProblemKind::UnknownKey);
        assert_eq!((problem.line, problem.column), (3, 1));
        assert!(problem.is_warning());
    }

    #[test]
    fn a_read_error_reports_the_file_and_position() {
        let temp = TempDir::new("read-error");
        let home = temp.path();
        let path = home.join("config.toml");
        fs::create_dir_all(&path).unwrap();

        let (file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].kind, ConfigProblemKind::Read);
        assert_eq!((problems[0].line, problems[0].column), (1, 1));
        assert!(!file.is_writable());
    }

    #[test]
    fn a_bad_value_falls_back_and_the_good_values_still_load() {
        let temp = TempDir::new("fallback");
        let home = temp.path();
        let path = temp.write(
            "config.toml",
            r#"[look]
scale = 0.57
base = 1.25
opacity = 0.7
font = "Inter"

[sidebars]
left = { collapsed = true, width = 250 }
right = { collapsed = true, width = 700 }
"#,
        );

        let (file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        let config = file.config();

        assert_eq!(config.look.scale, DEFAULT_SCALE);
        assert_eq!(config.look.base, 1.25);
        assert_eq!(config.look.opacity, 0.7);
        assert_eq!(config.look.font, "Inter");
        assert!(config.sidebars.left.collapsed);
        assert_eq!(config.sidebars.left.width, DEFAULT_LEFT_WIDTH);
        assert!(config.sidebars.right.collapsed);
        assert_eq!(config.sidebars.right.width, 700);

        assert!(problems.iter().any(|problem| problem.key == "look.scale"));
        assert!(
            problems
                .iter()
                .any(|problem| problem.key == "sidebars.left.width")
        );
    }

    #[test]
    fn unknown_keys_and_tables_are_warnings_and_stay_in_the_file() {
        let temp = TempDir::new("warnings");
        let home = temp.path();
        let path = temp.write(
            "config.toml",
            "[look]\nscale = 1.0\nmystery = 1\n\n[surprise]\nx = 1\n",
        );
        let (mut file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());

        assert_eq!(
            problems
                .iter()
                .filter(|problem| problem.is_warning())
                .count(),
            2
        );
        file.apply(Change::SetScale(0.95)).unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("mystery = 1"));
        assert!(saved.contains("[surprise]\nx = 1"));
    }

    // ---------------------------------------------------------------------
    // Hotkeys
    // ---------------------------------------------------------------------

    #[test]
    fn a_hotkeys_table_replaces_listed_keys_and_an_empty_value_unbinds() {
        let temp = TempDir::new("hotkeys");
        let home = temp.path();
        let path = temp.write(
            "config.toml",
            r#"[hotkeys]
"<Control>t" = "tab.close"
"<Control>w" = ""
"<Control>1" = { command = "tab.focus", index = 0 }
"#,
        );
        let (file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert!(problems.is_empty(), "{problems:#?}");

        let hotkeys = &file.config().hotkeys;
        assert_eq!(hotkeys["<Control>t"].command, "tab.close");
        assert!(!hotkeys.contains_key("<Control>w"));
        assert_eq!(hotkeys["<Control>1"].command, "tab.focus");
        assert_eq!(
            hotkeys["<Control>1"].arguments,
            vec![("index".to_string(), "0".to_string())]
        );
        assert_eq!(hotkeys["<Control>n"].command, "window.new");
        assert_eq!(hotkeys["Delete"].command, "file.trash");
    }

    #[test]
    fn a_hotkey_with_an_unknown_command_or_bad_arguments_keeps_the_default() {
        let temp = TempDir::new("bad-hotkeys");
        let home = temp.path();
        let path = temp.write(
            "config.toml",
            r#"[hotkeys]
"<Control>t" = "no.such-command"
"<Control>w" = { command = "tab.focus", index = -1 }
"<Control>n" = { command = "tab.focus" }
"#,
        );
        let (file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());

        assert_eq!(file.config().hotkeys["<Control>t"].command, "tab.new");
        assert_eq!(file.config().hotkeys["<Control>w"].command, "tab.close");
        assert_eq!(file.config().hotkeys["<Control>n"].command, "window.new");
        assert!(
            problems
                .iter()
                .any(|problem| problem.kind == ConfigProblemKind::UnknownCommand)
        );
        assert!(
            problems
                .iter()
                .any(|problem| problem.kind == ConfigProblemKind::InvalidHotkeyArguments)
        );
    }

    // ---------------------------------------------------------------------
    // Saving
    // ---------------------------------------------------------------------

    #[test]
    fn a_save_rewrites_only_the_changed_key_on_a_commented_file() {
        let temp = TempDir::new("save");
        let home = temp.path();
        let original = r#"# Gimbal Files settings
[look]
scale = 1.0        # the interface scale
base = 1.0         # what 100% means
opacity = 0.65     # see-through
font = ""          # the desktop's UI font

# this table is not ours
[custom]
answer = 42
note = "keep me"
"#;
        let path = temp.write("config.toml", original);

        let (mut file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert!(
            problems.iter().all(ConfigProblem::is_warning),
            "{problems:#?}"
        );

        file.apply(Change::SetOpacity(0.75)).unwrap();
        let expected = original.replace("opacity = 0.65", "opacity = 0.75");
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);

        file.apply(Change::SetFont("Inter".to_string())).unwrap();
        let expected = expected.replace("font = \"\"", "font = \"Inter\"");
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    }

    #[test]
    fn a_sidebar_and_section_save_keep_their_inline_formatting() {
        let temp = TempDir::new("save-inline");
        let home = temp.path();
        let original = r#"[sidebars]
left = { collapsed = false, width = 348 }    # left
right = { collapsed = false, width = 648 }   # right

[sections]
recent = "expanded"
devices = "expanded"    # devices
remote = "expanded"
"#;
        let path = temp.write("config.toml", original);

        let (mut file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert!(problems.is_empty(), "{problems:#?}");

        file.apply(Change::SetSidebarWidth {
            side: Side::Left,
            width: 400,
        })
        .unwrap();
        let expected = original.replace("width = 348", "width = 400");
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);

        file.apply(Change::SetSection {
            section: Section::Devices,
            state: SectionState::Collapsed,
        })
        .unwrap();
        let expected = expected.replace("devices = \"expanded\"", "devices = \"collapsed\"");
        assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    }

    #[test]
    fn an_unparsable_file_is_never_written() {
        let temp = TempDir::new("no-write");
        let home = temp.path();
        let original = "[look\nscale = 0.57\n";
        let path = temp.write("config.toml", original);

        let (mut file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert_eq!(problems.len(), 1);
        assert!(!file.is_writable());

        let error = file.apply(Change::SetScale(0.9)).unwrap_err();
        assert!(matches!(error, ApplyError::Unparsable { .. }));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        assert_eq!(file.config().look.scale, 0.9);
    }

    #[test]
    fn saving_is_atomic_and_keeps_permissions() {
        let temp = TempDir::new("atomic");
        let home = temp.path();
        let path = temp.write("config.toml", "[look]\nscale = 1.0\n");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        let (mut file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert!(problems.is_empty());
        file.apply(Change::SetScale(0.9)).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        assert_eq!(fs::read_to_string(&path).unwrap(), "[look]\nscale = 0.9\n");

        let leftovers: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".config.toml.tmp-")
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    #[test]
    fn saving_creates_the_folder_and_the_changed_key() {
        let temp = TempDir::new("create");
        let home = temp.path();
        let path = home.join("nested").join("config.toml");

        let (mut file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert!(problems.is_empty());

        file.apply(Change::SetOpacity(0.5)).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[look]\nopacity = 0.5\n"
        );
    }

    #[test]
    fn pin_changes_write_the_pins_list() {
        let temp = TempDir::new("pins-save");
        let home = temp.path();
        let path = temp.write(
            "config.toml",
            "[pins]\nfolders = [\"~\", \"~/Downloads\"]  # keep me\n",
        );
        let (mut file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert!(problems.is_empty());

        file.apply(Change::AddPin(home.join("Documents"))).unwrap();
        file.apply(Change::MovePin { from: 0, to: 2 }).unwrap();
        file.apply(Change::RemovePin(home.join("Downloads")))
            .unwrap();

        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("keep me"));
        assert!(saved.contains("Documents"));
        assert!(!saved.contains("Downloads"));
        assert_eq!(
            file.config().pins,
            vec![
                normalize_path(&home.join("Documents")),
                normalize_path(home),
            ]
        );
    }

    #[test]
    fn pin_changes_without_a_pin_list_keep_the_defaults() {
        // P2i found this: with no [pins] table, AddPin wrote only the new pin,
        // so the defaults were lost on the next load and a MovePin was ignored.
        let temp = TempDir::new("pins-default");
        let home = temp.path();
        let path = temp.write("config.toml", "[look]\nscale = 1.0 # keep\n");
        let (mut file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert!(problems.is_empty());
        let defaults = file.config().pins.clone();
        assert!(!defaults.is_empty());

        let added = PathBuf::from("/tmp/gimbal-p2c-extra-pin");
        file.apply(Change::AddPin(added.clone())).unwrap();
        let mut expected = defaults.clone();
        expected.push(added.clone());
        assert_eq!(file.config().pins, expected);
        let (reloaded, _) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert_eq!(reloaded.config().pins, expected);

        let last = expected.len() - 1;
        file.apply(Change::MovePin { from: last, to: 0 }).unwrap();
        let moved = expected.remove(last);
        expected.insert(0, moved);
        assert_eq!(file.config().pins, expected);
        let (reloaded, _) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert_eq!(reloaded.config().pins, expected);
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("scale = 1.0 # keep")
        );
    }

    #[test]
    fn sort_changes_write_one_global_setting_or_one_folder_rule() {
        let temp = TempDir::new("sort-save");
        let home = temp.path();
        let path = temp.write(
            "config.toml",
            r#"[sort]
by = "name"       # keep
order = "ascending"
folders-first = true
"#,
        );
        let (mut file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert!(problems.is_empty());

        file.apply(Change::SetSort {
            folder: None,
            by: SortBy::Size,
            order: SortOrder::Descending,
        })
        .unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("by = \"size\"       # keep"));
        assert!(saved.contains("order = \"descending\""));
        assert!(saved.contains("folders-first = true"));

        let folder = home.join("Downloads");
        file.apply(Change::SetSort {
            folder: Some(folder.clone()),
            by: SortBy::Modified,
            order: SortOrder::Descending,
        })
        .unwrap();
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("[[sort.folder]]"));
        assert!(saved.contains(&format!("path = \"{}\"", folder.display())));
        assert!(saved.contains("by = \"modified\""));
        // The folder rule must not touch the global sort: reload and check both.
        assert!(saved.contains("by = \"size\"       # keep"));
        let (reloaded, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());
        assert!(problems.is_empty());
        let global = reloaded.config().sort.global();
        assert_eq!(
            (global.by, global.order),
            (SortBy::Size, SortOrder::Descending)
        );
        let rule = reloaded.config().sort_for(&folder);
        assert_eq!(
            (rule.by, rule.order),
            (SortBy::Modified, SortOrder::Descending)
        );
    }

    // ---------------------------------------------------------------------
    // Commands
    // ---------------------------------------------------------------------

    #[test]
    fn every_settings_command_has_a_change_for() {
        let mut config = default_config(Path::new("/home/kyle"));
        config.look.scale = 0.9;
        let current_folder = Path::new("/home/kyle/Documents");
        config.pins = vec![PathBuf::from("/home/kyle"), PathBuf::from("/tmp")];

        let samples: &[(&str, &[&str])] = &[
            ("view.set-scale", &["view.set-scale", "scale=0.95"]),
            ("view.zoom-in", &["view.zoom-in"]),
            ("view.zoom-out", &["view.zoom-out"]),
            ("view.set-opacity", &["view.set-opacity", "opacity=0.8"]),
            ("view.toggle-sidebar", &["view.toggle-sidebar", "side=left"]),
            (
                "view.toggle-section",
                &["view.toggle-section", "section=devices"],
            ),
            (
                "view.set-sort",
                &["view.set-sort", "by=name", "order=ascending"],
            ),
            ("pins.add", &["pins.add", "path=/tmp"]),
            ("pins.remove", &["pins.remove", "path=/tmp"]),
            ("pins.move", &["pins.move", "from=0", "to=1"]),
        ];

        let settings: Vec<&str> = Registry::new()
            .commands()
            .iter()
            .filter(|spec| spec.effect == crate::command::Effect::Settings)
            .map(|spec| spec.name)
            .collect();
        assert_eq!(settings.len(), samples.len());

        for command_name in settings {
            let Some((_, tokens)) = samples.iter().find(|(name, _)| *name == command_name) else {
                panic!("no sample invocation for Settings command `{command_name}`");
            };
            let command = check(command_name, tokens);
            assert!(
                change_for(&command, &config, current_folder).is_some(),
                "Settings command `{command_name}` has no change"
            );
        }
    }

    #[test]
    fn zoom_stops_at_the_scale_limits_and_toggles_flip() {
        let mut config = default_config(Path::new("/home/kyle"));
        let current_folder = Path::new("/home/kyle");
        config.look.scale = 0.95;
        let zoom_in = check("view.zoom-in", &["view.zoom-in"]);
        assert_eq!(
            change_for(&zoom_in, &config, current_folder),
            Some(Change::SetScale(1.0))
        );

        config.look.scale = 1.0;
        assert_eq!(change_for(&zoom_in, &config, current_folder), None);

        config.look.scale = 0.5;
        let zoom_out = check("view.zoom-out", &["view.zoom-out"]);
        assert_eq!(change_for(&zoom_out, &config, current_folder), None);

        config.sections.devices = SectionState::Expanded;
        let toggle = check(
            "view.toggle-section",
            &["view.toggle-section", "section=devices"],
        );
        assert_eq!(
            change_for(&toggle, &config, current_folder),
            Some(Change::SetSection {
                section: Section::Devices,
                state: SectionState::Collapsed
            })
        );
    }

    #[test]
    fn set_scale_and_sort_map_to_the_expected_changes() {
        let config = default_config(Path::new("/home/kyle"));
        let current_folder = Path::new("/home/kyle/Documents");

        let scale = check("view.set-scale", &["view.set-scale", "scale=0.85"]);
        assert_eq!(
            change_for(&scale, &config, current_folder),
            Some(Change::SetScale(0.85))
        );

        let sort = check(
            "view.set-sort",
            &[
                "view.set-sort",
                "by=size",
                "order=descending",
                "scope=global",
            ],
        );
        assert_eq!(
            change_for(&sort, &config, current_folder),
            Some(Change::SetSort {
                folder: None,
                by: SortBy::Size,
                order: SortOrder::Descending
            })
        );
    }

    #[test]
    fn commands_that_need_the_current_folder_use_the_argument() {
        let config = default_config(Path::new("/home/kyle"));
        let current_folder = Path::new("/tmp/gimbal-p2e-current");

        let sort = check(
            "view.set-sort",
            &["view.set-sort", "by=modified", "order=descending"],
        );
        assert_eq!(
            change_for(&sort, &config, current_folder),
            Some(Change::SetSort {
                folder: Some(current_folder.to_path_buf()),
                by: SortBy::Modified,
                order: SortOrder::Descending
            })
        );

        let add = check("pins.add", &["pins.add"]);
        assert_eq!(
            change_for(&add, &config, current_folder),
            Some(Change::AddPin(current_folder.to_path_buf()))
        );

        let explicit = check("pins.add", &["pins.add", "path=/tmp/explicit"]);
        assert_eq!(
            change_for(&explicit, &config, current_folder),
            Some(Change::AddPin(PathBuf::from("/tmp/explicit")))
        );
    }

    #[test]
    fn prints_a_sample_problem_list() {
        let temp = TempDir::new("sample-problems");
        let home = temp.path();
        let path = temp.write(
            "config.toml",
            r#"[look]
scale = 0.57       # off the step
base = 3.0         # too large
font = "Inter"     # good

[sidebars]
left = { collapsed = false, width = 250 }

[hotkeys]
"<Control>t" = "no.such-command"

[surprise]
answer = 42
"#,
        );
        let (_file, problems) = ConfigFile::load_with(&path, home, &SpecialFolders::default());

        println!("sample problems:");
        for problem in &problems {
            println!("{problem}");
        }

        assert_eq!(problems.len(), 5);
    }
}
