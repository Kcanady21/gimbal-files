// SPDX-License-Identifier: GPL-3.0-or-later
//! The left sidebar: pinned folders with the spike's `PinnedRowLayout`, recent
//! directories, mounted devices with drive bars, Remote and the main drive.
//!
//! Recent directories are kept in memory per window for now; saving them is a
//! later session change.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use gtk::subclass::prelude::*;

use fm_core::command::{Caller, Invocation};
use fm_core::config::{self, Section, SectionState};
use fm_core::workspace::WindowId;

use crate::app::Process;
use crate::look::css::bar_class;
use crate::look::motion;
use crate::look::tokens::*;

const BAR_FILL_CLASSES: [&str; 11] = [
    "bar-lv0", "bar-lv1", "bar-lv2", "bar-lv3", "bar-lv4", "bar-lv5", "bar-lv6", "bar-lv7",
    "bar-lv8", "bar-lv9", "bar-full",
];
const BAR_LABEL_CLASSES: [&str; 3] = ["bar-label-lo", "bar-label-hi", "bar-label-err"];

pub struct Sidebar {
    root: gtk::Box,
    pinned: Rc<PinnedList>,
    recent_content: gtk::Box,
    devices_content: gtk::Box,
    revealers: RefCell<Vec<(Section, gtk::Revealer)>>,
    devices: RefCell<Vec<Rc<DriveRow>>>,
    main: Rc<DriveRow>,
    recent: RefCell<Vec<PathBuf>>,
    monitor: gio::VolumeMonitor,
    process: Weak<Process>,
    window: WindowId,
    home: PathBuf,
    scale: Cell<f64>,
    current: RefCell<Option<PathBuf>>,
}

impl Sidebar {
    pub fn new(process: &Rc<Process>, window: WindowId, scale: f64) -> Rc<Self> {
        let home = process.home();
        let config = process.config.borrow();
        let sections = config.config().sections;
        drop(config);

        let pinned = PinnedList::new(process, window, home.clone());
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_vexpand(true);

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_vexpand(true);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&pinned.root);
        body.append(&rule());

        let recent_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        recent_content.add_css_class("gl-section-content");
        let devices_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        devices_content.add_css_class("gl-section-content");
        let remote_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        remote_content.add_css_class("gl-section-content");
        remote_content.append(&dir_row("No remotes"));

        let mut revealers = Vec::new();
        for (name, section, content) in [
            ("Recent Directories", Section::Recent, &recent_content),
            ("Devices", Section::Devices, &devices_content),
            ("Remote", Section::Remote, &remote_content),
        ] {
            let expanded = sections.get(section) == SectionState::Expanded;
            let (view, revealer) = section_view(name, section, content, expanded, process, window);
            revealers.push((section, revealer));
            body.append(&view);
        }

        scroll.set_child(Some(&body));
        root.append(&scroll);
        root.append(&rule());
        let main = DriveRow::new("Main Drive", true, scale);
        root.append(&main.root);

        let this = Rc::new(Self {
            root,
            pinned,
            recent_content,
            devices_content,
            revealers: RefCell::new(revealers),
            devices: RefCell::new(Vec::new()),
            main,
            recent: RefCell::new(Vec::new()),
            monitor: gio::VolumeMonitor::get(),
            process: Rc::downgrade(process),
            window,
            home,
            scale: Cell::new(scale),
            current: RefCell::new(None),
        });

        let weak = Rc::downgrade(&this);
        this.monitor.connect_mount_added(move |_, _| {
            if let Some(sidebar) = weak.upgrade() {
                sidebar.refresh_devices();
                sidebar.refresh_main();
            }
        });
        let weak = Rc::downgrade(&this);
        this.monitor.connect_mount_removed(move |_, _| {
            if let Some(sidebar) = weak.upgrade() {
                sidebar.refresh_devices();
                sidebar.refresh_main();
            }
        });
        let weak = Rc::downgrade(&this);
        this.monitor.connect_mount_changed(move |_, _| {
            if let Some(sidebar) = weak.upgrade() {
                sidebar.refresh_devices();
                sidebar.refresh_main();
            }
        });

        this.reload_from_config();
        this.refresh_devices();
        this.refresh_main();
        this
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Follow a pin or section change made in the config.
    pub fn reload_from_config(&self) {
        let Some(process) = self.process.upgrade() else {
            return;
        };
        let (pins, sections) = {
            let config = process.config.borrow();
            let config = config.config();
            (config.pins.clone(), config.sections)
        };
        self.pinned.rebuild(&pins);
        for (section, revealer) in self.revealers.borrow().iter() {
            revealer.set_reveal_child(sections.get(*section) == SectionState::Expanded);
        }
        let current = self.current.borrow().clone();
        if let Some(current) = current {
            self.set_current_folder(Some(&current));
        } else {
            self.pinned.select(None);
        }
    }

    pub fn set_current_folder(&self, folder: Option<&Path>) {
        *self.current.borrow_mut() = folder.map(Path::to_path_buf);
        match folder {
            Some(folder) => self.pinned.select_for(folder, &self.home),
            None => self.pinned.select(None),
        }
    }

    pub fn push_recent(&self, folder: &Path) {
        {
            let mut recent = self.recent.borrow_mut();
            push_recent(&mut recent, folder);
        }
        self.rebuild_recent();
    }

    pub fn set_section(&self, section: Section, state: SectionState) {
        for (candidate, revealer) in self.revealers.borrow().iter() {
            if *candidate == section {
                revealer.set_reveal_child(state == SectionState::Expanded);
                return;
            }
        }
    }

    pub fn set_scale(&self, scale: f64) {
        self.scale.set(scale);
        self.pinned.set_scale(scale);
        self.main.set_scale(scale);
        for row in self.devices.borrow().iter() {
            row.set_scale(scale);
        }
    }

    fn rebuild_recent(&self) {
        clear_children(&self.recent_content);
        let recent = self.recent.borrow().clone();
        for folder in recent {
            let row = dir_row(&recent_label(&folder, &self.home));
            row.set_tooltip_text(Some(&folder.display().to_string()));
            attach_click(&row, &self.process, self.window, folder);
            self.recent_content.append(&row);
        }
    }

    fn refresh_devices(&self) {
        clear_children(&self.devices_content);
        let scale = self.scale.get();
        let mut rows = Vec::new();
        for mount in self.monitor.mounts() {
            let Some(root) = mount.root().path() else {
                continue;
            };
            if root == Path::new("/") {
                continue;
            }
            let name = mount.name().to_string();
            let name = if name.is_empty() {
                path_label(&root)
            } else {
                name
            };
            let row = DriveRow::new(&name, false, scale);
            if let Some((total, free)) = filesystem(&root) {
                row.set_values(total, free);
            }
            attach_click(&row.root, &self.process, self.window, root);
            self.devices_content.append(&row.root);
            rows.push(row);
        }
        *self.devices.borrow_mut() = rows;
    }

    fn refresh_main(&self) {
        let name = root_mount_name(&self.monitor);
        self.main.set_name(&name);
        if let Some((total, free)) = filesystem(Path::new("/")) {
            self.main.set_values(total, free);
        }
    }
}

// -------------------------------------------------------------------------
// Sections, rows and drive bars
// -------------------------------------------------------------------------

fn section_view(
    name: &str,
    section: Section,
    content: &gtk::Box,
    expanded: bool,
    process: &Rc<Process>,
    window: WindowId,
) -> (gtk::Box, gtk::Revealer) {
    let view = gtk::Box::new(gtk::Orientation::Vertical, 0);
    view.add_css_class("gl-section");
    let pill = gtk::Button::new();
    pill.add_css_class("gl-section-pill");
    let label = gtk::Label::new(Some(name));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    pill.set_child(Some(&label));
    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    revealer.set_transition_duration(motion::ms(200));
    revealer.set_reveal_child(expanded);
    revealer.set_child(Some(content));
    view.append(&pill);
    view.append(&revealer);
    let weak = Rc::downgrade(process);
    pill.connect_clicked(move |_| {
        if let Some(process) = weak.upgrade() {
            process.dispatch_for_window(
                window,
                Caller::Ui,
                Invocation::new(
                    "view.toggle-section",
                    vec![("section".to_string(), section.as_str().to_string())],
                ),
            );
        }
    });
    (view, revealer)
}

fn rule() -> gtk::Box {
    let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rule.add_css_class("gl-rule");
    rule
}

fn dir_row(text: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
    row.add_css_class("gl-dir-row");
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.5);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&label);
    row
}

fn attach_click(
    widget: &impl IsA<gtk::Widget>,
    process: &Weak<Process>,
    window: WindowId,
    folder: PathBuf,
) {
    let click = gtk::GestureClick::new();
    click.set_button(1);
    let weak = process.clone();
    click.connect_released(move |_, _, _, _| {
        if let Some(process) = weak.upgrade() {
            process.nav_go_path(window, folder.clone());
        }
    });
    widget.add_controller(click);
}

fn clear_children(widget: &gtk::Box) {
    while let Some(child) = widget.first_child() {
        child.unparent();
    }
}

/// The fill fraction of a filesystem, 0 when its size is unknown.
pub fn drive_fullness(total: u64, free: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (total.saturating_sub(free) as f64 / total as f64).clamp(0.0, 1.0)
    }
}

struct DriveRow {
    root: gtk::Box,
    name: gtk::Label,
    fill: gtk::Box,
    percent: gtk::Label,
    capacity: gtk::Label,
    total: Cell<u64>,
    free: Cell<u64>,
    scale: Cell<f64>,
    bar_width: f64,
    main: bool,
}

impl DriveRow {
    fn new(name: &str, main: bool, scale: f64) -> Rc<Self> {
        let track = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        track.add_css_class("gl-drive-track");
        if main {
            track.add_css_class("main");
        }
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&track));
        let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        fill.add_css_class("gl-drive-fill");
        if main {
            fill.add_css_class("main");
        }
        fill.set_halign(gtk::Align::Start);
        fill.set_valign(gtk::Align::Fill);
        overlay.add_overlay(&fill);
        let percent = gtk::Label::new(None);
        percent.add_css_class("gl-drive-label");
        if main {
            percent.add_css_class("gl-main-label");
        }
        percent.set_halign(gtk::Align::Center);
        percent.set_valign(gtk::Align::Center);
        overlay.add_overlay(&percent);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("gl-drive-block");
        let name = gtk::Label::new(Some(name));
        name.add_css_class("gl-drive-name");
        name.set_xalign(0.5);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        root.append(&name);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.set_halign(gtk::Align::Center);
        row.append(&overlay);
        let capacity = gtk::Label::new(None);
        capacity.add_css_class("gl-drive-cap");
        capacity.set_ellipsize(gtk::pango::EllipsizeMode::End);
        if main {
            capacity.add_css_class("gl-main-label");
        }
        row.append(&capacity);
        root.append(&row);

        let this = Rc::new(Self {
            root,
            name,
            fill,
            percent,
            capacity,
            total: Cell::new(0),
            free: Cell::new(0),
            scale: Cell::new(scale),
            bar_width: if main { MAIN_BAR_W } else { BAR_W },
            main,
        });
        this.set_values(0, 0);
        this
    }

    fn set_name(&self, name: &str) {
        self.name.set_text(name);
    }

    fn set_values(&self, total: u64, free: u64) {
        self.total.set(total);
        self.free.set(free);
        self.capacity.set_text(&glib::format_size(total));
        self.paint();
    }

    fn set_scale(&self, scale: f64) {
        self.scale.set(scale);
        self.paint();
    }

    fn paint(&self) {
        let total = self.total.get();
        let free = self.free.get();
        let fullness = drive_fullness(total, free);
        let scale = self.scale.get();
        let width = scaled(self.bar_width * fullness, scale).max(0);
        self.fill.set_width_request(width);
        let (fill_class, label_class) = bar_class(fullness);
        for class in BAR_FILL_CLASSES {
            self.fill.remove_css_class(class);
        }
        self.fill.add_css_class(fill_class);
        for class in BAR_LABEL_CLASSES {
            self.percent.remove_css_class(class);
        }
        self.percent.add_css_class(label_class);
        self.percent
            .set_text(&format!("{}% Full", (fullness * 100.0).round() as i32));
        if self.main {
            self.fill.add_css_class("main");
        }
    }
}

fn filesystem(path: &Path) -> Option<(u64, u64)> {
    let file = gio::File::for_path(path);
    let info = file
        .query_filesystem_info(
            "filesystem::size,filesystem::free",
            None::<&gio::Cancellable>,
        )
        .ok()?;
    Some((
        info.attribute_uint64("filesystem::size"),
        info.attribute_uint64("filesystem::free"),
    ))
}

fn root_mount_name(monitor: &gio::VolumeMonitor) -> String {
    monitor
        .mounts()
        .into_iter()
        .find(|mount| mount.root().path().as_deref() == Some(Path::new("/")))
        .map(|mount| mount.name().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Main Drive".to_string())
}

/// The folders in the Recent list: the newest first, distinct, at most five.
pub fn push_recent(recent: &mut Vec<PathBuf>, folder: &Path) {
    if let Some(position) = recent.iter().position(|candidate| candidate == folder) {
        recent.remove(position);
    }
    recent.insert(0, folder.to_path_buf());
    recent.truncate(5);
}

fn recent_label(path: &Path, home: &Path) -> String {
    if config::normalize_path(path) == config::normalize_path(home) {
        "Home".to_string()
    } else {
        path_label(path)
    }
}

fn path_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn pin_label(path: &Path, home: &Path) -> String {
    recent_label(path, home)
}

fn pin_icon(label: &str) -> &'static str {
    match label {
        "Home" => "user-home-symbolic",
        "Documents" => "folder-documents-symbolic",
        "Downloads" => "folder-download-symbolic",
        "Music" => "folder-music-symbolic",
        "Pictures" => "folder-pictures-symbolic",
        "Videos" => "folder-videos-symbolic",
        _ => "folder-symbolic",
    }
}

// -------------------------------------------------------------------------
// Pinned rows: one custom layout, absolute y, drag over the threshold only
// -------------------------------------------------------------------------

/// The row order after moving the pin at `from` to `to`: one remove/insert.
pub fn pin_order(n: usize, from: usize, to: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).filter(|&index| index != from).collect();
    if from < n && to <= order.len() {
        order.insert(to, from);
    }
    order
}

/// The live pinned-row layout: which slot the pointer is over and, indexed by
/// the row's original slot, the y every row sits at while the dragged row
/// follows the pointer.
pub fn pin_drag_layout(
    n: usize,
    row_h: i32,
    from: usize,
    pointer_y: i32,
    grab: i32,
) -> (Option<usize>, Vec<i32>) {
    if n == 0 || from >= n || row_h <= 0 {
        return (None, vec![]);
    }
    let target = (pointer_y.div_euclid(row_h)).clamp(0, (n - 1) as i32) as usize;
    let order = pin_order(n, from, target);
    let mut y = vec![0; n];
    for (slot, &row) in order.iter().enumerate() {
        y[row] = slot as i32 * row_h;
    }
    y[from] = pointer_y - grab;
    (Some(target), y)
}

struct PinDrag {
    from: usize,
    grab: i32,
    start: (f64, f64),
    bounds: (f64, f64),
    transform: (f64, f64),
    claimed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinRelease {
    Select { slot: usize },
    Open { slot: usize },
    Reorder { from: usize, to: usize },
}

/// The row a pointer at `y` (in the rows' own space) is over.
pub fn pin_slot(y: f64, row_h: i32, n: usize) -> usize {
    if n == 0 || row_h <= 0 {
        return 0;
    }
    ((y as i32).div_euclid(row_h)).clamp(0, (n - 1) as i32) as usize
}

/// The decision at release, pure: a still pointer selects (and a double-click
/// opens); only `moved` can reorder.
pub fn pin_release(
    n: usize,
    row_h: i32,
    from: usize,
    pointer_y: f64,
    n_press: i32,
    moved: bool,
) -> PinRelease {
    if n == 0 || row_h <= 0 {
        return PinRelease::Select { slot: 0 };
    }
    let from = from.min(n - 1);
    if !moved {
        return if n_press >= 2 {
            PinRelease::Open { slot: from }
        } else {
            PinRelease::Select { slot: from }
        };
    }
    let to = pin_slot(pointer_y, row_h, n);
    if to == from {
        PinRelease::Select { slot: from }
    } else {
        PinRelease::Reorder { from, to }
    }
}

/// The pointer in the rows' own space: the surface position minus the window's
/// surface transform and the rows' bounds in the window (P1m's lesson).
pub fn pin_local(
    surface: (f64, f64),
    surface_transform: (f64, f64),
    bounds: (f64, f64),
) -> (f64, f64) {
    (
        surface.0 - surface_transform.0 - bounds.0,
        surface.1 - surface_transform.1 - bounds.1,
    )
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PinnedRows {
        /// Every row's y in the container, keyed by the widget itself.
        pub positions: RefCell<Vec<(gtk::Widget, f64)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PinnedRows {
        const NAME: &'static str = "GimbalPinnedRows";
        type Type = super::PinnedRows;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for PinnedRows {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().set_layout_manager(Some(PinnedRowLayout::new()));
        }
    }

    impl WidgetImpl for PinnedRows {}
}

glib::wrapper! {
    pub struct PinnedRows(ObjectSubclass<imp::PinnedRows>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PinnedRows {
    fn new() -> Self {
        glib::Object::new::<Self>()
    }

    fn append_row(&self, row: &gtk::Box, y: f64) {
        row.insert_after(self, self.last_child().as_ref());
        self.set_y(row, y);
    }

    fn set_y(&self, row: &gtk::Box, y: f64) {
        let child: gtk::Widget = row.clone().upcast();
        {
            let mut positions = self.imp().positions.borrow_mut();
            match positions.iter_mut().find(|(widget, _)| *widget == child) {
                Some((_, slot)) => *slot = y,
                None => positions.push((child, y)),
            }
        }
        self.queue_allocate();
    }

    fn y_of_widget(&self, child: &gtk::Widget) -> f64 {
        self.imp()
            .positions
            .borrow()
            .iter()
            .find(|(widget, _)| widget == child)
            .map(|(_, y)| *y)
            .unwrap_or(0.0)
    }

    fn forget(&self, child: &gtk::Widget) {
        self.imp()
            .positions
            .borrow_mut()
            .retain(|(widget, _)| widget != child);
    }

    fn clear_rows(&self) {
        while let Some(child) = self.first_child() {
            self.forget(&child);
            child.unparent();
        }
        self.imp().positions.borrow_mut().clear();
        self.queue_allocate();
    }
}

mod layout_imp {
    use super::*;

    #[derive(Default)]
    pub struct PinnedRowLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for PinnedRowLayout {
        const NAME: &'static str = "GimbalPinnedRowLayout";
        type Type = super::PinnedRowLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for PinnedRowLayout {}

    impl LayoutManagerImpl for PinnedRowLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let rows = widget
                .downcast_ref::<PinnedRows>()
                .expect("PinnedRowLayout is only set on PinnedRows");
            let (mut minimum, mut natural) = (0, 0);
            let mut child = widget.first_child();
            while let Some(widget) = child {
                let (child_min, child_nat, _, _) = widget.measure(orientation, for_size);
                if orientation == gtk::Orientation::Vertical {
                    let y = rows.y_of_widget(&widget).round() as i32;
                    minimum = minimum.max(y + child_min);
                    natural = natural.max(y + child_nat);
                } else {
                    minimum = minimum.max(child_min);
                    natural = natural.max(child_nat);
                }
                child = widget.next_sibling();
            }
            (minimum, natural, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, _height: i32, _baseline: i32) {
            let rows = widget
                .downcast_ref::<PinnedRows>()
                .expect("PinnedRowLayout is only set on PinnedRows");
            let mut child = widget.first_child();
            while let Some(widget) = child {
                let y = rows.y_of_widget(&widget);
                let (_, natural, _, _) = widget.measure(gtk::Orientation::Vertical, width);
                let height = natural.max(1);
                let transform =
                    gtk::gsk::Transform::new().translate(&gtk::graphene::Point::new(0.0, y as f32));
                widget.allocate(width, height, -1, Some(transform));
                child = widget.next_sibling();
            }
        }
    }
}

glib::wrapper! {
    pub struct PinnedRowLayout(ObjectSubclass<layout_imp::PinnedRowLayout>) @extends gtk::LayoutManager;
}

impl PinnedRowLayout {
    fn new() -> Self {
        glib::Object::new::<Self>()
    }
}

struct PinnedList {
    root: gtk::Box,
    rows: PinnedRows,
    names: RefCell<Vec<PathBuf>>,
    widgets: RefCell<Vec<gtk::Box>>,
    gens: RefCell<Vec<Rc<Cell<u64>>>>,
    selected: Cell<Option<usize>>,
    scale: Cell<f64>,
    drag: RefCell<Option<PinDrag>>,
    animations: RefCell<Vec<adw::Animation>>,
    click: gtk::GestureClick,
    process: Weak<Process>,
    window: WindowId,
    home: PathBuf,
}

impl PinnedList {
    fn new(process: &Rc<Process>, window: WindowId, home: PathBuf) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("gl-pinned");
        let rows = PinnedRows::new();
        rows.set_hexpand(true);
        rows.set_valign(gtk::Align::Start);
        root.append(&rows);
        let click = gtk::GestureClick::new();
        let this = Rc::new(Self {
            root,
            rows,
            names: RefCell::new(Vec::new()),
            widgets: RefCell::new(Vec::new()),
            gens: RefCell::new(Vec::new()),
            selected: Cell::new(None),
            scale: Cell::new(1.0),
            drag: RefCell::new(None),
            animations: RefCell::new(Vec::new()),
            click: click.clone(),
            process: Rc::downgrade(process),
            window,
            home,
        });

        let weak = Rc::downgrade(&this);
        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin(move |_, x, y| {
            if let Some(this) = weak.upgrade() {
                this.begin_drag(x, y);
            }
        });
        let weak = Rc::downgrade(&this);
        drag.connect_drag_update(move |gesture, dx, dy| {
            if let Some(this) = weak.upgrade() {
                this.update_drag(gesture, dx, dy);
            }
        });
        let weak = Rc::downgrade(&this);
        drag.connect_drag_end(move |gesture, dx, dy| {
            if let Some(this) = weak.upgrade() {
                this.end_drag(gesture, dx, dy);
            }
        });
        let weak = Rc::downgrade(&this);
        drag.connect_cancel(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.cancel_drag();
            }
        });
        this.rows.add_controller(drag);

        let weak = Rc::downgrade(&this);
        let click = this.click.clone();
        click.set_button(1);
        click.connect_released(move |_, n_press, _, y| {
            if let Some(this) = weak.upgrade() {
                this.clicked(n_press, y);
            }
        });
        this.rows.add_controller(click);

        let weak = Rc::downgrade(&this);
        let right = gtk::GestureClick::new();
        right.set_button(3);
        right.connect_pressed(move |_, _, x, y| {
            if let Some(this) = weak.upgrade() {
                this.show_unpin(x, y);
            }
        });
        this.rows.add_controller(right);
        this
    }

    fn row_h(&self) -> i32 {
        scaled(PIN_ROW_H, self.scale.get())
    }

    fn paths(&self) -> Vec<PathBuf> {
        self.names.borrow().clone()
    }

    fn names_len(&self) -> usize {
        self.names.borrow().len()
    }

    fn rebuild(&self, paths: &[PathBuf]) {
        self.animations.borrow_mut().clear();
        *self.drag.borrow_mut() = None;
        self.rows.clear_rows();
        *self.names.borrow_mut() = paths.to_vec();
        let row_h = self.row_h();
        let mut widgets = Vec::with_capacity(paths.len());
        let mut gens = Vec::with_capacity(paths.len());
        for (index, path) in paths.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row.add_css_class("gl-pinned-row");
            row.set_size_request(-1, row_h);
            row.set_tooltip_text(Some(&path.display().to_string()));
            if self.selected.get() == Some(index) {
                row.add_css_class("sel");
            }
            let label = pin_label(path, &self.home);
            row.append(&gtk::Image::from_icon_name(pin_icon(&label)));
            let name = gtk::Label::new(Some(&label));
            name.set_hexpand(true);
            name.set_xalign(0.0);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.append(&name);
            self.rows.append_row(&row, (index as i32 * row_h) as f64);
            widgets.push(row);
            gens.push(Rc::new(Cell::new(0)));
        }
        *self.widgets.borrow_mut() = widgets;
        *self.gens.borrow_mut() = gens;
        self.rows.set_size_request(-1, paths.len() as i32 * row_h);
        if self
            .selected
            .get()
            .is_some_and(|index| index >= paths.len())
        {
            self.selected.set(None);
        }
    }

    fn set_scale(&self, scale: f64) {
        self.scale.set(scale);
        let row_h = self.row_h();
        self.animations.borrow_mut().clear();
        let widgets = self.widgets.borrow();
        for (index, row) in widgets.iter().enumerate() {
            self.bump_gen(index);
            row.set_size_request(-1, row_h);
            self.rows.set_y(row, (index as i32 * row_h) as f64);
        }
        self.rows
            .set_size_request(-1, self.names_len() as i32 * row_h);
    }

    fn select_for(&self, folder: &Path, home: &Path) {
        let target = config::normalize_path(folder);
        let index = self
            .paths()
            .iter()
            .position(|pin| config::normalize_path(&config::expand_home(pin, home)) == target);
        self.select(index);
    }

    fn select(&self, slot: Option<usize>) {
        let slot = slot.filter(|index| *index < self.names_len());
        self.selected.set(slot);
        for (index, row) in self.widgets.borrow().iter().enumerate() {
            row.remove_css_class("sel");
            if slot == Some(index) {
                row.add_css_class("sel");
            }
        }
    }

    fn clicked(&self, n_press: i32, y: f64) {
        let n = self.names_len();
        let row_h = self.row_h();
        if n == 0 || y < 0.0 {
            return;
        }
        let slot = pin_slot(y, row_h, n);
        match pin_release(n, row_h, slot, y, n_press, false) {
            PinRelease::Open { slot } => self.open_slot(slot),
            _ => self.select(Some(slot)),
        }
    }

    fn open_slot(&self, slot: usize) {
        let Some(path) = self.names.borrow().get(slot).cloned() else {
            return;
        };
        if let Some(process) = self.process.upgrade() {
            process.nav_go_path(self.window, path);
        }
    }

    fn show_unpin(&self, x: f64, y: f64) {
        let n = self.names_len();
        let row_h = self.row_h();
        if n == 0 || y < 0.0 || row_h <= 0 {
            return;
        }
        let slot = pin_slot(y, row_h, n);
        let Some(path) = self.names.borrow().get(slot).cloned() else {
            return;
        };
        let popover = gtk::Popover::new();
        popover.add_css_class("gl-menu");
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let button = gtk::Button::new();
        button.add_css_class("gl-menu-item");
        button.set_child(Some(&gtk::Label::new(Some("Unpin"))));
        menu.append(&button);
        popover.set_child(Some(&menu));
        popover.set_parent(&self.rows);
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        let process = self.process.clone();
        let window = self.window;
        let popover_for_button = popover.clone();
        button.connect_clicked(move |_| {
            popover_for_button.popdown();
            let Some(path) = path.to_str().map(str::to_string) else {
                crate::log!("pins", "cannot unpin a non-UTF-8 path");
                return;
            };
            if let Some(process) = process.upgrade() {
                process.dispatch_for_window(
                    window,
                    Caller::Ui,
                    Invocation::new("pins.remove", vec![("path".to_string(), path)]),
                );
            }
        });
        popover.connect_closed(|popover| popover.unparent());
        popover.popup();
    }

    fn dispatch(&self, invocation: Invocation) {
        if let Some(process) = self.process.upgrade() {
            process.dispatch_for_window(self.window, Caller::Ui, invocation);
        }
    }

    fn begin_drag(&self, x: f64, y: f64) {
        let n = self.names_len();
        let row_h = self.row_h();
        if n == 0 || row_h <= 0 || y < 0.0 {
            return;
        }
        let from = pin_slot(y, row_h, n);
        let transform = self
            .rows
            .native()
            .map(|native| native.surface_transform())
            .unwrap_or((0.0, 0.0));
        let bounds = self
            .rows
            .root()
            .and_then(|root| self.rows.compute_bounds(&root))
            .map(|bounds| (bounds.x() as f64, bounds.y() as f64))
            .unwrap_or((0.0, 0.0));
        let grab = y - from as f64 * row_h as f64;
        *self.drag.borrow_mut() = Some(PinDrag {
            from,
            grab: grab.round() as i32,
            start: (x, y),
            bounds,
            transform,
            claimed: false,
        });
    }

    fn update_drag(&self, gesture: &gtk::GestureDrag, dx: f64, dy: f64) {
        let Some(mut drag) = self.drag.borrow_mut().take() else {
            return;
        };
        let local = match gesture.current_event().and_then(|event| event.position()) {
            Some((sx, sy)) => pin_local((sx, sy), drag.transform, drag.bounds),
            None => (drag.start.0 + dx, drag.start.1 + dy),
        };
        let moved = self.rows.drag_check_threshold(
            0,
            0,
            (local.0 - drag.start.0).round() as i32,
            (local.1 - drag.start.1).round() as i32,
        );
        if !drag.claimed && moved && gesture.set_state(gtk::EventSequenceState::Claimed) {
            drag.claimed = true;
            self.click.set_state(gtk::EventSequenceState::Denied);
            let widgets = self.widgets.borrow();
            if let Some(row) = widgets.get(drag.from).cloned() {
                drop(widgets);
                if self.rows.last_child().as_ref() != Some(row.clone().upcast_ref()) {
                    let last = self.rows.last_child();
                    row.insert_after(&self.rows, last.as_ref());
                }
            }
            self.bump_gen(drag.from);
            crate::log!(
                "pins",
                "dragging '{}' from slot {}",
                self.names.borrow()[drag.from].display(),
                drag.from
            );
        }
        if !drag.claimed {
            *self.drag.borrow_mut() = Some(drag);
            return;
        }
        let n = self.names_len();
        let row_h = self.row_h();
        let (target, y) = pin_drag_layout(n, row_h, drag.from, local.1.round() as i32, drag.grab);
        self.layout_drag(drag.from, &y, row_h);
        crate::log!("pins", "drag over slot {target:?}");
        *self.drag.borrow_mut() = Some(drag);
    }

    fn layout_drag(&self, dragged: usize, y: &[i32], row_h: i32) {
        let widgets = self.widgets.borrow();
        for (index, row) in widgets.iter().enumerate() {
            let desired = y.get(index).copied().unwrap_or(index as i32 * row_h);
            if index == dragged {
                self.rows.set_y(row, desired as f64);
            } else {
                let current = self
                    .rows
                    .y_of_widget(&row.clone().upcast::<gtk::Widget>())
                    .round() as i32;
                if current != desired {
                    self.slide_y(index, row, desired);
                }
            }
        }
    }

    fn bump_gen(&self, row: usize) {
        if let Some(generation) = self.gens.borrow().get(row) {
            generation.set(generation.get() + 1);
        }
    }

    fn slide_y(&self, row: usize, widget: &gtk::Box, to: i32) {
        let start = self
            .rows
            .y_of_widget(&widget.clone().upcast::<gtk::Widget>());
        if (start - to as f64).abs() < 0.5 {
            self.rows.set_y(widget, to as f64);
            return;
        }
        let generation_cell = self.gens.borrow()[row].clone();
        let generation = generation_cell.get() + 1;
        generation_cell.set(generation);
        let rows = self.rows.clone();
        let widget = widget.clone();
        let animation_widget = widget.clone();
        let target = adw::CallbackAnimationTarget::new(move |value| {
            if generation_cell.get() != generation {
                return;
            }
            let y = start + (to as f64 - start) * value;
            rows.set_y(&widget, y);
        });
        let animation =
            adw::TimedAnimation::new(&animation_widget, 0.0, 1.0, motion::ms(150), target);
        self.animations
            .borrow_mut()
            .push(animation.clone().upcast());
        animation.play();
    }

    fn end_drag(&self, gesture: &gtk::GestureDrag, dx: f64, dy: f64) {
        let Some(drag) = self.drag.borrow_mut().take() else {
            return;
        };
        let local = match gesture.current_event().and_then(|event| event.position()) {
            Some((sx, sy)) => pin_local((sx, sy), drag.transform, drag.bounds),
            None => (drag.start.0 + dx, drag.start.1 + dy),
        };
        let n = self.names_len();
        let row_h = self.row_h();
        let release = pin_release(n, row_h, drag.from, local.1, 1, drag.claimed);
        self.settle();
        if let PinRelease::Reorder { from, to } = release {
            crate::log!(
                "pins",
                "moving '{}' from slot {from} to {to}",
                self.names.borrow()[from].display()
            );
            self.dispatch(Invocation::new(
                "pins.move",
                vec![
                    ("from".to_string(), from.to_string()),
                    ("to".to_string(), to.to_string()),
                ],
            ));
        }
    }

    fn cancel_drag(&self) {
        self.drag.borrow_mut().take();
        self.settle();
    }

    fn settle(&self) {
        self.animations.borrow_mut().clear();
        let row_h = self.row_h();
        let widgets = self.widgets.borrow();
        let mut sibling: Option<gtk::Widget> = None;
        for (index, row) in widgets.iter().enumerate() {
            row.insert_after(&self.rows, sibling.as_ref());
            row.remove_css_class("sel");
            if self.selected.get() == Some(index) {
                row.add_css_class("sel");
            }
            self.bump_gen(index);
            self.slide_y(index, row, index as i32 * row_h);
            sibling = Some(row.clone().upcast());
        }
        self.rows
            .set_size_request(-1, self.names_len() as i32 * row_h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fullness_class(total: u64, free: u64) -> (&'static str, &'static str) {
        bar_class(drive_fullness(total, free))
    }

    #[test]
    fn drive_fullness_classes_at_the_edges() {
        assert_eq!(fullness_class(1000, 1000), ("bar-lv0", "bar-label-lo"));
        assert_eq!(fullness_class(1000, 500), ("bar-lv5", "bar-label-hi"));
        assert_eq!(fullness_class(1000, 60), ("bar-lv9", "bar-label-hi"));
        assert_eq!(fullness_class(1000, 50), ("bar-full", "bar-label-err"));
        assert_eq!(fullness_class(1000, 0), ("bar-full", "bar-label-err"));
        assert_eq!(drive_fullness(0, 0), 0.0);
    }

    #[test]
    fn recent_list_dedupes_caps_and_puts_the_newest_first() {
        let mut recent = Vec::new();
        for path in ["a", "b", "c", "d", "e", "f"] {
            push_recent(&mut recent, Path::new(path));
        }
        let names: Vec<&str> = recent.iter().map(|path| path.to_str().unwrap()).collect();
        assert_eq!(names, ["f", "e", "d", "c", "b"]);

        push_recent(&mut recent, Path::new("d"));
        let names: Vec<&str> = recent.iter().map(|path| path.to_str().unwrap()).collect();
        assert_eq!(names, ["d", "f", "e", "c", "b"]);
    }

    #[test]
    fn every_pin_reorder_moves_exactly_one_row() {
        for n in 1..=7 {
            for from in 0..n {
                for to in 0..n {
                    let order = pin_order(n, from, to);
                    let mut sorted = order.clone();
                    sorted.sort_unstable();
                    assert_eq!(sorted, (0..n).collect::<Vec<_>>());
                    assert_eq!(order[to], from);
                    let others: Vec<usize> =
                        order.iter().copied().filter(|&row| row != from).collect();
                    let expected: Vec<usize> = (0..n).filter(|&row| row != from).collect();
                    assert_eq!(others, expected);
                }
            }
        }
    }

    #[test]
    fn only_a_past_threshold_press_reorders() {
        let h = 44;
        assert_eq!(
            pin_release(7, h, 0, 2.0 * h as f64 + 10.0, 1, true),
            PinRelease::Reorder { from: 0, to: 2 }
        );
        assert_eq!(
            pin_release(7, h, 4, 4.0 * h as f64 + 10.0, 2, false),
            PinRelease::Open { slot: 4 }
        );
        assert_eq!(
            pin_release(7, h, 3, 3.0 * h as f64 + 10.0, 1, false),
            PinRelease::Select { slot: 3 }
        );
        assert_eq!(
            pin_release(7, h, 3, 3.0 * h as f64 + 10.0, 1, true),
            PinRelease::Select { slot: 3 }
        );
    }

    #[test]
    fn drag_layout_uses_original_row_indices() {
        let h = 44;
        for from in 0..5usize {
            for to in 0..5usize {
                let (target, y) = pin_drag_layout(5, h, from, to as i32 * h + 10, 10);
                assert_eq!(target, Some(to));
                assert_eq!(y[from], to as i32 * h);
                let order = pin_order(5, from, to);
                for (slot, &row) in order.iter().enumerate() {
                    if row != from {
                        assert_eq!(y[row], slot as i32 * h);
                    }
                }
            }
        }
    }
}
