// SPDX-License-Identifier: GPL-3.0-or-later
use gtk::gio;
use gtk::gio::prelude::ListModelExt;
use gtk::glib::WeakRef;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering as CmpOrdering;
use std::rc::Rc;
use std::time::Instant;
// GTK prepends standard::name (gtkdirectorylist.c:559); size/is-symlink are dropped, nothing reads them.
const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::icon";
const STATE_KEY: &str = "gimbal-spike-folder-state";
const ICON_PX: i32 = 64;
const CELL_WIDTH: i32 = 128;
const LABEL_CHARS: i32 = 14;
// max-columns also sizes the pooled cells (gtkgridview.c:1236-1238, gtklistbase.c:2327); G_MAXUINT is fatal (890 MB), 64 covers 8192 px.
const MAX_COLUMNS: u32 = 64;
type NavigateCallback = Rc<dyn Fn(&gio::File)>;
struct State {
    dir: Rc<RefCell<gio::File>>,
    dir_list: WeakRef<gtk::DirectoryList>,
    started: Cell<Instant>,
    on_navigate: RefCell<Option<NavigateCallback>>,
}
/// Builds one folder view: DirectoryList -> SortListModel -> MultiSelection -> GridView.
pub fn build(dir: &gio::File, select: &[String]) -> gtk::ScrolledWindow {
    let dir_list = gtk::DirectoryList::new(Some(ATTRIBUTES), None::<&gio::File>);
    let first_logged = Rc::new(Cell::new(false));
    dir_list.connect_items_changed(move |list, _, _, added| {
        if added > 0 && !first_logged.replace(true) {
            crate::log!(
                "listing",
                "first items in model: +{} ({} so far)",
                added,
                list.n_items()
            );
        }
    });
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
    // Incremental TRUE sorts after load and the anchor follows its first item (gtklistitemmanager.c:1546); FALSE sorts each batch inline.
    let sorted = gtk::SortListModel::new(Some(dir_list.clone()), Some(sorter));
    let selection = gtk::MultiSelection::new(Some(sorted));
    let state = Rc::new(State {
        dir: Rc::new(RefCell::new(dir.clone())),
        dir_list: dir_list.downgrade(),
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
            let cell = make_cell();
            item.set_child(Some(&cell));
            crate::dnd::attach_drag_source(cell.upcast_ref(), item, &selection);
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
    let grid_view = gtk::GridView::new(Some(selection.clone()), Some(factory));
    // One until the viewport width is known; the page-size handler below raises it.
    grid_view.set_max_columns(1);
    grid_view.set_min_columns(1);
    {
        let state = Rc::downgrade(&state);
        let selection = selection.clone();
        grid_view.connect_activate(move |_, position| {
            let Some(state) = state.upgrade() else { return };
            let info = info_at(&selection, position);
            if info.file_type() != gio::FileType::Directory {
                return;
            }
            let target = {
                let dir = state.dir.borrow();
                dir.child(info.name())
            };
            navigate(&state, &target);
        });
    }
    scrolled.set_child(Some(&grid_view));
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scrolled.set_vexpand(true);
    {
        // max-columns also sizes the pool (gtkgridview.c:1236-1238, gtklistbase.c:2327), and every
        // pooled cell becomes an AT-SPI object once the grid has focus. Follow the viewport width.
        // Weak: the grid holds a ref to this adjustment, so a strong capture here would cycle.
        let view = grid_view.downgrade();
        scrolled
            .hadjustment()
            .connect_notify_local(Some("page-size"), move |adjustment, _| {
                let Some(view) = view.upgrade() else { return };
                let columns = adjustment.page_size() as u32 / CELL_WIDTH as u32 + 1;
                let columns = columns.clamp(1, MAX_COLUMNS);
                if view.max_columns() != columns {
                    view.set_max_columns(columns);
                }
            });
    }
    let pending = RefCell::new(select.to_vec());
    // Weak State/selection/view: a strong one cycles with the model chain (P1b review item 2).
    let state = Rc::downgrade(&state);
    let selection = selection.downgrade();
    let view = grid_view.downgrade();
    dir_list.connect_loading_notify(move |dir_list| {
        if dir_list.is_loading() {
            return;
        }
        let Some(state) = state.upgrade() else { return };
        let path = display_path(&dir_list.file().unwrap());
        crate::log!(
            "listing",
            "folder load finished: {} ({} items, {} ms)",
            path,
            dir_list.n_items(),
            state.started.get().elapsed().as_millis()
        );
        let Some(selection) = selection.upgrade() else {
            return;
        };
        let names = std::mem::take(&mut *pending.borrow_mut());
        let mut first = 0;
        let mut found = 0;
        for position in 0..selection.n_items() {
            let info = info_at(&selection, position);
            let wanted = names
                .iter()
                .any(|n| info.name().as_os_str() == std::ffi::OsStr::new(n));
            if wanted {
                let _ = selection.select_item(position, false);
                if found == 0 {
                    first = position;
                }
                found += 1;
            }
        }
        if !names.is_empty() {
            crate::log!("listing", "selected {found}/{} name(s)", names.len());
        }
        log_cell_widths(&selection);
        show_item(view.upgrade(), first);
    });
    crate::log!("listing", "folder load started: {}", display_path(dir));
    dir_list.set_file(Some(dir));
    scrolled
}
pub fn navigate_up(view: &gtk::Widget) {
    let Some(state) = state(view) else { return };
    // The borrow ends before navigate(), which borrows the same cell mutably.
    let parent = state.dir.borrow().parent();
    if let Some(parent) = parent {
        navigate(&state, &parent);
    }
}
pub fn dir_of(view: &gtk::Widget) -> Option<gio::File> {
    state(view).map(|state| state.dir.borrow().clone())
}
pub fn display_path(file: &gio::File) -> String {
    file.path()
        .map_or_else(|| file.uri().to_string(), |p| p.display().to_string())
}
pub fn set_on_navigate(view: &gtk::Widget, callback: impl Fn(&gio::File) + 'static) {
    if let Some(state) = state(view) {
        *state.on_navigate.borrow_mut() = Some(Rc::new(callback));
    }
}
fn navigate(state: &State, target: &gio::File) {
    *state.dir.borrow_mut() = target.clone();
    state.started.set(Instant::now());
    crate::log!("listing", "folder load started: {}", display_path(target));
    if let Some(dir_list) = state.dir_list.upgrade() {
        dir_list.set_file(Some(target));
    }
    // Clone the callback out: holding the RefCell borrow across the call would
    // deadlock if the callback ever navigated again.
    let callback = state.on_navigate.borrow().as_ref().cloned();
    if let Some(callback) = callback {
        callback(target);
    }
}
fn make_cell() -> gtk::Box {
    let cell = gtk::Box::new(gtk::Orientation::Vertical, 4);
    cell.set_size_request(CELL_WIDTH, -1);
    let icon = gtk::Image::new();
    icon.set_pixel_size(ICON_PX);
    let label = gtk::Label::new(None);
    label.set_xalign(0.5);
    label.set_justify(gtk::Justification::Center);
    label.set_wrap(true);
    // Char: a 32-char hex run has no break point; with Word it sets the column width (gtklabel.c:1283-1296).
    label.set_wrap_mode(gtk::pango::WrapMode::Char);
    label.set_lines(2);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_chars(LABEL_CHARS);
    label.set_max_width_chars(LABEL_CHARS);
    cell.append(&icon);
    cell.append(&label);
    cell
}
fn show_item(view: Option<gtk::GridView>, position: u32) {
    let Some(view) = view else { return };
    if position == 0 {
        // scroll_to() needs an allocated area (gtklistbase.c:899); the adjustment re-anchors the top (182-262).
        if view.is_mapped() {
            view.scroll_to(0, gtk::ListScrollFlags::NONE, None);
        }
        let _ = view.vadjustment().inspect(|a| a.set_value(0.0));
        return;
    }
    view.scroll_to(position, gtk::ListScrollFlags::NONE, None);
}
fn log_cell_widths(selection: &gtk::MultiSelection) {
    if !crate::log_enabled() || selection.n_items() == 0 {
        return;
    }
    let mut shortest = (usize::MAX, info_at(selection, 0));
    let mut longest = (0, shortest.1.clone());
    for position in 0..selection.n_items() {
        let info = info_at(selection, position);
        let len = info.display_name().chars().count();
        if len < shortest.0 {
            shortest = (len, info.clone());
        }
        if len > longest.0 {
            longest = (len, info);
        }
    }
    let (short_min, short_nat) = measure_cell(&shortest.1);
    let (long_min, long_nat) = measure_cell(&longest.1);
    let equal = short_min == long_min && short_nat == long_nat;
    let widths = format!("short {short_min}/{short_nat}, long {long_min}/{long_nat}");
    crate::log!("listing", "cell width min/nat {widths}, equal {equal}");
}
fn measure_cell(info: &gio::FileInfo) -> (i32, i32) {
    let cell = make_cell();
    let icon = cell
        .first_child()
        .unwrap()
        .downcast::<gtk::Image>()
        .unwrap();
    let label = cell.last_child().unwrap().downcast::<gtk::Label>().unwrap();
    if let Some(gicon) = info.icon() {
        icon.set_from_gicon(&gicon);
    }
    label.set_label(&info.display_name());
    let (minimum, natural, _, _) = cell.measure(gtk::Orientation::Horizontal, -1);
    (minimum, natural)
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
