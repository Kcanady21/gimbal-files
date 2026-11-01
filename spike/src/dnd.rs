// SPDX-License-Identifier: GPL-3.0-or-later
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

const ICON_SIZE: i32 = 48;

// Drag out: one DragSource per row, like Nautilus (P0e Q1).
pub fn attach_drag_source(
    row: &gtk::Widget,
    list_item: &gtk::ListItem,
    selection: &gtk::MultiSelection,
) {
    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
    let (selection, item) = (selection.clone(), list_item.clone());
    source.connect_prepare(move |source, _, _| {
        let pressed = item.item()?.downcast::<gio::FileInfo>().ok()?;
        let dir = dir_of_selection(&selection)?;
        let files: Vec<gio::File> = if selection.is_selected(item.position()) {
            (0..selection.n_items())
                .filter(|p| selection.is_selected(*p))
                .filter_map(|p| selection.item(p))
                .filter_map(|o| o.downcast::<gio::FileInfo>().ok())
                .map(|info| dir.child(info.name()))
                .collect()
        } else {
            vec![dir.child(pressed.name())]
        };
        let first = files
            .first()
            .map(crate::listing::display_path)
            .unwrap_or_default();
        crate::log!("dnd", "prepare: {} file(s), first {first}", files.len());
        set_icon(source, Some(&pressed));
        // gdk::FileList::from_array panics on an empty slice (glib 0.22.10
        // boxed.rs !ptr.is_null()); return None instead of building one.
        if files.is_empty() {
            return None;
        }
        // One typed provider: GTK adds text/uri-list, text/plain;charset=utf-8
        // and both portal types itself; no MIME is added by hand (P0e).
        Some(gdk::ContentProvider::for_value(
            &gdk::FileList::from_array(&files).to_value(),
        ))
    });
    source.connect_drag_end(|_, drag, delete_data| {
        // GTK4 sets delete_data when a drop succeeds with MOVE, and Firefox
        // reports MOVE on every Wayland drop (P0e review 2), so honouring it
        // would destroy sources on a copy drag. Moves are done by the drop
        // side; the value is logged and ignored.
        crate::log!(
            "dnd",
            "drag-end: action={:?} delete_data={} (ignored)",
            drag.selected_action(),
            delete_data
        );
    });
    row.add_controller(source);
}

fn dir_of_selection(selection: &gtk::MultiSelection) -> Option<gio::File> {
    let sorted = selection.model()?.downcast::<gtk::SortListModel>().ok()?;
    let dir_list = sorted.model()?.downcast::<gtk::DirectoryList>().ok()?;
    dir_list.file()
}

fn set_icon(source: &gtk::DragSource, info: Option<&gio::FileInfo>) -> Option<()> {
    let gicon = info.and_then(gio::FileInfo::icon)?;
    let widget = source.widget()?;
    let paintable = gtk::IconTheme::for_display(&widget.display()).lookup_by_gicon(
        &gicon,
        ICON_SIZE,
        1,
        gtk::TextDirection::None,
        gtk::IconLookupFlags::NONE,
    );
    source.set_icon(Some(&paintable), ICON_SIZE / 2, ICON_SIZE / 2);
    Some(())
}

// Drop in: one DropTarget for the gdk::FileList GType; GTK turns text/uri-list
// and the portal types into a FileList (deserializer side).
pub fn attach_drop_target(view: &gtk::Widget, dir: &Rc<RefCell<gio::File>>) {
    let actions = gdk::DragAction::COPY | gdk::DragAction::MOVE;
    let target = gtk::DropTarget::new(gdk::FileList::static_type(), actions);
    let dir = dir.clone();
    target.connect_drop(move |target, value, _, _| {
        let Ok(file_list) = value.get::<gdk::FileList>() else {
            return false;
        };
        let files = file_list.files();
        let folder = dir.borrow().clone();
        let into = crate::listing::display_path(&folder);
        let action = target
            .current_drop()
            .and_then(|drop| drop.drag())
            .map_or(gdk::DragAction::empty(), |drag| drag.selected_action());
        crate::log!(
            "dnd",
            "drop: {} file(s), action={action:?} into {into}",
            files.len()
        );
        // This is a spike: every drop is a copy, even for a MOVE drop.
        let started = files.iter().filter(|file| copy_in(file, &folder)).count();
        started > 0
    });
    view.add_controller(target);
}

fn copy_in(file: &gio::File, folder: &gio::File) -> bool {
    let path = crate::listing::display_path(file);
    let file_type = file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE);
    if file_type != gio::FileType::Regular {
        let why = if file_type == gio::FileType::Directory {
            "folders not supported in the spike"
        } else {
            "not a regular file"
        };
        crate::log!("dnd", "skipped {path}: {why}");
        return false;
    }
    if file.parent().is_some_and(|parent| parent.equal(folder)) {
        crate::log!("dnd", "skipped {path}: dropped onto its own folder");
        return false;
    }
    let Some(name) = file.basename() else {
        return false;
    };
    let exists = |candidate: &str| folder.child(candidate).query_exists(gio::Cancellable::NONE);
    let dest = folder.child(copy_name(&name.to_string_lossy(), &exists));
    let dest_path = crate::listing::display_path(&dest);
    crate::log!("dnd", "copying {path} -> {dest_path}");
    // FileCopyFlags::NONE: never overwrite (the name is unique anyway).
    file.copy_async(
        &dest,
        gio::FileCopyFlags::NONE,
        glib::Priority::DEFAULT,
        None::<&gio::Cancellable>,
        None,
        move |result| match result {
            Ok(()) => crate::log!("dnd", "copied {path} -> {dest_path}"),
            Err(error) => crate::log!("dnd", "copy failed {path} -> {dest_path}: {error}"),
        },
    );
    true
}

// Pure: the extension runs to the first non-leading dot (a.tar.gz -> a (copy).tar.gz).
fn copy_name(name: &str, exists: &dyn Fn(&str) -> bool) -> String {
    if !exists(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.find('.') {
        Some(0) | None => (name, ""),
        Some(i) => (&name[..i], &name[i..]),
    };
    let mut candidate = format!("{stem} (copy){ext}");
    let mut n = 2;
    while exists(&candidate) {
        candidate = format!("{stem} (copy {n}){ext}");
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::copy_name;

    fn taken<'a>(names: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |candidate| names.contains(&candidate)
    }

    #[test]
    fn conflicts_get_copies() {
        assert_eq!(copy_name("a.txt", &|_| false), "a.txt");
        let first = taken(&["a.txt"]);
        assert_eq!(copy_name("a.txt", &first), "a (copy).txt");
        let many = taken(&["a.txt", "a (copy).txt", "a (copy 2).txt"]);
        assert_eq!(copy_name("a.txt", &many), "a (copy 3).txt");
    }

    #[test]
    fn extension_edge_cases() {
        let plain = taken(&["README"]);
        assert_eq!(copy_name("README", &plain), "README (copy)");
        let dotfile = taken(&[".bashrc"]);
        assert_eq!(copy_name(".bashrc", &dotfile), ".bashrc (copy)");
        let two_dots = taken(&["a.tar.gz"]);
        assert_eq!(copy_name("a.tar.gz", &two_dots), "a (copy).tar.gz");
    }
}
