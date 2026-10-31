// SPDX-License-Identifier: GPL-3.0-or-later
use gtk::gio;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering as CmpOrdering;
use std::rc::Rc;
use std::time::Instant;
const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::icon,standard::size,standard::is-symlink";
const STATE_KEY: &str = "gimbal-spike-folder-state";
type NavigateCallback = Box<dyn Fn(&gio::File)>;
struct State {
    dir: Rc<RefCell<gio::File>>,
    dir_list: gtk::DirectoryList,
    started: Cell<Instant>,
    on_navigate: RefCell<Option<NavigateCallback>>,
}
/// Builds one folder view: DirectoryList -> SortListModel -> MultiSelection -> ListView.
pub fn build(dir: &gio::File, select: &[String]) -> gtk::ScrolledWindow {
    let dir_list = gtk::DirectoryList::new(Some(ATTRIBUTES), None::<&gio::File>);
    let sorter = gtk::CustomSorter::new(|a, b| {
        let a = a.downcast_ref::<gio::FileInfo>().unwrap();
        let b = b.downcast_ref::<gio::FileInfo>().unwrap();
        compare_entries(
            a.file_type() == gio::FileType::Directory,
            &a.display_name(),
            b.file_type() == gio::FileType::Directory,
            &b.display_name(),
        )
    });
    let sorted = gtk::SortListModel::new(Some(dir_list.clone()), Some(sorter));
    let selection = gtk::MultiSelection::new(Some(sorted));
    let state = Rc::new(State {
        dir: Rc::new(RefCell::new(dir.clone())),
        dir_list: dir_list.clone(),
        started: Cell::new(Instant::now()),
        on_navigate: RefCell::new(None),
    });
    let scrolled = gtk::ScrolledWindow::new();
    attach(scrolled.upcast_ref(), state.clone());
    crate::dnd::attach_drop_target(scrolled.upcast_ref(), &state.dir);
    let factory = gtk::SignalListItemFactory::new();
    {
        let selection = selection.clone();
        factory.connect_setup(move |_, object| {
            let item = object.downcast_ref::<gtk::ListItem>().unwrap();
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let icon = gtk::Image::new();
            let label = gtk::Label::new(None);
            label.set_xalign(0.0);
            row.append(&icon);
            row.append(&label);
            item.set_child(Some(&row));
            crate::dnd::attach_drag_source(row.upcast_ref(), item, &selection);
        });
    }
    factory.connect_bind(|_, object| {
        let item = object.downcast_ref::<gtk::ListItem>().unwrap();
        let info = item.item().unwrap().downcast::<gio::FileInfo>().unwrap();
        let row = item.child().unwrap().downcast::<gtk::Box>().unwrap();
        let icon = row.first_child().unwrap().downcast::<gtk::Image>().unwrap();
        let label = row.last_child().unwrap().downcast::<gtk::Label>().unwrap();
        if let Some(gicon) = info.icon() {
            icon.set_from_gicon(&gicon);
        }
        label.set_label(&info.display_name());
    });
    let list_view = gtk::ListView::new(Some(selection.clone()), Some(factory));
    {
        let state = state.clone();
        let selection = selection.clone();
        list_view.connect_activate(move |_, position| {
            let info = info_at(&selection, position);
            if info.file_type() != gio::FileType::Directory {
                return;
            }
            let target = state.dir_list.file().unwrap().child(info.name());
            navigate(&state, &target);
        });
    }
    scrolled.set_child(Some(&list_view));
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scrolled.set_vexpand(true);
    let pending = RefCell::new(select.to_vec());
    dir_list.connect_loading_notify(move |dir_list| {
        if dir_list.is_loading() {
            return;
        }
        let path = display_path(&dir_list.file().unwrap());
        crate::log!(
            "listing",
            "folder load finished: {} ({} items, {} ms)",
            path,
            dir_list.n_items(),
            state.started.get().elapsed().as_millis()
        );
        let names = std::mem::take(&mut *pending.borrow_mut());
        let mut found = 0;
        for position in 0..selection.n_items() {
            let info = info_at(&selection, position);
            let wanted = names
                .iter()
                .any(|n| info.name().as_os_str() == std::ffi::OsStr::new(n));
            if wanted {
                let _ = selection.select_item(position, false);
                found += 1;
            }
        }
        if !names.is_empty() {
            crate::log!("listing", "selected {found}/{} name(s)", names.len());
        }
    });
    crate::log!("listing", "folder load started: {}", display_path(dir));
    dir_list.set_file(Some(dir));
    scrolled
}
pub fn navigate_up(view: &gtk::Widget) {
    let Some(state) = state(view) else { return };
    let Some(parent) = state.dir_list.file().unwrap().parent() else {
        return;
    };
    navigate(&state, &parent);
}
pub fn dir_of(view: &gtk::Widget) -> Option<gio::File> {
    state(view).and_then(|state| state.dir_list.file())
}
pub fn display_path(file: &gio::File) -> String {
    let path = file.path();
    path.map_or_else(|| file.uri().to_string(), |p| p.display().to_string())
}
pub fn set_on_navigate(view: &gtk::Widget, callback: impl Fn(&gio::File) + 'static) {
    if let Some(state) = state(view) {
        *state.on_navigate.borrow_mut() = Some(Box::new(callback));
    }
}
fn navigate(state: &State, target: &gio::File) {
    *state.dir.borrow_mut() = target.clone();
    state.started.set(Instant::now());
    crate::log!("listing", "folder load started: {}", display_path(target));
    state.dir_list.set_file(Some(target));
    if let Some(callback) = state.on_navigate.borrow().as_ref() {
        callback(target);
    }
}
fn attach(widget: &gtk::Widget, state: Rc<State>) {
    unsafe { widget.set_data(STATE_KEY, state) };
}
fn state(widget: &gtk::Widget) -> Option<Rc<State>> {
    let data = unsafe { widget.data::<Rc<State>>(STATE_KEY) };
    data.map(|s| unsafe { s.as_ref().clone() })
}
fn info_at(selection: &gtk::MultiSelection, position: u32) -> gio::FileInfo {
    selection.item(position).unwrap().downcast().unwrap()
}
fn compare_entries(a_is_dir: bool, a_name: &str, b_is_dir: bool, b_name: &str) -> gtk::Ordering {
    let order = b_is_dir
        .cmp(&a_is_dir)
        .then_with(|| a_name.to_lowercase().cmp(&b_name.to_lowercase()));
    match order {
        CmpOrdering::Less => gtk::Ordering::Smaller,
        CmpOrdering::Equal => gtk::Ordering::Equal,
        CmpOrdering::Greater => gtk::Ordering::Larger,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sorts_folders_first_then_names() {
        assert_eq!(
            compare_entries(true, "zulu", false, "Alpha"),
            gtk::Ordering::Smaller
        );
        assert_eq!(
            compare_entries(false, "alpha", false, "Beta"),
            gtk::Ordering::Smaller
        );
    }
}
