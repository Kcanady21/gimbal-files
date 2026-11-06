// SPDX-License-Identifier: GPL-3.0-or-later
//! The Open With submenu, its chooser window, and the launch itself.
//!
//! `file.open-with` resolves the selection and the desktop id. The submenu is
//! only a view over `fm_core::openwith`; the chooser collects a choice and
//! dispatches the same command. The launch uses `gio::AppInfo::launch_uris`
//! with `file://` URIs and the window's GDK launch context, so the compositor
//! can focus the application that opens.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use fm_core::command::{Caller, Invocation, Outcome};
use fm_core::openwith::{self, AppEntry, ChooserSections};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::frame::WindowUi;
use crate::look::tokens::{PIN_ROW_H, SEARCH_W, scaled};

/// One GIO application together with the plain facts the pure rules use.
#[derive(Clone)]
struct Candidate {
    facts: AppEntry,
    info: gio::AppInfo,
}

/// Open the submenu under `Open With` for `paths`. The popover is anchored to
/// the widget that opened the item menu, exactly as the Settings submenu is
/// anchored to its button.
pub fn show_submenu(ui: &Rc<WindowUi>, anchor: &gtk::Widget, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }

    let types = content_types(&paths);
    let candidates = catalog(&types);
    let default = default_candidate(&types, &candidates);
    let recommended = recommended(&types, &candidates);
    let default_id = default.as_ref().map(|candidate| candidate.facts.id.clone());
    let entries = fm_core::openwith::submenu_apps(
        default.as_ref().map(|candidate| &candidate.facts),
        &recommended,
        &types,
    );

    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
    menu.add_css_class("gl-menu");

    let by_id: HashMap<String, Candidate> = candidates
        .into_iter()
        .map(|candidate| (candidate.facts.id.clone(), candidate))
        .collect();

    for entry in &entries {
        let candidate = by_id.get(&entry.id);
        let label = if default_id.as_deref() == Some(entry.id.as_str()) {
            format!("{} (default)", entry.name)
        } else {
            entry.name.clone()
        };
        let button = menu_row(
            candidate.and_then(|candidate| candidate.info.icon()),
            &label,
        );
        let popover_click = popover.clone();
        let ui_click = ui.clone();
        let app_id = entry.id.clone();
        let paths_click = paths.clone();
        button.connect_clicked(move |_| {
            take_down(&popover_click);
            ui_click.dispatch(
                Caller::Ui,
                Invocation::new("file.open-with", open_arguments(&app_id, &paths_click)),
            );
        });
        menu.append(&button);
    }

    let other = menu_row(None, "Other Application…");
    let popover_other = popover.clone();
    let ui_other = ui.clone();
    let paths_other = paths.clone();
    other.connect_clicked(move |_| {
        take_down(&popover_other);
        // Build the chooser after the submenu's popup surface is gone.
        let ui = ui_other.clone();
        let paths = paths_other.clone();
        glib::idle_add_local_once(move || show_chooser(&ui, paths));
    });
    menu.append(&other);

    popover.set_child(Some(&menu));
    popover.connect_closed(|popover| {
        if popover.parent().is_some() {
            popover.unparent();
        }
    });
    crate::popup::show(&popover);
}

/// The chooser window: a search entry, Suggested and All Applications sections,
/// the opt-in default checkbox, and Cancel/Open.
pub fn show_chooser(ui: &Rc<WindowUi>, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }

    let types = content_types(&paths);
    let candidates = catalog(&types);
    let facts: Vec<AppEntry> = candidates
        .iter()
        .map(|candidate| candidate.facts.clone())
        .collect();
    let sections = Rc::new(fm_core::openwith::chooser_sections(&facts, &types));
    let infos: Rc<HashMap<String, gio::AppInfo>> = Rc::new(
        candidates
            .into_iter()
            .map(|candidate| (candidate.facts.id.clone(), candidate.info))
            .collect(),
    );

    let window = gtk::Window::builder()
        .title("Open With")
        .modal(true)
        .transient_for(ui.window())
        .build();
    window.add_css_class("gimbal-look");
    window.add_css_class("gl-openwith");
    window.set_hide_on_close(false);
    window.set_destroy_with_parent(true);
    let scale = ui.process_upgrade().map_or(1.0, |process| process.scale());
    window.set_default_size(scaled(SEARCH_W, scale), -1);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("gl-openwith-root");
    window.set_child(Some(&root));

    let title = gtk::Label::builder()
        .label("Open With")
        .halign(gtk::Align::Start)
        .build();
    title.add_css_class("gl-title");
    root.append(&title);

    let search = gtk::Entry::new();
    search.set_placeholder_text(Some("Search applications"));
    search.set_hexpand(true);
    let field = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    field.add_css_class("gl-field");
    field.append(&search);
    root.append(&field);

    let list = gtk::ListBox::new();
    list.add_css_class("gl-openwith-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(false);

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_max_content_height(scaled(PIN_ROW_H * 6.0, scale));
    scroller.set_propagate_natural_height(true);
    scroller.set_child(Some(&list));
    root.append(&scroller);

    let ids: Rc<RefCell<Vec<Option<String>>>> = Rc::new(RefCell::new(Vec::new()));
    fill_list(&list, &ids, &sections, &infos, "");

    let always = (types.len() == 1).then(|| {
        let description = gio::content_type_get_description(&types[0]);
        let check = gtk::CheckButton::with_label(&format!("Always use for {description}"));
        check.add_css_class("gl-conflict-apply");
        check.set_active(false);
        root.append(&check);
        check
    });

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    actions.add_css_class("gl-openwith-actions");
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("gl-trash-action");
    let open = gtk::Button::with_label("Open");
    open.add_css_class("gl-trash-action");
    actions.append(&cancel);
    actions.append(&open);
    root.append(&actions);

    let window_cancel = window.clone();
    cancel.connect_clicked(move |_| window_cancel.close());

    let sections_search = sections.clone();
    let infos_search = infos.clone();
    let ids_search = ids.clone();
    let list_search = list.clone();
    search.connect_changed(move |entry| {
        fill_list(
            &list_search,
            &ids_search,
            &sections_search,
            &infos_search,
            entry.text().as_str(),
        );
    });

    let ui_enter = ui.clone();
    let window_enter = window.clone();
    let paths_enter = paths.clone();
    let types_enter = types.clone();
    let ids_enter = ids.clone();
    let list_enter = list.clone();
    let always_enter = always.clone();
    search.connect_activate(move |_| {
        let selected = list_enter.selected_row().and_then(|row| {
            ids_enter
                .borrow()
                .get(row.index() as usize)
                .cloned()
                .flatten()
        });
        if let Some(id) = selected {
            open_selected(
                &ui_enter,
                &window_enter,
                &paths_enter,
                &types_enter,
                always_enter.as_ref(),
                &id,
            );
        }
    });

    let ui_activate = ui.clone();
    let window_activate = window.clone();
    let paths_activate = paths.clone();
    let types_activate = types.clone();
    let ids_activate = ids.clone();
    let always_activate = always.clone();
    list.connect_row_activated(move |_, row| {
        let selected = ids_activate
            .borrow()
            .get(row.index() as usize)
            .cloned()
            .flatten();
        if let Some(id) = selected {
            open_selected(
                &ui_activate,
                &window_activate,
                &paths_activate,
                &types_activate,
                always_activate.as_ref(),
                &id,
            );
        }
    });

    let ui_open = ui.clone();
    let window_open = window.clone();
    let paths_open = paths.clone();
    let types_open = types.clone();
    let ids_open = ids.clone();
    let list_open = list.clone();
    let always_open = always.clone();
    open.connect_clicked(move |_| {
        let selected = list_open.selected_row().and_then(|row| {
            ids_open
                .borrow()
                .get(row.index() as usize)
                .cloned()
                .flatten()
        });
        if let Some(id) = selected {
            open_selected(
                &ui_open,
                &window_open,
                &paths_open,
                &types_open,
                always_open.as_ref(),
                &id,
            );
        }
    });

    let window_escape = window.clone();
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            window_escape.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);

    window.present();
    search.grab_focus();
}

fn fill_list(
    list: &gtk::ListBox,
    ids: &RefCell<Vec<Option<String>>>,
    sections: &ChooserSections,
    infos: &HashMap<String, gio::AppInfo>,
    query: &str,
) {
    list.remove_all();
    let mut rows: Vec<Option<String>> = Vec::new();

    for (heading, apps) in [
        ("Suggested", &sections.suggested),
        ("All Applications", &sections.all),
    ] {
        let shown: Vec<&AppEntry> = apps
            .iter()
            .filter(|app| openwith::search_matches(query, app))
            .collect();
        if shown.is_empty() {
            continue;
        }
        append_heading(list, heading);
        rows.push(None);
        for app in shown {
            let icon = infos.get(&app.id).and_then(|info| info.icon());
            append_app_row(list, app, icon.as_ref());
            rows.push(Some(app.id.clone()));
        }
    }

    if rows.iter().all(Option::is_none) {
        append_heading(list, "No applications match.");
        rows.push(None);
    }

    *ids.borrow_mut() = rows;
    let first = ids.borrow().iter().position(Option::is_some);
    if let Some(index) = first
        && let Some(row) = list.row_at_index(index as i32)
    {
        list.select_row(Some(&row));
    }
}

fn append_heading(list: &gtk::ListBox, text: &str) {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);
    let label = gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .build();
    label.add_css_class("gl-openwith-heading");
    row.set_child(Some(&label));
    list.append(&row);
}

fn append_app_row(list: &gtk::ListBox, app: &AppEntry, icon: Option<&gio::Icon>) {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("gl-openwith-row");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    content.append(&app_image(icon));
    let label = gtk::Label::builder()
        .label(&app.name)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    content.append(&label);
    row.set_child(Some(&content));
    list.append(&row);
}

fn menu_row(icon: Option<gio::Icon>, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("gl-menu-item");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.append(&app_image(icon.as_ref()));
    row.append(
        &gtk::Label::builder()
            .label(label)
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build(),
    );
    button.set_child(Some(&row));
    button
}

fn app_image(icon: Option<&gio::Icon>) -> gtk::Image {
    let image = match icon {
        Some(icon) => gtk::Image::from_gicon(icon),
        None => gtk::Image::from_icon_name("application-x-executable-symbolic"),
    };
    image.add_css_class("gl-app-icon");
    image
}

fn open_selected(
    ui: &Rc<WindowUi>,
    window: &gtk::Window,
    paths: &[PathBuf],
    types: &[String],
    always: Option<&gtk::CheckButton>,
    app_id: &str,
) {
    let process = ui.process_upgrade();
    let mut default_error = None;

    if always.is_some_and(|check| check.is_active())
        && types.len() == 1
        && let Some(process) = &process
    {
        let invocation = Invocation::new(
            "file.set-default-app",
            vec![
                ("app".to_string(), app_id.to_string()),
                ("type".to_string(), types[0].clone()),
            ],
        );
        if let Outcome::Failed(message) =
            process.dispatch_with_outcome(ui.id, Caller::Ui, &invocation)
        {
            default_error = Some(message);
        }
    }

    window.close();

    let outcome = process.map(|process| {
        process.dispatch_with_outcome(
            ui.id,
            Caller::Ui,
            &Invocation::new("file.open-with", open_arguments(app_id, paths)),
        )
    });
    if matches!(outcome, Some(Outcome::Done))
        && let Some(message) = default_error
    {
        ui.show_summary(&format!("The default wasn't changed: {message}"), None);
    }
}

fn open_arguments(app_id: &str, paths: &[PathBuf]) -> Vec<(String, String)> {
    let mut arguments = vec![("app".to_string(), app_id.to_string())];
    arguments.extend(
        paths
            .iter()
            .map(|path| ("paths".to_string(), path.to_string_lossy().into_owned())),
    );
    arguments
}

/// Launch `paths` with the application named by its desktop id.
pub fn launch_with(ui: &Rc<WindowUi>, app_id: &str, paths: &[PathBuf]) -> Result<(), String> {
    let Some(app) = find_app(app_id) else {
        let message = fm_core::openwith::OpenWithProblem::UnknownApp(app_id.to_string()).message();
        ui.show_summary(&message, None);
        return Err(message);
    };

    let facts = AppEntry::new(app_id, app.name().to_string());
    let facts = AppEntry {
        should_show: app.should_show(),
        ..facts
    };
    if let Err(problem) = fm_core::openwith::check_app(app_id, Some(&facts)) {
        let message = problem.message();
        ui.show_summary(&message, None);
        return Err(message);
    }

    if let Some(folder) = fm_core::openwith::folder_in_selection(paths, |path| path.is_dir()) {
        let message = fm_core::openwith::OpenWithProblem::Folder(folder).message();
        ui.show_summary(&message, None);
        return Err(message);
    }

    let types = content_types(paths);
    let uris: Vec<String> = paths
        .iter()
        .map(|path| gio::File::for_path(path).uri().to_string())
        .collect();
    let uris: Vec<&str> = uris.iter().map(String::as_str).collect();
    let context = ui.display().app_launch_context();
    if let Err(error) = app.launch_uris(&uris, Some(&context)) {
        let message = format!(
            "Could not open {} with {}: {error}",
            selection_name(paths),
            app.name()
        );
        crate::log!(
            "openwith",
            "could not launch {app_id} with {} file(s), types {}: {error}",
            paths.len(),
            types.join(", ")
        );
        ui.show_summary(&message, None);
        return Err(message);
    }

    for content_type in &types {
        if let Err(error) = app.set_as_last_used_for_type(content_type) {
            crate::log!(
                "openwith",
                "could not remember {app_id} for {content_type}: {error}"
            );
        }
    }

    crate::log!(
        "openwith",
        "launched {app_id} with {} file(s), types {}",
        paths.len(),
        types.join(", ")
    );
    Ok(())
}

/// Make `app_id` the default for `content_type`, through GIO.
pub fn set_default_app(app_id: &str, content_type: &str) -> Result<(), String> {
    let Some(app) = find_app(app_id) else {
        return Err(fm_core::openwith::OpenWithProblem::UnknownApp(app_id.to_string()).message());
    };
    app.set_as_default_for_type(content_type)
        .map_err(|error| error.to_string())?;
    crate::log!("openwith", "default for {content_type} is now {app_id}");
    Ok(())
}

fn find_app(app_id: &str) -> Option<gio::AppInfo> {
    gio::AppInfo::all()
        .into_iter()
        .find(|app| app.id().is_some_and(|id| id.as_str() == app_id))
}

fn selection_name(paths: &[PathBuf]) -> String {
    match paths {
        [path] => path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned(),
        _ => format!("{} files", paths.len()),
    }
}

/// The selected files' content types, in selection order, without duplicates.
/// GIO types every empty file `application/x-zerosize`; the name's type then
/// stands in, the same fallback `file.open-as-admin` uses.
fn content_types(paths: &[PathBuf]) -> Vec<String> {
    let mut types = Vec::new();
    for path in paths {
        let queried = gio::File::for_path(path)
            .query_info(
                "standard::content-type",
                gio::FileQueryInfoFlags::NONE,
                None::<&gio::Cancellable>,
            )
            .ok()
            .and_then(|info| info.content_type())
            .map(|content_type| content_type.to_string())
            .unwrap_or_else(|| gio::content_type_guess(Some(path), None).0.to_string());
        let by_name = gio::content_type_guess(Some(path), None).0.to_string();
        let content_type = fm_core::admin::lookup_type(&queried, &by_name).to_string();
        if !types.contains(&content_type) {
            types.push(content_type);
        }
    }
    types
}

fn catalog(types: &[String]) -> Vec<Candidate> {
    let mut order = Vec::new();
    let mut infos: HashMap<String, gio::AppInfo> = HashMap::new();
    let mut handles: HashMap<String, Vec<String>> = HashMap::new();

    for info in gio::AppInfo::all() {
        note_app(&info, None, &mut order, &mut infos, &mut handles);
    }
    for content_type in types {
        for info in gio::AppInfo::all_for_type(content_type) {
            note_app(
                &info,
                Some(content_type),
                &mut order,
                &mut infos,
                &mut handles,
            );
        }
        for info in gio::AppInfo::recommended_for_type(content_type) {
            note_app(
                &info,
                Some(content_type),
                &mut order,
                &mut infos,
                &mut handles,
            );
        }
        if let Some(info) = gio::AppInfo::default_for_type(content_type, false) {
            note_app(
                &info,
                Some(content_type),
                &mut order,
                &mut infos,
                &mut handles,
            );
        }
    }

    order
        .into_iter()
        .filter_map(|id| {
            let info = infos.remove(&id)?;
            let mut facts = AppEntry::new(id.clone(), info.name().to_string());
            facts.should_show = info.should_show();
            facts.handles = handles.remove(&id).unwrap_or_default();
            facts.keywords = desktop_keywords(&info);
            Some(Candidate { facts, info })
        })
        .collect()
}

fn note_app(
    info: &gio::AppInfo,
    content_type: Option<&String>,
    order: &mut Vec<String>,
    infos: &mut HashMap<String, gio::AppInfo>,
    handles: &mut HashMap<String, Vec<String>>,
) {
    let Some(id) = info.id().map(|id| id.to_string()) else {
        return;
    };
    if !infos.contains_key(&id) {
        order.push(id.clone());
        infos.insert(id.clone(), info.clone());
    }
    if let Some(content_type) = content_type {
        let list = handles.entry(id).or_default();
        if !list.contains(content_type) {
            list.push(content_type.clone());
        }
    }
}

fn default_candidate(types: &[String], candidates: &[Candidate]) -> Option<Candidate> {
    let content_type = types.first()?;
    let default = gio::AppInfo::default_for_type(content_type, false)?;
    let id = default.id()?;
    candidates
        .iter()
        .find(|candidate| candidate.facts.id == id.as_str())
        .cloned()
}

fn recommended(types: &[String], candidates: &[Candidate]) -> Vec<AppEntry> {
    let mut recommended = Vec::new();
    for content_type in types {
        for info in gio::AppInfo::recommended_for_type(content_type) {
            let Some(id) = info.id() else {
                continue;
            };
            if let Some(candidate) = candidates
                .iter()
                .find(|candidate| candidate.facts.id == id.as_str())
            {
                recommended.push(candidate.facts.clone());
            }
        }
    }
    recommended
}

/// The desktop entry's `Keywords` field. GLib 2.88 moved `GDesktopAppInfo`
/// into the GioUnix namespace, which gio 0.22.10 does not expose, so the
/// entry named by `AppInfo::id` is read through the data directories.
fn desktop_keywords(app: &gio::AppInfo) -> Vec<String> {
    let Some(id) = app.id() else {
        return Vec::new();
    };
    let mut directories = vec![glib::user_data_dir().join("applications")];
    directories.extend(
        glib::system_data_dirs()
            .into_iter()
            .map(|directory| directory.join("applications")),
    );
    for directory in [
        glib::user_data_dir().join("flatpak/exports/share/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
        PathBuf::from("/var/lib/snapd/desktop/applications"),
    ] {
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }

    let file = glib::KeyFile::new();
    if file
        .load_from_dirs(
            Path::new(id.as_str()),
            &directories,
            glib::KeyFileFlags::NONE,
        )
        .is_err()
    {
        return Vec::new();
    }
    let Ok(keywords) = file.value("Desktop Entry", "Keywords") else {
        return Vec::new();
    };
    keywords
        .split(';')
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
        .map(str::to_string)
        .collect()
}

fn take_down(popover: &gtk::Popover) {
    popover.popdown();
    if popover.parent().is_some() {
        popover.unparent();
    }
}
