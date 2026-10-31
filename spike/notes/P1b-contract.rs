// SPDX-License-Identifier: GPL-3.0-or-later
// The Phase 1 hook contract as accepted in the P1b review (Claude, Oct 31, 2026).
// Not compiled. Later briefs must keep these signatures exactly; reviews diff against this file.

// dnd.rs (P1c). Called from listing.rs: row factory `setup`, and right after the view is created.
pub fn attach_drag_source(row: &gtk::Widget, list_item: &gtk::ListItem, selection: &gtk::MultiSelection);
pub fn attach_drop_target(view: &gtk::Widget, dir: &Rc<RefCell<gtk::gio::File>>);

// tabs.rs (P1d). Called from window::new_window.
pub fn setup_tab_view(window: &adw::ApplicationWindow, tab_view: &adw::TabView);

// theme.rs (P1f). Connected to the application's `startup` signal in main.rs.
pub fn init(app: &adw::Application);

// window.rs (used by P1d).
pub fn new_window(app: &adw::Application) -> (adw::ApplicationWindow, adw::TabView);
pub fn open_dir_in_tab(tab_view: &adw::TabView, dir: &gtk::gio::File) -> adw::TabPage;
