// SPDX-License-Identifier: GPL-3.0-or-later
//! The conflict menu, the item and background menus, the rename and
//! new-folder popovers, and the folder chooser.
//!
//! Every action is a named command. The dialogs only collect an answer or a
//! name; the window's command path does the work.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use fm_core::command::{Caller, Invocation};
use fm_core::fileops::Decision;
use gtk::{gdk, gio, glib};

use crate::fileops::conflict::ConflictFlow;
use crate::fileops::menu::{MenuAction, MenuItem};
use crate::frame::WindowUi;

pub type ConflictDone = Rc<dyn Fn(fm_core::fileops::Decisions)>;

/// Show a popover built from the pure menu.
/// Show a command menu on `anchor`. `at` is a point in the anchor's
/// coordinates to aim at; without it the popover points at the whole anchor,
/// which suits a small widget like a grid cell.
pub fn show_menu(
    ui: &Rc<WindowUi>,
    anchor: &gtk::Widget,
    items: &[MenuItem],
    at: Option<(f64, f64)>,
) {
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    if let Some((x, y)) = at {
        popover.set_pointing_to(Some(&gdk::Rectangle::new(
            x.round() as i32,
            y.round() as i32,
            1,
            1,
        )));
    }
    let menu = vbox(&["gl-menu"]);

    if items.is_empty() {
        menu.append(
            &gtk::Label::builder()
                .label("No actions")
                .css_classes(["gl-menu-item"])
                .build(),
        );
    }

    for item in items {
        let button = gtk::Button::new();
        button.add_css_class("gl-menu-item");
        button.set_sensitive(item.enabled);
        button.set_child(Some(
            &gtk::Label::builder()
                .label(&item.label)
                .halign(gtk::Align::Start)
                .hexpand(true)
                .build(),
        ));
        let popover_click = popover.clone();
        let ui_click = ui.clone();
        let anchor_click = anchor.clone();
        let item = item.clone();
        button.connect_clicked(move |_| {
            take_down(&popover_click);
            run_action(&ui_click, &anchor_click, &item);
        });
        menu.append(&button);
    }

    popover.set_child(Some(&menu));
    close_on_closed(&popover);
    crate::popup::show(&popover);
}

fn run_action(ui: &Rc<WindowUi>, anchor: &gtk::Widget, item: &MenuItem) {
    match &item.action {
        MenuAction::Command(arguments) => {
            ui.dispatch(Caller::Ui, Invocation::new(item.command, arguments.clone()));
        }
        MenuAction::Rename { path } => open_rename(ui, anchor, path),
        MenuAction::NewFolder { folder } => open_new_folder(ui, anchor, folder),
        MenuAction::ChooseFolder { command, paths } => {
            choose_folder(ui, command, paths);
        }
    }
}

/// Rename through a small entry popover that pre-selects the name without its
/// extension.
pub fn open_rename(ui: &Rc<WindowUi>, anchor: &gtk::Widget, path: &Path) {
    let Some(path_text) = path.to_str() else {
        crate::log!(
            "fileops",
            "cannot rename a non-UTF-8 name: {}",
            path.display()
        );
        return;
    };
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    let box_ = vbox(&["gl-menu"]);
    let entry = gtk::Entry::new();
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    entry.set_text(&stem);
    entry.set_placeholder_text(Some("Name"));
    box_.append(&entry);

    let row = hbox(&[]);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("gl-menu-item");
    let popover_cancel = popover.clone();
    cancel.connect_clicked(move |_| take_down(&popover_cancel));

    let rename = gtk::Button::with_label("Rename");
    rename.add_css_class("gl-menu-item");
    let ui_rename = ui.clone();
    let popover_rename = popover.clone();
    let entry_rename = entry.clone();
    let path_rename = path_text.to_string();
    rename.connect_clicked(move |_| {
        let name = entry_rename.text().to_string();
        take_down(&popover_rename);
        dispatch_rename(&ui_rename, &path_rename, name);
    });
    row.append(&cancel);
    row.append(&rename);
    box_.append(&row);
    popover.set_child(Some(&box_));

    let ui_enter = ui.clone();
    let popover_enter = popover.clone();
    let path_enter = path_text.to_string();
    let entry_enter = entry.clone();
    entry.connect_activate(move |_| {
        let name = entry_enter.text().to_string();
        take_down(&popover_enter);
        dispatch_rename(&ui_enter, &path_enter, name);
    });

    ui.set_editing(true);
    let ui_closed = ui.clone();
    popover.connect_closed(move |popover| {
        ui_closed.set_editing(false);
        if popover.parent().is_some() {
            popover.unparent();
        }
    });
    crate::popup::show(&popover);
    entry.grab_focus();
    let selection = stem.chars().count() as i32;
    entry.select_region(0, selection);
}

fn dispatch_rename(ui: &Rc<WindowUi>, path: &str, name: String) {
    if name.is_empty() {
        crate::log!("fileops", "an empty rename was ignored");
        return;
    }
    ui.dispatch(
        Caller::Ui,
        Invocation::new(
            "file.rename",
            vec![
                ("path".to_string(), path.to_string()),
                ("name".to_string(), name),
            ],
        ),
    );
}

/// New Folder through an entry popover.
pub fn open_new_folder(ui: &Rc<WindowUi>, anchor: &gtk::Widget, folder: &Path) {
    let folder_text = folder.to_string_lossy().into_owned();
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    let box_ = vbox(&["gl-menu"]);
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Folder name"));
    box_.append(&entry);

    let row = hbox(&[]);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("gl-menu-item");
    let popover_cancel = popover.clone();
    cancel.connect_clicked(move |_| take_down(&popover_cancel));

    let create = gtk::Button::with_label("Create");
    create.add_css_class("gl-menu-item");
    let ui_create = ui.clone();
    let popover_create = popover.clone();
    let entry_create = entry.clone();
    let folder_create = folder_text.clone();
    create.connect_clicked(move |_| {
        let name = entry_create.text().trim().to_string();
        take_down(&popover_create);
        dispatch_new_folder(&ui_create, &folder_create, name);
    });
    row.append(&cancel);
    row.append(&create);
    box_.append(&row);
    popover.set_child(Some(&box_));

    let ui_enter = ui.clone();
    let popover_enter = popover.clone();
    let entry_enter = entry.clone();
    let folder_enter = folder_text;
    entry.connect_activate(move |_| {
        let name = entry_enter.text().trim().to_string();
        take_down(&popover_enter);
        dispatch_new_folder(&ui_enter, &folder_enter, name);
    });

    ui.set_editing(true);
    let ui_closed = ui.clone();
    popover.connect_closed(move |popover| {
        ui_closed.set_editing(false);
        if popover.parent().is_some() {
            popover.unparent();
        }
    });
    crate::popup::show(&popover);
    entry.grab_focus();
}

fn dispatch_new_folder(ui: &Rc<WindowUi>, folder: &str, name: String) {
    if name.is_empty() {
        crate::log!("fileops", "an empty folder name was ignored");
        return;
    }
    ui.dispatch(
        Caller::Ui,
        Invocation::new(
            "file.new-folder",
            vec![
                ("name".to_string(), name),
                ("in".to_string(), folder.to_string()),
            ],
        ),
    );
}

/// `Copy To…` / `Move To…`: choose a folder, then run the command with `to=`.
fn choose_folder(ui: &Rc<WindowUi>, command: &'static str, paths: &[PathBuf]) {
    let initial = ui.focused_folder();
    let title = if command == "file.copy" {
        "Copy To…"
    } else {
        "Move To…"
    };
    let mut builder = gtk::FileDialog::builder().modal(true).title(title);
    if let Some(folder) = &initial {
        builder = builder.initial_folder(&gio::File::for_path(folder));
    }
    let dialog = builder.build();
    let ui_ok = ui.clone();
    let paths = paths.to_vec();
    dialog.select_folder(
        Some(ui.window()),
        None::<&gio::Cancellable>,
        move |result| {
            let Ok(folder) = result else {
                crate::log!("fileops", "{command}: no folder chosen");
                return;
            };
            let Some(folder) = folder.path() else {
                crate::log!("fileops", "{command}: the chosen folder has no path");
                return;
            };
            let mut arguments: Vec<(String, String)> = paths
                .iter()
                .map(|path| ("paths".to_string(), path.to_string_lossy().into_owned()))
                .collect();
            arguments.push(("to".to_string(), folder.to_string_lossy().into_owned()));
            ui_ok.dispatch(Caller::Ui, Invocation::new(command, arguments));
        },
    );
}

/// One dialog per conflict, in the plan's order. "Apply this decision to all
/// files" answers every remaining conflict; Cancel runs nothing.
pub fn show_conflicts(ui: &Rc<WindowUi>, flow: ConflictFlow, done: ConflictDone) {
    present_conflict(ui, Rc::new(std::cell::RefCell::new(flow)), done);
}

fn present_conflict(
    ui: &Rc<WindowUi>,
    state: Rc<std::cell::RefCell<ConflictFlow>>,
    done: ConflictDone,
) {
    let Some(question) = state.borrow().question() else {
        done(state.borrow().decisions());
        return;
    };

    let dialog = adw::AlertDialog::new(
        Some(&format!("“{}” already exists", question.name)),
        Some(&format!(
            "A file or folder with this name already exists in the target folder. ({} of {})",
            question.position, question.total
        )),
    );
    dialog.add_css_class("gimbal-look");
    dialog.add_response("skip", "Skip");
    dialog.add_response("keep-both", "Keep Both");
    dialog.add_response("replace", "Replace");
    dialog.add_response("cancel", "Cancel");
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");
    dialog.set_default_response(Some("skip"));

    let apply = question.apply_all.then(|| {
        let check = gtk::CheckButton::with_label("Apply this decision to all files");
        check.add_css_class("gl-conflict-apply");
        dialog.set_extra_child(Some(&check));
        check
    });

    ui.begin_dialog();
    let ui_response = ui.clone();
    let ui_closed = ui.clone();
    let item = question.item.clone();
    let name = question.name.clone();
    let position = question.position;
    let state_response = state.clone();
    let done_response = done.clone();
    let answered = Rc::new(std::cell::Cell::new(false));
    let answered_response = answered.clone();
    dialog.connect_response(None, move |_, response| {
        answered_response.set(true);
        let decision = match response {
            "keep-both" => Decision::KeepBoth,
            "replace" => Decision::Replace,
            "cancel" => Decision::Cancel,
            _ => Decision::Skip,
        };
        let apply_to_all = apply.as_ref().is_some_and(|check| check.is_active());
        crate::log!(
            "fileops",
            "decision {position} {name}: {decision:?} (apply to all: {apply_to_all})"
        );
        let next = state_response
            .borrow_mut()
            .answer(&item, decision, apply_to_all);
        if next.is_some() {
            present_conflict(&ui_response, state_response.clone(), done_response.clone());
        } else {
            done_response(state_response.borrow().decisions());
        }
    });
    let state_closed = state.clone();
    let done_closed = done.clone();
    dialog.connect_closed(move |_| {
        ui_closed.end_dialog();
        // A window closed underneath an unanswered dialog must not leave the
        // worker waiting forever: cancel this question. libadwaita emits
        // `closed` *before* `response` when a button is clicked
        // (adw-alert-dialog.c:278-279), so check from an idle, after the
        // response handler has run (Claude's P2k review: every answer was
        // being turned into Cancel).
        let answered = answered.clone();
        let state_closed = state_closed.clone();
        let done_closed = done_closed.clone();
        glib::idle_add_local_once(move || {
            if answered.get() {
                return;
            }
            let mut flow = state_closed.borrow_mut();
            if let Some(question) = flow.question() {
                crate::log!(
                    "fileops",
                    "conflict dialog closed without an answer; cancelling {}",
                    question.name
                );
                flow.answer(&question.item, Decision::Cancel, false);
            }
            done_closed(flow.decisions());
        });
    });
    dialog.choose(Some(ui.window()), None::<&gio::Cancellable>, |_| {});
}

/// The details behind a status summary: each failed item and its message.
pub fn show_details(ui: &Rc<WindowUi>, text: &str) {
    let dialog = adw::AlertDialog::new(Some("File operation details"), Some(text));
    dialog.add_css_class("gimbal-look");
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    ui.begin_dialog();
    let ui_closed = ui.clone();
    dialog.connect_closed(move |_| ui_closed.end_dialog());
    dialog.choose(Some(ui.window()), None::<&gio::Cancellable>, |_| {});
}

fn take_down(popover: &gtk::Popover) {
    popover.popdown();
    if popover.parent().is_some() {
        popover.unparent();
    }
}

fn close_on_closed(popover: &gtk::Popover) {
    popover.connect_closed(|popover| {
        if popover.parent().is_some() {
            popover.unparent();
        }
    });
}

fn hbox(classes: &[&str]) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    for class in classes {
        box_.add_css_class(class);
    }
    box_
}

fn vbox(classes: &[&str]) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for class in classes {
        box_.add_css_class(class);
    }
    box_
}
