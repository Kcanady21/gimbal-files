// SPDX-License-Identifier: GPL-3.0-or-later
//! The Trash page: one expanding widget in the central content stack.
//!
//! The page lists the GIO `trash:///` namespace in a virtualised `ListView`,
//! keeps selection by validated URI, and sends `trash.restore`, `trash.empty`
//! or the selected `trash.delete-permanently` through the window's command
//! path. Listing happens on a short-lived thread and a `GFileMonitor` refreshes
//! the page when the backend changes. The page is transient UI state: it is not
//! a tab, never enters the workspace model, and shows the unchanged folder or
//! split page again when the user chooses Back to Files or another tab.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use adw::prelude::*;
use fm_core::command::{Caller, Invocation};
use fm_core::fileops::{Plan, Request, TrashIdentity, TrashItem};
use gtk::gio;
use gtk::glib;
use gtk::pango;

use crate::fileops::ProgressView;
use crate::frame::WindowUi;
use crate::look::icons::{self, MockupIcon};

const POLL_MS: u64 = 50;
const REFRESH_DELAY_MS: u32 = 150;

type ListResult = (u64, Result<Vec<TrashItem>, String>);

/// What repeated `trash.open` does. One window owns at most one page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAction {
    CreatePage,
    ShowExisting,
}

pub fn open_action(existing: bool) -> OpenAction {
    if existing {
        OpenAction::ShowExisting
    } else {
        OpenAction::CreatePage
    }
}

/// Whether nav.back walks the folder history or only returns from Trash.
/// Returning from Trash is not a folder-history step, so the hidden folder's
/// back list must be left exactly as it was.
pub fn back_consumes_history(trash_active: bool) -> bool {
    !trash_active
}

/// Why the content changed. A background listing or job refresh keeps the
/// Trash page; only an explicit navigation leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshCause {
    Background,
    Navigation,
}

pub fn keeps_trash_page(trash_active: bool, cause: RefreshCause) -> bool {
    trash_active && cause == RefreshCause::Background
}

pub fn leaves_trash_page(trash_active: bool, cause: RefreshCause) -> bool {
    trash_active && cause == RefreshCause::Navigation
}

/// What a command may do while the Trash page is the visible page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageRoute {
    /// Handle it normally; it is either allowed on Trash or unrelated.
    Files,
    SelectAll,
    SelectNone,
    RefreshTrash,
    /// nav.back returns to the prior folder or split page.
    BackToFiles,
    /// An explicit destination was requested; leave Trash, then navigate.
    LeaveTrash,
    /// Refuse with this visible-page status instead of touching hidden files.
    Refuse(&'static str),
}

/// Route the visible page, not the hidden folder selection.
pub fn route_on_trash_page(command: &str, trash_active: bool) -> PageRoute {
    if !trash_active {
        return PageRoute::Files;
    }
    match command {
        "selection.all" => PageRoute::SelectAll,
        "selection.none" => PageRoute::SelectNone,
        "nav.refresh" => PageRoute::RefreshTrash,
        "nav.back" => PageRoute::BackToFiles,
        "nav.go" | "tab.new" | "tab.focus" | "group.split" | "group.add-tab" => {
            PageRoute::LeaveTrash
        }
        "nav.up" => PageRoute::Refuse("Up is unavailable in Trash."),
        "nav.forward" => PageRoute::Refuse("Forward is unavailable in Trash."),
        "nav.focus-path" => PageRoute::Refuse("The path bar is unavailable in Trash."),
        "file.open" => PageRoute::Refuse("Files cannot be opened from Trash."),
        // Delete in Trash never permanently deletes. Empty Trash is the one
        // explicitly confirmed exception, and it is not this command.
        "file.trash" => PageRoute::Refuse("Items are already in Trash"),
        "file.cut"
        | "file.copy-to-clipboard"
        | "file.copy"
        | "file.move"
        | "file.paste"
        | "file.rename"
        | "file.new-folder"
        | "file.new-file" => {
            PageRoute::Refuse("This file action is unavailable while Trash is shown.")
        }
        "search.open" => PageRoute::Refuse("Search is unavailable while Trash is shown."),
        "app.disk-usage" => PageRoute::Refuse("Disk usage is unavailable while Trash is shown."),
        // Restore and Empty Trash carry explicit Trash identities. Undo is the
        // established global worker operation and stays available.
        "trash.open"
        | "trash.restore"
        | "trash.empty"
        | "trash.delete-permanently"
        | "file.undo" => PageRoute::Files,
        _ => PageRoute::Files,
    }
}

/// The operation a destructive confirmation belongs to. The kind is part of
/// the pending state, so an Empty Trash dialog can never authorise selected
/// deletion, or the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationKind {
    Empty,
    Selected,
}

impl ConfirmationKind {
    pub fn response(self) -> &'static str {
        match self {
            ConfirmationKind::Empty => "empty",
            ConfirmationKind::Selected => "delete",
        }
    }
}

/// A reply to a destructive Trash confirmation. The job id and operation kind
/// are part of the answer: a dialog for one job must never confirm or cancel
/// another window's job, or a different operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationAction {
    Confirm,
    Cancel,
    Ignore,
}

pub fn confirmation_action(
    pending: Option<(u64, ConfirmationKind)>,
    job: u64,
    kind: ConfirmationKind,
    response: Option<&str>,
) -> ConfirmationAction {
    if pending != Some((job, kind)) {
        return ConfirmationAction::Ignore;
    }
    match response {
        Some(response) if response == kind.response() => ConfirmationAction::Confirm,
        // Cancel, Escape and a close without a response all cancel this job.
        _ => ConfirmationAction::Cancel,
    }
}

/// Claim only the confirmation this reply belongs to. A stale callback must
/// not clear the pending state of a newer confirmation.
fn claim_confirmation(
    pending: &Cell<Option<(u64, ConfirmationKind)>>,
    job: u64,
    kind: ConfirmationKind,
    response: Option<&str>,
) -> ConfirmationAction {
    let action = confirmation_action(pending.get(), job, kind, response);
    if action != ConfirmationAction::Ignore {
        pending.set(None);
    }
    action
}

/// What the three controls may do in one browser state. Pure so the tests do
/// not need a display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlState {
    pub restore: bool,
    pub empty: bool,
    pub refresh: bool,
}

pub fn controls(
    loading: bool,
    unavailable: bool,
    total: usize,
    restorable: usize,
    confirming: bool,
    busy: bool,
) -> ControlState {
    let idle = !loading && !unavailable && !confirming && !busy;
    ControlState {
        restore: idle && restorable > 0,
        empty: idle && total > 0,
        refresh: !loading && !confirming && !busy,
    }
}

/// What the item menu may do for the current state. The destructive action is
/// available only for a valid selection and never while busy, confirming or
/// while the listing is unavailable. Missing original location or deletion
/// date is display metadata and does not disable deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemMenuState {
    pub restore: bool,
    pub delete: bool,
}

pub fn item_menu(
    selected: usize,
    restorable: usize,
    loading: bool,
    unavailable: bool,
    confirming: bool,
    busy: bool,
) -> ItemMenuState {
    let idle = !loading && !unavailable && !confirming && !busy;
    ItemMenuState {
        restore: idle && restorable > 0,
        delete: idle && selected > 0,
    }
}

/// A right-click inside the selection acts on the whole selected set; a
/// right-click on an unselected row acts on that row alone. Pure so the menu
/// scope is testable without a display.
pub fn menu_positions(selection: &[u32], clicked: u32, clicked_selected: bool) -> Vec<u32> {
    if clicked_selected {
        selection.to_vec()
    } else {
        vec![clicked]
    }
}

/// Freeze the identities at the click. Positions are read once; later
/// selection changes cannot alter the returned vector. Duplicate URIs collapse
/// to one item, matching the registry's deduplicated `items` set.
pub fn delete_snapshot(items: &[TrashItem], positions: &[u32]) -> Vec<TrashIdentity> {
    let mut selected = Vec::new();
    for position in positions {
        let Some(item) = items.get(*position as usize) else {
            continue;
        };
        let identity = item.identity().clone();
        if selected
            .iter()
            .any(|candidate: &TrashIdentity| candidate.uri() == identity.uri())
        {
            continue;
        }
        selected.push(identity);
    }
    selected
}

/// The destructive selected-deletion dialog's exact title and body.
pub fn delete_confirmation_text(count: usize, name: Option<&str>) -> (String, String) {
    let title = if count == 1 {
        "Permanently delete this item?".to_string()
    } else {
        format!("Permanently delete {count} items?")
    };
    let body = match (count, name) {
        (1, Some(name)) => {
            format!("{name}\nThese items will be permanently deleted. This cannot be undone.")
        }
        _ => "These items will be permanently deleted. This cannot be undone.".to_string(),
    };
    (title, body)
}

/// How many selected identities still name an item whose original location is
/// known and whose evidence still agrees with the fresh listing.
pub fn restorable_selection(selected: &[TrashIdentity], items: &[TrashItem]) -> usize {
    selected
        .iter()
        .filter(|identity| {
            items
                .iter()
                .any(|item| identity.agrees_with(item.identity()) && item.can_restore())
        })
        .count()
}

/// Positions in a fresh listing that keep an old identity. A stale selection
/// is dropped instead of acting on a later item at the same URI.
pub fn retained_positions(selected: &[TrashIdentity], items: &[TrashItem]) -> Vec<u32> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            selected
                .iter()
                .any(|identity| identity.agrees_with(item.identity()))
        })
        .map(|(index, _)| index as u32)
        .collect()
}

pub fn original_text(original: Option<&Path>) -> String {
    original
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}

pub fn date_text(deleted_at: Option<i64>) -> String {
    let Some(deleted_at) = deleted_at else {
        return "Unknown".to_string();
    };
    glib::DateTime::from_unix_local(deleted_at)
        .and_then(|date| date.format("%Y-%m-%d %H:%M"))
        .map(|text| text.to_string())
        .unwrap_or_else(|_| "Unknown".to_string())
}

pub struct TrashBrowser {
    ui: Weak<WindowUi>,
    root: gtk::Box,
    store: gio::ListStore,
    selection: gtk::MultiSelection,
    restore: gtk::Button,
    empty: gtk::Button,
    refresh_button: gtk::Button,
    scroller: gtk::ScrolledWindow,
    placeholder: gtk::Label,
    status: gtk::Label,
    details_button: gtk::Button,
    details: RefCell<Option<String>>,
    progress: gtk::Box,
    progress_label: gtk::Label,
    progress_bar: gtk::ProgressBar,
    cancel: gtk::Button,
    items: RefCell<Vec<TrashItem>>,
    selected: RefCell<Vec<TrashIdentity>>,
    loading: Cell<bool>,
    unavailable: Cell<bool>,
    confirming: Cell<bool>,
    busy: Cell<bool>,
    status_is_summary: Cell<bool>,
    pending_refresh: Cell<bool>,
    generation: Cell<u64>,
    /// The worker job and operation kind whose destructive confirmation this
    /// page owns, if any.
    pending: Cell<Option<(u64, ConfirmationKind)>>,
    /// Present only while that confirmation is on screen.
    confirmation: RefCell<Option<adw::AlertDialog>>,
    receiver: RefCell<Option<Receiver<ListResult>>>,
    poll: RefCell<Option<glib::SourceId>>,
    refresh_source: RefCell<Option<glib::SourceId>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
}

impl TrashBrowser {
    pub fn new(ui: &Rc<WindowUi>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("gl-trash-view");
        root.set_vexpand(true);
        root.set_hexpand(true);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        actions.add_css_class("gl-trash-actions");
        let title = gtk::Label::new(Some("Trash"));
        title.add_css_class("gl-trash-title");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        actions.append(&title);

        let back = gtk::Button::with_label("Back to Files");
        back.add_css_class("gl-trash-action");
        actions.append(&back);

        let restore = gtk::Button::with_label("Restore");
        restore.add_css_class("gl-trash-action");
        actions.append(&restore);

        let empty = gtk::Button::with_label("Empty Trash");
        empty.add_css_class("gl-trash-action");
        empty.add_css_class("destructive-action");
        actions.append(&empty);

        let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh.add_css_class("gl-icon-button");
        refresh.set_tooltip_text(Some("Refresh"));
        actions.append(&refresh);
        root.append(&actions);

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::MultiSelection::new(Some(store.clone()));
        let factory = gtk::SignalListItemFactory::new();
        let list = gtk::ListView::builder()
            .model(&selection)
            .factory(&factory)
            .show_separators(true)
            .build();
        list.add_css_class("gl-trash-list");
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_child(Some(&list));
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_propagate_natural_width(false);
        scroller.set_vexpand(true);
        scroller.set_hexpand(true);

        let placeholder = gtk::Label::new(Some("Loading Trash…"));
        placeholder.add_css_class("gl-trash-empty");
        placeholder.set_wrap(true);
        placeholder.set_justify(gtk::Justification::Center);
        placeholder.set_halign(gtk::Align::Center);
        placeholder.set_valign(gtk::Align::Center);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&scroller));
        overlay.add_overlay(&placeholder);
        overlay.set_vexpand(true);
        overlay.set_hexpand(true);
        root.append(&overlay);

        let status = gtk::Label::new(None);
        status.add_css_class("gl-trash-status");
        status.set_xalign(0.0);
        status.set_hexpand(true);
        status.set_ellipsize(pango::EllipsizeMode::End);
        let details_button = gtk::Button::with_label("Details");
        details_button.add_css_class("gl-trash-action");
        details_button.set_tooltip_text(Some("Show per-item results"));
        details_button.set_visible(false);
        let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        status_row.append(&status);
        status_row.append(&details_button);
        root.append(&status_row);

        let progress = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        progress.add_css_class("gl-progress");
        progress.add_css_class("gl-trash-progress");
        let progress_label = gtk::Label::new(None);
        progress_label.add_css_class("gl-progress-label");
        progress_label.set_hexpand(true);
        progress_label.set_xalign(0.0);
        progress_label.set_ellipsize(pango::EllipsizeMode::End);
        progress.append(&progress_label);
        let progress_bar = gtk::ProgressBar::new();
        progress_bar.add_css_class("gl-progress-bar");
        progress_bar.set_hexpand(true);
        progress_bar.set_valign(gtk::Align::Center);
        progress.append(&progress_bar);
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("gl-trash-action");
        progress.append(&cancel);
        progress.set_visible(false);
        root.append(&progress);

        let browser = Rc::new(Self {
            ui: Rc::downgrade(ui),
            root,
            store: store.clone(),
            selection: selection.clone(),
            restore: restore.clone(),
            empty: empty.clone(),
            refresh_button: refresh.clone(),
            scroller,
            placeholder,
            status,
            details_button,
            details: RefCell::new(None),
            progress,
            progress_label,
            progress_bar,
            cancel,
            items: RefCell::new(Vec::new()),
            selected: RefCell::new(Vec::new()),
            loading: Cell::new(false),
            unavailable: Cell::new(false),
            confirming: Cell::new(false),
            busy: Cell::new(false),
            status_is_summary: Cell::new(false),
            pending_refresh: Cell::new(false),
            generation: Cell::new(0),
            pending: Cell::new(None),
            confirmation: RefCell::new(None),
            receiver: RefCell::new(None),
            poll: RefCell::new(None),
            refresh_source: RefCell::new(None),
            monitor: RefCell::new(None),
        });

        browser.wire_factory(&factory);
        browser.wire_buttons(&back);
        browser.wire_selection();
        browser.start_monitor();
        browser.refresh();
        crate::log!("trash", "page created; listing trash:///");
        browser
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn job_started(&self) {
        self.busy.set(true);
        *self.details.borrow_mut() = None;
        self.details_button.set_visible(false);
        self.update_controls();
    }

    pub fn set_progress(&self, view: Option<ProgressView>) {
        let Some(view) = view else {
            self.progress.set_visible(false);
            return;
        };
        self.progress_label.set_text(&view.label);
        self.progress_bar.set_fraction(view.fraction);
        self.progress.set_visible(true);
    }

    pub fn job_finished(
        self: &Rc<Self>,
        request: &Request,
        summary: &str,
        details: Option<String>,
    ) {
        self.busy.set(false);
        self.confirming.set(false);
        self.set_progress(None);
        self.status_is_summary.set(true);
        self.status.set_text(summary);
        match details {
            Some(details) => {
                crate::log!("trash", "job details: {details}");
                *self.details.borrow_mut() = Some(details);
                self.details_button.set_visible(true);
            }
            None => {
                *self.details.borrow_mut() = None;
                self.details_button.set_visible(false);
            }
        }
        if matches!(
            request,
            Request::Trash { .. }
                | Request::Restore { .. }
                | Request::EmptyTrash
                | Request::DeleteSelected { .. }
                | Request::Undo
        ) {
            self.refresh();
        } else {
            self.update_controls();
        }
    }

    pub fn show_dry_run(&self, request: &Request, lines: &[String]) {
        self.clear_details();
        if matches!(
            request,
            Request::EmptyTrash
                | Request::DeleteSelected { .. }
                | Request::Restore { .. }
                | Request::Undo
        ) {
            self.busy.set(false);
            self.confirming.set(false);
            self.status_is_summary.set(true);
            self.status.set_text(&format!(
                "Dry run: {} line(s); nothing was changed",
                lines.len()
            ));
            self.update_controls();
        }
        crate::log!("trash", "dry run: {}", lines.join(" | "));
    }

    pub fn refused(&self, request: &Request, message: &str) {
        self.clear_details();
        self.status_is_summary.set(true);
        match request {
            Request::EmptyTrash => {
                self.confirming.set(false);
                self.busy.set(false);
                self.status
                    .set_text(&format!("Could not empty the Trash: {message}"));
            }
            Request::DeleteSelected { .. } => {
                self.confirming.set(false);
                self.busy.set(false);
                self.status
                    .set_text(&format!("Could not delete permanently: {message}"));
            }
            Request::Restore { .. } => {
                self.busy.set(false);
                self.status
                    .set_text(&format!("Could not restore: {message}"));
            }
            Request::Undo => {
                self.busy.set(false);
                self.status.set_text("Nothing to undo");
            }
            _ => return,
        }
        self.update_controls();
    }

    pub fn set_status(&self, text: &str) {
        self.clear_details();
        self.status_is_summary.set(true);
        self.status.set_text(text);
    }

    fn clear_details(&self) {
        *self.details.borrow_mut() = None;
        self.details_button.set_visible(false);
    }

    pub fn select_all(&self) {
        self.selection.select_all();
        self.capture_selection();
        self.update_controls();
    }

    pub fn select_none(&self) {
        self.selection.unselect_all();
        self.capture_selection();
        self.update_controls();
    }

    pub fn refresh_now(self: &Rc<Self>) {
        self.status_is_summary.set(false);
        self.refresh();
    }

    /// Leave the page without destroying it. A confirmation this page owns is
    /// resolved for its exact job so the worker never waits for a hidden page.
    pub fn leave_page(&self) {
        self.cancel_pending_confirmation();
    }

    fn cancel_pending_confirmation(&self) {
        let job = self.pending.take().map(|(job, _)| job);
        let dialog = self.confirmation.borrow_mut().take();
        self.confirming.set(false);
        if let Some(job) = job
            && let Some(fileops) = self
                .ui
                .upgrade()
                .and_then(|ui| ui.process_upgrade())
                .and_then(|process| process.fileops())
        {
            crate::log!(
                "trash",
                "page left while job {job} awaited confirmation; cancelling that job"
            );
            fileops.cancel_job(job);
        }
        if let Some(dialog) = dialog {
            dialog.close();
        }
    }

    /// Show the main-context confirmation for either permanent Trash
    /// operation. The plan kind picks the exact title, wording, response and
    /// worker approval; the two operations share this dialog lifecycle but
    /// never each other's authorisation.
    pub fn confirm_empty(self: &Rc<Self>, ui: &Rc<WindowUi>, id: u64, plan: &Plan) {
        let kind = match plan.request() {
            Request::EmptyTrash => ConfirmationKind::Empty,
            Request::DeleteSelected { .. } => ConfirmationKind::Selected,
            _ => return,
        };
        if plan.steps().is_empty() {
            self.confirming.set(false);
            self.update_controls();
            return;
        }
        if let Some((pending, pending_kind)) = self.pending.get() {
            crate::log!(
                "trash",
                "ignored a second confirmation while job {pending} ({pending_kind:?}) is pending"
            );
            return;
        }
        self.pending.set(Some((id, kind)));
        self.confirming.set(true);
        self.status_is_summary.set(false);
        self.update_controls();

        let count = plan.steps().len();
        let (title, body, confirm_label) = match kind {
            ConfirmationKind::Empty => (
                "Empty Trash?".to_string(),
                format!(
                    "{} item(s)\nThese items will be permanently deleted. This cannot be undone.",
                    count
                ),
                "Empty Trash",
            ),
            ConfirmationKind::Selected => {
                let name = (count == 1).then(|| {
                    let uri = plan.steps()[0].trash.as_ref().map(|trash| trash.uri());
                    uri.and_then(|uri| {
                        self.items
                            .borrow()
                            .iter()
                            .find(|item| item.uri() == uri)
                            .map(|item| item.display_name().to_string())
                    })
                    .or_else(|| uri.map(|uri| uri.name().to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "this item".to_string())
                });
                let (title, body) = delete_confirmation_text(count, name.as_deref());
                (title, body, "Delete Permanently")
            }
        };
        let dialog = adw::AlertDialog::new(Some(&title), Some(&body));
        dialog.add_css_class("gimbal-look");
        dialog.add_response("cancel", "Cancel");
        dialog.add_response(kind.response(), confirm_label);
        dialog.set_response_appearance(kind.response(), adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let answered = Rc::new(Cell::new(false));
        let weak_browser = Rc::downgrade(self);
        let weak_ui = Rc::downgrade(ui);
        let answered_response = answered.clone();
        dialog.connect_response(None, move |_, response| {
            answered_response.set(true);
            let Some(browser) = weak_browser.upgrade() else {
                return;
            };
            let action = claim_confirmation(&browser.pending, id, kind, Some(response));
            if action == ConfirmationAction::Ignore {
                crate::log!("trash", "ignored a reply for another confirmation job");
                return;
            }
            let _ = browser.confirmation.borrow_mut().take();
            browser.confirming.set(false);
            match action {
                ConfirmationAction::Confirm => {
                    if let Some(fileops) = weak_ui
                        .upgrade()
                        .and_then(|ui| ui.process_upgrade())
                        .and_then(|process| process.fileops())
                    {
                        match kind {
                            ConfirmationKind::Empty => fileops.confirm_empty(id),
                            ConfirmationKind::Selected => fileops.confirm_selected(id),
                        }
                    }
                }
                ConfirmationAction::Cancel => {
                    if let Some(fileops) = weak_ui
                        .upgrade()
                        .and_then(|ui| ui.process_upgrade())
                        .and_then(|process| process.fileops())
                    {
                        crate::log!("trash", "confirmation dismissed; nothing is deleted");
                        fileops.cancel_job(id);
                    }
                }
                ConfirmationAction::Ignore => {
                    crate::log!("trash", "ignored a reply for another confirmation job");
                }
            }
            browser.update_controls();
        });

        ui.begin_dialog();
        let weak_closed_browser = Rc::downgrade(self);
        let weak_closed_ui = Rc::downgrade(ui);
        let answered_closed = answered.clone();
        dialog.connect_closed(move |_| {
            if let Some(ui) = weak_closed_ui.upgrade() {
                ui.end_dialog();
            }
            let Some(browser) = weak_closed_browser.upgrade() else {
                return;
            };
            if browser.pending.get() != Some((id, kind)) {
                return;
            }
            browser.confirming.set(false);
            let _ = browser.confirmation.borrow_mut().take();
            browser.update_controls();
            // libadwaita emits `closed` before `response`
            // (adw-alert-dialog.c:278-279). Wait one idle so a real response is
            // not turned into a cancel (the P2k fix).
            let answered = answered_closed.clone();
            let weak_idle_browser = Rc::downgrade(&browser);
            let weak_idle_ui = weak_closed_ui.clone();
            glib::idle_add_local_once(move || {
                if answered.get() {
                    return;
                }
                let Some(browser) = weak_idle_browser.upgrade() else {
                    return;
                };
                let action = claim_confirmation(&browser.pending, id, kind, None);
                if action != ConfirmationAction::Cancel {
                    return;
                }
                crate::log!(
                    "trash",
                    "confirmation closed without a response; cancelling"
                );
                if let Some(fileops) = weak_idle_ui
                    .upgrade()
                    .and_then(|ui| ui.process_upgrade())
                    .and_then(|process| process.fileops())
                {
                    fileops.cancel_job(id);
                }
            });
        });

        *self.confirmation.borrow_mut() = Some(dialog.clone());
        dialog.choose(Some(ui.window()), None::<&gio::Cancellable>, |_| {});
        crate::log!("trash", "showing {kind:?} confirmation for {count} item(s)");
    }

    fn wire_buttons(self: &Rc<Self>, back: &gtk::Button) {
        {
            let weak = Rc::downgrade(self);
            back.connect_clicked(move |_| {
                if let Some(browser) = weak.upgrade()
                    && let Some(ui) = browser.ui.upgrade()
                {
                    ui.dispatch(Caller::Ui, Invocation::new("nav.back", Vec::new()));
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.restore.connect_clicked(move |_| {
                if let Some(browser) = weak.upgrade() {
                    browser.restore_selected();
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.empty.connect_clicked(move |_| {
                if let Some(browser) = weak.upgrade() {
                    browser.empty_clicked();
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.refresh_button.connect_clicked(move |_| {
                if let Some(browser) = weak.upgrade() {
                    browser.status_is_summary.set(false);
                    browser.refresh();
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.cancel.connect_clicked(move |_| {
                if let Some(browser) = weak.upgrade()
                    && let Some(ui) = browser.ui.upgrade()
                    && let Some(fileops) =
                        ui.process_upgrade().and_then(|process| process.fileops())
                {
                    fileops.cancel_current();
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.details_button.connect_clicked(move |_| {
                let Some(browser) = weak.upgrade() else {
                    return;
                };
                let details = browser.details.borrow().clone();
                if let (Some(details), Some(ui)) = (details, browser.ui.upgrade()) {
                    crate::fileops::dialogs::show_details(&ui, &details);
                }
            });
        }
    }

    fn wire_selection(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.selection.connect_selection_changed(move |_, _, _| {
            if let Some(browser) = weak.upgrade() {
                browser.capture_selection();
                browser.update_controls();
            }
        });
    }

    fn wire_factory(self: &Rc<Self>, factory: &gtk::SignalListItemFactory) {
        {
            let browser = Rc::downgrade(self);
            factory.connect_setup(move |_, object| {
                let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                row.add_css_class("gl-trash-row");
                let image = gtk::Image::new();
                image.add_css_class("gl-trash-icon");
                image.set_valign(gtk::Align::Center);
                row.append(&image);
                let labels = gtk::Box::new(gtk::Orientation::Vertical, 0);
                labels.set_hexpand(true);
                labels.set_valign(gtk::Align::Center);
                let name = gtk::Label::new(None);
                name.add_css_class("gl-trash-name");
                name.set_xalign(0.0);
                name.set_ellipsize(pango::EllipsizeMode::End);
                let original = gtk::Label::new(None);
                original.add_css_class("gl-trash-meta");
                original.set_xalign(0.0);
                original.set_ellipsize(pango::EllipsizeMode::Middle);
                let date = gtk::Label::new(None);
                date.add_css_class("gl-trash-meta");
                date.set_xalign(0.0);
                labels.append(&name);
                labels.append(&original);
                labels.append(&date);
                row.append(&labels);
                item.set_child(Some(&row));

                let right = gtk::GestureClick::new();
                right.set_button(3);
                let list_item = item.downgrade();
                let browser = browser.clone();
                right.connect_pressed(move |gesture, _, _, _| {
                    let Some(browser) = browser.upgrade() else {
                        return;
                    };
                    let Some(list_item) = list_item.upgrade() else {
                        return;
                    };
                    let Some(anchor) = gesture.widget() else {
                        return;
                    };
                    browser.right_click(list_item.position(), &anchor);
                });
                row.add_controller(right);
            });
        }
        {
            factory.connect_bind(move |_, object| {
                let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(boxed) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                    return;
                };
                let trash_item = boxed.borrow::<TrashItem>().clone();
                let Some(row) = item.child().and_downcast::<gtk::Box>() else {
                    return;
                };
                let Some(image) = row.first_child().and_downcast::<gtk::Image>() else {
                    return;
                };
                if let Some(icon_name) = trash_item.icon_name() {
                    image.set_icon_name(Some(icon_name));
                } else if trash_item.is_dir() {
                    image.set_paintable(Some(&icons::svg(MockupIcon::Folder)));
                } else {
                    image.set_icon_name(Some("text-x-generic"));
                }
                let Some(labels) = image.next_sibling().and_downcast::<gtk::Box>() else {
                    return;
                };
                let mut child = labels.first_child();
                if let Some(name) = child
                    .as_ref()
                    .and_then(|widget| widget.downcast_ref::<gtk::Label>())
                {
                    name.set_text(trash_item.display_name());
                }
                child = child.and_then(|widget| widget.next_sibling());
                if let Some(original) = child
                    .as_ref()
                    .and_then(|widget| widget.downcast_ref::<gtk::Label>())
                {
                    original.set_text(&original_text(trash_item.original()));
                }
                child = child.and_then(|widget| widget.next_sibling());
                if let Some(date) = child
                    .as_ref()
                    .and_then(|widget| widget.downcast_ref::<gtk::Label>())
                {
                    date.set_text(&date_text(trash_item.deleted_at()));
                }
            });
        }
    }

    fn right_click(self: &Rc<Self>, position: u32, anchor: &gtk::Widget) {
        // A right-click on an unselected row selects only that row. A
        // right-click inside the selection uses the whole selected set. Then
        // the action snapshot is built once, before the menu exists.
        let clicked_selected = self.selection.selection().contains(position);
        if !clicked_selected {
            self.selection.select_item(position, true);
        }
        self.capture_selection();
        let selected = self.selected.borrow().clone();
        let positions = menu_positions(&self.selected_positions(), position, clicked_selected);
        let (restorable, loading, unavailable, confirming, busy) = {
            let items = self.items.borrow();
            (
                restorable_selection(&selected, &items),
                self.loading.get(),
                self.unavailable.get(),
                self.confirming.get(),
                self.busy.get(),
            )
        };
        let state = item_menu(
            selected.len(),
            restorable,
            loading,
            unavailable,
            confirming,
            busy,
        );
        let snapshot = delete_snapshot(&self.items.borrow(), &positions);

        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
        menu.add_css_class("gl-menu");

        let restore = gtk::Button::new();
        restore.add_css_class("gl-menu-item");
        restore.set_sensitive(state.restore);
        restore.set_child(Some(
            &gtk::Label::builder()
                .label("Restore")
                .halign(gtk::Align::Start)
                .hexpand(true)
                .build(),
        ));
        let browser = Rc::downgrade(self);
        let popover_restore = popover.clone();
        restore.connect_clicked(move |_| {
            popover_restore.popdown();
            if popover_restore.parent().is_some() {
                popover_restore.unparent();
            }
            if let Some(browser) = browser.upgrade() {
                browser.restore_selected();
            }
        });
        menu.append(&restore);

        let delete = gtk::Button::new();
        delete.add_css_class("gl-menu-item");
        delete.add_css_class("destructive-action");
        delete.set_sensitive(state.delete);
        delete.set_tooltip_text(Some("Permanently delete only the selected items"));
        delete.set_child(Some(
            &gtk::Label::builder()
                .label("Delete Permanently")
                .halign(gtk::Align::Start)
                .hexpand(true)
                .build(),
        ));
        let browser = Rc::downgrade(self);
        let popover_delete = popover.clone();
        delete.connect_clicked(move |_| {
            popover_delete.popdown();
            if popover_delete.parent().is_some() {
                popover_delete.unparent();
            }
            if let Some(browser) = browser.upgrade() {
                browser.delete_selected(snapshot.clone());
            }
        });
        menu.append(&delete);

        popover.set_child(Some(&menu));
        popover.connect_closed(|popover| {
            if popover.parent().is_some() {
                popover.unparent();
            }
        });
        crate::popup::show(&popover);
    }

    fn selected_positions(&self) -> Vec<u32> {
        let bitset = self.selection.selection();
        (0..bitset.size())
            .map(|index| bitset.nth(index as u32))
            .collect()
    }

    fn capture_selection(&self) {
        let items = self.items.borrow();
        let bitset = self.selection.selection();
        let mut selected = Vec::new();
        for index in 0..bitset.size() {
            let position = bitset.nth(index as u32);
            if let Some(item) = items.get(position as usize) {
                selected.push(item.identity().clone());
            }
        }
        *self.selected.borrow_mut() = selected;
    }

    fn restore_selected(self: &Rc<Self>) {
        let selected = self.selected.borrow().clone();
        if selected.is_empty() {
            return;
        }
        let restorable: Vec<&TrashIdentity> = {
            let items = self.items.borrow();
            selected
                .iter()
                .filter(|identity| {
                    items
                        .iter()
                        .any(|item| identity.agrees_with(item.identity()) && item.can_restore())
                })
                .collect()
        };
        if restorable.is_empty() {
            self.status_is_summary.set(true);
            self.status
                .set_text("These items have no known original location.");
            return;
        }
        let arguments: Vec<(String, String)> = restorable
            .iter()
            .map(|identity| ("items".to_string(), identity.uri().as_str().to_string()))
            .collect();
        if let Some(ui) = self.ui.upgrade() {
            ui.dispatch(Caller::Ui, Invocation::new("trash.restore", arguments));
        }
    }

    /// Delete only the frozen selected identities. The snapshot is staged
    /// before the registry dispatch; the worker never re-reads the current
    /// selection or the current listing.
    fn delete_selected(self: &Rc<Self>, selected: Vec<TrashIdentity>) {
        if selected.is_empty() {
            return;
        }
        if self.loading.get() || self.unavailable.get() || self.confirming.get() || self.busy.get()
        {
            return;
        }
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        if let Some(process) = ui.process_upgrade() {
            process.stage_trash_delete(selected.clone());
        }
        let arguments: Vec<(String, String)> = selected
            .iter()
            .map(|identity| ("items".to_string(), identity.uri().as_str().to_string()))
            .collect();
        ui.dispatch(
            Caller::Ui,
            Invocation::new("trash.delete-permanently", arguments),
        );
    }

    fn empty_clicked(self: &Rc<Self>) {
        if self.items.borrow().is_empty() || self.confirming.get() {
            return;
        }
        self.confirming.set(true);
        self.update_controls();
        if let Some(ui) = self.ui.upgrade() {
            ui.dispatch(Caller::Ui, Invocation::new("trash.empty", Vec::new()));
        }
    }

    fn start_monitor(self: &Rc<Self>) {
        let root = gio::File::for_uri(fm_core::fileops::trash::TRASH_ROOT_URI);
        match root.monitor_directory(
            gio::FileMonitorFlags::WATCH_MOUNTS,
            None::<&gio::Cancellable>,
        ) {
            Ok(monitor) => {
                let browser = Rc::downgrade(self);
                monitor.connect_changed(move |_, _, _, _| {
                    if let Some(browser) = browser.upgrade() {
                        browser.schedule_refresh();
                    }
                });
                *self.monitor.borrow_mut() = Some(monitor);
            }
            Err(error) => {
                crate::log!("trash", "cannot monitor trash:///: {error}");
            }
        }
    }

    fn schedule_refresh(self: &Rc<Self>) {
        if self.loading.get() {
            self.pending_refresh.set(true);
            return;
        }
        if let Some(source) = self.refresh_source.borrow_mut().take() {
            source.remove();
        }
        let browser = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(
            Duration::from_millis(REFRESH_DELAY_MS as u64),
            move || {
                if let Some(browser) = browser.upgrade() {
                    browser.refresh_source.borrow_mut().take();
                    browser.refresh();
                }
            },
        );
        *self.refresh_source.borrow_mut() = Some(source);
    }

    fn refresh(self: &Rc<Self>) {
        if self.loading.get() {
            self.pending_refresh.set(true);
            return;
        }
        self.loading.set(true);
        self.unavailable.set(false);
        self.placeholder.set_text("Loading Trash…");
        self.placeholder.set_visible(true);
        self.update_controls();

        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let (sender, receiver) = std::sync::mpsc::channel();
        *self.receiver.borrow_mut() = Some(receiver);
        let spawned = std::thread::Builder::new()
            .name("gimbal-trash-list".to_string())
            .spawn(move || {
                let result = fm_core::fileops::list_trash().map_err(|error| error.to_string());
                let _ = sender.send((generation, result));
            });
        if let Err(error) = spawned {
            self.finish_load(
                generation,
                Err(format!("could not start the trash listing: {error}")),
            );
            return;
        }
        self.ensure_poll();
    }

    fn ensure_poll(self: &Rc<Self>) {
        if self.poll.borrow().is_some() {
            return;
        }
        let browser = Rc::downgrade(self);
        let source = glib::timeout_add_local(Duration::from_millis(POLL_MS), move || {
            let Some(browser) = browser.upgrade() else {
                return glib::ControlFlow::Break;
            };
            browser.poll_once()
        });
        *self.poll.borrow_mut() = Some(source);
    }

    fn poll_once(self: &Rc<Self>) -> glib::ControlFlow {
        let result = {
            let receiver = self.receiver.borrow();
            let Some(receiver) = receiver.as_ref() else {
                *self.poll.borrow_mut() = None;
                return glib::ControlFlow::Break;
            };
            match receiver.try_recv() {
                Ok(value) => Some(value),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some((
                    self.generation.get(),
                    Err("the trash listing thread stopped".to_string()),
                )),
            }
        };
        if let Some((generation, result)) = result {
            self.finish_load(generation, result);
            if self.loading.get() {
                return glib::ControlFlow::Continue;
            }
            *self.poll.borrow_mut() = None;
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    }

    fn finish_load(self: &Rc<Self>, generation: u64, result: Result<Vec<TrashItem>, String>) {
        if generation != self.generation.get() {
            return;
        }
        self.loading.set(false);
        let old = self.selected.borrow().clone();
        match result {
            Ok(items) => {
                self.unavailable.set(false);
                self.store.remove_all();
                for item in &items {
                    self.store.append(&glib::BoxedAnyObject::new(item.clone()));
                }
                *self.items.borrow_mut() = items;
                self.placeholder.set_visible(self.items.borrow().is_empty());
                self.placeholder.set_text("Trash is empty");
                self.scroller.set_visible(true);
                let positions = retained_positions(&old, &self.items.borrow());
                for position in positions {
                    self.selection.select_item(position, false);
                }
                self.capture_selection();
                if !self.status_is_summary.get() {
                    self.status
                        .set_text(&format!("{} item(s)", self.items.borrow().len()));
                }
                crate::log!(
                    "trash",
                    "listed {} item(s) (selection kept {})",
                    self.items.borrow().len(),
                    self.selected.borrow().len()
                );
            }
            Err(error) => {
                self.clear_details();
                self.unavailable.set(true);
                self.status_is_summary.set(false);
                self.store.remove_all();
                self.items.borrow_mut().clear();
                self.selected.borrow_mut().clear();
                self.placeholder.set_visible(true);
                self.placeholder
                    .set_text(&format!("Could not read the Trash.\n{error}"));
                self.status.set_text(&format!("Trash unavailable: {error}"));
                crate::log!("trash", "listing failed: {error}");
            }
        }
        self.update_controls();
        if self.pending_refresh.replace(false) {
            self.refresh();
        }
    }

    fn update_controls(&self) {
        let restorable = restorable_selection(&self.selected.borrow(), &self.items.borrow());
        let state = controls(
            self.loading.get(),
            self.unavailable.get(),
            self.items.borrow().len(),
            restorable,
            self.confirming.get(),
            self.busy.get(),
        );
        self.restore.set_sensitive(state.restore);
        self.empty.set_sensitive(state.empty);
        self.refresh_button.set_sensitive(state.refresh);
    }
}

impl Drop for TrashBrowser {
    fn drop(&mut self) {
        self.cancel_pending_confirmation();
        if let Some(source) = self.poll.borrow_mut().take() {
            source.remove();
        }
        if let Some(source) = self.refresh_source.borrow_mut().take() {
            source.remove();
        }
        if let Some(monitor) = self.monitor.borrow_mut().take() {
            monitor.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::fileops::{TrashFingerprint, TrashUri};
    use gtk::glib::translate::IntoGlib;
    use std::path::PathBuf;

    fn fingerprint(generation: &str) -> TrashFingerprint {
        let info = gio::FileInfo::new();
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILE.as_str(), generation);
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILESYSTEM.as_str(), "l1");
        info.set_attribute_uint32(
            gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str(),
            u32::try_from(gio::FileType::Regular.into_glib()).unwrap(),
        );
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_TIME_CHANGED.as_str(), 1_700_000_000);
        info.set_attribute_uint32("time::changed-nsec", 1);
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ETAG_VALUE.as_str(), generation);
        TrashFingerprint::from_file_info(&info)
    }

    fn identity(uri: &str, original: Option<&str>, generation: &str) -> TrashIdentity {
        TrashIdentity::new(
            TrashUri::parse(uri).unwrap(),
            original.map(PathBuf::from),
            None,
            Some(fingerprint(generation)),
        )
    }

    fn item(uri: &str, original: Option<&str>) -> TrashItem {
        let identity = identity(uri, original, "generation-1");
        TrashItem::new(identity, "name".to_string(), false, None)
    }

    #[test]
    fn selection_and_controls_follow_staleness_and_metadata() {
        let items = vec![
            item("trash:///a.txt", Some("/home/k/a.txt")),
            item("trash:///b.txt", None),
        ];
        let selected = vec![
            identity("trash:///a.txt", Some("/home/k/a.txt"), "generation-1"),
            identity("trash:///b.txt", None, "generation-1"),
        ];
        assert_eq!(restorable_selection(&selected, &items), 1);
        assert_eq!(
            controls(false, false, items.len(), 1, false, false),
            ControlState {
                restore: true,
                empty: true,
                refresh: true,
            }
        );
        assert!(!controls(false, false, 2, 0, false, false).restore);
        assert!(!controls(false, false, 0, 0, false, false).empty);
        assert!(!controls(false, true, 2, 2, false, false).empty);
        assert!(!controls(false, false, 2, 2, true, false).empty);
        assert!(!controls(false, false, 2, 2, false, true).restore);
        assert!(!controls(true, false, 2, 2, false, false).refresh);
    }

    #[test]
    fn a_refresh_keeps_only_identities_that_still_exist() {
        let items = [
            item("trash:///a.txt", Some("/home/k/a.txt")),
            item("trash:///c.txt", Some("/home/k/c.txt")),
        ];
        let old = vec![
            identity("trash:///a.txt", Some("/home/k/a.txt"), "generation-1"),
            identity("trash:///b.txt", Some("/home/k/b.txt"), "generation-1"),
            // Same URI, new object: the old selection must not move to it.
            identity("trash:///c.txt", Some("/home/k/c.txt"), "generation-1"),
        ];
        let items = vec![
            items[0].clone(),
            TrashItem::new(
                identity("trash:///c.txt", Some("/home/k/c.txt"), "generation-2"),
                "c".to_string(),
                false,
                None,
            ),
        ];
        assert_eq!(retained_positions(&old, &items), vec![0]);
    }

    #[test]
    fn a_closed_or_cancelled_empty_confirmation_does_not_delete() {
        assert_eq!(ConfirmationKind::Empty.response(), "empty");
        assert_eq!(ConfirmationKind::Selected.response(), "delete");
        assert_ne!(
            ConfirmationKind::Empty.response(),
            ConfirmationKind::Selected.response()
        );
        assert_eq!(
            confirmation_action(
                Some((3, ConfirmationKind::Empty)),
                3,
                ConfirmationKind::Empty,
                Some("cancel")
            ),
            ConfirmationAction::Cancel
        );
        assert_eq!(
            confirmation_action(
                Some((3, ConfirmationKind::Empty)),
                3,
                ConfirmationKind::Empty,
                None
            ),
            ConfirmationAction::Cancel
        );
    }

    #[test]
    fn missing_metadata_reads_unknown_and_does_not_block_selected_deletion() {
        assert_eq!(original_text(None), "Unknown");
        assert_eq!(date_text(None), "Unknown");
        assert!(item_menu(1, 0, false, false, false, false).delete);
    }

    #[test]
    fn open_and_reopen_use_one_page_and_back_does_not_consume_history() {
        assert_eq!(open_action(false), OpenAction::CreatePage);
        assert_eq!(open_action(true), OpenAction::ShowExisting);
        assert!(!back_consumes_history(true));
        assert_eq!(
            route_on_trash_page("nav.back", true),
            PageRoute::BackToFiles
        );
        // Normal folder history resumes once files are visible again.
        assert!(back_consumes_history(false));
        assert_eq!(route_on_trash_page("nav.back", false), PageRoute::Files);
    }

    #[test]
    fn background_refresh_keeps_trash_but_navigation_leaves_it() {
        assert!(keeps_trash_page(true, RefreshCause::Background));
        assert!(!keeps_trash_page(true, RefreshCause::Navigation));
        assert!(!keeps_trash_page(false, RefreshCause::Background));
        assert!(leaves_trash_page(true, RefreshCause::Navigation));
        assert!(!leaves_trash_page(true, RefreshCause::Background));
        assert_eq!(route_on_trash_page("nav.go", true), PageRoute::LeaveTrash);
        assert_eq!(route_on_trash_page("nav.go", false), PageRoute::Files);
    }

    #[test]
    fn selection_all_in_trash_uses_the_visible_page() {
        assert_eq!(
            route_on_trash_page("selection.all", true),
            PageRoute::SelectAll
        );
        assert_eq!(
            route_on_trash_page("selection.none", true),
            PageRoute::SelectNone
        );
        // The hidden folder's own selection command is not used as a fallback.
        assert_eq!(
            route_on_trash_page("selection.all", false),
            PageRoute::Files
        );
    }

    #[test]
    fn hidden_selection_actions_are_refused_while_trash_is_visible() {
        assert_eq!(
            route_on_trash_page("file.trash", true),
            PageRoute::Refuse("Items are already in Trash")
        );
        for command in [
            "file.open",
            "file.cut",
            "file.copy-to-clipboard",
            "file.copy",
            "file.move",
            "file.paste",
            "file.rename",
            "file.new-folder",
            "file.new-file",
        ] {
            assert!(
                matches!(route_on_trash_page(command, true), PageRoute::Refuse(_)),
                "{command} must not reach hidden files"
            );
            assert_eq!(
                route_on_trash_page(command, false),
                PageRoute::Files,
                "{command} resumes once files are visible"
            );
        }
        // Restore uses explicit selected Trash identities; Undo is the global
        // worker operation. Both remain available on the visible page.
        assert_eq!(route_on_trash_page("trash.restore", true), PageRoute::Files);
        assert_eq!(route_on_trash_page("trash.empty", true), PageRoute::Files);
        assert_eq!(
            route_on_trash_page("trash.delete-permanently", true),
            PageRoute::Files
        );
        assert_eq!(route_on_trash_page("file.undo", true), PageRoute::Files);
    }

    #[test]
    fn the_item_menu_scope_follows_selection_and_busy_state() {
        assert!(!item_menu(0, 0, false, false, false, false).delete);
        assert!(!item_menu(1, 1, true, false, false, false).delete);
        assert!(!item_menu(1, 1, false, true, false, false).delete);
        assert!(!item_menu(1, 1, false, false, true, false).delete);
        assert!(!item_menu(1, 1, false, false, false, true).delete);
        assert!(item_menu(1, 0, false, false, false, false).delete);
        assert!(item_menu(3, 3, false, false, false, false).delete);
        // A known original is needed for Restore, never for deletion.
        assert!(!item_menu(1, 0, false, false, false, false).restore);
        assert!(item_menu(1, 1, false, false, false, false).restore);
        assert!(!item_menu(0, 0, false, false, false, false).restore);
    }

    #[test]
    fn a_right_click_uses_the_selection_or_only_the_unselected_row() {
        assert_eq!(menu_positions(&[0, 2], 1, false), vec![1]);
        assert_eq!(menu_positions(&[0, 2], 2, true), vec![0, 2]);
        assert_eq!(menu_positions(&[], 4, false), vec![4]);
    }

    #[test]
    fn a_click_snapshot_survives_later_selection_changes() {
        let items = vec![
            item("trash:///a.txt", None),
            item("trash:///b.txt", None),
            item("trash:///c.txt", None),
        ];
        let first = delete_snapshot(&items, &[0, 2]);
        assert_eq!(
            first
                .iter()
                .map(|identity| identity.uri().as_str())
                .collect::<Vec<_>>(),
            vec!["trash:///a.txt", "trash:///c.txt"]
        );
        // The browser selection or listing changing later produces a different
        // snapshot and cannot mutate the one already taken.
        let later = delete_snapshot(&items[1..], &[0, 1]);
        assert_eq!(
            later
                .iter()
                .map(|identity| identity.uri().as_str())
                .collect::<Vec<_>>(),
            vec!["trash:///b.txt", "trash:///c.txt"]
        );
        assert_eq!(first.len(), 2);
        // The registry's URI set is deduplicated in the same way.
        assert_eq!(delete_snapshot(&items, &[0, 0, 2, 2]).len(), 2);
    }

    #[test]
    fn selected_delete_confirmation_names_one_and_counts_many() {
        let (title, body) = delete_confirmation_text(1, Some("a.txt"));
        assert_eq!(title, "Permanently delete this item?");
        assert!(body.contains("a.txt"));
        assert!(body.contains("These items will be permanently deleted. This cannot be undone."));

        let (title, body) = delete_confirmation_text(3, None);
        assert_eq!(title, "Permanently delete 3 items?");
        assert_eq!(
            body,
            "These items will be permanently deleted. This cannot be undone."
        );
    }

    #[test]
    fn confirmation_replies_are_scoped_to_job_and_operation_kind() {
        // Exercise the same state mutation the response and idle callbacks use.
        let pending = Cell::new(Some((7, ConfirmationKind::Empty)));
        assert_eq!(
            claim_confirmation(&pending, 8, ConfirmationKind::Empty, Some("empty")),
            ConfirmationAction::Ignore
        );
        assert_eq!(pending.get(), Some((7, ConfirmationKind::Empty)));
        assert_eq!(
            claim_confirmation(&pending, 8, ConfirmationKind::Empty, None),
            ConfirmationAction::Ignore
        );
        assert_eq!(pending.get(), Some((7, ConfirmationKind::Empty)));
        assert_eq!(
            claim_confirmation(&pending, 7, ConfirmationKind::Empty, Some("cancel")),
            ConfirmationAction::Cancel
        );
        assert_eq!(pending.get(), None);

        pending.set(Some((9, ConfirmationKind::Selected)));
        assert_eq!(
            claim_confirmation(&pending, 9, ConfirmationKind::Empty, Some("empty")),
            ConfirmationAction::Ignore,
            "an Empty Trash reply cannot claim a selected confirmation"
        );
        assert_eq!(
            claim_confirmation(&pending, 9, ConfirmationKind::Selected, Some("delete")),
            ConfirmationAction::Confirm
        );
        assert_eq!(pending.get(), None);

        assert_eq!(
            confirmation_action(
                Some((7, ConfirmationKind::Empty)),
                7,
                ConfirmationKind::Empty,
                Some("empty")
            ),
            ConfirmationAction::Confirm
        );
        assert_eq!(
            confirmation_action(
                Some((7, ConfirmationKind::Selected)),
                7,
                ConfirmationKind::Selected,
                Some("empty")
            ),
            ConfirmationAction::Cancel,
            "the selected dialog only accepts its own Delete Permanently response"
        );
        assert_eq!(
            confirmation_action(
                Some((7, ConfirmationKind::Empty)),
                7,
                ConfirmationKind::Selected,
                Some("delete")
            ),
            ConfirmationAction::Ignore,
            "operation kinds never cross"
        );
    }

    #[test]
    fn stale_or_missing_job_replies_never_delete() {
        // A reply for an old job, a reply after the page forgot its job, and a
        // reply for a job that never owned the dialog are all ignored.
        assert_eq!(
            confirmation_action(
                Some((21, ConfirmationKind::Selected)),
                20,
                ConfirmationKind::Selected,
                Some("delete")
            ),
            ConfirmationAction::Ignore
        );
        assert_eq!(
            confirmation_action(None, 21, ConfirmationKind::Selected, Some("delete")),
            ConfirmationAction::Ignore
        );
        assert_eq!(
            claim_confirmation(
                &Cell::new(Some((21, ConfirmationKind::Selected))),
                20,
                ConfirmationKind::Selected,
                Some("delete")
            ),
            ConfirmationAction::Ignore
        );
    }

    #[test]
    fn escape_close_and_confirmation_states_stay_with_their_job() {
        // Escape or a close without a response cancels only the job whose
        // confirmation this page owns.
        assert_eq!(
            confirmation_action(
                Some((11, ConfirmationKind::Selected)),
                11,
                ConfirmationKind::Selected,
                None
            ),
            ConfirmationAction::Cancel
        );
        assert_eq!(
            confirmation_action(
                Some((11, ConfirmationKind::Selected)),
                12,
                ConfirmationKind::Selected,
                None
            ),
            ConfirmationAction::Ignore
        );
        // Once the page left and cleared its pending id, a late close or reply
        // must not cancel whichever job is current then.
        assert_eq!(
            confirmation_action(None, 11, ConfirmationKind::Selected, Some("delete")),
            ConfirmationAction::Ignore
        );
    }
}
