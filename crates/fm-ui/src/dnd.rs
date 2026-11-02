// SPDX-License-Identifier: GPL-3.0-or-later
//! Drag and drop: the outgoing `gdk::FileList` and every drop target.
//!
//! The pure rules (target resolution, the copy-or-move decision, refusals and
//! the count badge) sit at the top and are unit-tested. The GTK plumbing below
//! only reads those rules and submits work through the P2k worker. An outgoing
//! drag never deletes or moves its source; the drop side does every move
//! (Phase 0's rule, `CLAUDE.md`).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use fm_core::workspace::WindowId;

use crate::app::Process;
use crate::look::tokens::ICON_GRID;

/// The spring-loaded tab delay.
const SPRING_MS: u64 = 700;
/// A filesystem query that takes longer falls back to copy.
const FS_QUERY_TIMEOUT_MS: u64 = 250;

// ---------------------------------------------------------------------------
// Pure rules
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropModifiers {
    None,
    Ctrl,
    Shift,
}

impl DropModifiers {
    pub fn from_state(state: gdk::ModifierType) -> Self {
        if state.contains(gdk::ModifierType::CONTROL_MASK) {
            Self::Ctrl
        } else if state.contains(gdk::ModifierType::SHIFT_MASK) {
            Self::Shift
        } else {
            Self::None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Ctrl => "Ctrl",
            Self::Shift => "Shift",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropAction {
    Copy,
    Move,
}

/// Nautilus/Dolphin's default, then the modifiers: Ctrl forces copy and Shift
/// forces move. Ctrl wins when both are down, as the grid's clicks do.
pub fn action_for(same_filesystem: bool, modifiers: DropModifiers) -> DropAction {
    match modifiers {
        DropModifiers::Ctrl => DropAction::Copy,
        DropModifiers::Shift => DropAction::Move,
        DropModifiers::None if same_filesystem => DropAction::Move,
        DropModifiers::None => DropAction::Copy,
    }
}

/// Every place a `gdk::FileList` can land.
#[allow(dead_code)] // every variant is exercised by the target-resolution tests
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Grid(PathBuf),
    FolderCell(PathBuf),
    TabPill(PathBuf),
    TilePill(PathBuf),
    PinRow(PathBuf),
    Device(PathBuf),
    FileCell,
    PinList,
    Trash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Folder(PathBuf),
    Pins,
    Trash,
}

/// Which folder a target means, or why it refuses. A file cell is the only
/// cell that is not a target.
pub fn destination(target: &Target) -> Result<Destination, String> {
    match target {
        Target::Grid(folder)
        | Target::FolderCell(folder)
        | Target::TabPill(folder)
        | Target::TilePill(folder)
        | Target::PinRow(folder)
        | Target::Device(folder) => Ok(Destination::Folder(folder.clone())),
        Target::FileCell => Err("only folders are drop targets".to_string()),
        Target::PinList => Ok(Destination::Pins),
        Target::Trash => Ok(Destination::Trash),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Copy { to: PathBuf },
    Move { to: PathBuf },
    Trash,
    Pin,
}

/// Refuse a folder drop before any worker runs. Paths are all this reads: no
/// filesystem call is made on the main thread to decide a drop.
pub fn validate_folder_drop(folder: &Path, paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Err("there is nothing to drop".to_string());
    }
    if paths.iter().all(|path| path.parent() == Some(folder)) {
        return Err(format!("{} is already in this folder", folder.display()));
    }
    for path in paths {
        if path == folder || folder.starts_with(path) {
            return Err("a folder cannot be dropped into itself".to_string());
        }
    }
    Ok(())
}

pub fn operation_for(
    target: &Target,
    paths: &[PathBuf],
    same_filesystem: bool,
    modifiers: DropModifiers,
) -> Result<Operation, String> {
    match destination(target)? {
        Destination::Folder(folder) => {
            validate_folder_drop(&folder, paths)?;
            Ok(match action_for(same_filesystem, modifiers) {
                DropAction::Copy => Operation::Copy { to: folder },
                DropAction::Move => Operation::Move { to: folder },
            })
        }
        Destination::Trash => Ok(Operation::Trash),
        Destination::Pins => Ok(Operation::Pin),
    }
}

/// A drag's item, with the type the grid already knows. The pin list needs
/// this to ignore files without a filesystem call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragItem {
    pub path: PathBuf,
    pub is_dir: bool,
}

pub fn folder_paths(items: &[DragItem]) -> Vec<PathBuf> {
    items
        .iter()
        .filter(|item| item.is_dir)
        .map(|item| item.path.clone())
        .collect()
}

/// The drag icon's badge. One item needs none.
pub fn badge_text(count: usize) -> Option<String> {
    (count > 1).then(|| count.to_string())
}

// ---------------------------------------------------------------------------
// The drag in progress
// ---------------------------------------------------------------------------

thread_local! {
    /// The item types of our own drag. GTK's content provider carries paths
    /// only; keeping the types here avoids an `is_dir` on the drop path. It is
    /// cleared on drag-end and drag-cancel.
    static INTERNAL_DRAG: RefCell<Option<Vec<DragItem>>> = const { RefCell::new(None) };
    /// How many drop targets currently have a drop hovering over them. The
    /// spring-loaded pill must not focus tabs on an ordinary pointer hover.
    static DROP_HOVERS: Cell<u32> = const { Cell::new(0) };
}

fn drop_hover_enter() {
    DROP_HOVERS.with(|count| count.set(count.get().saturating_add(1)));
}

fn drop_hover_leave() {
    DROP_HOVERS.with(|count| count.set(count.get().saturating_sub(1)));
}

fn drop_hover_active() -> bool {
    DROP_HOVERS.with(Cell::get) > 0
}

pub fn set_internal_drag(items: Vec<DragItem>) {
    INTERNAL_DRAG.with(|drag| *drag.borrow_mut() = Some(items));
}

pub fn clear_internal_drag() {
    INTERNAL_DRAG.with(|drag| *drag.borrow_mut() = None);
    DROP_HOVERS.with(|count| count.set(0));
}

fn internal_drag() -> Option<Vec<DragItem>> {
    INTERNAL_DRAG.with(|drag| drag.borrow().clone())
}

// ---------------------------------------------------------------------------
// GTK helpers
// ---------------------------------------------------------------------------

fn paths_from_value(value: &glib::Value) -> Vec<PathBuf> {
    let Ok(list) = value.get::<gdk::FileList>() else {
        return Vec::new();
    };
    list.files().iter().filter_map(|file| file.path()).collect()
}

fn target_paths(target: &gtk::DropTarget) -> Option<Vec<PathBuf>> {
    let value = target.value()?;
    let paths = paths_from_value(&value);
    (!paths.is_empty()).then_some(paths)
}

fn target_modifiers(target: &gtk::DropTarget) -> DropModifiers {
    let state = target
        .current_drop()
        .and_then(|drop| drop.drag())
        .map(|drag| drag.device().modifier_state())
        .unwrap_or_else(gdk::ModifierType::empty);
    DropModifiers::from_state(state)
}

/// A unique action for `connect_enter`/`connect_motion`, narrowed to what the
/// source offers. The default is move, matching the same-filesystem case; the
/// filesystem check happens only on drop.
fn preferred_action(target: &gtk::DropTarget, modifiers: DropModifiers) -> gdk::DragAction {
    let want = match modifiers {
        DropModifiers::Ctrl => gdk::DragAction::COPY,
        DropModifiers::Shift => gdk::DragAction::MOVE,
        DropModifiers::None => gdk::DragAction::MOVE,
    };
    let available = target
        .current_drop()
        .map(|drop| drop.actions())
        .unwrap_or(gdk::DragAction::COPY | gdk::DragAction::MOVE);
    if available.contains(want) {
        want
    } else if available.contains(gdk::DragAction::COPY) {
        gdk::DragAction::COPY
    } else {
        gdk::DragAction::MOVE
    }
}

fn selected_action(target: &gtk::DropTarget) -> Option<gdk::DragAction> {
    target
        .current_drop()
        .and_then(|drop| drop.drag())
        .map(|drag| drag.selected_action())
}

fn log_hover(target: &gtk::DropTarget, what: &str, last: &Cell<Option<gdk::DragAction>>) {
    let action = selected_action(target);
    if action == last.get() {
        return;
    }
    last.set(action);
    let modifiers = target_modifiers(target);
    crate::log!(
        "dnd",
        "hover {what}: source reports {:?}, target prefers {:?}, modifiers {}",
        action,
        preferred_action(target, modifiers),
        modifiers.as_str()
    );
}

fn make_target() -> gtk::DropTarget {
    let target = gtk::DropTarget::new(
        gdk::FileList::static_type(),
        gdk::DragAction::COPY | gdk::DragAction::MOVE,
    );
    // The local FileList is available synchronously; a remote provider fills
    // it later. Preloading lets the highlight follow the value.
    target.set_preload(true);
    target
}

fn set_highlight(widget: &gtk::Widget, on: bool) {
    if on {
        widget.add_css_class("gl-drop-target");
    } else {
        widget.remove_css_class("gl-drop-target");
    }
}

fn refresh_folder_highlight(
    target: &gtk::DropTarget,
    widget: &gtk::Widget,
    folder: &Rc<dyn Fn() -> Option<PathBuf>>,
    mask: Option<&Rc<dyn Fn(f64, f64) -> bool>>,
    at: (f64, f64),
) {
    if mask.is_some_and(|mask| !mask(at.0, at.1)) {
        set_highlight(widget, false);
        return;
    }
    let valid = match folder() {
        Some(folder) => match target_paths(target) {
            Some(paths) => validate_folder_drop(&folder, &paths).is_ok(),
            // The value has not arrived yet: stay a candidate until it does.
            None => true,
        },
        None => false,
    };
    set_highlight(widget, valid);
}

/// A target that drops into a folder: a grid's empty space, a folder cell, a
/// pill, a pinned row or a device row. `mask` lets a parent target bow out
/// while the pointer is over a child target (the grid over its cells).
pub fn attach_folder_target(
    widget: &impl IsA<gtk::Widget>,
    process: Weak<Process>,
    window: WindowId,
    folder: Rc<dyn Fn() -> Option<PathBuf>>,
    none_reason: Option<&'static str>,
    mask: Option<Rc<dyn Fn(f64, f64) -> bool>>,
) {
    let target = make_target();
    let widget: gtk::Widget = widget.clone().upcast();
    let entered = Rc::new(Cell::new(false));
    let last_action = Rc::new(Cell::new(None));
    let last_pos = Rc::new(Cell::new((0.0, 0.0)));

    {
        let widget = widget.clone();
        let folder = folder.clone();
        let entered = entered.clone();
        let last_action = last_action.clone();
        let last_pos = last_pos.clone();
        let mask = mask.clone();
        target.connect_enter(move |target, x, y| {
            drop_hover_enter();
            entered.set(true);
            last_pos.set((x, y));
            refresh_folder_highlight(target, &widget, &folder, mask.as_ref(), (x, y));
            log_hover(target, "folder target", &last_action);
            preferred_action(target, target_modifiers(target))
        });
    }
    {
        let widget = widget.clone();
        let folder = folder.clone();
        let last_action = last_action.clone();
        let last_pos = last_pos.clone();
        let mask = mask.clone();
        target.connect_motion(move |target, x, y| {
            last_pos.set((x, y));
            refresh_folder_highlight(target, &widget, &folder, mask.as_ref(), (x, y));
            log_hover(target, "folder target", &last_action);
            preferred_action(target, target_modifiers(target))
        });
    }
    {
        let widget = widget.clone();
        let folder = folder.clone();
        let entered = entered.clone();
        let last_pos = last_pos.clone();
        let mask = mask.clone();
        target.connect_value_notify(move |target| {
            if entered.get() && target.current_drop().is_some() {
                let at = last_pos.get();
                refresh_folder_highlight(target, &widget, &folder, mask.as_ref(), at);
            }
        });
    }
    {
        let widget = widget.clone();
        let entered = entered.clone();
        target.connect_leave(move |_| {
            drop_hover_leave();
            entered.set(false);
            set_highlight(&widget, false);
        });
    }
    {
        let process = process.clone();
        let mask = mask.clone();
        target.connect_drop(move |target, value, x, y| {
            if mask.as_ref().is_some_and(|mask| !mask(x, y)) {
                return false;
            }
            let paths = paths_from_value(value);
            if paths.is_empty() {
                return false;
            }
            let modifiers = target_modifiers(target);
            match folder() {
                Some(folder) => {
                    if let Err(reason) = validate_folder_drop(&folder, &paths) {
                        if let Some(process) = process.upgrade() {
                            process.show_drop_reason(window, &reason);
                        }
                        return false;
                    }
                    if let Some(process) = process.upgrade() {
                        process.drop_on_folder(window, folder, paths, modifiers);
                    }
                    true
                }
                None => {
                    if let (Some(process), Some(reason)) = (process.upgrade(), none_reason) {
                        process.show_drop_reason(window, reason);
                    }
                    false
                }
            }
        });
    }
    widget.add_controller(target);
}

/// The Trash button. It ignores modifiers and asks the worker for
/// `Request::Trash`.
pub fn attach_trash_target(
    widget: &impl IsA<gtk::Widget>,
    process: Weak<Process>,
    window: WindowId,
) {
    let target = make_target();
    let widget: gtk::Widget = widget.clone().upcast();
    let entered = Rc::new(Cell::new(false));
    let last_action = Rc::new(Cell::new(None));
    {
        let widget = widget.clone();
        let entered = entered.clone();
        let last_action = last_action.clone();
        target.connect_enter(move |target, _, _| {
            drop_hover_enter();
            entered.set(true);
            set_highlight(&widget, target_paths(target).is_some());
            log_hover(target, "Trash", &last_action);
            gdk::DragAction::COPY
        });
    }
    {
        let widget = widget.clone();
        let last_action = last_action.clone();
        target.connect_motion(move |target, _, _| {
            set_highlight(&widget, target_paths(target).is_some());
            log_hover(target, "Trash", &last_action);
            gdk::DragAction::COPY
        });
    }
    {
        let widget = widget.clone();
        let entered = entered.clone();
        target.connect_value_notify(move |target| {
            if entered.get() && target.current_drop().is_some() {
                set_highlight(&widget, target_paths(target).is_some());
            }
        });
    }
    {
        let widget = widget.clone();
        let entered = entered.clone();
        target.connect_leave(move |_| {
            drop_hover_leave();
            entered.set(false);
            set_highlight(&widget, false);
        });
    }
    target.connect_drop(move |_target, value, _, _| {
        let paths = paths_from_value(value);
        if paths.is_empty() {
            return false;
        }
        if let Some(process) = process.upgrade() {
            process.drop_on_trash(window, paths);
            true
        } else {
            false
        }
    });
    widget.add_controller(target);
}

/// The empty part of the Pinned Folders list: dropped folders are pinned, one
/// per folder; files are ignored.
pub fn attach_pin_target(widget: &impl IsA<gtk::Widget>, process: Weak<Process>, window: WindowId) {
    let target = make_target();
    let widget: gtk::Widget = widget.clone().upcast();
    let entered = Rc::new(Cell::new(false));
    let generation = Rc::new(Cell::new(0u64));
    let checking = Rc::new(Cell::new(false));
    let last_action = Rc::new(Cell::new(None));

    let refresh = {
        let widget = widget.clone();
        let entered = entered.clone();
        let generation = generation.clone();
        let checking = checking.clone();
        Rc::new(move |target: &gtk::DropTarget| {
            if !entered.get() {
                return;
            }
            let mine = generation.get() + 1;
            generation.set(mine);
            if let Some(items) = internal_drag() {
                set_highlight(&widget, items.iter().any(|item| item.is_dir));
                return;
            }
            let Some(paths) = target_paths(target) else {
                set_highlight(&widget, false);
                return;
            };
            if checking.replace(true) {
                return;
            }
            let widget = widget.clone();
            let entered = entered.clone();
            let generation = generation.clone();
            let checking = checking.clone();
            folder_flags(paths, move |flags| {
                checking.set(false);
                if entered.get() && generation.get() == mine {
                    set_highlight(&widget, flags.iter().any(|flag| *flag));
                }
            });
        })
    };

    {
        let refresh = refresh.clone();
        let last_action = last_action.clone();
        let entered = entered.clone();
        target.connect_enter(move |target, _, _| {
            drop_hover_enter();
            entered.set(true);
            refresh(target);
            log_hover(target, "pins", &last_action);
            gdk::DragAction::COPY
        });
    }
    {
        let refresh = refresh.clone();
        let last_action = last_action.clone();
        target.connect_motion(move |target, _, _| {
            refresh(target);
            log_hover(target, "pins", &last_action);
            gdk::DragAction::COPY
        });
    }
    {
        let refresh = refresh.clone();
        target.connect_value_notify(move |target| {
            refresh(target);
        });
    }
    {
        let widget = widget.clone();
        let entered = entered.clone();
        let generation = generation.clone();
        let checking = checking.clone();
        target.connect_leave(move |_| {
            drop_hover_leave();
            entered.set(false);
            checking.set(false);
            generation.set(generation.get() + 1);
            set_highlight(&widget, false);
        });
    }
    target.connect_drop(move |_target, value, _, _| {
        let paths = paths_from_value(value);
        if paths.is_empty() {
            return false;
        }
        let internal = internal_drag();
        if let Some(items) = internal {
            let folders: Vec<PathBuf> = folder_paths(&items)
                .into_iter()
                .filter(|path| paths.contains(path))
                .collect();
            return finish_pin_drop(&process, window, folders);
        }
        if process.upgrade().is_none() {
            return false;
        }
        pin_after_type_check(process.clone(), window, paths);
        true
    });
    widget.add_controller(target);
}

fn finish_pin_drop(process: &Weak<Process>, window: WindowId, folders: Vec<PathBuf>) -> bool {
    if folders.is_empty() {
        if let Some(process) = process.upgrade() {
            process.show_drop_reason(window, "only folders can be pinned");
        }
        return false;
    }
    if let Some(process) = process.upgrade() {
        process.pin_folders(window, folders);
        true
    } else {
        false
    }
}

/// Query each path's type off the drop path, then pin the folders.
fn pin_after_type_check(process: Weak<Process>, window: WindowId, paths: Vec<PathBuf>) {
    folder_flags(paths.clone(), move |flags| {
        let folders: Vec<PathBuf> = paths
            .iter()
            .zip(flags.iter())
            .filter(|(_, is_dir)| **is_dir)
            .map(|(path, _)| path.clone())
            .collect();
        let _ = finish_pin_drop(&process, window, folders);
    });
}

type FolderDone = Box<dyn FnOnce(Vec<bool>)>;
type BoolDone = Box<dyn FnOnce(bool)>;

fn folder_flags(paths: Vec<PathBuf>, done: impl FnOnce(Vec<bool>) + 'static) {
    let total = paths.len();
    if total == 0 {
        done(Vec::new());
        return;
    }
    let flags: Rc<RefCell<Vec<Option<bool>>>> = Rc::new(RefCell::new(vec![None; total]));
    let count = Rc::new(Cell::new(0usize));
    let done: Rc<RefCell<Option<FolderDone>>> = Rc::new(RefCell::new(Some(Box::new(done))));
    for (index, path) in paths.into_iter().enumerate() {
        let flags = flags.clone();
        let count = count.clone();
        let done = done.clone();
        gio::File::for_path(&path).query_info_async(
            "standard::type",
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
            None::<&gio::Cancellable>,
            move |result| {
                flags.borrow_mut()[index] = Some(
                    result
                        .ok()
                        .is_some_and(|info| info.file_type() == gio::FileType::Directory),
                );
                let seen = count.get() + 1;
                count.set(seen);
                if seen == total
                    && let Some(callback) = done.borrow_mut().take()
                {
                    callback(
                        flags
                            .borrow()
                            .iter()
                            .map(|flag| flag.unwrap_or(false))
                            .collect(),
                    );
                }
            },
        );
    }
}

/// Hovering a pill for 700 ms runs `action`, so a drag can open that tab
/// without being dropped first. Leaving cancels it.
///
/// A `DropControllerMotion`, not an `EventControllerMotion`: during a drag
/// GTK delivers crossing events only to drop controllers, so the plain
/// motion controller never saw the drag enter (Claude's P2l review).
pub fn spring_load(widget: &impl IsA<gtk::Widget>, action: Rc<dyn Fn()>) {
    let motion = gtk::DropControllerMotion::new();
    let generation = Rc::new(Cell::new(0u64));
    {
        let generation = generation.clone();
        motion.connect_enter(move |_, _, _| {
            let mine = generation.get() + 1;
            generation.set(mine);
            let generation = generation.clone();
            let action = action.clone();
            glib::timeout_add_local_once(Duration::from_millis(SPRING_MS), move || {
                if generation.get() == mine && drop_hover_active() {
                    action();
                }
            });
        });
    }
    {
        let generation = generation.clone();
        motion.connect_leave(move |_| {
            generation.set(generation.get() + 1);
        });
    }
    widget.add_controller(motion);
}

/// The drag icon: the first item's icon, with a small count badge.
pub fn drag_icon(icon: Option<&gio::Icon>, count: usize, scale: f64) -> gtk::Widget {
    let image = gtk::Image::new();
    match icon {
        Some(icon) => image.set_from_gicon(icon),
        None => image.set_icon_name(Some("text-x-generic")),
    }
    image.set_pixel_size(scaled(ICON_GRID, scale));
    let overlay = gtk::Overlay::new();
    overlay.add_css_class("gl-drag-icon");
    overlay.set_child(Some(&image));
    if let Some(text) = badge_text(count) {
        let badge = gtk::Label::new(Some(&text));
        badge.add_css_class("gl-drag-badge");
        badge.set_halign(gtk::Align::End);
        badge.set_valign(gtk::Align::End);
        overlay.add_overlay(&badge);
    }
    let size = scaled(ICON_GRID, scale).max(1);
    overlay.set_size_request(size, size);
    // A `WidgetPaintable` snapshots the widget's allocation. An icon built
    // for a drag has never been through a layout, so measure and allocate it
    // once here or the drag would show an empty 0x0 image.
    let (_, width, _, _) = overlay.measure(gtk::Orientation::Horizontal, -1);
    let (_, height, _, _) = overlay.measure(gtk::Orientation::Vertical, width);
    overlay.allocate(width.max(size), height.max(size), -1, None);
    overlay.upcast()
}

pub fn content_for(paths: &[PathBuf]) -> Option<gdk::ContentProvider> {
    if paths.is_empty() {
        return None;
    }
    let files: Vec<gio::File> = paths.iter().map(gio::File::for_path).collect();
    // One typed provider: GTK adds text/uri-list, text/plain;charset=utf-8 and
    // both portal types itself. No MIME type is added by hand (P0e).
    Some(gdk::ContentProvider::for_value(
        &gdk::FileList::from_array(&files).to_value(),
    ))
}

/// Compare `id::filesystem` for the target and the first source. Both queries
/// are asynchronous; any failure or a missing attribute means "different",
/// which copies. A query that has not finished after a short timeout also
/// copies.
pub fn same_filesystem_async(first: PathBuf, target: PathBuf, done: impl FnOnce(bool) + 'static) {
    let results: Rc<RefCell<[Option<String>; 2]>> = Rc::new(RefCell::new([None, None]));
    let count = Rc::new(Cell::new(0usize));
    let finished = Rc::new(Cell::new(false));
    let callback: Rc<RefCell<Option<BoolDone>>> = Rc::new(RefCell::new(Some(Box::new(done))));
    let finish: Rc<dyn Fn(bool)> = {
        let finished = finished.clone();
        let callback = callback.clone();
        Rc::new(move |same: bool| {
            if !finished.replace(true)
                && let Some(callback) = callback.borrow_mut().take()
            {
                callback(same);
            }
        })
    };

    for (index, path) in [first, target].into_iter().enumerate() {
        let results = results.clone();
        let count = count.clone();
        let finish = finish.clone();
        gio::File::for_path(&path).query_info_async(
            "id::filesystem",
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
            None::<&gio::Cancellable>,
            move |result| {
                let filesystem = result
                    .ok()
                    .and_then(|info| info.attribute_string("id::filesystem"))
                    .map(|value| value.to_string())
                    .filter(|value| !value.is_empty());
                results.borrow_mut()[index] = filesystem;
                let seen = count.get() + 1;
                count.set(seen);
                if seen == 2 {
                    let same = {
                        let results = results.borrow();
                        matches!(
                            (&results[0], &results[1]),
                            (Some(a), Some(b)) if a == b
                        )
                    };
                    finish(same);
                }
            },
        );
    }

    let timeout = finish.clone();
    glib::timeout_add_local_once(Duration::from_millis(FS_QUERY_TIMEOUT_MS), move || {
        timeout(false);
    });
}

fn scaled(token: f64, scale: f64) -> i32 {
    (token * scale).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(raw: &str) -> PathBuf {
        PathBuf::from(raw)
    }

    fn paths(raw: &[&str]) -> Vec<PathBuf> {
        raw.iter().map(|raw| path(raw)).collect()
    }

    #[test]
    fn same_filesystem_moves_and_another_filesystem_copies() {
        assert_eq!(action_for(true, DropModifiers::None), DropAction::Move);
        assert_eq!(action_for(false, DropModifiers::None), DropAction::Copy);
    }

    #[test]
    fn ctrl_forces_copy_and_shift_forces_move() {
        assert_eq!(action_for(true, DropModifiers::Ctrl), DropAction::Copy);
        assert_eq!(action_for(false, DropModifiers::Ctrl), DropAction::Copy);
        assert_eq!(action_for(true, DropModifiers::Shift), DropAction::Move);
        assert_eq!(action_for(false, DropModifiers::Shift), DropAction::Move);
    }

    #[test]
    fn every_folder_target_resolves_to_its_folder() {
        let folder = path("/home/kyle/Documents");
        for target in [
            Target::Grid(folder.clone()),
            Target::FolderCell(folder.clone()),
            Target::TabPill(folder.clone()),
            Target::TilePill(folder.clone()),
            Target::PinRow(folder.clone()),
            Target::Device(folder.clone()),
        ] {
            assert_eq!(
                destination(&target),
                Ok(Destination::Folder(folder.clone()))
            );
        }
        assert_eq!(destination(&Target::Trash), Ok(Destination::Trash));
        assert_eq!(destination(&Target::PinList), Ok(Destination::Pins));
    }

    #[test]
    fn a_file_cell_refuses() {
        assert_eq!(
            destination(&Target::FileCell),
            Err("only folders are drop targets".to_string())
        );
    }

    #[test]
    fn dropping_into_the_same_folder_is_refused() {
        assert!(validate_folder_drop(&path("/tmp/a"), &paths(&["/tmp/a/one.txt"])).is_err());
        assert!(
            validate_folder_drop(
                &path("/tmp/a"),
                &paths(&["/tmp/a/one.txt", "/tmp/a/two.txt"])
            )
            .is_err()
        );
    }

    #[test]
    fn a_folder_into_itself_or_a_subfolder_is_refused() {
        assert!(validate_folder_drop(&path("/tmp/a"), &paths(&["/tmp/a"])).is_err());
        assert!(validate_folder_drop(&path("/tmp/a/b"), &paths(&["/tmp/a"])).is_err());
        assert!(validate_folder_drop(&path("/tmp/b"), &paths(&["/tmp/a"])).is_ok());
    }

    #[test]
    fn a_drop_builds_the_expected_operation() {
        assert_eq!(
            operation_for(
                &Target::Grid(path("/tmp/b")),
                &paths(&["/tmp/a/one.txt"]),
                true,
                DropModifiers::None
            ),
            Ok(Operation::Move { to: path("/tmp/b") })
        );
        assert_eq!(
            operation_for(
                &Target::FolderCell(path("/tmp/b")),
                &paths(&["/tmp/a/one.txt"]),
                false,
                DropModifiers::None
            ),
            Ok(Operation::Copy { to: path("/tmp/b") })
        );
    }

    #[test]
    fn trash_and_pins_ignore_modifiers() {
        for modifiers in [
            DropModifiers::None,
            DropModifiers::Ctrl,
            DropModifiers::Shift,
        ] {
            assert_eq!(
                operation_for(&Target::Trash, &paths(&["/tmp/a/one"]), false, modifiers),
                Ok(Operation::Trash)
            );
            assert_eq!(
                operation_for(&Target::PinList, &paths(&["/tmp/a/one"]), true, modifiers),
                Ok(Operation::Pin)
            );
        }
    }

    #[test]
    fn the_badge_only_shows_for_several_items() {
        assert_eq!(badge_text(0), None);
        assert_eq!(badge_text(1), None);
        assert_eq!(badge_text(2), Some("2".to_string()));
        assert_eq!(badge_text(3), Some("3".to_string()));
    }

    #[test]
    fn only_folders_are_pinned() {
        let items = vec![
            DragItem {
                path: path("/tmp/a/folder"),
                is_dir: true,
            },
            DragItem {
                path: path("/tmp/a/file.txt"),
                is_dir: false,
            },
        ];
        assert_eq!(folder_paths(&items), vec![path("/tmp/a/folder")]);
    }
}
