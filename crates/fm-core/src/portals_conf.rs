// SPDX-License-Identifier: GPL-3.0-or-later
//! The user's `portals.conf`: reading the FileChooser chain, adding Gimbal to
//! it and taking Gimbal back out.
//!
//! `xdg-desktop-portal` 1.21 reads every configuration directory in
//! precedence order and asks each one for the interface. Within one directory
//! the desktop-specific `<desktop>-portals.conf` wins over `portals.conf`.
//! This module writes a desktop-specific user file, copying the first
//! configuration that is in effect so no other interface setting is lost.
//!
//! Nothing here touches the real environment: every function takes the
//! folders and the desktop list. [`Locations::from_environment`] is the one
//! exception, for the Settings switch.

use std::cell::Cell;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use crate::fsutil::save_atomic;

/// The interface key whose backend chain decides the Open and Save dialogs.
pub const FILE_CHOOSER_KEY: &str = "org.freedesktop.impl.portal.FileChooser";
/// The key that applies to every interface not named explicitly.
pub const DEFAULT_KEY: &str = "default";
/// The backend name Gimbal installs as.
pub const GIMBAL: &str = "gimbal";
/// The backend the frontend falls back to when Gimbal is gone.
pub const GTK: &str = "gtk";

const PREFERRED: &str = "preferred";
const COPY_HEADER_PREFIX: &str = "# Copied from ";
const COPY_HEADER_SUFFIX: &str =
    " by Gimbal Files when \"Use Gimbal for Open and Save dialogs\" was turned on.";
const CREATED_HEADER: &str = "# Created by Gimbal Files when \"Use Gimbal for Open and Save \
dialogs\" was turned on; there was no portals.conf to copy.";
const MARKER_PREFIX: &str = "# Gimbal Files: was ";
const MARKER_NONE: &str = "none";

/// A refusal, always shown by the Settings switch. Nothing is guessed and
/// nothing is overwritten when one is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    message: String,
}

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// The folders the frontend searches, in precedence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locations {
    /// `$XDG_CONFIG_HOME` (the per-user configuration directory).
    pub config_home: PathBuf,
    /// `$XDG_CONFIG_DIRS`, the build-time `sysconfdir`, and
    /// `$XDG_DATA_HOME`, in that order.
    pub config_dirs: Vec<PathBuf>,
    /// `$XDG_DATA_DIRS` and the build-time datadir, in that order.
    pub data_dirs: Vec<PathBuf>,
}

impl Locations {
    /// The real folders, resolved the way `xdg-desktop-portal` resolves them.
    ///
    /// GLib supplies the environment defaults. The build-time `/etc/xdg` and
    /// `/usr/share` are appended to the same lists the frontend uses; a path
    /// already present is not repeated.
    pub fn from_environment() -> Self {
        let config_home = glib::user_config_dir();
        let mut config_dirs = glib::system_config_dirs();
        push_unique(&mut config_dirs, PathBuf::from("/etc/xdg"));
        push_unique(&mut config_dirs, glib::user_data_dir());
        let mut data_dirs = glib::system_data_dirs();
        push_unique(&mut data_dirs, PathBuf::from("/usr/share"));
        Self {
            config_home,
            config_dirs,
            data_dirs,
        }
    }
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

/// `XDG_CURRENT_DESKTOP`, split and lower-cased, with invalid names dropped.
/// The frontend accepts only ASCII alphanumerics, `-` and `_`.
pub fn current_desktops() -> Vec<String> {
    let value = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    desktops_from(&value)
}

fn desktops_from(value: &str) -> Vec<String> {
    value
        .split(':')
        .filter(|desktop| {
            !desktop.is_empty()
                && desktop
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .map(|desktop| desktop.to_ascii_lowercase())
        .collect()
}

/// The configuration file the frontend uses for the first interface it can
/// resolve: for each desktop, each root's `<desktop>-portals.conf`, then each
/// root's `portals.conf`. A file without a `[preferred]` group is skipped,
/// exactly as the frontend skips it.
pub fn effective_file(
    desktops: &[String],
    config_home: &Path,
    config_dirs: &[PathBuf],
    data_dirs: &[PathBuf],
) -> Option<PathBuf> {
    let mut roots = vec![config_home];
    roots.extend(config_dirs.iter().map(PathBuf::as_path));
    roots.extend(data_dirs.iter().map(PathBuf::as_path));

    for desktop in desktops {
        for root in &roots {
            let path = root
                .join("xdg-desktop-portal")
                .join(format!("{desktop}-portals.conf"));
            if reads_as_portals_conf(&path) {
                return Some(path);
            }
        }
    }
    for root in roots {
        let path = root.join("xdg-desktop-portal").join("portals.conf");
        if reads_as_portals_conf(&path) {
            return Some(path);
        }
    }
    None
}

/// Every configuration file the frontend would load, in precedence order.
/// Unlike [`effective_file`], this keeps the files from lower directories:
/// 1.21 layers them per interface instead of using only the first.
pub fn config_files(
    desktops: &[String],
    config_home: &Path,
    config_dirs: &[PathBuf],
    data_dirs: &[PathBuf],
) -> Vec<PathBuf> {
    let mut roots = vec![config_home];
    roots.extend(config_dirs.iter().map(PathBuf::as_path));
    roots.extend(data_dirs.iter().map(PathBuf::as_path));

    let mut files = Vec::new();
    for root in roots {
        let mut chosen = None;
        for desktop in desktops {
            let path = root
                .join("xdg-desktop-portal")
                .join(format!("{desktop}-portals.conf"));
            if reads_as_portals_conf(&path) {
                chosen = Some(path);
                break;
            }
        }
        let path = chosen.or_else(|| {
            let path = root.join("xdg-desktop-portal").join("portals.conf");
            reads_as_portals_conf(&path).then_some(path)
        });
        if let Some(path) = path {
            files.push(path);
        }
    }
    files
}

fn reads_as_portals_conf(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    Document::parse(&text).is_ok_and(|document| document.has_preferred())
}

/// The FileChooser chain a single file names: its own interface line, else
/// `default`. An empty vector means the file names neither.
pub fn file_chooser_chain(file: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(file) else {
        return Vec::new();
    };
    let Ok(document) = Document::parse(&text) else {
        return Vec::new();
    };
    document
        .value(FILE_CHOOSER_KEY)
        .or_else(|| document.value(DEFAULT_KEY))
        .map(|value| split_list(&value))
        .unwrap_or_default()
}

/// The chain the frontend resolves first: the first configuration file whose
/// chain names an installed backend (or `*`), or `none`. Lower files apply
/// when a higher one has no opinion about FileChooser.
pub fn effective_chain(files: &[PathBuf], installed: &[String]) -> Vec<String> {
    for file in files {
        let chain = file_chooser_chain(file);
        if chain.is_empty() {
            continue;
        }
        if chain.iter().any(|name| name == "none") {
            return chain;
        }
        if chain
            .iter()
            .any(|name| name == "*" || installed.iter().any(|backend| backend.as_str() == name))
        {
            return chain;
        }
    }
    Vec::new()
}

/// Whether Gimbal is the first backend the chain would use.
pub fn gimbal_is_first(chain: &[String]) -> bool {
    chain.first().is_some_and(|name| name == GIMBAL)
}

/// Whether this chain names Gimbal anywhere.
fn names_gimbal(chain: &[String]) -> bool {
    chain.iter().any(|name| name == GIMBAL)
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// What a change did, for the Settings row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// False when the file already said what was asked, so nothing was
    /// written and the service was not restarted.
    pub changed: bool,
    /// The restart failed. The file is written and survives; the session
    /// needs a sign-out to pick it up.
    pub restart_error: Option<String>,
}

/// Something that can restart `xdg-desktop-portal.service`. Production calls
/// systemd on the session bus; tests count the calls.
pub trait Restarter {
    fn restart(&self) -> Result<(), String>;
}

thread_local! {
    /// Set by a failed restart so every later Settings row keeps saying the
    /// change waits for a sign-out. Cleared by the next restart that works.
    /// Thread-local so tests on separate threads cannot see each other; the
    /// app's windows all run on the main thread.
    static RESTART_FAILED: Cell<bool> = const { Cell::new(false) };
}

/// Whether a saved change is waiting for a sign-out.
pub fn restart_pending() -> bool {
    RESTART_FAILED.with(Cell::get)
}

fn set_restart_pending(value: bool) {
    RESTART_FAILED.with(|pending| pending.set(value));
}

#[cfg(test)]
fn clear_restart_pending() {
    set_restart_pending(false);
}

/// The Settings row's state, kept as a pure value so its text is testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    NotInstalled,
    PendingRestart { enabled: bool },
    Gimbal,
    Backend(String),
    None,
}

/// What the switch should say. `installed` covers both the `.portal` file and
/// the bus's activatable name; `available` is every installed backend's name.
pub fn health(
    installed: bool,
    chain: &[String],
    available: &[String],
    pending_restart: bool,
) -> Health {
    if !installed {
        return Health::NotInstalled;
    }
    if pending_restart {
        return Health::PendingRestart {
            enabled: gimbal_is_first(chain),
        };
    }
    if gimbal_is_first(chain) {
        return Health::Gimbal;
    }
    if chain.first().is_some_and(|name| name == "none") {
        return Health::None;
    }
    if let Some(name) = chain
        .iter()
        .find(|name| available.iter().any(|backend| backend == *name))
    {
        return Health::Backend(name.clone());
    }
    if available.iter().any(|name| name == GTK) {
        return Health::Backend(GTK.to_string());
    }
    Health::None
}

/// Whether the switch shows as on.
pub fn health_is_enabled(health: &Health) -> bool {
    match health {
        Health::Gimbal => true,
        Health::PendingRestart { enabled } => *enabled,
        Health::NotInstalled | Health::Backend(_) | Health::None => false,
    }
}

/// The exact text under the Settings switch.
pub fn status_text(health: &Health) -> String {
    match health {
        Health::NotInstalled => "Not installed: the portal backend files are missing".to_string(),
        Health::PendingRestart { .. } => "Saved; sign out and back in to apply".to_string(),
        Health::Gimbal => "Open and Save dialogs: Gimbal".to_string(),
        Health::Backend(name) => {
            format!("Open and Save dialogs: {}", display_backend(name))
        }
        Health::None => "Open and Save dialogs: none".to_string(),
    }
}

fn display_backend(name: &str) -> String {
    match name {
        "gtk" => "GTK".to_string(),
        "gnome" => "GNOME".to_string(),
        "kde" => "KDE".to_string(),
        other => other.to_string(),
    }
}

/// The folders `xdg-desktop-portal` reads `.portal` files from:
/// `$XDG_DATA_HOME`, each `$XDG_DATA_DIRS`, then `/usr/share`. 1.21.1 reads
/// all of them; `XDG_DESKTOP_PORTAL_DIR` replaces the list.
pub fn portal_dirs() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DESKTOP_PORTAL_DIR") {
        return vec![PathBuf::from(dir)];
    }
    let mut dirs = vec![glib::user_data_dir()];
    dirs.extend(glib::system_data_dirs());
    push_unique(&mut dirs, PathBuf::from("/usr/share"));
    dirs.into_iter()
        .map(|root| root.join("xdg-desktop-portal").join("portals"))
        .collect()
}

/// The source names of every installed `.portal` file.
pub fn installed_backends(dirs: &[PathBuf]) -> Vec<String> {
    let mut names = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(source) = name.strip_suffix(".portal")
                && !source.is_empty()
                && !names.iter().any(|existing| existing == source)
            {
                names.push(source.to_string());
            }
        }
    }
    names.sort();
    names
}

/// Whether Gimbal's `.portal` file is in a folder the frontend reads.
pub fn gimbal_portal_installed(dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|dir| dir.join("gimbal.portal").is_file())
}

/// The file Gimbal writes for this session.
pub fn target_path(desktops: &[String], config_home: &Path) -> PathBuf {
    let name = match desktops.first() {
        Some(desktop) if !desktop.is_empty() => {
            format!("{}-portals.conf", desktop.to_ascii_lowercase())
        }
        _ => "portals.conf".to_string(),
    };
    config_home.join("xdg-desktop-portal").join(name)
}

/// Turn the switch on: write the marked FileChooser line, then restart.
pub fn enable(
    desktops: &[String],
    locations: &Locations,
    restarter: &dyn Restarter,
) -> Result<Applied, Error> {
    let target = target_path(desktops, &locations.config_home);
    let existed = target.exists();

    let mut document = if existed {
        let text = read_text(&target)?;
        let document = Document::parse(&text).map_err(|_| {
            Error::new(format!(
                "cannot parse {}; not changing it",
                target.display()
            ))
        })?;
        document.edit_analysis().map_err(|reason| {
            Error::new(format!("{}: {reason}; not changing it", target.display()))
        })?;
        check_writable(&target)?;
        document
    } else {
        let source = effective_file(
            desktops,
            &locations.config_home,
            &locations.config_dirs,
            &locations.data_dirs,
        );
        let (header, body) = match source {
            Some(source) => {
                let body = read_text(&source)?;
                let header = format!(
                    "{COPY_HEADER_PREFIX}{}{COPY_HEADER_SUFFIX}",
                    source.display()
                );
                (header, body)
            }
            None => (
                CREATED_HEADER.to_string(),
                format!("[{PREFERRED}]\n{DEFAULT_KEY}={GTK}\n"),
            ),
        };
        let ending = if body.contains("\r\n") { "\r\n" } else { "\n" };
        let mut contents = header + ending;
        contents.push_str(&body);
        if !contents.ends_with('\n') {
            contents.push_str(ending);
        }
        Document::parse(&contents).map_err(|_| {
            Error::new(format!(
                "cannot build {}; not changing it",
                target.display()
            ))
        })?
    };

    let analysis = document
        .edit_analysis()
        .map_err(|reason| Error::new(format!("{}: {reason}; not changing it", target.display())))?;
    if let Some(index) = analysis.file_chooser {
        let chain = split_list(&document.value_at(index));
        if gimbal_is_first(&chain) {
            return Ok(Applied {
                changed: false,
                restart_error: None,
            });
        }
    }

    let previous_chain = analysis
        .file_chooser
        .map(|index| split_list(&document.value_at(index)))
        .or_else(|| {
            analysis
                .default
                .map(|index| split_list(&document.value_at(index)))
        })
        .unwrap_or_default();
    let chain = gimbal_chain(&previous_chain);
    let line = format!("{FILE_CHOOSER_KEY}={chain};");

    let already_marked = analysis
        .file_chooser
        .is_some_and(|index| document.marker_text(index).is_some());
    let marker = match analysis.file_chooser {
        Some(index) => format!("{MARKER_PREFIX}{}", document.lines[index].text),
        None => format!("{MARKER_PREFIX}{MARKER_NONE}"),
    };

    match analysis.file_chooser {
        Some(index) => {
            document.lines[index].text = line;
            if !already_marked {
                document.insert_before(index, &marker);
            }
        }
        None => {
            let at = analysis
                .preferred_last_key
                .map(|index| index + 1)
                .unwrap_or(analysis.preferred_header + 1);
            document.insert_two(at, &marker, &line);
        }
    }

    let contents = document.render();
    check_writable_or_parent(&target, existed)?;
    save_atomic(&target, &contents)
        .map_err(|error| Error::new(format!("cannot write {}: {error}", target.display())))?;
    if !existed {
        set_created_mode(&target);
    }

    let restart = restarter.restart();
    finish(restart)
}

/// Turn the switch off: restore the marked line, remove Gimbal's additions,
/// and delete the file only when it is still Gimbal's unchanged copy.
pub fn disable(
    desktops: &[String],
    locations: &Locations,
    restarter: &dyn Restarter,
) -> Result<Applied, Error> {
    let target = target_path(desktops, &locations.config_home);
    if !target.exists() {
        return Ok(Applied {
            changed: false,
            restart_error: None,
        });
    }

    let text = read_text(&target)?;
    let mut document = Document::parse(&text).map_err(|_| {
        Error::new(format!(
            "cannot parse {}; not changing it",
            target.display()
        ))
    })?;
    let analysis = document
        .edit_analysis()
        .map_err(|reason| Error::new(format!("{}: {reason}; not changing it", target.display())))?;
    check_writable(&target)?;

    let Some(index) = analysis.file_chooser else {
        return Ok(Applied {
            changed: false,
            restart_error: None,
        });
    };
    let chain = split_list(&document.value_at(index));
    if !names_gimbal(&chain) {
        return Ok(Applied {
            changed: false,
            restart_error: None,
        });
    }

    let marked = document.marker_text(index).is_some();
    if marked {
        let marker = document.lines[index - 1].text.clone();
        let previous =
            marker[marker.find(MARKER_PREFIX).unwrap_or(0) + MARKER_PREFIX.len()..].to_string();
        document.lines.remove(index - 1);
        if previous == MARKER_NONE {
            document.lines.remove(index - 1);
        } else {
            document.lines[index - 1].text = previous;
        }
    } else {
        let rest: Vec<String> = chain.into_iter().filter(|name| name != GIMBAL).collect();
        let line = index;
        if rest.is_empty() {
            document.lines.remove(line);
        } else {
            document.lines[line].text = format!("{FILE_CHOOSER_KEY}={};", rest.join(";"));
        }
    }

    if created_copy_is_unchanged(&document) {
        fs::remove_file(&target)
            .map_err(|error| Error::new(format!("cannot remove {}: {error}", target.display())))?;
    } else {
        let contents = document.render();
        save_atomic(&target, &contents)
            .map_err(|error| Error::new(format!("cannot write {}: {error}", target.display())))?;
    }

    let restart = restarter.restart();
    finish(restart)
}

fn finish(restart: Result<(), String>) -> Result<Applied, Error> {
    match restart {
        Ok(()) => {
            set_restart_pending(false);
            Ok(Applied {
                changed: true,
                restart_error: None,
            })
        }
        Err(error) => {
            set_restart_pending(true);
            Ok(Applied {
                changed: true,
                restart_error: Some(error),
            })
        }
    }
}

/// The new chain: Gimbal first, then the old backends without Gimbal, and GTK
/// last so a missing or removed Gimbal falls back cleanly. `none` and `*`
/// would stop the chain before GTK, so neither is kept.
fn gimbal_chain(previous: &[String]) -> String {
    let mut chain = vec![GIMBAL.to_string()];
    for name in previous {
        if name == GIMBAL || name == "none" || name == "*" {
            continue;
        }
        if !chain.iter().any(|existing| existing == name) {
            chain.push(name.clone());
        }
    }
    if !chain.iter().any(|name| name == GTK) {
        chain.push(GTK.to_string());
    }
    chain.join(";")
}

fn read_text(path: &Path) -> Result<String, Error> {
    fs::read_to_string(path)
        .map_err(|error| Error::new(format!("cannot read {}: {error}", path.display())))
}

fn check_writable(path: &Path) -> Result<(), Error> {
    match OpenOptions::new().append(true).open(path) {
        Ok(_) => Ok(()),
        Err(error) => Err(Error::new(format!(
            "cannot write {}: {error}; not changing it",
            path.display()
        ))),
    }
}

fn check_writable_or_parent(path: &Path, existed: bool) -> Result<(), Error> {
    if existed {
        return check_writable(path);
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    match fs::create_dir_all(parent) {
        Ok(()) => Ok(()),
        Err(error) => Err(Error::new(format!(
            "cannot create {}: {error}",
            parent.display()
        ))),
    }
}

fn set_created_mode(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o644));
}

// ---------------------------------------------------------------------------
// The text document
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    text: String,
    ending: String,
}

impl Line {
    fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ending: "\n".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Document {
    lines: Vec<Line>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Analysis {
    preferred_header: usize,
    preferred_last_key: Option<usize>,
    preferred_count: usize,
    file_chooser: Option<usize>,
    file_chooser_count: usize,
    default: Option<usize>,
    default_count: usize,
}

impl Document {
    fn parse(text: &str) -> Result<Self, ()> {
        let mut lines = Vec::new();
        let mut start = 0;
        while let Some(offset) = text[start..].find('\n') {
            let end = start + offset;
            let (body, ending) = if text[start..end].ends_with('\r') {
                (&text[start..end - 1], "\r\n")
            } else {
                (&text[start..end], "\n")
            };
            lines.push(Line {
                text: body.to_string(),
                ending: ending.to_string(),
            });
            start = end + 1;
        }
        if start < text.len() {
            lines.push(Line {
                text: text[start..].to_string(),
                ending: String::new(),
            });
        }
        let document = Self { lines };
        document.scan()?;
        Ok(document)
    }

    /// Strict scan for editing: one `[preferred]` group, one of each key.
    fn scan(&self) -> Result<Analysis, ()> {
        let mut group: Option<String> = None;
        let mut preferred_count = 0;
        let mut preferred_header = None;
        let mut preferred_last_key = None;
        let mut file_chooser = None;
        let mut file_chooser_count = 0;
        let mut default = None;
        let mut default_count = 0;

        for (index, line) in self.lines.iter().enumerate() {
            let text = line.text.trim();
            if text.is_empty() || text.starts_with('#') {
                continue;
            }
            if text.starts_with('[') {
                if !text.ends_with(']') || text.len() < 2 {
                    return Err(());
                }
                let name = text[1..text.len() - 1].trim().to_string();
                if name == PREFERRED {
                    preferred_count += 1;
                    if preferred_header.is_none() {
                        preferred_header = Some(index);
                    }
                    preferred_last_key = None;
                }
                group = Some(name);
                continue;
            }
            let Some(equals) = line.text.find('=') else {
                return Err(());
            };
            let key = line.text[..equals].trim();
            if key.is_empty() {
                return Err(());
            }
            if group.as_deref() != Some(PREFERRED) {
                continue;
            }
            preferred_last_key = Some(index);
            match key {
                FILE_CHOOSER_KEY => {
                    file_chooser = Some(index);
                    file_chooser_count += 1;
                }
                DEFAULT_KEY => {
                    default = Some(index);
                    default_count += 1;
                }
                _ => {}
            }
        }

        let Some(preferred_header) = preferred_header else {
            // A file with no preferred group is not a portals configuration.
            return Err(());
        };
        Ok(Analysis {
            preferred_header,
            preferred_last_key,
            preferred_count,
            file_chooser,
            file_chooser_count,
            default,
            default_count,
        })
    }

    fn has_preferred(&self) -> bool {
        self.lines.iter().any(|line| {
            let text = line.text.trim();
            text == format!("[{PREFERRED}]")
        })
    }

    fn edit_analysis(&self) -> Result<Analysis, String> {
        let analysis = self
            .scan()
            .map_err(|_| "it is not a readable portals.conf".to_string())?;
        if analysis.preferred_count > 1 {
            return Err("it has more than one [preferred] group".to_string());
        }
        if analysis.file_chooser_count > 1 {
            return Err("it has more than one FileChooser line".to_string());
        }
        if analysis.default_count > 1 {
            return Err("it has more than one default line".to_string());
        }
        Ok(analysis)
    }

    fn value_at(&self, index: usize) -> String {
        let line = &self.lines[index].text;
        line.find('=')
            .map(|equals| line[equals + 1..].trim().to_string())
            .unwrap_or_default()
    }

    fn value(&self, key: &str) -> Option<String> {
        let mut group = None;
        let mut value = None;
        for line in &self.lines {
            let text = line.text.trim();
            if text.is_empty() || text.starts_with('#') {
                continue;
            }
            if text.starts_with('[') && text.ends_with(']') {
                group = Some(text[1..text.len() - 1].trim());
                continue;
            }
            if group != Some(PREFERRED) {
                continue;
            }
            let Some(equals) = line.text.find('=') else {
                continue;
            };
            if line.text[..equals].trim() == key {
                value = Some(line.text[equals + 1..].trim().to_string());
            }
        }
        value
    }

    /// A marker directly above `index`, inside the group.
    fn marker_text(&self, index: usize) -> Option<&str> {
        let line = self.lines.get(index.checked_sub(1)?)?;
        let text = line.text.trim_start();
        text.starts_with(MARKER_PREFIX).then_some(text)
    }

    fn insert_before(&mut self, index: usize, text: &str) {
        self.ensure_ending_before(index);
        let marker = Line::new(text);
        self.lines.insert(index, marker);
    }

    fn insert_two(&mut self, index: usize, first: &str, second: &str) {
        self.ensure_ending_before(index);
        self.lines.insert(index, Line::new(second));
        self.lines.insert(index, Line::new(first));
    }

    fn ensure_ending_before(&mut self, index: usize) {
        if index > 0 && self.lines[index - 1].ending.is_empty() {
            let ending = self
                .lines
                .iter()
                .find(|line| !line.ending.is_empty())
                .map(|line| line.ending.clone())
                .unwrap_or_else(|| "\n".to_string());
            self.lines[index - 1].ending = ending;
        }
    }

    fn render(&self) -> String {
        let mut text = String::new();
        for line in &self.lines {
            text.push_str(&line.text);
            text.push_str(&line.ending);
        }
        text
    }
}

fn created_copy_is_unchanged(document: &Document) -> bool {
    let header = document
        .lines
        .iter()
        .position(|line| line.text.starts_with(COPY_HEADER_PREFIX) || line.text == CREATED_HEADER);
    let Some(header) = header else {
        return false;
    };

    let mut body = String::new();
    for line in document.lines.iter().skip(header + 1) {
        body.push_str(&line.text);
        body.push_str(&line.ending);
    }

    if document.lines[header].text == CREATED_HEADER {
        return normalised(&body) == normalised(&format!("[{PREFERRED}]\n{DEFAULT_KEY}={GTK}\n"));
    }

    let Some(source) = copy_source(&document.lines[header].text) else {
        return false;
    };
    let Ok(original) = fs::read_to_string(source) else {
        return false;
    };
    normalised(&body) == normalised(&original)
}

fn copy_source(header: &str) -> Option<&str> {
    header
        .strip_prefix(COPY_HEADER_PREFIX)?
        .strip_suffix(COPY_HEADER_SUFFIX)
}

fn normalised(text: &str) -> String {
    text.replace("\r\n", "\n")
        .trim_end_matches('\n')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;
    use std::rc::Rc;

    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let root = PathBuf::from(format!(
                "/tmp/gimbal-p2av-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|time| time.as_nanos())
                    .unwrap_or(0)
            ));
            fs::create_dir_all(&root).expect("scratch root");
            Self { root }
        }

        fn user(&self) -> PathBuf {
            self.root.join("user")
        }

        fn etc(&self) -> PathBuf {
            self.root.join("etc")
        }

        fn usr(&self) -> PathBuf {
            self.root.join("usr")
        }

        fn locations(&self) -> Locations {
            Locations {
                config_home: self.user(),
                config_dirs: vec![self.etc()],
                data_dirs: vec![self.usr()],
            }
        }

        fn write(&self, root: &Path, name: &str, text: &str) -> PathBuf {
            let dir = root.join("xdg-desktop-portal");
            fs::create_dir_all(&dir).expect("conf dir");
            let path = dir.join(name);
            fs::write(&path, text).expect("write conf");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[derive(Default)]
    struct Counter {
        calls: Rc<Cell<u32>>,
        fail: bool,
    }

    impl Counter {
        fn failing() -> Self {
            Self {
                calls: Rc::new(Cell::new(0)),
                fail: true,
            }
        }
    }

    impl Restarter for Counter {
        fn restart(&self) -> Result<(), String> {
            self.calls.set(self.calls.get() + 1);
            if self.fail {
                Err("no systemd".to_string())
            } else {
                Ok(())
            }
        }
    }

    fn desktops() -> Vec<String> {
        vec!["mango".to_string()]
    }

    fn two_desktops() -> Vec<String> {
        vec!["budgie".to_string(), "gnome".to_string()]
    }

    fn run_enable(scratch: &Scratch, desktops: &[String], restarter: &Counter) -> Applied {
        run_enable_at(scratch, desktops, restarter).expect("enable")
    }

    fn run_enable_at(
        scratch: &Scratch,
        desktops: &[String],
        restarter: &Counter,
    ) -> Result<Applied, Error> {
        enable(desktops, &scratch.locations(), restarter)
    }

    fn run_disable_at(
        scratch: &Scratch,
        desktops: &[String],
        restarter: &Counter,
    ) -> Result<Applied, Error> {
        disable(desktops, &scratch.locations(), restarter)
    }

    #[test]
    fn the_first_desktop_wins_across_every_root() {
        let scratch = Scratch::new("order");
        assert_eq!(
            effective_file(
                &two_desktops(),
                &scratch.user(),
                &[scratch.etc()],
                &[scratch.usr()]
            ),
            None
        );

        scratch.write(
            &scratch.usr(),
            "budgie-portals.conf",
            "[preferred]\ndefault=gtk\n",
        );
        scratch.write(
            &scratch.etc(),
            "gnome-portals.conf",
            "[preferred]\ndefault=gtk\n",
        );
        // Budgie's file is in the lowest root but budgie is the first desktop.
        assert_eq!(
            effective_file(
                &two_desktops(),
                &scratch.user(),
                &[scratch.etc()],
                &[scratch.usr()]
            ),
            Some(scratch.usr().join("xdg-desktop-portal/budgie-portals.conf"))
        );

        scratch.write(
            &scratch.etc(),
            "budgie-portals.conf",
            "[preferred]\ndefault=gtk\n",
        );
        assert_eq!(
            effective_file(
                &two_desktops(),
                &scratch.user(),
                &[scratch.etc()],
                &[scratch.usr()]
            ),
            Some(scratch.etc().join("xdg-desktop-portal/budgie-portals.conf"))
        );

        scratch.write(
            &scratch.user(),
            "budgie-portals.conf",
            "[preferred]\ndefault=gtk\n",
        );
        assert_eq!(
            effective_file(
                &two_desktops(),
                &scratch.user(),
                &[scratch.etc()],
                &[scratch.usr()]
            ),
            Some(
                scratch
                    .user()
                    .join("xdg-desktop-portal/budgie-portals.conf")
            )
        );
    }

    #[test]
    fn a_desktop_file_beats_a_plain_user_file() {
        let scratch = Scratch::new("plain");
        scratch.write(
            &scratch.user(),
            "portals.conf",
            "[preferred]\ndefault=gtk\n",
        );
        scratch.write(
            &scratch.etc(),
            "mango-portals.conf",
            "[preferred]\ndefault=gtk\n",
        );
        assert_eq!(
            effective_file(&desktops(), &scratch.user(), &[scratch.etc()], &[]),
            Some(scratch.etc().join("xdg-desktop-portal/mango-portals.conf"))
        );
    }

    #[test]
    fn a_plain_file_is_used_when_no_desktop_file_exists() {
        let scratch = Scratch::new("plain-only");
        let path = scratch.write(
            &scratch.user(),
            "portals.conf",
            "[preferred]\ndefault=gtk\n",
        );
        assert_eq!(
            effective_file(&desktops(), &scratch.user(), &[scratch.etc()], &[]),
            Some(path.clone())
        );
        assert_eq!(
            config_files(&desktops(), &scratch.user(), &[scratch.etc()], &[]),
            vec![path]
        );
    }

    #[test]
    fn config_files_keeps_the_lower_layers() {
        let scratch = Scratch::new("layers");
        let user = scratch.write(
            &scratch.user(),
            "mango-portals.conf",
            "[preferred]\norg.freedesktop.impl.portal.FileChooser=gnome;\n",
        );
        let usr = scratch.write(&scratch.usr(), "portals.conf", "[preferred]\ndefault=gtk\n");
        assert_eq!(
            config_files(&desktops(), &scratch.user(), &[], &[scratch.usr()]),
            vec![user, usr]
        );
    }

    #[test]
    fn a_file_without_preferred_is_skipped() {
        let scratch = Scratch::new("unpreferred");
        scratch.write(&scratch.user(), "portals.conf", "# nothing here\n");
        let path = scratch.write(&scratch.usr(), "portals.conf", "[preferred]\ndefault=gtk\n");
        assert_eq!(
            effective_file(&desktops(), &scratch.user(), &[], &[scratch.usr()]),
            Some(path)
        );
    }

    #[test]
    fn chains_read_interface_then_default_and_keep_semicolon_lists() {
        let scratch = Scratch::new("chain");
        let path = scratch.write(
            &scratch.user(),
            "portals.conf",
            "[preferred]\norg.freedesktop.impl.portal.FileChooser=gnome;gtk;\n",
        );
        assert_eq!(file_chooser_chain(&path), vec!["gnome", "gtk"]);

        let path = scratch.write(
            &scratch.user(),
            "portals.conf",
            "[preferred]\ndefault=gtk\n",
        );
        assert_eq!(file_chooser_chain(&path), vec!["gtk"]);

        let path = scratch.write(
            &scratch.user(),
            "portals.conf",
            "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.ScreenCast=wlr\n",
        );
        assert_eq!(file_chooser_chain(&path), vec!["gtk"]);

        let path = scratch.write(&scratch.user(), "portals.conf", "[preferred]\n");
        assert!(file_chooser_chain(&path).is_empty());
    }

    #[test]
    fn the_effective_chain_layers_and_skips_uninstalled_backends() {
        let scratch = Scratch::new("effective-chain");
        let user = scratch.write(
            &scratch.user(),
            "mango-portals.conf",
            "[preferred]\norg.freedesktop.impl.portal.ScreenCast=wlr\n",
        );
        let usr = scratch.write(&scratch.usr(), "portals.conf", "[preferred]\ndefault=gtk\n");
        let installed = vec!["wlr".to_string(), "gtk".to_string()];
        assert_eq!(
            effective_chain(&[user, usr.clone()], &installed),
            vec!["gtk"]
        );

        let user = scratch.write(
            &scratch.user(),
            "mango-portals.conf",
            "[preferred]\norg.freedesktop.impl.portal.FileChooser=gnome;\n",
        );
        assert_eq!(
            effective_chain(&[user, usr], &installed),
            vec!["gtk"],
            "an uninstalled gnome must not stop the lower file"
        );
    }

    #[test]
    fn enable_creates_a_copy_of_the_system_file_and_keeps_other_interfaces() {
        let scratch = Scratch::new("copy");
        scratch.write(
            &scratch.usr(),
            "mango-portals.conf",
            "[preferred]\ndefault=gtk\norg.freedesktop.impl.portal.ScreenCast=wlr\n",
        );
        let restarter = Counter::default();
        let applied = run_enable(&scratch, &desktops(), &restarter);
        assert!(applied.changed);
        assert_eq!(applied.restart_error, None);
        assert_eq!(restarter.calls.get(), 1);

        let target = scratch.user().join("xdg-desktop-portal/mango-portals.conf");
        let text = fs::read_to_string(&target).expect("target");
        assert!(text.starts_with("# Copied from "), "{text}");
        assert!(text.contains(
            "by Gimbal Files when \"Use Gimbal for Open and Save dialogs\" was turned on."
        ));
        assert!(
            text.contains("org.freedesktop.impl.portal.ScreenCast=wlr"),
            "{text}"
        );
        assert!(text.contains("# Gimbal Files: was none"), "{text}");
        assert!(
            text.contains("org.freedesktop.impl.portal.FileChooser=gimbal;gtk;"),
            "{text}"
        );
        let screen = text
            .find("org.freedesktop.impl.portal.ScreenCast=wlr")
            .unwrap();
        let marker = text.find("# Gimbal Files: was none").unwrap();
        let chooser = text
            .find("org.freedesktop.impl.portal.FileChooser=")
            .unwrap();
        assert!(screen < marker && marker < chooser, "{text}");
        assert_eq!(file_chooser_chain(&target), vec!["gimbal", "gtk"]);
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn enable_keeps_an_existing_filechooser_and_marks_the_exact_old_line() {
        let scratch = Scratch::new("existing");
        let target = scratch.write(
            &scratch.user(),
            "mango-portals.conf",
            "[preferred]\norg.freedesktop.impl.portal.FileChooser=gnome;\n",
        );
        let restarter = Counter::default();
        run_enable(&scratch, &desktops(), &restarter);
        let text = fs::read_to_string(&target).unwrap();
        assert!(
            text.contains("# Gimbal Files: was org.freedesktop.impl.portal.FileChooser=gnome;"),
            "{text}"
        );
        assert!(
            text.contains("org.freedesktop.impl.portal.FileChooser=gimbal;gnome;gtk;"),
            "{text}"
        );
        assert_eq!(file_chooser_chain(&target), vec!["gimbal", "gnome", "gtk"]);
    }

    #[test]
    fn enable_when_already_on_changes_nothing_and_does_not_restart() {
        let scratch = Scratch::new("already");
        let before = "[preferred]\norg.freedesktop.impl.portal.FileChooser=gimbal;gtk;\n";
        let target = scratch.write(&scratch.user(), "mango-portals.conf", before);
        let restarter = Counter::default();
        let applied = run_enable(&scratch, &desktops(), &restarter);
        assert!(!applied.changed);
        assert_eq!(restarter.calls.get(), 0);
        assert_eq!(fs::read_to_string(&target).unwrap(), before);
    }

    #[test]
    fn enable_with_no_effective_file_creates_a_minimal_one() {
        let scratch = Scratch::new("minimal");
        let restarter = Counter::default();
        run_enable(&scratch, &desktops(), &restarter);
        let target = scratch.user().join("xdg-desktop-portal/mango-portals.conf");
        let text = fs::read_to_string(&target).unwrap();
        assert!(text.starts_with("# Created by Gimbal Files"), "{text}");
        assert!(text.contains("[preferred]"), "{text}");
        assert!(text.contains("default=gtk"), "{text}");
        assert!(
            text.contains("org.freedesktop.impl.portal.FileChooser=gimbal;gtk;"),
            "{text}"
        );
    }

    #[test]
    fn disable_restores_a_user_file_byte_for_byte_and_deletes_a_copy() {
        let scratch = Scratch::new("restore-user");
        let before = "# my comment\n[preferred]\n# keep me\ndefault=gtk\n\
                      org.freedesktop.impl.portal.ScreenCast=wlr\n";
        let target = scratch.write(&scratch.user(), "mango-portals.conf", before);
        let restarter = Counter::default();
        run_enable(&scratch, &desktops(), &restarter);
        assert!(fs::read_to_string(&target).unwrap() != before);
        let applied = run_disable_at(&scratch, &desktops(), &restarter).unwrap();
        assert!(applied.changed);
        assert_eq!(restarter.calls.get(), 2);
        assert_eq!(fs::read_to_string(&target).unwrap(), before);

        let scratch = Scratch::new("restore-copy");
        scratch.write(&scratch.usr(), "portals.conf", "[preferred]\ndefault=gtk\n");
        let restarter = Counter::default();
        run_enable(&scratch, &desktops(), &restarter);
        let target = scratch.user().join("xdg-desktop-portal/mango-portals.conf");
        assert!(target.is_file());
        let applied = run_disable_at(&scratch, &desktops(), &restarter).unwrap();
        assert!(applied.changed);
        assert!(!target.exists(), "an unchanged copy is removed");
    }

    #[test]
    fn disable_leaves_a_copy_that_was_edited_after_enabling() {
        let scratch = Scratch::new("edited-copy");
        scratch.write(&scratch.usr(), "portals.conf", "[preferred]\ndefault=gtk\n");
        let restarter = Counter::default();
        run_enable(&scratch, &desktops(), &restarter);
        let target = scratch.user().join("xdg-desktop-portal/mango-portals.conf");
        let mut text = fs::read_to_string(&target).unwrap();
        text.push_str("org.freedesktop.impl.portal.ScreenCast=wlr\n");
        fs::write(&target, text).unwrap();

        run_disable_at(&scratch, &desktops(), &restarter).unwrap();
        let text = fs::read_to_string(&target).expect("the edited copy stays");
        assert!(text.contains("ScreenCast=wlr"), "{text}");
        assert!(!text.contains(FILE_CHOOSER_KEY), "{text}");
        assert!(!text.contains("# Gimbal Files: was"), "{text}");
    }

    #[test]
    fn disable_removes_a_hand_written_gimbal_with_no_marker() {
        let scratch = Scratch::new("hand-written");
        let target = scratch.write(
            &scratch.user(),
            "mango-portals.conf",
            "[preferred]\norg.freedesktop.impl.portal.FileChooser=gimbal;gnome;gtk;\n",
        );
        let restarter = Counter::default();
        let applied = run_disable_at(&scratch, &desktops(), &restarter).unwrap();
        assert!(applied.changed);
        let text = fs::read_to_string(&target).unwrap();
        assert!(
            text.contains("org.freedesktop.impl.portal.FileChooser=gnome;gtk;"),
            "{text}"
        );
        assert!(!text.contains("gimbal"), "{text}");

        let target = scratch.write(
            &scratch.user(),
            "mango-portals.conf",
            "[preferred]\norg.freedesktop.impl.portal.FileChooser=gimbal;\n",
        );
        run_disable_at(&scratch, &desktops(), &restarter).unwrap();
        let text = fs::read_to_string(&target).unwrap();
        assert!(!text.contains(FILE_CHOOSER_KEY), "{text}");
    }

    #[test]
    fn disable_is_a_no_op_when_the_file_does_not_name_gimbal() {
        let scratch = Scratch::new("off");
        let before = "[preferred]\ndefault=gtk\n";
        let target = scratch.write(&scratch.user(), "mango-portals.conf", before);
        let restarter = Counter::default();
        let applied = run_disable_at(&scratch, &desktops(), &restarter).unwrap();
        assert!(!applied.changed);
        assert_eq!(restarter.calls.get(), 0);
        assert_eq!(fs::read_to_string(&target).unwrap(), before);

        let applied = run_disable_at(&scratch, &desktops(), &restarter).unwrap();
        assert!(!applied.changed);
    }

    #[test]
    fn an_unparsable_target_is_refused_and_left_alone() {
        let scratch = Scratch::new("bad");
        let before = "this is not a portals.conf\n";
        let target = scratch.write(&scratch.user(), "mango-portals.conf", before);
        let restarter = Counter::default();
        assert!(run_enable_at(&scratch, &desktops(), &restarter).is_err());
        assert!(run_disable_at(&scratch, &desktops(), &restarter).is_err());
        assert_eq!(restarter.calls.get(), 0);
        assert_eq!(fs::read_to_string(&target).unwrap(), before);
    }

    #[test]
    fn an_unwritable_target_is_refused_and_left_alone() {
        let scratch = Scratch::new("readonly");
        let before = "[preferred]\ndefault=gtk\n";
        let target = scratch.write(&scratch.user(), "mango-portals.conf", before);
        fs::set_permissions(&target, fs::Permissions::from_mode(0o444)).unwrap();
        // A test process running as root can still open it; skip the assertion
        // then, but still prove no restart happened.
        let writable = OpenOptions::new().append(true).open(&target).is_ok();
        let restarter = Counter::default();
        let result = run_enable_at(&scratch, &desktops(), &restarter);
        if !writable {
            assert!(result.is_err());
            assert_eq!(fs::read_to_string(&target).unwrap(), before);
        }
        assert_eq!(restarter.calls.get(), 0);
    }

    #[test]
    fn a_failed_restart_keeps_the_file_and_reports_it() {
        let scratch = Scratch::new("restart-fail");
        let restarter = Counter::failing();
        clear_restart_pending();
        let applied = run_enable(&scratch, &desktops(), &restarter);
        assert_eq!(restarter.calls.get(), 1);
        assert!(applied.changed);
        assert_eq!(applied.restart_error.as_deref(), Some("no systemd"));
        assert!(restart_pending());
        let target = scratch.user().join("xdg-desktop-portal/mango-portals.conf");
        assert!(target.is_file());
        assert_eq!(file_chooser_chain(&target), vec!["gimbal", "gtk"]);
        clear_restart_pending();
    }

    #[test]
    fn a_successful_restart_clears_the_pending_flag() {
        let scratch = Scratch::new("restart-ok");
        let failing = Counter::failing();
        clear_restart_pending();
        run_enable(&scratch, &desktops(), &failing);
        assert!(restart_pending());
        let working = Counter::default();
        run_enable(&scratch, &desktops(), &working);
        // Already on: no change and no restart, so the flag stays.
        assert!(restart_pending());
        run_disable_at(&scratch, &desktops(), &working).unwrap();
        assert!(!restart_pending());
        clear_restart_pending();
    }

    #[test]
    fn round_trips_keep_every_other_byte() {
        let scratch = Scratch::new("round-trip");
        let before = "# top\n\n[preferred]\n# comment\ndefault=gtk\n\
                      org.freedesktop.impl.portal.ScreenCast=wlr\n\n[other]\nkey=value\n";
        let target = scratch.write(&scratch.user(), "mango-portals.conf", before);
        let restarter = Counter::default();
        run_enable(&scratch, &desktops(), &restarter);
        run_disable_at(&scratch, &desktops(), &restarter).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), before);
    }

    #[test]
    fn health_and_status_text_cover_every_state() {
        let available = vec!["gtk".to_string(), "gnome".to_string()];
        assert_eq!(
            status_text(&health(false, &[], &available, false)),
            "Not installed: the portal backend files are missing"
        );
        assert_eq!(
            status_text(&health(
                true,
                &["gimbal".into(), "gtk".into()],
                &available,
                false
            )),
            "Open and Save dialogs: Gimbal"
        );
        assert_eq!(
            status_text(&health(
                true,
                &["gnome".into(), "gtk".into()],
                &available,
                false
            )),
            "Open and Save dialogs: GNOME"
        );
        assert_eq!(
            status_text(&health(true, &["gtk".into()], &available, false)),
            "Open and Save dialogs: GTK"
        );
        assert_eq!(
            status_text(&health(true, &[], &available, false)),
            "Open and Save dialogs: GTK"
        );
        assert_eq!(
            status_text(&health(true, &["none".into()], &available, false)),
            "Open and Save dialogs: none"
        );
        assert_eq!(
            status_text(&health(true, &["gimbal".into()], &available, true)),
            "Saved; sign out and back in to apply"
        );
        assert_eq!(
            status_text(&health(true, &["none".into()], &[], false)),
            "Open and Save dialogs: none"
        );
    }

    #[test]
    fn desktop_names_are_split_validated_and_lowercased() {
        assert_eq!(
            desktops_from("Mango:GNOME:bad name::x_y"),
            vec!["mango", "gnome", "x_y"]
        );
        assert!(desktops_from("").is_empty());
        assert_eq!(
            target_path(&[], Path::new("/home/k")),
            PathBuf::from("/home/k/xdg-desktop-portal/portals.conf")
        );
    }

    #[test]
    fn portal_dirs_follow_the_environment_override() {
        // Read-only: only proves the override shape. The real environment is
        // never changed here.
        if std::env::var_os("XDG_DESKTOP_PORTAL_DIR").is_some() {
            assert_eq!(portal_dirs().len(), 1);
        }
    }
}
