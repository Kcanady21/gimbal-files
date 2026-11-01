// SPDX-License-Identifier: GPL-3.0-or-later
//! The header: nav, expanding search, pill strip, path bar and window controls.
use crate::strip::{Closed, Entry, Model};
use crate::style;
use crate::{Args, log};
use adw::prelude::*;
use gtk::{gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const FADE_MS: u64 = 200;

/// The mockup's float icon, drawn as a `gtk::Svg` paintable: the icon theme's older
/// symbolic path ignores `gpa:` slots, so loading it by name would lose the fills.
static FLOAT_ICON: &[u8] = include_bytes!("icons/float.svg");

#[derive(Clone, Copy)]
struct DragState {
    index: usize,
    start_x: f64,
    start_y: f64,
    base: f64,
    to: usize,
}

struct Ui {
    window: adw::ApplicationWindow,
    strip: gtk::Fixed,
    pills: RefCell<Vec<gtk::Widget>>,
    drag: Cell<Option<DragState>>,
    /// The visible header width, from the scroller's horizontal page size.
    available: Cell<i32>,
    path: gtk::Box,
    path_label: gtk::Label,
    /// The `gl-frame` around the body; a click anywhere in it dismisses an empty search.
    frame: gtk::Box,
    body: gtk::Label,
    search_icon: gtk::Button,
    search: gtk::Revealer,
    search_field: gtk::Box,
    search_entry: gtk::Entry,
    scale: Cell<f64>,
    opacity: Cell<f64>,
    font: Option<String>,
    provider: gtk::CssProvider,
    blur: crate::blur::Blur,
    model: RefCell<Model>,
    overflow: Cell<bool>,
    search_open: Cell<bool>,
    animate_group: Cell<Option<usize>>,
    /// Non-zero while an `adw::AlertDialog` owns the keyboard.
    dialogs: Cell<u32>,
}

/// The one place where the desktop decides the window controls.
fn desktop_buttons() -> &'static [(&'static str, &'static str)] {
    if std::env::var_os("NIRI_SOCKET").is_some() {
        &[
            ("window-restore-symbolic", "float"),
            ("window-close-symbolic", "close"),
        ]
    } else {
        &[
            ("window-minimize-symbolic", "minimize"),
            ("window-maximize-symbolic", "maximize"),
            ("window-close-symbolic", "close"),
        ]
    }
}

pub fn show(app: &adw::Application, args: &Args) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Gimbal Files · Look Header")
        .default_width(style::DEFAULT_W)
        .default_height(style::DEFAULT_H)
        .build();
    window.add_css_class("gimbal-look");
    let scale = style::base() * args.scale;
    let opacity = style::opacity();
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&style::css(scale, args.font.as_deref(), opacity));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    let row = hbox(&["gl-header"]);
    let menu_button = icon_button("open-menu-symbolic", "nav");
    row.append(&menu_button);
    for (icon, name) in [
        ("go-previous-symbolic", "back"),
        ("go-next-symbolic", "forward"),
        ("go-up-symbolic", "up"),
    ] {
        let button = icon_button(icon, "nav");
        button.connect_clicked(move |_| log!("nav", "{name}"));
        row.append(&button);
    }
    let search_icon = icon_button("system-search-symbolic", "searchbtn");
    let field = hbox(&["gl-field", "gl-search-field"]);
    field.set_valign(gtk::Align::Center);
    let glyph = gtk::Image::from_icon_name("system-search-symbolic");
    glyph.add_css_class("gl-search-glyph");
    field.append(&glyph);
    let search_entry = gtk::Entry::new();
    search_entry.set_placeholder_text(Some("Search"));
    field.append(&search_entry);
    let search = gtk::Revealer::new();
    // Crossfade, not SlideRight: a slide translates the field left by
    // `width - child_width` and the revealer does not clip, so its outline painted
    // under the nav buttons while it closed (P1l item 2).
    search.set_transition_type(gtk::RevealerTransitionType::Crossfade);
    search.set_transition_duration(200);
    search.set_child(Some(&field));
    // A closed crossfade revealer still measures its child, so the field starts
    // hidden; `open_search` shows it again just before the fade in.
    field.set_visible(false);
    let search_box = hbox(&["gl-search"]);
    search_box.set_valign(gtk::Align::Center);
    search_box.append(&search_icon);
    search_box.append(&search);
    row.append(&search_box);
    let strip = gtk::Fixed::new();
    strip.add_css_class("gl-strip");
    strip.set_valign(gtk::Align::Center);
    row.append(&strip);
    // No spacer and no upper cap: the path stretches into every free pixel.
    let path = hbox(&["gl-field", "gl-path"]);
    path.set_valign(gtk::Align::Center);
    path.set_hexpand(true);
    let path_label = gtk::Label::builder()
        .label("/home/kyle/Documents")
        .css_classes(["gl-path-label"])
        .hexpand(true)
        .halign(gtk::Align::Fill)
        .ellipsize(pango::EllipsizeMode::End)
        .build();
    path.append(&path_label);
    let refresh = icon_button("view-refresh-symbolic", "");
    path.append(&refresh);
    row.append(&path);
    let trash = gtk::Button::new();
    trash.add_css_class("gl-trash");
    trash.set_child(Some(&gtk::Image::from_icon_name("user-trash-symbolic")));
    trash.set_valign(gtk::Align::Center);
    row.append(&trash);
    let mut win_buttons = Vec::new();
    for (icon, action) in desktop_buttons() {
        let button = if *action == "float" {
            float_button()
        } else {
            icon_button(icon, "winbtn")
        };
        row.append(&button);
        win_buttons.push((button, *action));
    }
    // A ScrolledWindow with a zero minimum lets the header shrink while the path and pills
    // re-flow; the row still receives the viewport's width. `External` takes the branch at
    // gtkscrolledwindow.c:1848-1850, so the child is not measured for size and the minimum
    // is just `min_content_width` (1929-1932); it still updates the adjustment's page size
    // (gtkviewport.c:178-184, 540), which is the width the header lays out to.
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
    scroller.set_min_content_width(0);
    scroller.set_propagate_natural_width(false);
    scroller.set_has_frame(false);
    scroller.set_child(Some(&row));
    let handle = gtk::WindowHandle::builder().child(&scroller).build();
    let body = vbox(&["gl-frame"]);
    body.set_hexpand(true);
    body.set_vexpand(true);
    let body_label = gtk::Label::builder()
        .label("Tab 1")
        .css_classes(["gl-title"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build();
    body.append(&body_label);
    let root = vbox(&[]);
    root.append(&handle);
    root.append(&body);
    window.set_content(Some(&root));
    let blur = crate::blur::Blur::attach(window.upcast_ref(), scale, opacity);
    let ui = Rc::new(Ui {
        window: window.clone(),
        strip,
        pills: RefCell::new(Vec::new()),
        drag: Cell::new(None),
        available: Cell::new(style::DEFAULT_W),
        path,
        path_label: path_label.clone(),
        frame: body,
        body: body_label,
        search_icon: search_icon.clone(),
        search,
        search_field: field,
        search_entry: search_entry.clone(),
        scale: Cell::new(scale),
        opacity: Cell::new(opacity),
        font: args.font.clone(),
        provider,
        blur,
        model: RefCell::new(Model::new(args.tabs)),
        overflow: Cell::new(false),
        search_open: Cell::new(false),
        animate_group: Cell::new(None),
        dialogs: Cell::new(0),
    });
    let anchor = menu_button.clone();
    menu_button.connect_clicked(button_handler(&ui, move |ui| open_main_menu(ui, &anchor)));
    // A click on the path bar or the body leaves focus on the search entry (neither is
    // focusable), so the entry's focus-leave never fires. Close an empty search here.
    dismiss_on_click(&ui, &ui.path);
    dismiss_on_click(&ui, &ui.frame);
    search_icon.connect_clicked(button_handler(&ui, |ui| ui.open_search()));
    search_entry.connect_activate(|entry| log!("search", "search: {}", entry.text()));
    let focus = gtk::EventControllerFocus::new();
    focus.connect_leave({
        let ui = ui.clone();
        let entry = search_entry.clone();
        move |_| {
            if ui.search_open.get() && entry.text().is_empty() {
                ui.close_search();
            }
        }
    });
    search_entry.add_controller(focus);
    refresh.connect_clicked(button_handler(&ui, |ui| {
        log!("path", "refresh {}", ui.path_text())
    }));
    trash.connect_clicked(|_| log!("trash", "open Trash"));
    trash.add_controller(trash_drop());
    for (button, action) in win_buttons {
        let window = window.clone();
        button.connect_clicked(move |_| window_action(&window, action));
    }
    // Bubble phase: a dialog gets Escape (and its entry every other key) first. The
    // `dialogs` guard covers keys the entry does not consume, like Ctrl+T.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Bubble);
    keys.connect_key_pressed({
        let ui = ui.clone();
        move |_, key, _, modifiers| ui.key(key, modifiers)
    });
    window.add_controller(keys);
    // Layout follows the scroller's viewport width instead of polling.
    let weak = Rc::downgrade(&ui);
    scroller
        .hadjustment()
        .connect_notify_local(Some("page-size"), move |adjustment, _| {
            let width = adjustment.page_size() as i32;
            if let Some(ui) = weak.upgrade()
                && width > 0
                && width != ui.available.get()
            {
                ui.available.set(width);
                ui.relayout();
            }
        });
    ui.refresh();
    window.present();
    log!("window", "header window shown");
}

/// Any `Button`-argument signal on a widget whose callback needs the `Ui`.
fn button_handler<F: Fn(&Rc<Ui>) + 'static>(ui: &Rc<Ui>, f: F) -> impl Fn(&gtk::Button) + 'static {
    let ui = ui.clone();
    move |_| f(&ui)
}

/// Dismiss an open, empty search when `target` (or a child) is clicked (P1i-r4, item 3).
fn dismiss_on_click(ui: &Rc<Ui>, target: &impl IsA<gtk::Widget>) {
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let ui = ui.clone();
    click.connect_released(move |_, _, _, _| ui.dismiss_empty_search());
    target.add_controller(click);
}

fn window_action(window: &adw::ApplicationWindow, action: &str) {
    match action {
        "minimize" => {
            window.minimize();
            log!("window", "minimize");
        }
        "float" => {
            toggle_floating();
            log!("window", "float");
        }
        "maximize" if window.is_maximized() => window.unmaximize(),
        "maximize" => window.maximize(),
        _ => window.close(),
    }
}

fn trash_drop() -> gtk::DropTarget {
    let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    drop.connect_drop(|_, value, _, _| {
        if let Ok(list) = value.get::<gdk::FileList>() {
            let files: Vec<String> = list
                .files()
                .iter()
                .filter_map(|file| file.path())
                .map(|path| path.display().to_string())
                .collect();
            log!(
                "trash",
                "would trash {} files: {}",
                files.len(),
                files.join(", ")
            );
        }
        true
    });
    drop
}

fn hbox(classes: &[&str]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    for c in classes {
        b.add_css_class(c);
    }
    b
}

fn vbox(classes: &[&str]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for c in classes {
        b.add_css_class(c);
    }
    b
}

fn icon_button(icon_name: &str, extra: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("gl-icon-button");
    if !extra.is_empty() {
        button.add_css_class(extra);
    }
    button.set_child(Some(&gtk::Image::from_icon_name(icon_name)));
    button.set_valign(gtk::Align::Center);
    button
}

/// The float control: the mockup's two windows as a symbolic `gtk::Svg` paintable.
/// `gtk::Image` passes its CSS `color` (foreground) and `-gtk-icon-palette` (the
/// front window's `error` slot, remapped to the window background) to the SVG.
fn float_button() -> gtk::Button {
    let svg = gtk::Svg::from_bytes(&glib::Bytes::from_static(FLOAT_ICON));
    let button = gtk::Button::new();
    button.add_css_class("gl-icon-button");
    button.add_css_class("winbtn");
    button.add_css_class("float");
    button.set_child(Some(&gtk::Image::from_paintable(Some(&svg))));
    button.set_valign(gtk::Align::Center);
    button
}

fn menu_item(
    ui: &Rc<Ui>,
    popover: &gtk::Popover,
    menu: &gtk::Box,
    text: &str,
    enabled: bool,
    action: impl Fn(&Rc<Ui>) + 'static,
) {
    let item = gtk::Button::new();
    item.add_css_class("gl-menu-item");
    item.set_sensitive(enabled);
    item.set_child(Some(
        &gtk::Label::builder()
            .label(text)
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build(),
    ));
    let ui = ui.clone();
    let popover = popover.clone();
    item.connect_clicked(move |_| {
        take_down(&popover);
        action(&ui);
    });
    menu.append(&item);
}

fn take_down(popover: &gtk::Popover) {
    popover.popdown();
    if popover.parent().is_some() {
        popover.unparent();
    }
}

fn close_on_closed(popover: &gtk::Popover) {
    popover.connect_closed(|p| {
        if p.parent().is_some() {
            p.unparent();
        }
    });
}

fn open_main_menu(ui: &Rc<Ui>, anchor: &gtk::Button) {
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    let menu = vbox(&["gl-menu"]);
    menu_item(ui, &popover, &menu, "New Tab (Ctrl+T)", true, |ui| {
        ui.add_tab()
    });
    menu_item(
        ui,
        &popover,
        &menu,
        "Open in New Window (Ctrl+N, logs only)",
        true,
        |_| log!("window", "would open a new window"),
    );
    popover.set_child(Some(&menu));
    close_on_closed(&popover);
    popover.popup();
}

fn open_pill_menu(ui: &Rc<Ui>, index: usize, anchor: &gtk::Widget) {
    let (group, selected) = {
        let model = ui.model.borrow();
        let group = model
            .entries
            .get(index)
            .is_some_and(|entry| entry.is_group());
        (group, model.selected_tabs())
    };
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    let menu = vbox(&["gl-menu"]);
    menu_item(ui, &popover, &menu, "Split View", selected >= 2, |ui| {
        ui.split_selected()
    });
    menu_item(ui, &popover, &menu, "Close", true, move |ui| {
        ui.close_menu_click(index)
    });
    menu_item(ui, &popover, &menu, "Open in New Window", true, |_| {
        log!("window", "would open a new window")
    });
    if group {
        menu_item(ui, &popover, &menu, "Rename…", true, move |ui| {
            ui.rename_dialog(index)
        });
        menu_item(ui, &popover, &menu, "Ungroup", true, move |ui| {
            ui.model.borrow_mut().ungroup(index);
            ui.refresh();
        });
    }
    popover.set_child(Some(&menu));
    close_on_closed(&popover);
    popover.popup();
}

/// The pointer's surface position. Not the gesture's own offset: the dragged pill moves
/// under the pointer, which would corrupt widget-relative offsets.
fn pointer_pos(gesture: &gtk::GestureDrag) -> Option<(f64, f64)> {
    gesture.last_event(None)?.position()
}

fn make_pill(
    ui: &Rc<Ui>,
    index: usize,
    entry: &Entry,
    active: bool,
    selected: bool,
) -> gtk::Widget {
    let group = entry.is_group();
    let pill = gtk::CenterBox::new();
    pill.add_css_class("gl-pill");
    pill.add_css_class(if group { "group" } else { "tab" });
    pill.set_valign(gtk::Align::Center);
    if active {
        pill.add_css_class("active");
    }
    if selected {
        pill.add_css_class("selected");
    }
    let name = gtk::Label::builder()
        .label(entry.label())
        .ellipsize(pango::EllipsizeMode::End)
        .max_width_chars(48)
        .build();
    pill.set_center_widget(Some(&name));
    let close = gtk::Button::new();
    close.add_css_class("gl-pill-close");
    close.set_valign(gtk::Align::Center);
    close.set_child(Some(&gtk::Image::from_icon_name("window-close-symbolic")));
    close.connect_clicked({
        let ui = ui.clone();
        move |_| ui.close_or_confirm(index, group)
    });
    pill.set_end_widget(Some(&close));
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.connect_released({
        let ui = ui.clone();
        move |gesture, _, _, _| {
            ui.pill_clicked(
                index,
                gesture
                    .last_event(None)
                    .map(|e| e.modifier_state())
                    .unwrap_or(gdk::ModifierType::empty()),
            )
        }
    });
    pill.add_controller(click);
    let right = gtk::GestureClick::new();
    right.set_button(3);
    right.connect_pressed({
        let ui = ui.clone();
        move |gesture, _, _, _| {
            if let Some(anchor) = gesture.widget() {
                open_pill_menu(&ui, index, &anchor);
            }
        }
    });
    pill.add_controller(right);
    let drag = gtk::GestureDrag::new();
    drag.connect_drag_begin({
        let ui = ui.clone();
        move |gesture, _, _| {
            if let Some((x, y)) = pointer_pos(gesture) {
                ui.drag_begin(index, x, y);
            }
        }
    });
    drag.connect_drag_update({
        let ui = ui.clone();
        move |gesture, _, _| {
            let Some((x, y)) = pointer_pos(gesture) else {
                return;
            };
            let Some(state) = ui.drag.get() else { return };
            let dx = x - state.start_x;
            let dy = y - state.start_y;
            // Claim once the surface distance from the press passes the drag threshold.
            // Surface, not the gesture's widget-relative offset: the pill follows the
            // pointer, so widget-relative dx stays near zero. Not in drag-begin: that
            // would also deny the pill's own click gesture.
            if let Some(pill) = gesture.widget()
                && pill.drag_check_threshold(0, 0, dx.round() as i32, dy.round() as i32)
                && gesture.set_state(gtk::EventSequenceState::Claimed)
            {
                log!("drag", "claimed the sequence at dx {dx:.1}");
            }
            log!("drag", "update dx {dx:.1}");
            ui.drag_move(x);
        }
    });
    drag.connect_drag_end({
        let ui = ui.clone();
        move |gesture, _, _| {
            if let Some((x, _)) = pointer_pos(gesture) {
                ui.drag_move(x);
            }
            ui.drag_end();
        }
    });
    drag.connect_cancel({
        let ui = ui.clone();
        move |_, _| {
            log!("drag", "cancel");
            ui.drag_cancel();
        }
    });
    pill.add_controller(drag);
    let wrapper = pill_revealer(&pill);
    if ui.animate_group.get() == Some(index) {
        wrapper.set_reveal_child(false);
        let weak = wrapper.downgrade();
        glib::timeout_add_local_once(Duration::from_millis(20), move || {
            if let Some(wrapper) = weak.upgrade() {
                wrapper.set_reveal_child(true);
            }
        });
    }
    wrapper.upcast()
}

/// A crossfade around one pill, so the P1i group transition does not fade
/// text through CSS `opacity` on the pill itself.
fn pill_revealer(pill: &impl IsA<gtk::Widget>) -> gtk::Revealer {
    let revealer = gtk::Revealer::new();
    revealer.add_css_class("gl-pill-wrap");
    revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
    revealer.set_transition_duration(FADE_MS as u32);
    revealer.set_child(Some(pill));
    revealer.set_reveal_child(true);
    revealer
}

fn make_accordion(ui: &Rc<Ui>) -> gtk::Widget {
    let pill = gtk::CenterBox::new();
    pill.add_css_class("gl-pill");
    let label = gtk::Label::new(None);
    pill.set_center_widget(Some(&label));
    let popover = gtk::Popover::new();
    popover.set_parent(&pill);
    let menu = vbox(&["gl-menu"]);
    fill_accordion(ui, &popover, &menu, &label);
    popover.set_child(Some(&menu));
    let motion = gtk::EventControllerMotion::new();
    let weak = popover.downgrade();
    motion.connect_enter(move |_, _, _| {
        if let Some(popover) = weak.upgrade() {
            popover.popup();
        }
    });
    pill.add_controller(motion);
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let weak = popover.downgrade();
    click.connect_released(move |_, _, _, _| {
        if let Some(popover) = weak.upgrade() {
            popover.popup();
        }
    });
    pill.add_controller(click);
    let weak = popover.downgrade();
    pill.connect_destroy(move |_| {
        if let Some(popover) = weak.upgrade() {
            popover.popdown();
            popover.unparent();
        }
    });
    pill_revealer(&pill).upcast()
}

/// Fill the accordion popover's list from the model: one row per entry with a ✕ that
/// closes it. Re-run after a close so the open list updates in place (P1l item 3).
fn fill_accordion(ui: &Rc<Ui>, popover: &gtk::Popover, menu: &gtk::Box, label: &gtk::Label) {
    while let Some(child) = menu.first_child() {
        menu.remove(&child);
    }
    let entries = ui.model.borrow().entries.clone();
    label.set_label(&format!("{} tabs", ui.model.borrow().count()));
    for (index, entry) in entries.iter().enumerate() {
        let row = hbox(&["gl-accordion-row"]);
        let choose = gtk::Button::new();
        choose.add_css_class("gl-menu-item");
        choose.set_hexpand(true);
        choose.set_child(Some(
            &gtk::Label::builder()
                .label(entry.label())
                .halign(gtk::Align::Start)
                .hexpand(true)
                .build(),
        ));
        let ui_choose = ui.clone();
        let popover_choose = popover.clone();
        choose.connect_clicked(move |_| {
            take_down(&popover_choose);
            ui_choose.model.borrow_mut().focus(index, false);
            ui_choose.refresh();
        });
        row.append(&choose);
        let close = gtk::Button::new();
        close.add_css_class("gl-pill-close");
        close.set_valign(gtk::Align::Center);
        close.set_child(Some(&gtk::Image::from_icon_name("window-close-symbolic")));
        let ui_close = ui.clone();
        let popover_close = popover.clone();
        let menu_close = menu.clone();
        let label_close = label.clone();
        close.connect_clicked(move |_| {
            let group = ui_close
                .model
                .borrow()
                .entries
                .get(index)
                .is_some_and(Entry::is_group);
            if group {
                let ui_ask = ui_close.clone();
                let popover_ask = popover_close.clone();
                let menu_ask = menu_close.clone();
                let label_ask = label_close.clone();
                ui_close.confirm_group(index, move |_| {
                    close_list_entry(&ui_ask, index, &popover_ask, &menu_ask, &label_ask)
                });
            } else {
                close_list_entry(&ui_close, index, &popover_close, &menu_close, &label_close);
            }
        });
        row.append(&close);
        menu.append(&row);
    }
}

/// Close one row's entry from the open accordion list. The list is filled again in
/// place while the header stays collapsed; when the pills fit again, a normal rebuild
/// replaces the accordion (and its popover) with pills.
fn close_list_entry(
    ui: &Rc<Ui>,
    index: usize,
    popover: &gtk::Popover,
    menu: &gtk::Box,
    label: &gtk::Label,
) {
    let Some(closed) = ui.model.borrow_mut().close_entry(index) else {
        return;
    };
    log!("tabs", "closed from the list: {closed:?}");
    if closed == Closed::WindowEmpty {
        log!("tabs", "last tab closed, closing window");
        ui.window.close();
        return;
    }
    ui.update_text();
    ui.update_layout();
    if !ui.overflow.get() {
        ui.rebuild();
        return;
    }
    fill_accordion(ui, popover, menu, label);
    popover.popup();
}

impl Ui {
    /// The distance between two pill slots, from the same tokens as the CSS.
    fn pitch(&self) -> f64 {
        (style::scaled(style::PILL_W, self.scale.get())
            + style::scaled(style::GAP_L, self.scale.get())) as f64
    }

    fn refresh(self: &Rc<Self>) {
        self.update_layout();
        self.rebuild();
    }

    fn relayout(self: &Rc<Self>) {
        if self.update_layout() {
            self.rebuild();
        }
    }

    fn path_text(&self) -> String {
        let model = self.model.borrow();
        match model.entries.get(model.focused) {
            Some(entry) => format!("/home/kyle/Documents/{}", entry.label()),
            None => "/home/kyle/Documents".into(),
        }
    }

    fn rebuild(self: &Rc<Self>) {
        while let Some(child) = self.strip.first_child() {
            self.strip.remove(&child);
        }
        self.pills.borrow_mut().clear();
        let (entries, focused, selection) = {
            let model = self.model.borrow();
            (
                model.entries.clone(),
                model.focused,
                model.selection.clone(),
            )
        };
        let pills: Vec<gtk::Widget> = if self.overflow.get() {
            vec![make_accordion(self)]
        } else {
            entries
                .iter()
                .enumerate()
                .map(|(i, e)| make_pill(self, i, e, i == focused, selection.contains(&i)))
                .collect()
        };
        for (index, pill) in pills.into_iter().enumerate() {
            self.strip.put(&pill, index as f64 * self.pitch(), 0.0);
            self.pills.borrow_mut().push(pill);
        }
        self.update_text();
        self.animate_group.set(None);
    }

    /// The body title and the path bar's label, from the focused entry.
    fn update_text(&self) {
        let (title, path) = {
            let model = self.model.borrow();
            let title = match model.entries.get(model.focused) {
                Some(Entry::Group { name, .. }) => format!("{name} · split view"),
                Some(entry) => entry.label(),
                None => "No tabs".into(),
            };
            let path = model
                .entries
                .get(model.focused)
                .map(|entry| format!("/home/kyle/Documents/{}", entry.label()))
                .unwrap_or_else(|| "/home/kyle/Documents".into());
            (title, path)
        };
        self.body.set_label(&title);
        self.path_label.set_label(&path);
    }

    /// Recompute the overflow decision. True if the pill display has to be rebuilt.
    fn update_layout(&self) -> bool {
        let scale = self.scale.get();
        // The field is still drawn during its 200 ms fade-out, so reserve its
        // space until it is actually hidden (not just until `search_open` clears).
        let searching = self.search_open.get() || self.search_field.get_visible();
        let search = if searching {
            ((style::SEARCH_W - (style::ICON_SEARCH + 2.0 * style::GAP_S)) * scale).round() as i32
        } else {
            0
        };
        let desktop = if std::env::var_os("NIRI_SOCKET").is_some() {
            0
        } else {
            ((style::ICON_WINDOW + 2.0 * style::GAP_S + style::GAP_L) * scale).round() as i32
        };
        let (entries, tabs) = {
            let model = self.model.borrow();
            (model.entries.len(), model.count())
        };
        let fit = style::header_fits(self.available.get() - search - desktop, entries, scale);
        // Only a token minimum: the path then takes the free width with hexpand. Never
        // feed the measured free/allocated width back in as a minimum (P1i-r3, item 2).
        let path_min = style::scaled(style::PATH_MIN, scale);
        self.path.set_width_request(path_min);
        log!(
            "layout",
            "window {}, path min {}, fits {}, overflow {}",
            self.available.get(),
            path_min,
            fit.fits,
            !fit.fits && tabs > 1
        );
        // One tab never needs an accordion, even if the window is very narrow.
        let collapsed = !fit.fits && tabs > 1;
        let changed = collapsed != self.overflow.get();
        self.overflow.set(collapsed);
        changed
    }

    fn add_tab(self: &Rc<Self>) {
        let name = {
            let mut model = self.model.borrow_mut();
            let index = model.add_tab();
            model.focused = index;
            model.selection.clear();
            model.entries[index].label()
        };
        log!("tabs", "new tab: {name}");
        self.refresh();
    }

    fn pill_clicked(self: &Rc<Self>, index: usize, modifiers: gdk::ModifierType) {
        self.dismiss_empty_search();
        let mut model = self.model.borrow_mut();
        if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
            model.toggle(index);
        } else {
            model.focus(index, modifiers.contains(gdk::ModifierType::SHIFT_MASK));
        }
        drop(model);
        self.rebuild();
    }

    fn close_menu_click(self: &Rc<Self>, index: usize) {
        let model = self.model.borrow();
        let indices: Vec<usize> = if model.selection.is_empty() {
            vec![index]
        } else {
            model.selection.iter().copied().collect()
        };
        drop(model);
        self.close_indices(&indices);
    }

    fn close_indices(self: &Rc<Self>, indices: &[usize]) {
        if self.model.borrow_mut().close(indices) {
            log!("tabs", "last pill closed, closing window");
            self.window.close();
        } else {
            self.refresh();
        }
    }

    fn close_or_confirm(self: &Rc<Self>, index: usize, group: bool) {
        if !group {
            return self.close_indices(&[index]);
        }
        self.confirm_group(index, move |ui| ui.close_indices(&[index]));
    }

    /// The group-close confirmation shared by a group pill's ✕ and the accordion's ✕.
    fn confirm_group(self: &Rc<Self>, index: usize, action: impl Fn(&Rc<Ui>) + 'static) {
        let Some((name, count)) = group_of(&self.model.borrow(), index) else {
            return;
        };
        self.ask(
            &format!("Close all {count} tabs in {name}?"),
            None,
            "close",
            "Close",
            true,
            action,
        );
    }

    fn rename_dialog(self: &Rc<Self>, index: usize) {
        let Some((name, _)) = group_of(&self.model.borrow(), index) else {
            return;
        };
        let entry = gtk::Entry::new();
        entry.set_text(&name);
        let input = entry.clone();
        self.ask(
            "Rename group",
            Some(entry.upcast_ref()),
            "rename",
            "Rename",
            false,
            move |ui| {
                ui.model.borrow_mut().rename(index, input.text().as_str());
                ui.refresh();
            },
        );
    }

    /// Present a dialog and keep our key handler out of its way until it closes.
    fn ask(
        self: &Rc<Self>,
        title: &str,
        extra: Option<&gtk::Widget>,
        id: &'static str,
        label: &str,
        destructive: bool,
        action: impl Fn(&Rc<Ui>) + 'static,
    ) {
        let dialog = adw::AlertDialog::new(Some(title), None);
        if let Some(child) = extra {
            dialog.set_extra_child(Some(child));
        }
        dialog.add_response("cancel", "Cancel");
        dialog.add_response(id, label);
        if destructive {
            dialog.set_response_appearance(id, adw::ResponseAppearance::Destructive);
        }
        dialog.set_default_response(Some("cancel"));
        let ui = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response == id {
                action(&ui);
            }
        });
        dialog.connect_closed({
            let weak = Rc::downgrade(self);
            move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.dialogs.set(ui.dialogs.get().saturating_sub(1));
                }
            }
        });
        self.dialogs.set(self.dialogs.get() + 1);
        dialog.choose(Some(&self.window), None::<&gio::Cancellable>, |_| {});
    }

    fn split_selected(self: &Rc<Self>) {
        let indices: Vec<usize> = {
            let model = self.model.borrow();
            model
                .selection
                .iter()
                .copied()
                .filter(|i| matches!(model.entries.get(*i), Some(Entry::Tab(_))))
                .collect()
        };
        if indices.len() < 2 {
            return log!("groups", "split view needs two or more selected tabs");
        }
        // Clone the handles so the `pills` borrow ends before the CSS calls; the timer
        // below then borrows the model only after `group_selected` has returned.
        let pills: Vec<gtk::Widget> = self.pills.borrow().clone();
        for (index, pill) in pills.iter().enumerate() {
            if indices.contains(&index)
                && let Some(revealer) = pill.downcast_ref::<gtk::Revealer>()
            {
                revealer.set_reveal_child(false);
            }
        }
        let ui = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(FADE_MS), move || {
            // Take the result into a `let` first: an `if let` scrutinee's temporary lives
            // through the whole block, so the `borrow_mut` would still be held when
            // `refresh()` borrows the model again (P1i-r4, item 1).
            let first = ui.model.borrow_mut().group_selected();
            if let Some(first) = first {
                ui.animate_group.set(Some(first));
                ui.refresh();
            }
        });
    }

    fn open_search(self: &Rc<Self>) {
        if self.search_open.get() {
            self.search_entry.grab_focus();
            return;
        }
        self.search_field.set_visible(true);
        self.search_open.set(true);
        self.search_icon.set_visible(false);
        self.search.set_reveal_child(true);
        self.search_entry.grab_focus();
        let weak = self.search_entry.downgrade();
        glib::timeout_add_local_once(Duration::from_millis(50), move || {
            if let Some(entry) = weak.upgrade() {
                entry.grab_focus();
            }
        });
        self.relayout();
        log!("search", "opened");
    }

    fn close_search(self: &Rc<Self>) {
        if !self.search_open.get() {
            return;
        }
        self.search_open.set(false);
        self.search_entry.set_text("");
        self.search.set_reveal_child(false);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_millis(210), move || {
            if let Some(ui) = weak.upgrade()
                && !ui.search_open.get()
            {
                ui.search_field.set_visible(false);
                ui.search_icon.set_visible(true);
                ui.relayout();
            }
        });
        self.relayout();
        log!("search", "closed");
    }

    /// The same rule as focus leaving the entry: only an open, empty search closes.
    fn dismiss_empty_search(self: &Rc<Self>) {
        if self.search_open.get() && self.search_entry.text().is_empty() {
            self.close_search();
        }
    }

    fn step_scale(self: &Rc<Self>, delta: f64) {
        let base = style::base();
        let next = (((self.scale.get() / base + delta) * 20.0).round() / 20.0).clamp(0.5, 1.0);
        self.scale.set(base * next);
        self.provider.load_from_string(&style::css(
            self.scale.get(),
            self.font.as_deref(),
            self.opacity.get(),
        ));
        self.blur.set_scale(self.scale.get());
        log!("scale", "setting {next:.2}, scale {:.3}", self.scale.get());
        self.refresh();
    }

    fn drag_begin(self: &Rc<Self>, index: usize, pointer_x: f64, pointer_y: f64) {
        let base = index as f64 * self.pitch();
        self.drag.set(Some(DragState {
            index,
            start_x: pointer_x,
            start_y: pointer_y,
            base,
            to: index,
        }));
        // Clone the handle, then drop the `pills` borrow before touching GTK (P1i-r4,
        // item 2). The old `if let` scrutinee held the borrow across the reorder.
        let pill = self.pills.borrow().get(index).cloned();
        if let Some(pill) = pill {
            // GtkFixed paints in child order, so relinking the pill last puts it on top.
            // Never unparent it: unparenting unmaps the pill, unmapping resets its
            // controllers, and a reset gesture cancels itself inside its own drag-begin.
            pill.insert_after(&self.strip, self.strip.last_child().as_ref());
            self.strip.move_(&pill, base, 0.0);
        }
    }

    fn drag_move(self: &Rc<Self>, pointer_x: f64) {
        let Some(drag) = self.drag.get() else { return };
        let count = self.pills.borrow().len();
        let layout = style::drag_layout(drag.index, pointer_x - drag.start_x, self.pitch(), count);
        // Each lookup clones the widget and ends its `pills` borrow at the statement, so
        // no borrow is held across a `GtkFixed` call (P1i-r4, item 2).
        let dragged = self.pills.borrow().get(drag.index).cloned();
        if let Some(pill) = dragged {
            self.strip.move_(&pill, drag.base + layout.offset, 0.0);
        }
        if layout.to != drag.to {
            let pills: Vec<gtk::Widget> = self.pills.borrow().clone();
            for (index, slide) in layout.slides.iter().enumerate() {
                if index != drag.index
                    && let Some(pill) = pills.get(index)
                {
                    set_slide(pill, *slide);
                }
            }
        }
        self.drag.set(Some(DragState {
            to: layout.to,
            ..drag
        }));
    }

    fn drag_end(self: &Rc<Self>) {
        let Some(drag) = self.drag.take() else { return };
        // Rebuild after the gesture's handler returns: it removes the pill it fired on.
        let ui = self.clone();
        glib::idle_add_local_once(move || {
            if drag.to != drag.index {
                ui.model
                    .borrow_mut()
                    .move_entry(drag.index, drag.to as isize - drag.index as isize);
                log!("tabs", "moved pill {} to {}", drag.index, drag.to);
            }
            ui.rebuild();
        });
    }

    fn drag_cancel(self: &Rc<Self>) {
        self.drag.set(None);
        let ui = self.clone();
        glib::idle_add_local_once(move || ui.rebuild());
    }

    fn key(self: &Rc<Self>, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        if self.dialogs.get() > 0 {
            return glib::Propagation::Proceed;
        }
        if key == gdk::Key::Escape {
            if self.search_open.get() {
                self.close_search();
                return glib::Propagation::Stop;
            }
            if !self.model.borrow().selection.is_empty() {
                self.model.borrow_mut().selection.clear();
                self.rebuild();
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        if !modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
            return glib::Propagation::Proceed;
        }
        match key {
            gdk::Key::equal | gdk::Key::plus | gdk::Key::KP_Add => self.step_scale(0.05),
            gdk::Key::minus | gdk::Key::KP_Subtract => self.step_scale(-0.05),
            gdk::Key::t => self.add_tab(),
            gdk::Key::n => log!("window", "would open a new window"),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }
}

/// Let CSS slide a passed pill one slot (150 ms); `offset` is that slot's sign.
fn set_slide(pill: &gtk::Widget, offset: f64) {
    pill.remove_css_class("slide-left");
    pill.remove_css_class("slide-right");
    if offset < 0.0 {
        pill.add_css_class("slide-left");
    } else if offset > 0.0 {
        pill.add_css_class("slide-right");
    }
}

fn group_of(model: &Model, index: usize) -> Option<(String, usize)> {
    match model.entries.get(index) {
        Some(Entry::Group { name, tabs }) => Some((name.clone(), tabs.len())),
        _ => None,
    }
}

fn toggle_floating() {
    let argv = ["niri", "msg", "action", "toggle-window-floating"].map(std::ffi::OsStr::new);
    match gio::Subprocess::newv(&argv, gio::SubprocessFlags::NONE) {
        Ok(process) => process.wait_check_async(None::<&gio::Cancellable>, |result| match result {
            Ok(()) => log!("float", "niri toggle-window-floating: exit 0"),
            Err(error) => log!("float", "niri toggle-window-floating failed: {error}"),
        }),
        Err(error) => log!("float", "could not run niri: {error}"),
    }
}
