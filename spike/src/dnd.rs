// SPDX-License-Identifier: GPL-3.0-or-later
use std::cell::RefCell;
use std::rc::Rc;

// Filled by P1c
pub fn attach_drag_source(
    row: &gtk::Widget,
    list_item: &gtk::ListItem,
    selection: &gtk::MultiSelection,
) {
    let _ = (row, list_item, selection);
}
// Filled by P1c
pub fn attach_drop_target(view: &gtk::Widget, dir: &Rc<RefCell<gtk::gio::File>>) {
    let _ = (view, dir);
}
