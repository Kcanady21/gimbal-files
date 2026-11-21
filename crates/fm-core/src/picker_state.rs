// SPDX-License-Identifier: GPL-3.0-or-later
//! The file chooser's own state: the last folder per application and the one
//! remembered dialog size.
//!
//! This is deliberately a separate file from the session and the config. The
//! main app never reads or writes it, and the picker writes no other state:
//! `$XDG_STATE_HOME/gimbal-files/picker-state.toml` (P2au). A dialog that is
//! cancelled writes no folder; the size is written whenever a dialog closes.
//!
//! Everything here is GTK-free and pure enough to test on scratch folders. The
//! file is a small TOML document:
//!
//! ```toml
//! [window]
//! width = 1100
//! height = 700
//!
//! [[folders]]
//! app_id = "org.mozilla.firefox"
//! folder = "/home/kyle/Downloads"
//! last_used = 1763660000
//! ```
//!
//! An application id is a D-Bus name, so the empty one (an unsandboxed app that
//! did not say) is stored under [`NO_APP_ID`], a key no real id can spell.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use toml_edit::{ArrayOfTables, Document, DocumentMut, Item, Table, Value};

/// The file under `$XDG_STATE_HOME/gimbal-files/`.
pub const STATE_FILE: &str = "picker-state.toml";
/// The most applications the folder table keeps. The least recently used one
/// is dropped when a new one would make it 201.
pub const MAX_APPS: usize = 200;
/// The key for a call whose `app_id` is empty. It contains characters a D-Bus
/// name may not, so a real application can never collide with it.
pub const NO_APP_ID: &str = "(no app id)";

/// One remembered folder and when it was last used, in Unix seconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RememberedFolder {
    pub folder: PathBuf,
    pub last_used: u64,
}

/// The picker's state file. The maps are private so every write goes through
/// the trimming rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickerState {
    folders: BTreeMap<String, RememberedFolder>,
    size: Option<(i32, i32)>,
}

impl PickerState {
    /// The folder table, keyed by app id (with [`NO_APP_ID`] for the empty one).
    pub fn folders(&self) -> &BTreeMap<String, RememberedFolder> {
        &self.folders
    }

    /// The remembered folder for `app_id`, whether or not it still exists. The
    /// caller checks `is_dir`: a folder that went away falls through to home.
    pub fn folder_for(&self, app_id: &str) -> Option<&Path> {
        self.folders
            .get(key_for(app_id))
            .map(|entry| entry.folder.as_path())
    }

    /// The one shared dialog size, if one has been remembered.
    pub fn size(&self) -> Option<(i32, i32)> {
        self.size
    }

    /// Remember `folder` for `app_id`. A relative or non-UTF-8 path is refused
    /// rather than mangled or written as lossy text; the dialog still works and
    /// simply has nothing to remember. Returns whether the folder was stored.
    pub fn set_folder(&mut self, app_id: &str, folder: &Path, last_used: u64) -> bool {
        if !folder.is_absolute() {
            return false;
        }
        let Some(folder) = folder.to_str() else {
            return false;
        };
        let key = key_for(app_id).to_string();
        if !self.folders.contains_key(&key) && self.folders.len() >= MAX_APPS {
            self.evict_least_recently_used();
        }
        self.folders.insert(
            key,
            RememberedFolder {
                folder: PathBuf::from(folder),
                last_used,
            },
        );
        true
    }

    /// Remember the shared dialog size. A zero or negative side is ignored.
    pub fn set_size(&mut self, width: i32, height: i32) {
        if width > 0 && height > 0 {
            self.size = Some((width, height));
        }
    }

    fn evict_least_recently_used(&mut self) {
        let victim = self
            .folders
            .iter()
            .min_by_key(|(app_id, entry)| (entry.last_used, app_id.as_str()))
            .map(|(app_id, _)| app_id.clone());
        if let Some(victim) = victim {
            self.folders.remove(&victim);
        }
    }

    /// Keep at most [`MAX_APPS`] entries. A file written by an older build, or
    /// edited by hand, may hold more; the excess goes on the next load.
    fn trim(&mut self) {
        while self.folders.len() > MAX_APPS {
            self.evict_least_recently_used();
        }
    }

    /// Read a state document. A syntax error is an error; a single entry with a
    /// missing or wrong-typed field is skipped, so one bad hand edit does not
    /// throw away every other application's folder.
    pub fn parse(text: &str) -> Result<Self, String> {
        let document = Document::parse(text).map_err(|error| error.to_string())?;
        let root = document.as_table();
        let mut state = Self::default();

        if let Some(item) = root.get("folders") {
            let tables = item
                .as_array_of_tables()
                .ok_or_else(|| "`folders` is not an array of tables".to_string())?;
            for table in tables.iter() {
                let Some(app_id) = table.get("app_id").and_then(Item::as_str) else {
                    continue;
                };
                let Some(folder) = table.get("folder").and_then(Item::as_str) else {
                    continue;
                };
                let Some(last_used) = table
                    .get("last_used")
                    .and_then(Item::as_integer)
                    .and_then(|value| u64::try_from(value).ok())
                else {
                    continue;
                };
                let path = Path::new(folder);
                if !path.is_absolute() {
                    continue;
                }
                state.folders.insert(
                    key_for(app_id).to_string(),
                    RememberedFolder {
                        folder: path.to_path_buf(),
                        last_used,
                    },
                );
            }
        }

        if let Some(window) = root.get("window") {
            let window = window
                .as_table()
                .ok_or_else(|| "`window` is not a table".to_string())?;
            let width = window
                .get("width")
                .and_then(Item::as_integer)
                .and_then(|value| i32::try_from(value).ok());
            let height = window
                .get("height")
                .and_then(Item::as_integer)
                .and_then(|value| i32::try_from(value).ok());
            if let (Some(width), Some(height)) = (width, height) {
                state.set_size(width, height);
            }
        }

        state.trim();
        Ok(state)
    }

    /// The TOML document this state writes.
    pub fn render(&self) -> String {
        let mut document = DocumentMut::new();
        let root = document.as_table_mut();

        let mut folders = ArrayOfTables::new();
        for (app_id, entry) in &self.folders {
            let mut table = Table::new();
            table.insert("app_id", Item::Value(Value::from(app_id.as_str())));
            // Only UTF-8 paths ever reach the map (`set_folder` refuses the
            // rest), so this cannot mangle a name.
            table.insert(
                "folder",
                Item::Value(Value::from(entry.folder.to_string_lossy().into_owned())),
            );
            table.insert(
                "last_used",
                Item::Value(Value::from(
                    i64::try_from(entry.last_used).unwrap_or(i64::MAX),
                )),
            );
            folders.push(table);
        }
        root.insert("folders", Item::ArrayOfTables(folders));

        if let Some((width, height)) = self.size {
            let mut window = Table::new();
            window.insert("width", Item::Value(Value::from(i64::from(width))));
            window.insert("height", Item::Value(Value::from(i64::from(height))));
            root.insert("window", Item::Table(window));
        }

        format!(
            "# SPDX-License-Identifier: GPL-3.0-or-later\n\
             # Gimbal Files picker state: the last folder per app id and the dialog size.\n\
             # The main app never reads or writes this file.\n\n{document}"
        )
    }

    /// Read the state file. A missing file is an empty state, not a problem. A
    /// file that cannot be read or parsed yields the empty state and one message
    /// for the picker's log; the next write replaces it.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!("cannot read {}: {error}", path.display())),
                );
            }
        };
        match Self::parse(&text) {
            Ok(state) => (state, None),
            Err(error) => (
                Self::default(),
                Some(format!(
                    "ignoring {} because it does not parse (it will be replaced): {error}",
                    path.display()
                )),
            ),
        }
    }

    /// Write the whole state atomically: a temp file in the same folder, then a
    /// rename. The folder is created when missing.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        crate::fsutil::save_atomic(path, &self.render())
    }
}

/// The key an app id is stored under. The empty id shares [`NO_APP_ID`].
pub fn key_for(app_id: &str) -> &str {
    if app_id.is_empty() { NO_APP_ID } else { app_id }
}

/// `$XDG_STATE_HOME/gimbal-files/picker-state.toml`, resolved the way the logs
/// resolve their folder. `None` when neither `XDG_STATE_HOME` nor `HOME` is set.
pub fn default_path() -> Option<PathBuf> {
    crate::logfile::default_state_dir().map(|state| state.join(STATE_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "gimbal-picker-state-{tag}-{}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|time| time.as_nanos())
                    .unwrap_or(0)
            ));
            fs::create_dir_all(&path).expect("a scratch folder");
            Self(path)
        }

        fn state(&self) -> PathBuf {
            self.0.join(STATE_FILE)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_state_round_trips_with_folders_and_the_window_size() {
        let scratch = Scratch::new("round-trip");
        let path = scratch.state();
        let mut state = PickerState::default();
        assert!(state.set_folder("org.example.App", Path::new("/tmp/a"), 10));
        assert!(state.set_folder("org.example.Other", Path::new("/tmp/b"), 20));
        state.set_size(1200, 800);
        state.save(&path).expect("save");

        let text = fs::read_to_string(&path).expect("the state file");
        assert!(text.starts_with("# SPDX-License-Identifier: GPL-3.0-or-later"));
        assert!(text.contains("app_id = \"org.example.App\""));
        assert!(text.contains("width = 1200"));

        let (loaded, problem) = PickerState::load(&path);
        assert_eq!(problem, None);
        assert_eq!(loaded, state);
        assert_eq!(
            loaded.folder_for("org.example.App"),
            Some(Path::new("/tmp/a"))
        );
        assert_eq!(loaded.size(), Some((1200, 800)));
    }

    #[test]
    fn a_missing_file_is_an_empty_state_and_a_bad_one_is_one_message() {
        let scratch = Scratch::new("missing");
        let (state, problem) = PickerState::load(&scratch.state());
        assert_eq!(state, PickerState::default());
        assert_eq!(problem, None);

        fs::write(scratch.state(), "this is not [ toml").expect("a broken file");
        let (state, problem) = PickerState::load(&scratch.state());
        assert_eq!(state, PickerState::default());
        let problem = problem.expect("a parse problem");
        assert!(problem.contains("does not parse"), "{problem}");
        assert!(problem.contains("replaced"), "{problem}");
    }

    #[test]
    fn a_broken_file_is_replaced_on_the_next_write() {
        let scratch = Scratch::new("replace");
        let path = scratch.state();
        fs::write(&path, "[[folders]]\napp_id = ").expect("a broken file");
        assert!(PickerState::load(&path).1.is_some());

        let mut state = PickerState::default();
        assert!(state.set_folder("a.b", Path::new("/tmp/kept"), 1));
        state.save(&path).expect("replace");
        let (loaded, problem) = PickerState::load(&path);
        assert_eq!(problem, None);
        assert_eq!(loaded.folder_for("a.b"), Some(Path::new("/tmp/kept")));
    }

    #[test]
    fn the_least_recently_used_app_is_dropped_at_two_hundred_and_one() {
        let mut state = PickerState::default();
        for index in 0..MAX_APPS {
            assert!(state.set_folder(&format!("app.{index}"), Path::new("/tmp"), index as u64));
        }
        assert_eq!(state.folders().len(), MAX_APPS);
        assert!(state.folder_for("app.0").is_some());

        // Updating an existing entry at the limit does not evict anyone.
        assert!(state.set_folder("app.100", Path::new("/tmp/new"), 999));
        assert_eq!(state.folders().len(), MAX_APPS);
        assert!(state.folder_for("app.0").is_some());

        // One new app makes room by dropping the oldest.
        assert!(state.set_folder("app.new", Path::new("/tmp/newest"), 1000));
        assert_eq!(state.folders().len(), MAX_APPS);
        assert_eq!(state.folder_for("app.0"), None);
        assert_eq!(state.folder_for("app.1"), Some(Path::new("/tmp")));
        assert_eq!(state.folder_for("app.new"), Some(Path::new("/tmp/newest")));
    }

    #[test]
    fn the_empty_app_id_has_one_slot_under_a_key_no_real_id_can_use() {
        let mut state = PickerState::default();
        assert!(state.set_folder("", Path::new("/tmp/first"), 1));
        assert!(state.set_folder("", Path::new("/tmp/second"), 2));
        // Two different unsandboxed apps share the one empty slot.
        assert_eq!(state.folder_for(""), Some(Path::new("/tmp/second")));
        assert_eq!(state.folders().len(), 1);
        assert!(state.folders().contains_key(NO_APP_ID));

        // A real id is a separate key, even one that looks similar.
        assert!(state.set_folder("(no.app.id)", Path::new("/tmp/real"), 3));
        assert_eq!(state.folders().len(), 2);
        assert_eq!(state.folder_for(""), Some(Path::new("/tmp/second")));
        assert_eq!(
            state.folder_for("(no.app.id)"),
            Some(Path::new("/tmp/real"))
        );

        // The marker is what goes to disk, and comes back as the empty slot.
        let scratch = Scratch::new("empty-id");
        state.save(&scratch.state()).expect("save");
        let text = fs::read_to_string(scratch.state()).expect("the file");
        assert!(text.contains("app_id = \"(no app id)\""));
        let (loaded, problem) = PickerState::load(&scratch.state());
        assert_eq!(problem, None);
        assert_eq!(loaded.folder_for(""), Some(Path::new("/tmp/second")));
    }

    #[test]
    fn a_non_utf8_or_relative_folder_is_skipped_not_mangled() {
        let mut state = PickerState::default();
        // A folder name no UTF-8 string can spell.
        let bytes = b"/tmp/bad-\xff\xfe".to_vec();
        let non_utf8 = PathBuf::from(OsString::from_vec(bytes));
        assert!(!state.set_folder("a.b", &non_utf8, 1));
        assert!(state.folders().is_empty());

        assert!(!state.set_folder("a.b", Path::new("relative/folder"), 1));
        assert!(state.folders().is_empty());

        assert!(state.set_folder("a.b", Path::new("/tmp/good"), 1));
        assert_eq!(state.folder_for("a.b"), Some(Path::new("/tmp/good")));
    }

    #[test]
    fn a_failed_write_leaves_no_partial_file() {
        let scratch = Scratch::new("failed-write");
        let path = scratch.state();
        // A directory where the state file belongs makes the final rename fail.
        fs::create_dir(&path).expect("a directory in the file's place");
        let state = PickerState::default();
        assert!(state.save(&path).is_err());
        assert!(path.is_dir(), "the target was not replaced");
        let leftovers: Vec<_> = fs::read_dir(&scratch.0)
            .expect("scratch")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!(".{STATE_FILE}.tmp-"))
            })
            .collect();
        assert!(leftovers.is_empty(), "a partial temp file was left behind");
    }

    #[test]
    fn a_failed_write_keeps_the_previous_file_intact() {
        let scratch = Scratch::new("keep-old");
        let path = scratch.state();
        let mut state = PickerState::default();
        assert!(state.set_folder("a.b", Path::new("/tmp/old"), 1));
        state.save(&path).expect("the first save");
        let before = fs::read_to_string(&path).expect("the first file");

        // Make the folder read-only. If this process can still create files
        // there (for example, running as root), the permissions prove nothing
        // and the test has nothing to say.
        let permissions = fs::metadata(&scratch.0).expect("metadata").permissions();
        let mut readonly = permissions.clone();
        use std::os::unix::fs::PermissionsExt;
        readonly.set_mode(0o500);
        fs::set_permissions(&scratch.0, readonly).expect("read-only folder");
        let probe = scratch.0.join("probe");
        if fs::write(&probe, b"").is_ok() {
            let _ = fs::remove_file(&probe);
            fs::set_permissions(&scratch.0, permissions).expect("restore");
            return;
        }

        let mut changed = PickerState::default();
        assert!(changed.set_folder("a.b", Path::new("/tmp/new"), 2));
        let result = changed.save(&path);
        let after = fs::read_to_string(&path).expect("the old file");
        fs::set_permissions(&scratch.0, permissions).expect("restore");
        assert!(result.is_err());
        assert_eq!(
            after, before,
            "a failed save must not truncate the previous state"
        );
    }

    #[test]
    fn one_bad_entry_does_not_throw_away_the_others() {
        let text = "\
[[folders]]
app_id = \"a.b\"
folder = \"/tmp/kept\"
last_used = 3

[[folders]]
app_id = \"c.d\"
folder = \"relative\"
last_used = 4

[[folders]]
app_id = \"e.f\"
last_used = 5

[[folders]]
app_id = \"g.h\"
folder = \"/tmp/also\"
last_used = \"yesterday\"
";
        let state = PickerState::parse(text).expect("the document parses");
        assert_eq!(state.folders().len(), 1);
        assert_eq!(state.folder_for("a.b"), Some(Path::new("/tmp/kept")));
    }

    #[test]
    fn a_wrong_section_type_is_a_parse_error_so_the_file_is_replaced() {
        assert!(PickerState::parse("folders = 3").is_err());
        assert!(PickerState::parse("window = 3").is_err());
    }
}
