// SPDX-License-Identifier: GPL-3.0-or-later
//! The Open dialog itself.
//!
//! A picker is an ordinary model window in the workspace — one group, one tab,
//! one folder — with a small view of its own instead of `WindowUi`: a nav row,
//! the main app's `Sidebar` and `FolderView`, and two buttons. The model gives
//! it navigation, history, selection and sorting for free; the registry entry
//! below is how the rest of the app knows the window is read-only.
//!
//! The portal service ([`super::service`]) hands each call a [`Request`]. The
//! picker answers it exactly once: accept, cancel, Close from the portal, or a
//! window destroyed some other way.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::gio;
use gtk::glib;

use fm_core::command::{Caller, CheckedCommand, Invocation, Outcome};
use fm_core::config::{Change, SortRule};
use fm_core::workspace::{Window, WindowId};

use super::service::{self, Call, DialogHost, Request};
use crate::app::Process;
use crate::folder::{FolderView, StatusLine};
use crate::look::blur::Blur;
use crate::look::tokens::*;
use crate::sidebar::Sidebar;

thread_local! {
    /// The pickers in this process, keyed by their model window. Pending
    /// entries are options waiting for `build_window` to reach them.
    static PICKERS: RefCell<BTreeMap<WindowId, Entry>> = const { RefCell::new(BTreeMap::new()) };
}

enum Entry {
    Pending(PickerOptions),
    Live(Rc<Picker>),
}

/// What `build_window` needs to construct one picker: the call and the request
/// that must be answered.
pub struct PickerOptions {
    pub request: Rc<Request>,
    pub call: Call,
    /// The application hold taken for this dialog. It is released when the
    /// request is answered (or when the options are dropped, if the window was
    /// never built), never before: the process must outlive its dialog.
    pub hold: gio::ApplicationHoldGuard,
}

/// Park a picker's options for the model window `id`, before `apply_events`
/// builds it.
pub(crate) fn set_pending(id: WindowId, options: PickerOptions) {
    PICKERS.with(|map| {
        map.borrow_mut().insert(id, Entry::Pending(options));
    });
}

/// Take the parked options exactly once, so a rebuild cannot reuse them.
pub(crate) fn take_pending(id: WindowId) -> Option<PickerOptions> {
    PICKERS.with(|map| {
        let mut map = map.borrow_mut();
        if !matches!(map.get(&id), Some(Entry::Pending(_))) {
            return None;
        }
        match map.remove(&id) {
            Some(Entry::Pending(options)) => Some(options),
            _ => None,
        }
    })
}

/// Whether `id` is a live picker window.
pub(crate) fn is_live(id: WindowId) -> bool {
    PICKERS.with(|map| matches!(map.borrow().get(&id), Some(Entry::Live(_))))
}

/// The picker whose model window is `id`.
pub(crate) fn get(id: WindowId) -> Option<Rc<Picker>> {
    PICKERS.with(|map| match map.borrow().get(&id) {
        Some(Entry::Live(picker)) => Some(picker.clone()),
        _ => None,
    })
}

/// Drop one picker and close its window. Returns whether `id` was a picker.
pub(crate) fn forget(id: WindowId) -> bool {
    let picker = PICKERS.with(|map| map.borrow_mut().remove(&id));
    match picker {
        Some(Entry::Live(picker)) => {
            picker.close_now();
            true
        }
        Some(Entry::Pending(_)) | None => false,
    }
}

/// The names this picker's grid lists, for Select All.
pub(crate) fn listed_names(id: WindowId) -> Option<Vec<std::ffi::OsString>> {
    get(id).map(|picker| picker.folder.listed_names())
}

/// Follow a model change: back/forward/up, the address, the grid and Open.
pub(crate) fn refresh(process: &Rc<Process>, id: WindowId) {
    let (Some(picker), Some(window)) = (get(id), process.window_snapshot(id)) else {
        return;
    };
    picker.show(process, &window);
}

/// A double-click or Enter: a folder is entered, a file is accepted.
pub(crate) fn open_path(process: &Rc<Process>, id: WindowId, path: PathBuf) {
    if path.is_dir() {
        process.nav_go_path(id, path);
        return;
    }
    if path.is_file()
        && let Some(picker) = get(id)
    {
        picker.accept_path(&path);
    }
}

/// Build one picker for a model window that already exists.
pub(crate) fn build(
    process: &Rc<Process>,
    application: &adw::Application,
    id: WindowId,
    options: PickerOptions,
) -> Rc<Picker> {
    let picker = Picker::new(process, application, id, options);
    PICKERS.with(|map| {
        map.borrow_mut().insert(id, Entry::Live(picker.clone()));
    });
    picker.present();
    picker
}

/// Sort and the hidden-files switch, applied to this dialog's grid only. The
/// picker never writes `config.toml`.
pub(crate) fn apply_setting(
    process: &Rc<Process>,
    id: WindowId,
    command: &CheckedCommand,
) -> Outcome {
    let Some(picker) = get(id) else {
        return Outcome::Failed(format!("{} has no picker window", command.name()));
    };
    match command.name() {
        "view.set-sort" => {
            let folder = process
                .window_snapshot(id)
                .and_then(|window| window.tab(window.focused_tab).map(|tab| tab.folder.clone()));
            let Some(folder) = folder else {
                return Outcome::Failed("the dialog has no folder".to_string());
            };
            let change = {
                let config = process.config.borrow();
                fm_core::config::change_for(command, config.config(), &folder)
            };
            match change {
                Some(Change::SetSort { by, order, .. }) => {
                    picker.folder.set_sort(SortRule {
                        by,
                        order,
                        folders_first: true,
                    });
                    crate::log!(
                        "portal",
                        "dialog sorted by {} {}",
                        by.as_str(),
                        order.as_str()
                    );
                    Outcome::Done
                }
                _ => Outcome::Failed("view.set-sort has no sort rule".to_string()),
            }
        }
        "view.clear-folder-sort" => {
            let rule = process.config.borrow().config().sort.global();
            picker.folder.set_sort(rule);
            Outcome::Done
        }
        "view.set-hidden" => {
            let Some(fm_core::command::Value::Bool(show)) = command.get("enabled") else {
                return Outcome::Failed("view.set-hidden needs a boolean".to_string());
            };
            picker.set_hidden(process, *show);
            Outcome::Done
        }
        "view.toggle-hidden" => {
            let next = !picker.hidden.get();
            picker.set_hidden(process, next);
            Outcome::Done
        }
        _ => Outcome::Failed(format!(
            "{} is not available in the file chooser",
            command.name()
        )),
    }
}

pub(crate) struct Picker {
    id: WindowId,
    process: Weak<Process>,
    window: adw::ApplicationWindow,
    /// The window's nav row and the address entry.
    path: gtk::Box,
    path_entry: gtk::Entry,
    back: gtk::Button,
    forward: gtk::Button,
    up: gtk::Button,
    sidebar: Rc<Sidebar>,
    folder: Rc<FolderView>,
    status: Rc<StatusLine>,
    open: gtk::Button,
    /// The folder and tab the grid is showing, so a refresh only reloads when
    /// the model's folder actually changed.
    displayed: RefCell<Option<(fm_core::workspace::TabId, PathBuf)>>,
    /// This dialog's hidden-files switch, independent of `config.toml`.
    hidden: Cell<bool>,
    request: RefCell<Option<Rc<Request>>>,
    blur: RefCell<Option<Blur>>,
}

impl Picker {
    fn new(
        process: &Rc<Process>,
        application: &adw::Application,
        id: WindowId,
        options: PickerOptions,
    ) -> Rc<Self> {
        let scale = process.scale();
        let opacity = process.opacity();
        let PickerOptions {
            request,
            call,
            hold,
        } = options;

        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title(call.title())
            .build();
        window.set_default_size(
            scaled(f64::from(PICKER_W), scale),
            scaled(f64::from(PICKER_H), scale),
        );
        window.add_css_class("gimbal-look");
        // The dialog is modal to the calling application through the portal's
        // parent window; Gimbal's own process has nothing else to keep usable.

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.set_content(Some(&root));

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.add_css_class("gl-header");
        row.set_hexpand(true);
        let back = icon_button("go-previous-symbolic");
        let forward = icon_button("go-next-symbolic");
        let up = icon_button("go-up-symbolic");
        row.append(&back);
        row.append(&forward);
        row.append(&up);

        let path = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        path.add_css_class("gl-field");
        path.add_css_class("gl-path");
        path.set_valign(gtk::Align::Center);
        path.set_hexpand(true);
        let path_entry = gtk::Entry::new();
        gtk::prelude::EntryExt::set_alignment(&path_entry, 0.0);
        path_entry.set_hexpand(true);
        path_entry.set_placeholder_text(Some("Path"));
        path.append(&path_entry);
        let refresh = icon_button("view-refresh-symbolic");
        path.append(&refresh);
        row.append(&path);

        let close = crate::look::icons::button(crate::look::icons::MockupIcon::Close);
        close.add_css_class("winbtn");
        close.add_css_class("close");
        close.set_valign(gtk::Align::Center);
        row.append(&close);

        let handle = gtk::WindowHandle::builder().child(&row).build();
        root.append(&handle);

        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add_css_class("gl-frame");
        frame.set_vexpand(true);
        root.append(&frame);

        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        frame.append(&paned);
        paned.set_resize_start_child(false);
        paned.set_resize_end_child(true);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(true);

        let sidebar = Sidebar::new(process, id, scale);
        paned.set_start_child(Some(sidebar.widget()));
        // The main window's own sidebar width, so the dialog looks like it.
        let left_width = process.config.borrow().config().sidebars.left.width;
        paned.set_position(scaled(f64::from(left_width), scale));

        let status = StatusLine::new();
        let folder = FolderView::new(process, id, status.clone(), scale, None, None);
        let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        right.set_vexpand(true);
        right.append(folder.widget());
        right.append(status.widget());
        paned.set_end_child(Some(&right));

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        footer.add_css_class("gl-action-row");
        footer.set_halign(gtk::Align::End);
        footer.set_spacing(scaled(GAP_S, scale));
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);
        let cancel = action_button("Cancel", false);
        let open = action_button(call.accept_label(), true);
        footer.append(&cancel);
        footer.append(&open);
        root.append(&footer);

        let picker = Rc::new(Self {
            id,
            process: Rc::downgrade(process),
            window: window.clone(),
            path,
            path_entry: path_entry.clone(),
            back: back.clone(),
            forward: forward.clone(),
            up: up.clone(),
            sidebar,
            folder: folder.clone(),
            status,
            open: open.clone(),
            displayed: RefCell::new(None),
            hidden: Cell::new(process.show_hidden()),
            request: RefCell::new(Some(request.clone())),
            blur: RefCell::new(None),
        });

        // The request's completion closes this window and releases the
        // application hold. Installed before Present so a Close from the
        // portal in the same tick still finds it.
        let weak_process = Rc::downgrade(process);
        request.set_on_done(move || {
            // Dropping the guard is the release (`gio`'s hold is RAII).
            drop(hold);
            if let Some(process) = weak_process.upgrade() {
                process.close_picker_window(id);
            }
        });

        // Every path out of the dialog answers once.
        window.connect_close_request({
            let weak = Rc::downgrade(&picker);
            move |_| {
                if let Some(picker) = weak.upgrade() {
                    picker.cancel();
                }
                glib::Propagation::Stop
            }
        });
        window.connect_destroy({
            let weak = Rc::downgrade(&picker);
            move |_| {
                // A destroy from any other route (the model closing the
                // window, the app shutting down) still answers the call.
                if let Some(picker) = weak.upgrade() {
                    picker.cancel();
                }
            }
        });

        connect_nav(&picker, &back, "nav.back");
        connect_nav(&picker, &forward, "nav.forward");
        connect_nav(&picker, &up, "nav.up");
        refresh.connect_clicked({
            let picker = Rc::downgrade(&picker);
            move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.dispatch(Invocation::new("nav.refresh", Vec::new()));
                }
            }
        });
        close.connect_clicked({
            let picker = Rc::downgrade(&picker);
            move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.cancel();
                }
            }
        });
        path_entry.connect_activate({
            let picker = Rc::downgrade(&picker);
            move |entry| {
                if let Some(picker) = picker.upgrade() {
                    picker.path_entered(entry);
                }
            }
        });
        cancel.connect_clicked({
            let picker = Rc::downgrade(&picker);
            move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.cancel();
                }
            }
        });
        open.connect_clicked({
            let picker = Rc::downgrade(&picker);
            move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.accept();
                }
            }
        });

        // Escape cancels whatever has focus; the navigation shortcuts work
        // whenever the address entry does not (so Ctrl+A there still selects
        // the typed text). The main app's configurable hotkeys are not
        // installed: no file-effect key exists in the dialog.
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&picker);
        key.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(picker) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if key == gtk::gdk::Key::Escape {
                picker.cancel();
                return glib::Propagation::Stop;
            }
            if picker.path_entry.has_focus() {
                return glib::Propagation::Proceed;
            }
            match picker_shortcut(key, modifiers) {
                Some(name) => {
                    picker.dispatch(Invocation::new(name, Vec::new()));
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        window.add_controller(key);

        *picker.blur.borrow_mut() = Some(Blur::attach(
            window.upcast_ref::<gtk::Window>(),
            scale,
            opacity,
        ));

        // Draw the first state now; the model's WindowOpened event already
        // ran `refresh` before this view existed.
        if let Some(window) = process.window_snapshot(id) {
            picker.show(process, &window);
        }
        picker
    }

    fn present(&self) {
        self.window.present();
        // Focus the grid, not the address entry: the dialog opens ready to
        // receive arrow keys and Enter.
        self.folder.grab_grid_focus();
    }

    /// Destroy the GTK window. The request is answered by `destroy` if nothing
    /// has answered it yet.
    fn close_now(&self) {
        self.window.destroy();
    }

    fn dispatch(&self, invocation: Invocation) {
        if let Some(process) = self.process.upgrade() {
            process.dispatch_for_window(self.id, Caller::Ui, invocation);
        }
    }

    /// Follow the model: history buttons, the address, the grid, Open.
    fn show(&self, process: &Rc<Process>, window: &Window) {
        let view = crate::header::view(window);
        self.back.set_sensitive(view.can_back);
        self.forward.set_sensitive(view.can_forward);
        self.up.set_sensitive(view.can_up);
        if !self.path_entry.has_focus() {
            let text = view.folder.display().to_string();
            if self.path_entry.text() != text {
                // A navigation the user asked for wins over a typed path that
                // did not exist.
                self.path.remove_css_class("invalid");
                self.path_entry.set_text(&text);
            }
        }
        let Some(tab) = window.tab(window.focused_tab) else {
            return;
        };
        let changed = self
            .displayed
            .borrow()
            .as_ref()
            .is_none_or(|(tab_id, folder)| *tab_id != tab.id || *folder != tab.folder);
        if changed {
            // A local sort survives while the folder stays; a new folder starts
            // from its own configured rule.
            let rule: SortRule = process.config.borrow().config().sort_for(&tab.folder);
            self.folder
                .set_folder(&tab.folder, rule, tab.scroll_anchor.clone(), true, false);
            *self.displayed.borrow_mut() = Some((tab.id, tab.folder.clone()));
        }
        self.folder.set_selection(&tab.selection);
        self.open.set_sensitive(selected_file(window).is_some());
        self.sidebar.set_current_folder(Some(&view.folder));
    }

    /// The hidden-files switch in this dialog only. `Process::apply_show_hidden`
    /// would flip every grid in the process, so a second open dialog would
    /// change too (Claude's review): this touches only this dialog's grid and
    /// selection.
    fn set_hidden(&self, process: &Rc<Process>, show: bool) {
        if self.hidden.replace(show) == show {
            return;
        }
        if !show {
            // Nothing invisible may stay selected: Open would answer with a
            // file the user cannot see.
            let hidden_selected = process.window_snapshot(self.id).is_some_and(|window| {
                window.tab(window.focused_tab).is_some_and(|tab| {
                    tab.selection
                        .names
                        .iter()
                        .any(|name| crate::folder::is_hidden_name(Path::new(name)))
                })
            });
            if hidden_selected {
                process.selection_none(self.id);
            }
        }
        self.folder.set_show_hidden(show);
        crate::log!(
            "portal",
            "dialog {} hidden files",
            if show { "shows" } else { "hides" }
        );
    }

    pub(crate) fn reload(&self) {
        self.folder.reload();
    }

    pub(crate) fn focus_path(&self) {
        self.path.remove_css_class("invalid");
        self.path_entry.grab_focus();
        self.path_entry.select_region(0, -1);
    }

    /// The address bar's Enter. A path to a file is accepted; a directory is
    /// navigated to; anything else marks the field invalid.
    fn path_entered(&self, entry: &gtk::Entry) {
        let Some(process) = self.process.upgrade() else {
            return;
        };
        let text = entry.text().to_string();
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let home = process.home();
        let mut target = expand_home(text, &home);
        if !target.is_absolute() {
            let base = process
                .window_snapshot(self.id)
                .and_then(|window| window.tab(window.focused_tab).map(|tab| tab.folder.clone()))
                .unwrap_or_else(|| home.clone());
            target = base.join(target);
        }
        if !target.exists() {
            self.path.add_css_class("invalid");
            crate::log!("portal", "{} does not exist", target.display());
            return;
        }
        self.path.remove_css_class("invalid");
        if target.is_file() {
            self.accept_path(&target);
            return;
        }
        self.dispatch(Invocation::new(
            "nav.go",
            vec![("path".to_string(), target.to_string_lossy().into_owned())],
        ));
        self.path_entry.set_text(&target.display().to_string());
    }

    /// Answer the call with the one selected file, if there is one.
    pub(crate) fn accept(&self) -> Outcome {
        let Some(process) = self.process.upgrade() else {
            return Outcome::Failed("the file chooser has no process".to_string());
        };
        let Some(window) = process.window_snapshot(self.id) else {
            return Outcome::Failed("the file chooser has no window".to_string());
        };
        match selected_file(&window) {
            Some(path) => {
                self.accept_path(&path);
                Outcome::Done
            }
            None => {
                let message = "Select one file to open";
                self.status.show_summary(message);
                crate::log!("portal", "accept refused: {message}");
                Outcome::Failed(message.to_string())
            }
        }
    }

    fn accept_path(&self, path: &Path) {
        let Some(request) = self.request.borrow_mut().take() else {
            crate::log!("portal", "the dialog had no request to answer");
            return;
        };
        request.accept(path);
    }

    /// Answer "user cancelled", unless the call was already answered.
    pub(crate) fn cancel(&self) -> Outcome {
        let Some(request) = self.request.borrow_mut().take() else {
            // The window is still closing after its answer; the second path
            // (click then destroy, or Esc then close) must stay quiet.
            return Outcome::Done;
        };
        request.complete(service::RESPONSE_CANCELLED, service::empty_results());
        Outcome::Done
    }
}

/// The one selected file, or `None` when the selection is empty, plural, or
/// names something that is not a regular file.
fn selected_file(window: &Window) -> Option<PathBuf> {
    let tab = window.tab(window.focused_tab)?;
    if tab.selection.names.len() != 1 {
        return None;
    }
    let name = tab.selection.names.iter().next()?;
    let path = tab.folder.join(name);
    path.is_file().then_some(path)
}

fn connect_nav(picker: &Rc<Picker>, button: &gtk::Button, name: &'static str) {
    let weak = Rc::downgrade(picker);
    button.connect_clicked(move |_| {
        if let Some(picker) = weak.upgrade()
            && let Some(process) = picker.process.upgrade()
        {
            process.dispatch_for_window(picker.id, Caller::Ui, Invocation::new(name, Vec::new()));
        }
    });
}

/// The dialog's own shortcuts: the navigation half of the main app's defaults
/// plus Ctrl+H. Every one is in the picker allowlist, so a key can never reach
/// a file effect.
fn picker_shortcut(key: gtk::gdk::Key, modifiers: gtk::gdk::ModifierType) -> Option<&'static str> {
    let control = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(gtk::gdk::ModifierType::ALT_MASK);
    let name = match key {
        gtk::gdk::Key::a if control => "selection.all",
        gtk::gdk::Key::h if control => "view.toggle-hidden",
        gtk::gdk::Key::l if control => "nav.focus-path",
        gtk::gdk::Key::F5 => "nav.refresh",
        gtk::gdk::Key::Left if alt => "nav.back",
        gtk::gdk::Key::Right if alt => "nav.forward",
        gtk::gdk::Key::Up if alt => "nav.up",
        _ => return None,
    };
    Some(name)
}

fn icon_button(icon_name: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("gl-icon-button");
    button.add_css_class("nav");
    button.set_child(Some(&gtk::Image::from_icon_name(icon_name)));
    button
}

fn action_button(text: &str, suggested: bool) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("gl-menu-item");
    if suggested {
        button.add_css_class("suggested-action");
    }
    let label = gtk::Label::new(Some(text));
    label.set_use_underline(true);
    button.set_child(Some(&label));
    button
}

fn expand_home(text: &str, home: &Path) -> PathBuf {
    if text == "~" {
        home.to_path_buf()
    } else if let Some(rest) = text.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(text)
    }
}

/// The picker process's dialog host. Created once per process.
pub struct PickerHost {
    process: Rc<Process>,
    application: adw::Application,
}

impl PickerHost {
    pub fn new(process: &Rc<Process>, application: &adw::Application) -> Rc<Self> {
        Rc::new(Self {
            process: process.clone(),
            application: application.clone(),
        })
    }
}

impl DialogHost for PickerHost {
    fn open(&self, request: Rc<Request>, call: &Call) -> Result<(), String> {
        let home = self.process.home();
        let folder = service::starting_folder(call.options.current_folder.as_deref(), &home);
        crate::log!(
            "portal",
            "opening the dialog at {} for {:?}",
            folder.display(),
            call.app_id
        );
        let options = PickerOptions {
            request: request.clone(),
            call: call.clone(),
            hold: self.application.hold(),
        };
        // A refused window drops the options, which releases the hold.
        match self.process.open_picker_window(folder, options) {
            Some(_id) => Ok(()),
            None => Err("the model refused a new window".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::workspace::Workspace;
    use std::ffi::OsString;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "gimbal-picker-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|time| time.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&path).expect("a scratch folder");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// One model window at `folder`, with nothing selected. Only the model is
    /// built: these tests never construct a GTK window.
    fn model_at(folder: &Path) -> (Workspace, Window) {
        let mut workspace = Workspace::with_home(PathBuf::from("/home/kyle"));
        let events = workspace
            .window_new(Some(folder.to_path_buf()))
            .expect("a window");
        let id = events
            .iter()
            .find_map(|event| match event {
                fm_core::workspace::Event::WindowOpened(id) => Some(*id),
                _ => None,
            })
            .expect("a WindowOpened event");
        let window = workspace
            .windows
            .iter()
            .find(|window| window.id == id)
            .cloned()
            .expect("the window");
        (workspace, window)
    }

    #[test]
    fn one_selected_file_is_the_answer() {
        let scratch = Scratch::new("one");
        let folder = &scratch.0;
        std::fs::write(folder.join("photo.png"), b"x").expect("file");
        let (mut workspace, _) = model_at(folder);
        workspace
            .file_click(OsString::from("photo.png"))
            .expect("select");
        let window = &workspace.windows[0];
        assert_eq!(selected_file(window), Some(folder.join("photo.png")));
    }

    #[test]
    fn two_files_a_folder_and_a_vanished_name_are_not_an_answer() {
        let scratch = Scratch::new("many");
        let folder = &scratch.0;
        std::fs::write(folder.join("a.txt"), b"x").expect("file");
        std::fs::write(folder.join("b.txt"), b"x").expect("file");
        std::fs::create_dir_all(folder.join("sub")).expect("directory");

        let (mut workspace, _) = model_at(folder);

        // A folder is not a file to open.
        workspace.file_click(OsString::from("sub")).expect("select");
        assert_eq!(selected_file(&workspace.windows[0]), None);

        // Two files are not one file.
        workspace
            .file_click(OsString::from("a.txt"))
            .expect("select");
        workspace
            .file_ctrl_click(OsString::from("b.txt"))
            .expect("extend");
        assert_eq!(selected_file(&workspace.windows[0]), None);

        // The listed name is gone by the time Open is pressed: choosing it
        // would answer with a URI that no longer exists.
        workspace
            .file_click(OsString::from("b.txt"))
            .expect("select");
        std::fs::remove_file(folder.join("b.txt")).expect("remove");
        assert_eq!(selected_file(&workspace.windows[0]), None);
    }

    #[test]
    fn no_selection_is_not_an_answer() {
        let scratch = Scratch::new("none");
        let (_, window) = model_at(&scratch.0);
        assert_eq!(selected_file(&window), None);
    }

    #[test]
    fn every_shortcut_is_an_allowlisted_command_and_no_file_key_is_bound() {
        use gtk::gdk::{Key, ModifierType};

        let none = ModifierType::empty();
        let control = ModifierType::CONTROL_MASK;
        let alt = ModifierType::ALT_MASK;
        for (key, modifiers, expected) in [
            (Key::a, control, "selection.all"),
            (Key::h, control, "view.toggle-hidden"),
            (Key::l, control, "nav.focus-path"),
            (Key::F5, none, "nav.refresh"),
            (Key::Left, alt, "nav.back"),
            (Key::Right, alt, "nav.forward"),
            (Key::Up, alt, "nav.up"),
        ] {
            assert_eq!(picker_shortcut(key, modifiers), Some(expected));
            assert!(
                crate::app::picker_accepts(expected),
                "{expected} is bound but not allowed"
            );
        }

        // The main app's file keys are not bound at all in the dialog.
        for (key, modifiers) in [
            (Key::Delete, none),
            (Key::v, control),
            (Key::c, control),
            (Key::x, control),
            (Key::n, control),
            (Key::t, control),
            (Key::w, control),
        ] {
            assert_eq!(picker_shortcut(key, modifiers), None);
        }
        // A modifier-less letter is not a shortcut either.
        assert_eq!(picker_shortcut(Key::a, none), None);
        assert_eq!(picker_shortcut(Key::F5, control), Some("nav.refresh"));
    }

    #[test]
    fn the_address_field_expands_home_and_keeps_other_paths() {
        let home = Path::new("/home/kyle");
        assert_eq!(expand_home("~", home), home);
        assert_eq!(expand_home("~/Pictures", home), home.join("Pictures"));
        assert_eq!(expand_home("/srv/data", home), PathBuf::from("/srv/data"));
        assert_eq!(expand_home("relative", home), PathBuf::from("relative"));
    }
}
