// SPDX-License-Identifier: GPL-3.0-or-later
//! The Properties window.
//!
//! One transient window per invocation, built like P2aa's Open With chooser:
//! a plain `gtk::Window` with the shared look class, theme colours, token
//! sizes and Escape to close. GIO reads the metadata through the tab's scope;
//! folder totals come from the existing recursive walker in `fm_core`.

use std::cell::{Cell, RefCell};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::TryRecvError;
use std::time::Duration;

use adw::prelude::*;
use fm_core::admin::{Scope, gfile};
use fm_core::folder_size::{
    DirReader, EntryKind, EntryMeta, NativeSource, RootIdentity, RootProbe, RootState, ScanPolicy,
    ScanResult, ScanSource, SystemClock, probe_native, scan_with_root,
};
use fm_core::properties::{self, SummaryItem};
use gtk::{gdk, gio, glib};

use crate::frame::WindowUi;
use crate::look::icons::{self, MockupIcon};
use crate::look::tokens::{GAP_M, GAP_S, SEARCH_W, scaled};

const INFO_ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::size,standard::content-type,standard::icon,standard::is-symlink,standard::symlink-target,owner::user,owner::group,unix::uid,unix::gid,unix::mode,time::created,time::access,time::modified";

const SIZE_ATTRIBUTES: &str = "filesystem::free,filesystem::size";

const LIST_ATTRIBUTES: &str = "standard::name,standard::type,standard::size,standard::is-symlink,unix::device,unix::inode,unix::mode,unix::nlink,unix::blocks,time::modified,time::modified-usec";

/// One item's read-only facts, as GIO reported them in the tab's scope.
struct Facts {
    path: PathBuf,
    name: String,
    is_dir: bool,
    is_file: bool,
    symlink: Option<String>,
    broken_link: bool,
    content_type: String,
    type_description: String,
    size: u64,
    mode: u32,
    owner: String,
    group: String,
    created: Option<i64>,
    modified: Option<i64>,
    accessed: Option<i64>,
    icon: Option<gio::Icon>,
}

impl Facts {
    fn read(path: &Path, scope: Scope) -> Self {
        // Follow the link for the target's ordinary facts. A broken link has
        // no followed info, so the second lstat result stands in and the row
        // marks the target as broken.
        let followed = query(path, scope, gio::FileQueryInfoFlags::NONE);
        let link_info = query(path, scope, gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS);
        let info = followed.as_ref().or(link_info.as_ref());
        let symlink = link_info
            .as_ref()
            .filter(|info| info.file_type() == gio::FileType::SymbolicLink)
            .and_then(|info| info.symlink_target())
            .map(|target| target.to_string_lossy().into_owned());
        // GIO does not fail a followed query on a broken link: it falls back to
        // the link's own info, still typed SymbolicLink. So the link is broken
        // when following it still yields a link (or nothing at all).
        let broken_link = symlink.is_some()
            && followed
                .as_ref()
                .is_none_or(|info| info.file_type() == gio::FileType::SymbolicLink);

        let file_type = info.map(gio::FileInfo::file_type);
        let is_dir = file_type == Some(gio::FileType::Directory);
        let is_file = file_type == Some(gio::FileType::Regular);
        let size = info
            .map(|info| u64::try_from(info.size()).unwrap_or(0))
            .unwrap_or(0);

        let queried = info
            .and_then(gio::FileInfo::content_type)
            .map(|content_type| content_type.to_string())
            .unwrap_or_else(|| gio::content_type_guess(Some(path), None).0.to_string());
        let by_name = gio::content_type_guess(Some(path), None).0.to_string();
        let content_type = fm_core::admin::lookup_type(&queried, &by_name).to_string();
        let description = gio::content_type_get_description(&content_type).to_string();
        let type_description = if description.is_empty() {
            if content_type.is_empty() {
                properties::UNKNOWN.to_string()
            } else {
                content_type.clone()
            }
        } else {
            description
        };

        Self {
            path: path.to_path_buf(),
            name: info
                .map(|info| info.display_name().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| properties::display_name(path)),
            is_dir,
            is_file,
            symlink,
            broken_link,
            content_type,
            type_description,
            size,
            mode: info.map_or(0, |info| info.attribute_uint32("unix::mode") & 0o7777),
            owner: owner_or_id(info, "owner::user", "unix::uid"),
            group: owner_or_id(info, "owner::group", "unix::gid"),
            created: time_attribute(info, "time::created"),
            modified: time_attribute(info, "time::modified"),
            accessed: time_attribute(info, "time::access"),
            icon: info.and_then(gio::FileInfo::icon),
        }
    }
}

fn query(path: &Path, scope: Scope, flags: gio::FileQueryInfoFlags) -> Option<gio::FileInfo> {
    gfile(path, scope)
        .query_info(INFO_ATTRIBUTES, flags, None::<&gio::Cancellable>)
        .ok()
}

fn owner_or_id(info: Option<&gio::FileInfo>, attribute: &str, id_attribute: &str) -> String {
    if let Some(name) = info
        .and_then(|info| info.attribute_string(attribute))
        .filter(|name| !name.is_empty())
    {
        return name.to_string();
    }
    match info {
        Some(info) if info.has_attribute(id_attribute) => {
            info.attribute_uint32(id_attribute).to_string()
        }
        _ => properties::UNKNOWN.to_string(),
    }
}

fn time_attribute(info: Option<&gio::FileInfo>, attribute: &str) -> Option<i64> {
    let info = info.filter(|info| info.has_attribute(attribute))?;
    Some(i64::try_from(info.attribute_uint64(attribute)).unwrap_or(i64::MAX))
}

/// Show the read-only Properties window for `paths` in `scope`.
pub fn show(ui: &Rc<WindowUi>, paths: &[PathBuf], scope: Scope) {
    if paths.is_empty() {
        return;
    }

    let scale = ui.process_upgrade().map_or(1.0, |process| process.scale());
    let facts: Vec<Facts> = paths.iter().map(|path| Facts::read(path, scope)).collect();
    let live = Rc::new(Cell::new(true));

    let window = gtk::Window::builder()
        .title("Properties")
        .transient_for(ui.window())
        .destroy_with_parent(true)
        .build();
    window.add_css_class("gimbal-look");
    window.add_css_class("gl-properties");
    window.set_default_size(scaled(SEARCH_W, scale), -1);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("gl-properties-root");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, scaled(GAP_M, scale));
    header.add_css_class("gl-properties-header");
    if let [fact] = facts.as_slice() {
        let icon = if fact.is_dir {
            icons::image(MockupIcon::Folder)
        } else {
            match &fact.icon {
                Some(icon) => gtk::Image::from_gicon(icon),
                None => gtk::Image::from_icon_name("text-x-generic-symbolic"),
            }
        };
        icon.add_css_class("gl-properties-icon");
        header.append(&icon);
        let name = gtk::Label::builder()
            .label(&fact.name)
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .wrap(true)
            .selectable(true)
            .build();
        name.add_css_class("gl-properties-name");
        header.append(&name);
    } else {
        let title = gtk::Label::builder()
            .label(properties::item_count(&summaries(&facts)))
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .wrap(true)
            .build();
        title.add_css_class("gl-properties-title");
        header.append(&title);
    }
    root.append(&header);

    let rule = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rule.add_css_class("gl-rule");
    root.append(&rule);

    let grid = gtk::Grid::builder()
        .column_spacing(scaled(GAP_M, scale))
        .row_spacing(scaled(GAP_S, scale))
        .margin_top(scaled(GAP_M, scale))
        .build();
    grid.add_css_class("gl-properties-grid");
    root.append(&grid);

    let mut row = 0;
    if let [fact] = facts.as_slice() {
        let kind = format!("{} ({})", fact.type_description, fact.content_type);
        add_row(&grid, row, "Type", &kind);
        row += 1;

        if fact.is_dir {
            let size = add_row(&grid, row, "Size", properties::CALCULATING);
            row += 1;
            let alive = live.clone();
            start_folder_size(
                fact.path.clone(),
                scope,
                alive,
                Rc::new(move |result| {
                    let text = match result {
                        ScanResult::Sized(size) => {
                            let unreadable =
                                properties::has_unreadable_entries(&size.totals.partial);
                            properties::folder_size_text(
                                size.totals.contents_bytes,
                                &size.totals.contains_text(),
                                unreadable,
                            )
                        }
                        ScanResult::Unavailable(RootState::PermissionDenied) => {
                            "Unavailable (permission denied)".to_string()
                        }
                        ScanResult::Unavailable(RootState::Remote) => {
                            "Not calculated for remote folders".to_string()
                        }
                        ScanResult::Unavailable(_) | ScanResult::Cancelled => {
                            "Unavailable".to_string()
                        }
                    };
                    set_value(&size, "Size", &text);
                }),
            );
        } else {
            add_row(&grid, row, "Size", &properties::file_size_text(fact.size));
            row += 1;
        }

        if let Some(parent) = fact.path.parent() {
            add_location(&grid, row, ui, parent);
            row += 1;
        }

        if let Some(target) = &fact.symlink {
            let text = if fact.broken_link {
                format!("{target} (broken link)")
            } else {
                target.clone()
            };
            add_row(&grid, row, "Link target", &text);
            row += 1;
        }

        add_row(&grid, row, "Created", &properties::date_text(fact.created));
        row += 1;
        add_row(
            &grid,
            row,
            "Modified",
            &properties::date_text(fact.modified),
        );
        row += 1;
        add_row(
            &grid,
            row,
            "Accessed",
            &properties::date_text(fact.accessed),
        );
        row += 1;

        add_row(&grid, row, "Owner", &fact.owner);
        row += 1;
        add_row(&grid, row, "Group", &fact.group);
        row += 1;

        if fact.broken_link {
            // A link's own mode is always 0777 on Linux and means nothing; the
            // target that would carry real permissions is missing.
            add_row(
                &grid,
                row,
                "Permissions",
                properties::BROKEN_LINK_PERMISSIONS,
            );
            row += 1;
        } else {
            let permissions = properties::permissions(fact.mode, fact.is_dir);
            add_row(&grid, row, "Owner permissions", permissions.owner);
            row += 1;
            add_row(&grid, row, "Group permissions", permissions.group);
            row += 1;
            add_row(&grid, row, "Others permissions", permissions.others);
            row += 1;
            if permissions.executable {
                add_row(&grid, row, "Executable", "Yes");
                row += 1;
            }
            add_row(&grid, row, "Mode", &permissions.octal);
            row += 1;
        }

        if fact.is_file && !fact.is_dir {
            add_opens_with(&grid, row, ui, fact, scale);
            row += 1;
        }

        add_disk(&grid, row, ui, fact, scope, scale);
    } else {
        add_multi_size(&grid, &mut row, &facts, scope, live.clone());

        if let Some(parent) = properties::common_parent(paths) {
            add_location(&grid, row, ui, &parent);
            row += 1;
        }

        add_row(
            &grid,
            row,
            "Type",
            &properties::type_tally(&summaries(&facts)),
        );
    }

    window.set_child(Some(&root));

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let window_keys = window.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            window_keys.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);

    let window_live = live.clone();
    window.connect_destroy(move |_| window_live.set(false));

    crate::log!(
        "properties",
        "opened for {} item(s) in the {} scope",
        facts.len(),
        scope.as_str()
    );
    window.present();
}

fn summaries(facts: &[Facts]) -> Vec<SummaryItem> {
    facts
        .iter()
        .map(|fact| SummaryItem {
            is_dir: fact.is_dir,
            type_description: fact.type_description.clone(),
        })
        .collect()
}

fn add_row(grid: &gtk::Grid, row: i32, name: &str, value: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(name)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .build();
    label.add_css_class("gl-properties-label");
    grid.attach(&label, 0, row, 1, 1);

    let value_label = gtk::Label::builder()
        .label(value)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .hexpand(true)
        .build();
    value_label.add_css_class("gl-properties-value");
    set_value(&value_label, name, value);
    grid.attach(&value_label, 1, row, 1, 1);
    value_label
}

fn set_value(label: &gtk::Label, name: &str, value: &str) {
    label.set_label(value);
    let accessible = format!("{name}: {value}");
    label.update_property(&[gtk::accessible::Property::Label(&accessible)]);
}

fn value_box(scale: f64) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, scaled(GAP_S, scale))
}

fn add_clickable_row(
    grid: &gtk::Grid,
    row: i32,
    name: &str,
    widget: &impl IsA<gtk::Widget>,
    accessible: &str,
) {
    let label = gtk::Label::builder()
        .label(name)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .build();
    label.add_css_class("gl-properties-label");
    grid.attach(&label, 0, row, 1, 1);
    let value = widget.upcast_ref::<gtk::Widget>();
    value.update_property(&[gtk::accessible::Property::Label(accessible)]);
    grid.attach(value, 1, row, 1, 1);
}

fn add_location(grid: &gtk::Grid, row: i32, ui: &Rc<WindowUi>, folder: &Path) {
    let text = folder.to_string_lossy().into_owned();
    let uri = glib::filename_to_uri(folder, None)
        .map(|uri| uri.to_string())
        .unwrap_or_else(|_| "file:///".to_string());
    let link = gtk::LinkButton::with_label(&uri, &text);
    link.add_css_class("gl-properties-link");
    link.set_halign(gtk::Align::Start);
    link.set_tooltip_text(Some("Open this folder in a new tab"));
    let ui = ui.clone();
    let path = text.clone();
    link.connect_activate_link(move |_| {
        ui.dispatch(
            fm_core::command::Caller::Ui,
            fm_core::command::Invocation::new("tab.new", vec![("path".to_string(), path.clone())]),
        );
        glib::Propagation::Stop
    });
    add_clickable_row(grid, row, "Location", &link, &format!("Location: {text}"));
}

fn add_opens_with(grid: &gtk::Grid, row: i32, ui: &Rc<WindowUi>, fact: &Facts, scale: f64) {
    let default = gio::AppInfo::default_for_type(&fact.content_type, false);
    let row_box = value_box(scale);
    row_box.set_hexpand(true);
    if let Some(app) = &default {
        if let Some(icon) = app.icon() {
            let image = gtk::Image::from_gicon(&icon);
            image.add_css_class("gl-properties-icon");
            row_box.append(&image);
        }
        let name = gtk::Label::builder()
            .label(app.name().to_string())
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .hexpand(true)
            .build();
        name.add_css_class("gl-properties-value");
        row_box.append(&name);
    } else {
        let name = gtk::Label::builder()
            .label("No application")
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .hexpand(true)
            .build();
        name.add_css_class("gl-properties-value");
        row_box.append(&name);
    }

    let change = gtk::Button::with_label("Change…");
    change.add_css_class("gl-properties-button");
    change.set_valign(gtk::Align::Center);
    change.set_tooltip_text(Some("Choose the application that opens this file"));
    change.update_property(&[gtk::accessible::Property::Label(
        "Change the application that opens this file",
    )]);
    let ui_change = ui.clone();
    let path = fact.path.clone();
    change.connect_clicked(move |_| {
        crate::app::openwith::show_chooser(&ui_change, vec![path.clone()]);
    });
    row_box.append(&change);

    let accessible = default
        .map(|app| format!("Opens with: {}", app.name()))
        .unwrap_or_else(|| "Opens with: no application".to_string());
    add_clickable_row(grid, row, "Opens with", &row_box, &accessible);
}

fn add_disk(grid: &gtk::Grid, row: i32, ui: &Rc<WindowUi>, fact: &Facts, scope: Scope, scale: f64) {
    let row_box = value_box(scale);
    row_box.set_hexpand(true);
    let info = gfile(&fact.path, scope)
        .query_filesystem_info(SIZE_ATTRIBUTES, None::<&gio::Cancellable>)
        .ok();
    let text = match info {
        Some(info) => {
            let free = info.attribute_uint64("filesystem::free");
            let total = info.attribute_uint64("filesystem::size");
            format!(
                "{} free of {}",
                fm_core::preview::format_size(free),
                fm_core::preview::format_size(total)
            )
        }
        None => properties::UNKNOWN.to_string(),
    };
    let value = gtk::Label::builder()
        .label(&text)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .hexpand(true)
        .build();
    value.add_css_class("gl-properties-value");
    row_box.append(&value);

    if fact.is_dir {
        let baobab =
            crate::fileops::menu::find_on_path("baobab", std::env::var_os("PATH").as_deref())
                .is_some();
        let usage = gtk::Button::from_icon_name("drive-harddisk-symbolic");
        usage.add_css_class("gl-icon-button");
        usage.set_valign(gtk::Align::Center);
        usage.set_sensitive(baobab);
        usage.set_tooltip_text(Some(if baobab {
            "Disk Usage"
        } else {
            crate::fileops::menu::BAOBAB_MISSING
        }));
        usage.update_property(&[gtk::accessible::Property::Label("Disk Usage")]);
        let ui_usage = ui.clone();
        let folder = fact.path.to_string_lossy().into_owned();
        usage.connect_clicked(move |_| {
            ui_usage.dispatch(
                fm_core::command::Caller::Ui,
                fm_core::command::Invocation::new(
                    "app.disk-usage",
                    vec![("path".to_string(), folder.clone())],
                ),
            );
        });
        row_box.append(&usage);
    }

    add_clickable_row(grid, row, "Disk", &row_box, &format!("Disk: {text}"));
}

fn add_multi_size(
    grid: &gtk::Grid,
    row: &mut i32,
    facts: &[Facts],
    scope: Scope,
    live: Rc<Cell<bool>>,
) {
    let file_bytes: u64 = facts
        .iter()
        .filter(|fact| !fact.is_dir)
        .map(|fact| fact.size)
        .sum();
    let folder_count = facts.iter().filter(|fact| fact.is_dir).count();
    let initial = if folder_count > 0 {
        properties::CALCULATING.to_string()
    } else {
        fm_core::preview::format_size(file_bytes)
    };
    let label = add_row(grid, *row, "Size", &initial);
    *row += 1;
    if folder_count == 0 {
        return;
    }

    let state = Rc::new(RefCell::new(MultiSizeState {
        total: file_bytes,
        pending: folder_count,
        unreadable: false,
    }));
    for fact in facts.iter().filter(|fact| fact.is_dir) {
        let state = state.clone();
        let label = label.clone();
        let live = live.clone();
        start_folder_size(
            fact.path.clone(),
            scope,
            live,
            Rc::new(move |result| {
                let mut state = state.borrow_mut();
                match result {
                    ScanResult::Sized(size) => {
                        state.total = state.total.saturating_add(size.totals.contents_bytes);
                        state.unreadable |=
                            properties::has_unreadable_entries(&size.totals.partial);
                    }
                    ScanResult::Unavailable(RootState::PermissionDenied)
                    | ScanResult::Unavailable(RootState::Unavailable) => {
                        state.unreadable = true;
                    }
                    ScanResult::Unavailable(_) | ScanResult::Cancelled => {}
                }
                state.pending = state.pending.saturating_sub(1);
                if state.pending == 0 {
                    let mut text = fm_core::preview::format_size(state.total);
                    if state.unreadable {
                        text.push('\n');
                        text.push_str(properties::UNREADABLE_NOTE);
                    }
                    set_value(&label, "Size", &text);
                }
            }),
        );
    }
}

struct MultiSizeState {
    total: u64,
    pending: usize,
    unreadable: bool,
}

/// Run the existing folder-size walk for one Properties window. The walker is
/// the same `scan_with_root`; the administrator scope only swaps the GIO
/// source so a root-only tree is read through GVfs.
fn start_folder_size(
    path: PathBuf,
    scope: Scope,
    live: Rc<Cell<bool>>,
    apply: Rc<dyn Fn(ScanResult)>,
) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("properties-size".to_string())
        .spawn(move || {
            let result = size_folder(&path, scope);
            let _ = sender.send(result);
        });

    glib::timeout_add_local(Duration::from_millis(100), move || {
        if !live.get() {
            return glib::ControlFlow::Break;
        }
        match receiver.try_recv() {
            Ok(result) => {
                apply(result);
                glib::ControlFlow::Break
            }
            Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn size_folder(path: &Path, scope: Scope) -> ScanResult {
    let (probe, source): (RootProbe, Box<dyn ScanSource>) = match scope {
        Scope::User => (probe_native(path), Box::new(NativeSource)),
        Scope::Admin => (admin_probe(path), Box::new(AdminSource)),
    };
    let Some(identity) = probe.identity.clone() else {
        return ScanResult::Unavailable(probe.state);
    };
    scan_with_root(
        &identity,
        &probe,
        ScanPolicy::default(),
        source.as_ref(),
        &SystemClock,
        &|| false,
        &mut |_| {},
    )
}

/// The GIO source for a folder in an administrator tab. The walk itself is
/// still `fm_core::folder_size::scan_with_root`; only the filesystem boundary is
/// different.
struct AdminSource;

impl ScanSource for AdminSource {
    fn probe(&self, path: &Path) -> RootProbe {
        admin_probe(path)
    }

    fn lstat(&self, path: &Path) -> io::Result<EntryMeta> {
        admin_info(path).map(|info| entry_meta(&info))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Box<dyn DirReader + '_>> {
        let file = gfile(path, Scope::Admin);
        let enumerator = file
            .enumerate_children(
                LIST_ATTRIBUTES,
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                None::<&gio::Cancellable>,
            )
            .map_err(io_error)?;
        Ok(Box::new(AdminDir {
            parent: path.to_path_buf(),
            enumerator,
        }))
    }
}

struct AdminDir {
    parent: PathBuf,
    enumerator: gio::FileEnumerator,
}

impl DirReader for AdminDir {
    fn next_entry(&mut self) -> Option<io::Result<PathBuf>> {
        match self.enumerator.next() {
            Some(Ok(info)) => Some(Ok(self.parent.join(info.name()))),
            Some(Err(error)) => Some(Err(io_error(error))),
            None => None,
        }
    }
}

fn admin_probe(path: &Path) -> RootProbe {
    let info = match admin_info(path) {
        Ok(info) => info,
        Err(error) => {
            let state = match error.kind() {
                io::ErrorKind::NotFound => RootState::Missing,
                io::ErrorKind::PermissionDenied => RootState::PermissionDenied,
                _ => RootState::Unavailable,
            };
            return RootProbe::unavailable(state);
        }
    };
    let meta = entry_meta(&info);
    let identity = RootIdentity {
        path: path.to_path_buf(),
        device: meta.device,
        inode: meta.inode,
        modified_sec: meta.modified_sec,
        modified_nsec: meta.modified_nsec,
    };
    let state = match meta.kind {
        EntryKind::Directory => RootState::Ready,
        EntryKind::Symlink => RootState::Symlink,
        EntryKind::File | EntryKind::Other => RootState::Unavailable,
    };
    RootProbe {
        state,
        identity: Some(identity),
        meta: Some(meta),
    }
}

fn admin_info(path: &Path) -> Result<gio::FileInfo, io::Error> {
    gfile(path, Scope::Admin)
        .query_info(
            LIST_ATTRIBUTES,
            gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
            None::<&gio::Cancellable>,
        )
        .map_err(io_error)
}

fn entry_meta(info: &gio::FileInfo) -> EntryMeta {
    let kind = match info.file_type() {
        gio::FileType::Regular => EntryKind::File,
        gio::FileType::Directory => EntryKind::Directory,
        gio::FileType::SymbolicLink => EntryKind::Symlink,
        _ => EntryKind::Other,
    };
    let usec = i64::from(info.attribute_uint32("time::modified-usec"));
    EntryMeta {
        device: u64::from(info.attribute_uint32("unix::device")),
        inode: info.attribute_uint64("unix::inode"),
        size: u64::try_from(info.size()).unwrap_or(0),
        blocks: info.attribute_uint64("unix::blocks"),
        nlink: u64::from(info.attribute_uint32("unix::nlink")),
        kind,
        modified_sec: i64::try_from(info.attribute_uint64("time::modified")).unwrap_or(0),
        modified_nsec: usec.saturating_mul(1_000),
    }
}

fn io_error(error: glib::Error) -> io::Error {
    let message = error.message().to_string();
    let kind = if message.contains("No such file") || message.contains("not found") {
        io::ErrorKind::NotFound
    } else if message.contains("Permission denied") || message.contains("permission") {
        io::ErrorKind::PermissionDenied
    } else {
        io::ErrorKind::Other
    };
    io::Error::new(kind, message)
}
