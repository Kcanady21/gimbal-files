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
use crate::look::tokens::{GAP_S, ICON_GRID};

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
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Ctrl => "Ctrl",
            Self::Shift => "Shift",
        }
    }
}

/// Why a modifier state is (or is not) known. `unknown` is a distinct value,
/// never the same as `none`: Wayland can clear the compositor's key state when
/// a drag grab starts, and a missing observation must not be read as "no key
/// held" (`gdkseat-wayland.c:1452-1477`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModifierEvidence {
    modifiers: Option<DropModifiers>,
    origin: &'static str,
}

impl ModifierEvidence {
    pub fn new(modifiers: Option<DropModifiers>, origin: &'static str) -> Self {
        Self { modifiers, origin }
    }

    #[cfg(test)]
    pub fn unknown() -> Self {
        Self::new(None, "unknown")
    }

    #[cfg(test)]
    pub fn test(modifiers: Option<DropModifiers>) -> Self {
        Self::new(modifiers, "test")
    }

    pub fn modifiers(self) -> Option<DropModifiers> {
        self.modifiers
    }

    pub fn origin(self) -> &'static str {
        self.origin
    }

    pub fn text(self) -> String {
        format!("{} ({})", modifiers_text(self.modifiers), self.origin)
    }
}

/// The keys this process has actually observed. `origin` says whether that is
/// live key events, a positive compositor/gdk state captured at drag start, or
/// nothing at all. Only positive evidence is seeded at drag start, because an
/// empty Wayland seat state is not proof that no key is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TrackedKeys {
    ctrl: bool,
    shift: bool,
    origin: TrackedOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrackedOrigin {
    Unknown,
    Live,
    DragStart,
}

impl TrackedKeys {
    const UNKNOWN: Self = Self {
        ctrl: false,
        shift: false,
        origin: TrackedOrigin::Unknown,
    };

    fn from_mask(state: gdk::ModifierType) -> Self {
        Self {
            ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
            shift: state.contains(gdk::ModifierType::SHIFT_MASK),
            origin: TrackedOrigin::Live,
        }
    }

    /// A key event for one of the four modifier keys. Other keys leave the
    /// state untouched. Ctrl wins when both are held, matching `action_for`.
    fn key(self, key: gdk::Key, pressed: bool) -> Option<Self> {
        let mut next = self;
        match key {
            gdk::Key::Control_L | gdk::Key::Control_R => next.ctrl = pressed,
            gdk::Key::Shift_L | gdk::Key::Shift_R => next.shift = pressed,
            _ => return None,
        }
        next.origin = TrackedOrigin::Live;
        Some(next)
    }

    fn modifiers(self) -> Option<DropModifiers> {
        match self.origin {
            TrackedOrigin::Unknown => None,
            TrackedOrigin::Live | TrackedOrigin::DragStart if self.ctrl => {
                Some(DropModifiers::Ctrl)
            }
            TrackedOrigin::Live | TrackedOrigin::DragStart if self.shift => {
                Some(DropModifiers::Shift)
            }
            TrackedOrigin::Live | TrackedOrigin::DragStart => Some(DropModifiers::None),
        }
    }

    fn evidence(self) -> ModifierEvidence {
        let origin = match self.origin {
            TrackedOrigin::Unknown => "unknown",
            TrackedOrigin::Live => "key events",
            TrackedOrigin::DragStart => "drag start",
        };
        ModifierEvidence::new(self.modifiers(), origin)
    }

    /// The drag is about to start. A positive seat state is trustworthy and
    /// covers a key pressed before the press that started this drag. An empty
    /// seat state at a Wayland drag start is not.
    fn at_drag_start(self, seat: Option<DropModifiers>) -> Self {
        match seat {
            Some(DropModifiers::Ctrl) => Self {
                ctrl: true,
                shift: false,
                origin: TrackedOrigin::DragStart,
            },
            Some(DropModifiers::Shift) => Self {
                ctrl: false,
                shift: true,
                origin: TrackedOrigin::DragStart,
            },
            Some(DropModifiers::None) => Self {
                ctrl: false,
                shift: false,
                origin: if self.origin == TrackedOrigin::Live {
                    TrackedOrigin::Live
                } else {
                    TrackedOrigin::DragStart
                },
            },
            // An empty Wayland seat is unknown. A seed from a prepare that
            // never became a drag must not survive into the next drag.
            None => match self.origin {
                TrackedOrigin::DragStart => Self::UNKNOWN,
                TrackedOrigin::Live | TrackedOrigin::Unknown => self,
            },
        }
    }

    /// A drag-start seed belongs to that drag only. Live key observations do
    /// not become a lasting start-time key.
    fn end_drag(self) -> Self {
        match self.origin {
            TrackedOrigin::DragStart => Self::UNKNOWN,
            TrackedOrigin::Live | TrackedOrigin::Unknown => self,
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

fn offered(actions: gdk::DragAction, action: DropAction) -> bool {
    match action {
        DropAction::Copy => actions.contains(gdk::DragAction::COPY),
        DropAction::Move => actions.contains(gdk::DragAction::MOVE),
    }
}

/// Where a target can learn a drag's capability. Our own drag exposes the
/// source's full offer through `GdkDrag::actions`. An external offer has no
/// `GdkDrag`; Wayland narrows `GdkDrop::actions` to the selected action once
/// the compositor has one (`gdkdrop-wayland.c:286-326`), so that view must be
/// re-read for every event and never cached as an immutable source mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    Local(gdk::DragAction),
    External,
}

fn supported_action(actions: gdk::DragAction, want: DropAction) -> Option<DropAction> {
    if offered(actions, want) {
        return Some(want);
    }
    if offered(actions, DropAction::Copy) {
        return Some(DropAction::Copy);
    }
    if offered(actions, DropAction::Move) {
        return Some(DropAction::Move);
    }
    None
}

impl Capability {
    pub fn allows(self, current: gdk::DragAction, action: DropAction) -> bool {
        match self {
            Self::Local(mask) => offered(mask, action),
            Self::External => offered(current, action),
        }
    }

    /// The exact action when this capability offers it. A local drag uses its
    /// full source mask; an external offer only knows the live view.
    pub fn exact(self, current: gdk::DragAction, action: DropAction) -> Option<DropAction> {
        self.allows(current, action).then_some(action)
    }

    /// What to ask the compositor to prefer. A local source's full mask clamps
    /// this. An external `current` view may already be narrowed to the selected
    /// action, so it is only a hint: asking for the filesystem default here
    /// does not claim that the source supports it, and the final action is
    /// still validated against the live view when the drop arrives.
    fn preference(self, want: DropAction) -> DropAction {
        match self {
            Self::Local(mask) => supported_action(mask, want).unwrap_or(want),
            Self::External => want,
        }
    }

    /// The log spelling. An external view may be narrowed, so it is `unknown`
    /// capability evidence even when the current view is a singleton.
    pub fn text(self, current: gdk::DragAction) -> String {
        match self {
            Self::Local(mask) => format!("local {}", actions_text(mask)),
            Self::External => format!("external unknown (view {})", actions_text(current)),
        }
    }
}

/// A unique action from a mask. `COPY | MOVE` is the source's offer, not a
/// choice, so it is ambiguous.
pub fn unique_action(actions: gdk::DragAction) -> Option<DropAction> {
    match (
        actions.contains(gdk::DragAction::COPY),
        actions.contains(gdk::DragAction::MOVE),
    ) {
        (true, false) => Some(DropAction::Copy),
        (false, true) => Some(DropAction::Move),
        _ => None,
    }
}

/// The compositor's selected action. The target-side offer is usually the
/// freshest view: GDK collapses `Drop::actions()` to the selected action once
/// the offer reports one. The local `GdkDrag` is the fallback (and the only
/// view when a platform keeps the offer mask broad).
pub fn negotiated_action(
    local: Option<DropAction>,
    drop_actions: gdk::DragAction,
) -> Option<DropAction> {
    unique_action(drop_actions).or(local)
}

/// One target's live negotiation. `selected` is the compositor's negotiated
/// action and `preference` is the filesystem default requested by this target.
/// Modifier overrides belong to the compositor, not the target preference. A drop
/// uses `selected` as the final operation; it is never recalculated from a
/// later key or filesystem read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Negotiation {
    capability: Capability,
    current: gdk::DragAction,
    selected: Option<DropAction>,
    preference: Option<DropAction>,
    filesystem: Option<bool>,
    modifiers: Option<DropModifiers>,
    /// The last action this target asked for because it observed Ctrl/Shift.
    /// It lets a release undo our own stale echo without turning the target
    /// preference into a lasting override.
    override_action: Option<DropAction>,
}

impl Negotiation {
    /// Begin hovering. `current` is `GdkDrop::actions` at enter; for an
    /// external offer it may already be narrowed. `local_selected` is
    /// `GdkDrag::selected_action` and is `None` for an external offer.
    /// `modifiers` is observed key evidence, not an assumed value.
    pub fn enter(
        capability: Capability,
        current: gdk::DragAction,
        local_selected: Option<DropAction>,
        modifiers: Option<DropModifiers>,
    ) -> Self {
        // Until the filesystem default is known, ask for Copy. Copy is the
        // safe action: it cannot turn into an unexpected move.
        let provisional = capability.preference(DropAction::Copy);
        let mut negotiation = Self {
            capability,
            current,
            selected: None,
            preference: Some(provisional),
            filesystem: None,
            modifiers,
            override_action: None,
        };
        negotiation.note_modifiers(modifiers);
        negotiation.observe(local_selected, current, modifiers);
        negotiation
    }

    /// One motion, negotiated-action event or observed key change. The
    /// returned action is the target's next preference. Keep asking for the
    /// filesystem default when no key is observed so releasing a key restores
    /// it. The selected action still determines the operation when the
    /// platform applies a modifier itself.
    pub fn observe(
        &mut self,
        local_selected: Option<DropAction>,
        current: gdk::DragAction,
        modifiers: Option<DropModifiers>,
    ) -> DropAction {
        self.current = current;
        self.modifiers = modifiers;
        self.note_modifiers(modifiers);
        let selected = negotiated_action(local_selected, current)
            .filter(|action| self.capability.allows(current, *action));
        self.selected = selected;
        self.preference = Some(self.requested());
        self.preferred()
    }

    fn note_modifiers(&mut self, modifiers: Option<DropModifiers>) {
        match modifiers {
            Some(DropModifiers::Ctrl) => self.override_action = Some(DropAction::Copy),
            Some(DropModifiers::Shift) => self.override_action = Some(DropAction::Move),
            // None (trustworthy) and unknown both leave the last override in
            // place until the compositor's echo changes. The preference itself
            // is always recomputed below.
            Some(DropModifiers::None) | None => {}
        }
    }

    /// What this target asks the compositor to prefer. Observed Ctrl/Shift
    /// wins; otherwise the filesystem default (or the safe provisional Copy
    /// before it is known). Unknown evidence is not treated as "no key held";
    /// it simply cannot supply an override.
    fn requested(&self) -> DropAction {
        let default = self.filesystem.map_or(DropAction::Copy, |same| {
            action_for(same, DropModifiers::None)
        });
        let want = match self.modifiers {
            Some(DropModifiers::Ctrl) => DropAction::Copy,
            Some(DropModifiers::Shift) => DropAction::Move,
            Some(DropModifiers::None) | None => default,
        };
        self.capability.preference(want)
    }

    /// The asynchronous filesystem result. It refreshes the target's default
    /// preference even if the pointer is stationary; the caller must send the
    /// returned preference to `GdkDrop::status`. A selected action captured
    /// before this result remains the final action until the compositor
    /// answers the new preference, unless an observed key overrides it.
    pub fn filesystem_result(
        &mut self,
        same: bool,
        modifiers: Option<DropModifiers>,
    ) -> DropAction {
        self.filesystem = Some(same);
        self.modifiers = modifiers;
        self.note_modifiers(modifiers);
        self.preference = Some(self.requested());
        self.preferred()
    }

    pub fn preferred(&self) -> DropAction {
        let want = self.preference.unwrap_or(DropAction::Copy);
        self.capability.preference(want)
    }

    /// Freeze the action at drop. Observed Ctrl/Shift is authoritative and is
    /// refused when the drag does not offer it. Trustworthy "no key" can undo
    /// a stale echo of our own override. Unknown evidence falls back to the
    /// compositor's selected action, and Copy remains the safe supported
    /// fallback when nothing has been selected.
    pub fn final_action_for(&self, evidence: ModifierEvidence) -> Result<DropAction, &'static str> {
        match evidence.modifiers() {
            Some(DropModifiers::Ctrl) => {
                self.exact(DropAction::Copy, "this drag does not offer Copy")
            }
            Some(DropModifiers::Shift) => {
                self.exact(DropAction::Move, "this drag does not offer Move")
            }
            Some(DropModifiers::None) => {
                if let (Some(_), Some(same)) = (self.override_action, self.filesystem) {
                    let default = action_for(same, DropModifiers::None);
                    if let Some(action) = self.capability.exact(self.current, default) {
                        return Ok(action);
                    }
                }
                self.final_action()
            }
            None => self.final_action(),
        }
    }

    /// Freeze the action at drop from the compositor's selected action. The
    /// selected action is used when it is still supported. When no action has
    /// been selected, Copy is the safe supported fallback; a move-only offer
    /// with no selected action is refused rather than silently moved.
    pub fn final_action(&self) -> Result<DropAction, &'static str> {
        if let Some(action) = self.selected
            && self.capability.allows(self.current, action)
        {
            return Ok(action);
        }
        if self.capability.allows(self.current, DropAction::Copy) {
            return Ok(DropAction::Copy);
        }
        Err("drag action is not settled")
    }

    fn exact(&self, want: DropAction, error: &'static str) -> Result<DropAction, &'static str> {
        self.capability.exact(self.current, want).ok_or(error)
    }

    pub fn capability(self) -> Capability {
        self.capability
    }

    pub fn current(self) -> gdk::DragAction {
        self.current
    }

    pub fn selected(self) -> Option<DropAction> {
        self.selected
    }

    pub fn preference(self) -> Option<DropAction> {
        self.preference
    }

    pub fn filesystem(self) -> Option<bool> {
        self.filesystem
    }
}

/// The decision captured synchronously at `drop`, after the last negotiation
/// event and before any worker job. `Process::drop_on_folder` uses `action`;
/// it never runs a second filesystem query that could reinterpret the drop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropIntent {
    action: DropAction,
    capability: Capability,
    current: gdk::DragAction,
    selected: Option<DropAction>,
    requested: Option<DropAction>,
    filesystem: Option<bool>,
    modifiers: Option<DropModifiers>,
    modifier_origin: &'static str,
}

impl DropIntent {
    pub fn capture(
        negotiation: &Negotiation,
        evidence: ModifierEvidence,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            action: negotiation.final_action_for(evidence)?,
            capability: negotiation.capability,
            current: negotiation.current,
            selected: negotiation.selected,
            requested: negotiation.preference,
            filesystem: negotiation.filesystem,
            modifiers: evidence.modifiers(),
            modifier_origin: evidence.origin(),
        })
    }

    pub fn action(self) -> DropAction {
        self.action
    }

    pub fn source_text(self) -> String {
        self.capability.text(self.current)
    }

    pub fn selected(self) -> Option<DropAction> {
        self.selected
    }

    pub fn requested(self) -> Option<DropAction> {
        self.requested
    }

    pub fn filesystem(self) -> Option<bool> {
        self.filesystem
    }

    pub fn modifiers_text(self) -> String {
        ModifierEvidence::new(self.modifiers, self.modifier_origin).text()
    }
}

/// The log spellings: an empty mask or a missing value is `unknown`, never a
/// claim that no key was held.
pub fn actions_text(actions: gdk::DragAction) -> &'static str {
    match (
        actions.contains(gdk::DragAction::COPY),
        actions.contains(gdk::DragAction::MOVE),
    ) {
        (true, true) => "COPY|MOVE",
        (true, false) => "COPY",
        (false, true) => "MOVE",
        _ => "unknown",
    }
}

pub fn action_text(action: Option<DropAction>) -> &'static str {
    match action {
        Some(DropAction::Copy) => "Copy",
        Some(DropAction::Move) => "Move",
        None => "unknown",
    }
}

pub fn filesystem_text(same: Option<bool>) -> &'static str {
    match same {
        Some(true) => "same",
        Some(false) => "different",
        None => "unknown",
    }
}

pub fn modifiers_text(modifiers: Option<DropModifiers>) -> &'static str {
    modifiers.map(DropModifiers::as_str).unwrap_or("unknown")
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

pub fn operation_for_action(
    target: &Target,
    paths: &[PathBuf],
    action: DropAction,
) -> Result<Operation, String> {
    match destination(target)? {
        Destination::Folder(folder) => {
            validate_folder_drop(&folder, paths)?;
            Ok(match action {
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

/// A cell's rectangle in the folder viewport's coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl DropRect {
    pub fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// The viewport background owns a point unless it is inside a cell. The
/// padding around cells and the blank area below a short or empty listing are
/// therefore all background.
pub fn background_owns(x: f64, y: f64, cells: &[DropRect]) -> bool {
    !cells.iter().any(|cell| cell.contains(x, y))
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
    /// The modifier keys this process has observed. GTK's Wayland seat can
    /// clear its modifier mask when the DnD grab starts, so this state comes
    /// from key and modifier events on the window, plus a positive seat reading
    /// at drag start.
    static TRACKED_KEYS: Cell<TrackedKeys> = const { Cell::new(TrackedKeys::UNKNOWN) };
    /// One id per drop-target widget, for logs that distinguish target
    /// crossings from negotiation resets.
    static NEXT_TARGET_ID: Cell<u64> = const { Cell::new(1) };
    /// One id per currently hovered target that should react to key changes.
    static NEXT_ACTIVE_ID: Cell<u64> = const { Cell::new(1) };
    static ACTIVE_TARGETS: RefCell<Vec<(u64, ActiveTarget)>> = const { RefCell::new(Vec::new()) };
}

struct ActiveTarget {
    target_id: u64,
    target: glib::WeakRef<gtk::DropTarget>,
    negotiation: Rc<Cell<Option<Negotiation>>>,
    last_log: Rc<Cell<Option<HoverKey>>>,
}

fn next_target_id() -> u64 {
    NEXT_TARGET_ID.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    })
}

fn register_active(
    target_id: u64,
    target: &gtk::DropTarget,
    negotiation: &Rc<Cell<Option<Negotiation>>>,
    last_log: &Rc<Cell<Option<HoverKey>>>,
) -> u64 {
    let id = NEXT_ACTIVE_ID.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    });
    ACTIVE_TARGETS.with(|active| {
        active.borrow_mut().push((
            id,
            ActiveTarget {
                target_id,
                target: target.downgrade(),
                negotiation: negotiation.clone(),
                last_log: last_log.clone(),
            },
        ));
    });
    id
}

fn unregister_active(slot: &Cell<Option<u64>>) {
    let Some(id) = slot.take() else {
        return;
    };
    ACTIVE_TARGETS.with(|active| active.borrow_mut().retain(|(key, _)| *key != id));
}

/// A positive seat state only. An empty Wayland state is unknown because the
/// compositor may have taken the keyboard for the drag
/// (`gdkseat-wayland.c:1452-1477`).
fn seat_modifiers() -> ModifierEvidence {
    let state = gdk::Display::default()
        .and_then(|display| display.default_seat())
        .and_then(|seat| seat.keyboard())
        .map(|keyboard| keyboard.modifier_state())
        .unwrap_or_else(gdk::ModifierType::empty);
    if state.contains(gdk::ModifierType::CONTROL_MASK) {
        ModifierEvidence::new(Some(DropModifiers::Ctrl), "seat")
    } else if state.contains(gdk::ModifierType::SHIFT_MASK) {
        ModifierEvidence::new(Some(DropModifiers::Shift), "seat")
    } else if gdk::Display::default()
        .is_some_and(|display| display.is::<gdk4_wayland::WaylandDisplay>())
    {
        ModifierEvidence::new(None, "seat")
    } else {
        ModifierEvidence::new(Some(DropModifiers::None), "seat")
    }
}

/// The log spelling of the current modifier evidence, for the source's
/// drag-begin/drag-end lines. It prefers live key tracking and falls back to
/// the same positive-seat rule used at `prepare`.
pub fn current_modifiers_text() -> String {
    let tracked = TRACKED_KEYS.with(Cell::get);
    if tracked.modifiers().is_some() {
        tracked.evidence().text()
    } else {
        seat_modifiers().text()
    }
}

/// Called from the drag source's `prepare`, before GDK starts the drag. A
/// key already held is positively reported by the seat here; a positive value
/// becomes this drag's start evidence until live key events replace it.
pub fn prepare_drag() {
    let seat = seat_modifiers();
    let tracked = TRACKED_KEYS.with(|cell| {
        let next = cell.get().at_drag_start(seat.modifiers());
        cell.set(next);
        next
    });
    crate::log!(
        "dnd",
        "prepare modifiers: seat {} -> {}",
        seat.text(),
        tracked.evidence().text()
    );
}

/// Install the window-level key tracker. The controller is on the window, not
/// a cell: GTK resets the drag source widget's controllers when a drag begins
/// (`gtkdragsource.c:610-612`), and the window keeps keyboard focus while a
/// pointer drag is active.
pub fn install_modifier_tracking(window: &gtk::Window) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(|_, key, _, _| {
        note_key(key, true);
        glib::Propagation::Proceed
    });
    keys.connect_key_released(|_, key, _, _| {
        note_key(key, false);
    });
    keys.connect_modifiers(|_, state| {
        note_modifier_mask(state);
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    window.connect_is_active_notify(|window| {
        if window.is_active() {
            let seat = seat_modifiers();
            let tracked = TRACKED_KEYS.with(|cell| {
                let next = cell.get().at_drag_start(seat.modifiers());
                cell.set(next);
                next
            });
            crate::log!(
                "dnd",
                "window active: seat {} -> {}",
                seat.text(),
                tracked.evidence().text()
            );
        } else {
            TRACKED_KEYS.with(|cell| cell.set(TrackedKeys::UNKNOWN));
            crate::log!("dnd", "window inactive: modifiers unknown");
        }
    });
}

fn note_key(key: gdk::Key, pressed: bool) {
    let previous = TRACKED_KEYS.with(Cell::get);
    let Some(next) = previous.key(key, pressed) else {
        return;
    };
    TRACKED_KEYS.with(|cell| cell.set(next));
    let evidence = next.evidence();
    crate::log!(
        "dnd",
        "key {} {}: {} -> {}",
        key.name().unwrap_or_default(),
        if pressed { "down" } else { "up" },
        previous.evidence().text(),
        evidence.text()
    );
    if next != previous {
        refresh_active_targets(evidence);
    }
}

fn note_modifier_mask(state: gdk::ModifierType) {
    let previous = TRACKED_KEYS.with(Cell::get);
    let next = TrackedKeys::from_mask(state);
    if next == previous {
        return;
    }
    TRACKED_KEYS.with(|cell| cell.set(next));
    let evidence = next.evidence();
    crate::log!(
        "dnd",
        "modifier event: {} -> {}",
        previous.evidence().text(),
        evidence.text()
    );
    refresh_active_targets(evidence);
}

/// Push a key change to every target that is currently hosting this drag, even
/// if the pointer is stationary. `GdkDrop::status` is the protocol operation
/// that asks the compositor to prefer the new action; a motion event is not
/// guaranteed after a key press or release.
fn refresh_active_targets(evidence: ModifierEvidence) {
    let targets: Vec<_> = ACTIVE_TARGETS.with(|active| {
        active
            .borrow()
            .iter()
            .map(|(_, target)| {
                (
                    target.target_id,
                    target.target.clone(),
                    target.negotiation.clone(),
                    target.last_log.clone(),
                )
            })
            .collect()
    });
    for (target_id, weak, negotiation, last_log) in targets {
        let Some(target) = weak.upgrade() else {
            continue;
        };
        let Some(mut negotiation_now) = negotiation.get() else {
            continue;
        };
        if target.current_drop().is_none() {
            continue;
        }
        let preferred = negotiation_now.observe(
            local_selected(&target),
            current_actions(&target),
            evidence.modifiers(),
        );
        negotiation.set(Some(negotiation_now));
        if let Some(drop) = target.current_drop() {
            drop.status(target.actions(), gdk_action(preferred));
        }
        log_hover(
            "key change",
            target_id,
            negotiation_now.capability(),
            negotiation_now.current(),
            negotiation_now.selected(),
            negotiation_now.preference(),
            negotiation_now.filesystem(),
            evidence,
            &last_log,
        );
    }
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
    TRACKED_KEYS.with(|cell| cell.set(cell.get().end_drag()));
    ACTIVE_TARGETS.with(|active| active.borrow_mut().clear());
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

/// The live view of `GdkDrop::actions`. For an external offer Wayland may
/// already have narrowed this to the selected action; it is re-read on every
/// event and never cached as a source mask.
fn current_actions(target: &gtk::DropTarget) -> gdk::DragAction {
    target
        .current_drop()
        .map(|drop| drop.actions())
        .unwrap_or_else(gdk::DragAction::empty)
}

/// `GdkDrag::selected_action` for our own drag, `None` for an external offer.
fn local_selected(target: &gtk::DropTarget) -> Option<DropAction> {
    target
        .current_drop()
        .and_then(|drop| drop.drag())
        .and_then(|drag| unique_action(drag.selected_action()))
}

/// A local drag has the source's full offer through `GdkDrag::actions`. An
/// external offer has no such object, so only the live `GdkDrop::actions` view
/// is known.
fn capability_for(target: &gtk::DropTarget) -> Capability {
    target
        .current_drop()
        .and_then(|drop| drop.drag())
        .map(|drag| drag.actions())
        .filter(|actions| !actions.is_empty())
        .map(Capability::Local)
        .unwrap_or(Capability::External)
}

/// The compositor's selected action. For a local drag that is
/// `GdkDrag::selected_action` (`wl_data_source.action`); an external offer
/// has no `GdkDrag`, so the narrowed `GdkDrop::actions` is the only view.
fn selected_action(target: &gtk::DropTarget) -> Option<DropAction> {
    let drop = target.current_drop()?;
    negotiated_action(local_selected(target), drop.actions())
}

/// The modifier evidence for this event. Key and modifier events on the
/// window are the primary source: GTK resets the drag source's controllers at
/// drag begin, and its Wayland seat can clear the modifier mask when the DnD
/// grab starts (`gdkseat-wayland.c:1452-1477`). A positive state on the drag
/// device is a fallback; an empty Wayland state stays `unknown`.
fn modifier_observation(target: &gtk::DropTarget) -> ModifierEvidence {
    let tracked = TRACKED_KEYS.with(Cell::get);
    if tracked.modifiers().is_some() {
        return tracked.evidence();
    }
    let state = target
        .current_drop()
        .and_then(|drop| drop.drag())
        .map(|drag| drag.device().modifier_state())
        .unwrap_or_else(gdk::ModifierType::empty);
    if state.contains(gdk::ModifierType::CONTROL_MASK) {
        ModifierEvidence::new(Some(DropModifiers::Ctrl), "drag device")
    } else if state.contains(gdk::ModifierType::SHIFT_MASK) {
        ModifierEvidence::new(Some(DropModifiers::Shift), "drag device")
    } else if gdk::Display::default()
        .is_some_and(|display| display.is::<gdk4_wayland::WaylandDisplay>())
    {
        // Wayland clears the seat's keys when the DnD grab takes the
        // keyboard, so an empty state is not evidence that nothing is held.
        ModifierEvidence::new(None, "unknown")
    } else {
        ModifierEvidence::new(Some(DropModifiers::None), "drag device")
    }
}

fn gdk_action(action: DropAction) -> gdk::DragAction {
    match action {
        DropAction::Copy => gdk::DragAction::COPY,
        DropAction::Move => gdk::DragAction::MOVE,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HoverKey {
    capability: Capability,
    current: gdk::DragAction,
    selected: Option<DropAction>,
    requested: Option<DropAction>,
    filesystem: Option<bool>,
    modifiers: Option<DropModifiers>,
    modifier_origin: &'static str,
}

#[allow(clippy::too_many_arguments)]
fn log_hover(
    what: &str,
    target_id: u64,
    capability: Capability,
    current: gdk::DragAction,
    selected: Option<DropAction>,
    requested: Option<DropAction>,
    filesystem: Option<bool>,
    evidence: ModifierEvidence,
    last: &Cell<Option<HoverKey>>,
) {
    let key = HoverKey {
        capability,
        current,
        selected,
        requested,
        filesystem,
        modifiers: evidence.modifiers(),
        modifier_origin: evidence.origin(),
    };
    if last.get() == Some(key) {
        return;
    }
    last.set(Some(key));
    crate::log!(
        "dnd",
        "hover {what} target #{target_id}: source {}, selected {}, target prefers {}, filesystem {}, modifiers {}",
        capability.text(current),
        action_text(selected),
        action_text(requested),
        filesystem_text(filesystem),
        evidence.text()
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

/// Send the filesystem answer back to the compositor, even when the pointer is
/// stationary. `GdkDrop::status` is the public GDK operation that becomes
/// `wl_data_offer.set_actions` + `accept` on Wayland
/// (`gdkdrop-wayland.c:122-144`); calling the GTK signal alone would not send
/// protocol status. GDK documents calling it again once the destination knows
/// its exact actions (`gdkdrop.c:552-571`).
fn refresh_preference(
    target: &gtk::DropTarget,
    target_id: u64,
    negotiation: &Cell<Option<Negotiation>>,
    same_filesystem: bool,
    evidence: ModifierEvidence,
) -> Option<Negotiation> {
    let mut negotiation_now = negotiation.get()?;
    let preference = negotiation_now.filesystem_result(same_filesystem, evidence.modifiers());
    negotiation.set(Some(negotiation_now));
    if let Some(drop) = target.current_drop() {
        drop.status(target.actions(), gdk_action(preference));
    }
    crate::log!(
        "dnd",
        "filesystem target #{target_id} {}: status prefers {}, modifiers {}",
        filesystem_text(Some(same_filesystem)),
        action_text(Some(preference)),
        evidence.text()
    );
    Some(negotiation_now)
}

/// Start one hover filesystem query for the current ownership generation. The
/// result refreshes this target's preference and sends protocol status; a
/// result from an old generation is ignored.
#[allow(clippy::too_many_arguments)]
fn query_same_filesystem(
    target: &gtk::DropTarget,
    target_id: u64,
    folder: &Rc<dyn Fn() -> Option<PathBuf>>,
    entered: &Rc<Cell<bool>>,
    generation: &Rc<Cell<u64>>,
    queried: &Rc<Cell<Option<u64>>>,
    same_filesystem: &Rc<Cell<Option<bool>>>,
    negotiation: &Rc<Cell<Option<Negotiation>>>,
    last_log: &Rc<Cell<Option<HoverKey>>>,
) {
    if same_filesystem.get().is_some() {
        return;
    }
    let mine = generation.get();
    if queried.get() == Some(mine) {
        return;
    }
    let Some(first) = target_paths(target).and_then(|paths| paths.into_iter().next()) else {
        return;
    };
    let Some(folder) = folder() else {
        return;
    };
    queried.set(Some(mine));
    let target = target.clone();
    let entered = entered.clone();
    let generation = generation.clone();
    let same_filesystem = same_filesystem.clone();
    let negotiation = negotiation.clone();
    let last_log = last_log.clone();
    same_filesystem_async(first, folder, move |same| {
        if !entered.get() || generation.get() != mine {
            return;
        }
        same_filesystem.set(Some(same));
        let evidence = modifier_observation(&target);
        if let Some(negotiation) =
            refresh_preference(&target, target_id, &negotiation, same, evidence)
        {
            log_hover(
                "folder target",
                target_id,
                negotiation.capability(),
                negotiation.current(),
                negotiation.selected(),
                negotiation.preference(),
                negotiation.filesystem(),
                evidence,
                &last_log,
            );
        }
    });
}

/// A target that drops into a folder: a viewport's empty space, a folder cell,
/// a pill, a pinned row or a device row. `mask` lets a parent target bow out
/// while the pointer is over a child target (the viewport over its cells).
pub fn attach_folder_target(
    widget: &impl IsA<gtk::Widget>,
    process: Weak<Process>,
    window: WindowId,
    folder: Rc<dyn Fn() -> Option<PathBuf>>,
    none_reason: Option<&'static str>,
    mask: Option<Rc<dyn Fn(f64, f64) -> bool>>,
) {
    let target = make_target();
    let target_id = next_target_id();
    let widget: gtk::Widget = widget.clone().upcast();
    let entered = Rc::new(Cell::new(false));
    let last_log = Rc::new(Cell::new(None));
    let last_pos = Rc::new(Cell::new((0.0, 0.0)));
    let same_filesystem = Rc::new(Cell::new(None));
    let generation = Rc::new(Cell::new(0u64));
    let queried = Rc::new(Cell::new(None::<u64>));
    let negotiation = Rc::new(Cell::new(None::<Negotiation>));
    let active_id = Rc::new(Cell::new(None::<u64>));

    {
        let widget = widget.clone();
        let folder = folder.clone();
        let entered = entered.clone();
        let last_log = last_log.clone();
        let last_pos = last_pos.clone();
        let same_filesystem = same_filesystem.clone();
        let generation = generation.clone();
        let queried = queried.clone();
        let negotiation = negotiation.clone();
        let active_id = active_id.clone();
        let mask = mask.clone();
        target.connect_enter(move |target, x, y| {
            drop_hover_enter();
            entered.set(true);
            last_pos.set((x, y));
            same_filesystem.set(None);
            generation.set(generation.get() + 1);
            queried.set(None);
            negotiation.set(None);
            unregister_active(&active_id);
            let masked = mask.as_ref().is_some_and(|mask| !mask(x, y));
            refresh_folder_highlight(target, &widget, &folder, mask.as_ref(), (x, y));
            if masked {
                // The child target under the pointer owns this negotiation.
                return gdk::DragAction::empty();
            }
            let evidence = modifier_observation(target);
            let negotiation_now = Negotiation::enter(
                capability_for(target),
                current_actions(target),
                local_selected(target),
                evidence.modifiers(),
            );
            let preferred = negotiation_now.preferred();
            negotiation.set(Some(negotiation_now));
            active_id.set(Some(register_active(
                target_id,
                target,
                &negotiation,
                &last_log,
            )));
            log_hover(
                "folder target",
                target_id,
                negotiation_now.capability(),
                negotiation_now.current(),
                negotiation_now.selected(),
                negotiation_now.preference(),
                negotiation_now.filesystem(),
                evidence,
                &last_log,
            );
            query_same_filesystem(
                target,
                target_id,
                &folder,
                &entered,
                &generation,
                &queried,
                &same_filesystem,
                &negotiation,
                &last_log,
            );
            gdk_action(preferred)
        });
    }
    {
        let widget = widget.clone();
        let folder = folder.clone();
        let entered = entered.clone();
        let last_log = last_log.clone();
        let last_pos = last_pos.clone();
        let same_filesystem = same_filesystem.clone();
        let generation = generation.clone();
        let queried = queried.clone();
        let negotiation = negotiation.clone();
        let active_id = active_id.clone();
        let mask = mask.clone();
        target.connect_motion(move |target, x, y| {
            last_pos.set((x, y));
            let masked = mask.as_ref().is_some_and(|mask| !mask(x, y));
            refresh_folder_highlight(target, &widget, &folder, mask.as_ref(), (x, y));
            if masked {
                // Stop treating another target's response as our own choice.
                negotiation.set(None);
                unregister_active(&active_id);
                same_filesystem.set(None);
                generation.set(generation.get() + 1);
                queried.set(None);
                return gdk::DragAction::empty();
            }
            let evidence = modifier_observation(target);
            let mut negotiation_now = match negotiation.get() {
                Some(negotiation_now) => negotiation_now,
                None => {
                    // This target became the owner without an unmasked enter.
                    same_filesystem.set(None);
                    generation.set(generation.get() + 1);
                    queried.set(None);
                    Negotiation::enter(
                        capability_for(target),
                        current_actions(target),
                        local_selected(target),
                        evidence.modifiers(),
                    )
                }
            };
            let preferred = negotiation_now.observe(
                local_selected(target),
                current_actions(target),
                evidence.modifiers(),
            );
            negotiation.set(Some(negotiation_now));
            if active_id.get().is_none() {
                active_id.set(Some(register_active(
                    target_id,
                    target,
                    &negotiation,
                    &last_log,
                )));
            }
            log_hover(
                "folder target",
                target_id,
                negotiation_now.capability(),
                negotiation_now.current(),
                negotiation_now.selected(),
                negotiation_now.preference(),
                negotiation_now.filesystem(),
                evidence,
                &last_log,
            );
            query_same_filesystem(
                target,
                target_id,
                &folder,
                &entered,
                &generation,
                &queried,
                &same_filesystem,
                &negotiation,
                &last_log,
            );
            gdk_action(preferred)
        });
    }
    {
        let widget = widget.clone();
        let folder = folder.clone();
        let entered = entered.clone();
        let last_pos = last_pos.clone();
        let same_filesystem = same_filesystem.clone();
        let generation = generation.clone();
        let queried = queried.clone();
        let negotiation = negotiation.clone();
        let mask = mask.clone();
        target.connect_value_notify(move |target| {
            if entered.get() && target.current_drop().is_some() {
                let at = last_pos.get();
                refresh_folder_highlight(target, &widget, &folder, mask.as_ref(), at);
                if mask.as_ref().is_none_or(|mask| mask(at.0, at.1)) {
                    query_same_filesystem(
                        target,
                        target_id,
                        &folder,
                        &entered,
                        &generation,
                        &queried,
                        &same_filesystem,
                        &negotiation,
                        &last_log,
                    );
                }
            }
        });
    }
    {
        let widget = widget.clone();
        let entered = entered.clone();
        let generation = generation.clone();
        let queried = queried.clone();
        let negotiation = negotiation.clone();
        let active_id = active_id.clone();
        target.connect_leave(move |_| {
            drop_hover_leave();
            entered.set(false);
            generation.set(generation.get() + 1);
            queried.set(None);
            negotiation.set(None);
            unregister_active(&active_id);
            set_highlight(&widget, false);
        });
    }
    {
        // A cancelled drag can end without a separate leave on every
        // compositor. When the target loses its drop, clear the outline too.
        let widget = widget.clone();
        let entered = entered.clone();
        let generation = generation.clone();
        let queried = queried.clone();
        let negotiation = negotiation.clone();
        let active_id = active_id.clone();
        target.connect_current_drop_notify(move |target| {
            if target.current_drop().is_none() {
                entered.set(false);
                generation.set(generation.get() + 1);
                queried.set(None);
                negotiation.set(None);
                unregister_active(&active_id);
                set_highlight(&widget, false);
            }
        });
    }
    {
        let process = process.clone();
        let mask = mask.clone();
        let negotiation = negotiation.clone();
        let active_id = active_id.clone();
        let drop_widget = widget.clone();
        target.connect_drop(move |target, value, x, y| {
            set_highlight(&drop_widget, false);
            unregister_active(&active_id);
            if mask.as_ref().is_some_and(|mask| !mask(x, y)) {
                return false;
            }
            let paths = paths_from_value(value);
            if paths.is_empty() {
                return false;
            }
            let evidence = modifier_observation(target);
            let mut negotiation_now = match negotiation.get() {
                Some(negotiation_now) => negotiation_now,
                None => Negotiation::enter(
                    capability_for(target),
                    current_actions(target),
                    local_selected(target),
                    evidence.modifiers(),
                ),
            };
            negotiation_now.observe(
                local_selected(target),
                current_actions(target),
                evidence.modifiers(),
            );
            negotiation.set(Some(negotiation_now));
            crate::log!(
                "dnd",
                "drop event target #{target_id}: live {}, selected {}, target prefers {}, filesystem {}, modifiers {}",
                action_text(selected_action(target)),
                action_text(negotiation_now.selected()),
                action_text(negotiation_now.preference()),
                filesystem_text(negotiation_now.filesystem()),
                evidence.text()
            );
            let intent = match DropIntent::capture(&negotiation_now, evidence) {
                Ok(intent) => intent,
                Err(reason) => {
                    if let Some(process) = process.upgrade() {
                        process.show_drop_reason(window, reason);
                    }
                    return false;
                }
            };
            match folder() {
                Some(folder) => {
                    if let Err(reason) = validate_folder_drop(&folder, &paths) {
                        if let Some(process) = process.upgrade() {
                            process.show_drop_reason(window, &reason);
                        }
                        return false;
                    }
                    if let Some(process) = process.upgrade() {
                        process.drop_on_folder(window, folder, paths, intent);
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
    let target_id = next_target_id();
    let widget: gtk::Widget = widget.clone().upcast();
    let entered = Rc::new(Cell::new(false));
    let last_log = Rc::new(Cell::new(None));
    {
        let widget = widget.clone();
        let entered = entered.clone();
        let last_log = last_log.clone();
        target.connect_enter(move |target, _, _| {
            drop_hover_enter();
            entered.set(true);
            set_highlight(&widget, target_paths(target).is_some());
            log_hover(
                "Trash",
                target_id,
                capability_for(target),
                current_actions(target),
                selected_action(target),
                Some(DropAction::Copy),
                None,
                modifier_observation(target),
                &last_log,
            );
            gdk::DragAction::COPY
        });
    }
    {
        let widget = widget.clone();
        let last_log = last_log.clone();
        target.connect_motion(move |target, _, _| {
            set_highlight(&widget, target_paths(target).is_some());
            log_hover(
                "Trash",
                target_id,
                capability_for(target),
                current_actions(target),
                selected_action(target),
                Some(DropAction::Copy),
                None,
                modifier_observation(target),
                &last_log,
            );
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
    let target_id = next_target_id();
    let widget: gtk::Widget = widget.clone().upcast();
    let entered = Rc::new(Cell::new(false));
    let generation = Rc::new(Cell::new(0u64));
    let checking = Rc::new(Cell::new(false));
    let last_log = Rc::new(Cell::new(None));

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
        let last_log = last_log.clone();
        let entered = entered.clone();
        target.connect_enter(move |target, _, _| {
            drop_hover_enter();
            entered.set(true);
            refresh(target);
            log_hover(
                "pins",
                target_id,
                capability_for(target),
                current_actions(target),
                selected_action(target),
                Some(DropAction::Copy),
                None,
                modifier_observation(target),
                &last_log,
            );
            gdk::DragAction::COPY
        });
    }
    {
        let refresh = refresh.clone();
        let last_log = last_log.clone();
        target.connect_motion(move |target, _, _| {
            refresh(target);
            log_hover(
                "pins",
                target_id,
                capability_for(target),
                current_actions(target),
                selected_action(target),
                Some(DropAction::Copy),
                None,
                modifier_observation(target),
                &last_log,
            );
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

/// The drag icon: the first item's icon, with a small count badge. GTK's
/// `GtkDragIcon` hosts and lays this widget out during the drag, so no manual
/// measure or allocate is needed.
pub fn drag_icon(icon: Option<&gio::Icon>, count: usize, scale: f64) -> gtk::Widget {
    let image = gtk::Image::new();
    match icon {
        Some(icon) => image.set_from_gicon(icon),
        None => image.set_icon_name(Some("text-x-generic")),
    }
    image.set_pixel_size(scaled(ICON_GRID, scale).max(1));
    let overlay = gtk::Overlay::new();
    overlay.add_css_class("gl-drag-icon");
    overlay.set_child(Some(&image));
    if let Some(text) = badge_text(count) {
        let badge = gtk::Label::new(Some(&text));
        badge.add_css_class("gl-drag-badge");
        badge.set_halign(gtk::Align::End);
        badge.set_valign(gtk::Align::End);
        // Keep the badge inside the icon's own bounds so the drag surface
        // cannot clip a two- or three-digit count.
        badge.set_margin_end(scaled(GAP_S, scale).max(1));
        badge.set_margin_bottom(scaled(GAP_S, scale).max(1));
        overlay.add_overlay(&badge);
    }
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

    fn both() -> gdk::DragAction {
        gdk::DragAction::COPY | gdk::DragAction::MOVE
    }

    fn local() -> Capability {
        Capability::Local(both())
    }

    fn no_keys() -> Option<DropModifiers> {
        Some(DropModifiers::None)
    }

    fn intent(negotiation: &Negotiation) -> DropIntent {
        intent_with(negotiation, no_keys())
    }

    fn intent_with(negotiation: &Negotiation, modifiers: Option<DropModifiers>) -> DropIntent {
        DropIntent::capture(negotiation, ModifierEvidence::test(modifiers)).expect("settled action")
    }

    #[test]
    fn drop_action_defaults_by_filesystem() {
        assert_eq!(action_for(true, DropModifiers::None), DropAction::Move);
        assert_eq!(action_for(false, DropModifiers::None), DropAction::Copy);
    }

    #[test]
    fn drop_action_ctrl_copies_and_shift_moves() {
        for same in [true, false] {
            assert_eq!(action_for(same, DropModifiers::Ctrl), DropAction::Copy);
            assert_eq!(action_for(same, DropModifiers::Shift), DropAction::Move);
        }
    }

    #[test]
    fn drop_action_respects_supported_actions() {
        // A copy-only source must never become an unadvertised move, even
        // when Shift is held and the target is on the same filesystem.
        let copy_only = Capability::Local(gdk::DragAction::COPY);
        assert!(!copy_only.allows(both(), DropAction::Move));
        assert_eq!(copy_only.preference(DropAction::Move), DropAction::Copy);

        // With only MOVE offered, the advertised move is the only possibility.
        let move_only = Capability::Local(gdk::DragAction::MOVE);
        assert!(!move_only.allows(both(), DropAction::Copy));
        assert_eq!(move_only.preference(DropAction::Copy), DropAction::Move);

        let neither = Capability::External;
        assert!(!neither.allows(gdk::DragAction::empty(), DropAction::Copy));
        assert!(!neither.allows(gdk::DragAction::empty(), DropAction::Move));
    }

    #[test]
    fn drop_action_external_without_local_drag() {
        assert_eq!(
            negotiated_action(None, gdk::DragAction::COPY),
            Some(DropAction::Copy)
        );
        assert_eq!(
            negotiated_action(None, gdk::DragAction::MOVE),
            Some(DropAction::Move)
        );
        // The source's whole COPY|MOVE mask is not a choice.
        assert_eq!(negotiated_action(None, both()), None);
        // A unique target-side answer is fresher than the local fallback.
        assert_eq!(
            negotiated_action(Some(DropAction::Move), gdk::DragAction::COPY),
            Some(DropAction::Copy)
        );
        assert_eq!(
            negotiated_action(Some(DropAction::Move), both()),
            Some(DropAction::Move)
        );

        // An external unique view is used as the current negotiation, not
        // cached as the immutable source mask.
        let copy = Negotiation::enter(Capability::External, gdk::DragAction::COPY, None, no_keys());
        assert_eq!(copy.selected(), Some(DropAction::Copy));
        assert_eq!(intent(&copy).action(), DropAction::Copy);

        let move_now =
            Negotiation::enter(Capability::External, gdk::DragAction::MOVE, None, no_keys());
        assert_eq!(move_now.selected(), Some(DropAction::Move));
        assert_eq!(intent(&move_now).action(), DropAction::Move);

        // No selected action yet: copy is the safe supported fallback.
        let ambiguous = Negotiation::enter(Capability::External, both(), None, no_keys());
        assert_eq!(ambiguous.selected(), None);
        assert_eq!(intent(&ambiguous).action(), DropAction::Copy);
    }

    #[test]
    fn drop_negotiation_freezes_provisional_copy_before_filesystem_result() {
        // Enter requests provisional Copy and the compositor answers Copy.
        // The same-filesystem result would prefer Move, but no key was ever
        // observed and no later motion confirmed Move; the drop stays Copy.
        let mut negotiation = Negotiation::enter(
            local(),
            gdk::DragAction::COPY,
            Some(DropAction::Copy),
            no_keys(),
        );
        assert_eq!(negotiation.preferred(), DropAction::Copy);
        assert_eq!(
            negotiation.filesystem_result(true, no_keys()),
            DropAction::Move
        );
        assert_eq!(negotiation.preferred(), DropAction::Move);
        assert_eq!(negotiation.selected(), Some(DropAction::Copy));
        assert_eq!(intent(&negotiation).action(), DropAction::Copy);
    }

    #[test]
    fn drop_negotiation_preserves_shift_move_across_drives_before_filesystem_result() {
        // Shift already selected Move before the different-filesystem answer
        // arrives. The answer's Copy default must not overwrite it.
        let mut negotiation = Negotiation::enter(
            local(),
            gdk::DragAction::MOVE,
            Some(DropAction::Move),
            Some(DropModifiers::Shift),
        );
        assert_eq!(
            negotiation.filesystem_result(false, Some(DropModifiers::Shift)),
            DropAction::Move
        );
        assert_eq!(negotiation.selected(), Some(DropAction::Move));
        assert_eq!(
            intent_with(&negotiation, Some(DropModifiers::Shift)).action(),
            DropAction::Move
        );
    }

    #[test]
    fn drop_negotiation_keeps_selected_action_when_preference_changes() {
        // The same-filesystem answer changes our preference to Move while the
        // compositor's selected action stays Copy (for example Ctrl is held).
        let mut negotiation = Negotiation::enter(
            local(),
            gdk::DragAction::COPY,
            Some(DropAction::Copy),
            Some(DropModifiers::Ctrl),
        );
        assert_eq!(
            negotiation.filesystem_result(true, Some(DropModifiers::Ctrl)),
            DropAction::Copy
        );
        assert_eq!(
            negotiation.observe(
                Some(DropAction::Copy),
                gdk::DragAction::COPY,
                Some(DropModifiers::Ctrl)
            ),
            DropAction::Copy
        );
        assert_eq!(negotiation.selected(), Some(DropAction::Copy));
        assert_eq!(
            intent_with(&negotiation, Some(DropModifiers::Ctrl)).action(),
            DropAction::Copy
        );
    }

    #[test]
    fn drop_negotiation_release_during_renegotiation() {
        // Ctrl-copy on one filesystem: releasing Ctrl while stationary asks
        // for Move and the final action follows the filesystem default.
        let mut ctrl = Negotiation::enter(
            local(),
            gdk::DragAction::COPY,
            Some(DropAction::Copy),
            Some(DropModifiers::Ctrl),
        );
        assert_eq!(
            ctrl.filesystem_result(true, Some(DropModifiers::Ctrl)),
            DropAction::Copy
        );
        assert_eq!(
            ctrl.observe(
                Some(DropAction::Copy),
                gdk::DragAction::COPY,
                Some(DropModifiers::Ctrl)
            ),
            DropAction::Copy
        );
        assert_eq!(
            ctrl.observe(Some(DropAction::Copy), gdk::DragAction::COPY, no_keys()),
            DropAction::Move
        );
        assert_eq!(intent(&ctrl).action(), DropAction::Move);

        // Shift-move across drives: releasing Shift while stationary asks
        // for Copy and the final action follows the filesystem default.
        let mut shift = Negotiation::enter(
            local(),
            gdk::DragAction::MOVE,
            Some(DropAction::Move),
            Some(DropModifiers::Shift),
        );
        assert_eq!(
            shift.filesystem_result(false, Some(DropModifiers::Shift)),
            DropAction::Move
        );
        assert_eq!(
            shift.observe(Some(DropAction::Move), gdk::DragAction::MOVE, no_keys()),
            DropAction::Copy
        );
        assert_eq!(intent(&shift).action(), DropAction::Copy);
    }

    #[test]
    fn drop_negotiation_external_preselected_copy_then_shift_moves() {
        let mut negotiation =
            Negotiation::enter(Capability::External, gdk::DragAction::COPY, None, no_keys());
        assert_eq!(intent(&negotiation).action(), DropAction::Copy);
        // The narrowed COPY view is not a cap: Shift asks for Move and the
        // current view becomes MOVE once the compositor answers.
        assert_eq!(
            negotiation.observe(None, gdk::DragAction::MOVE, Some(DropModifiers::Shift)),
            DropAction::Move
        );
        assert_eq!(
            intent_with(&negotiation, Some(DropModifiers::Shift)).action(),
            DropAction::Move
        );
    }

    #[test]
    fn drop_negotiation_external_preselected_move_then_ctrl_copies() {
        let mut negotiation =
            Negotiation::enter(Capability::External, gdk::DragAction::MOVE, None, no_keys());
        assert_eq!(intent(&negotiation).action(), DropAction::Move);
        assert_eq!(
            negotiation.observe(None, gdk::DragAction::COPY, Some(DropModifiers::Ctrl)),
            DropAction::Copy
        );
        assert_eq!(
            intent_with(&negotiation, Some(DropModifiers::Ctrl)).action(),
            DropAction::Copy
        );
    }

    #[test]
    fn drop_negotiation_external_release_returns_to_the_selected_default() {
        let mut negotiation = Negotiation::enter(
            Capability::External,
            gdk::DragAction::MOVE,
            None,
            Some(DropModifiers::Shift),
        );
        assert_eq!(
            negotiation.filesystem_result(false, Some(DropModifiers::Shift)),
            DropAction::Move
        );
        // Releasing Shift asks for the different-filesystem Copy default; the
        // compositor answers with COPY and the final action is Copy.
        assert_eq!(
            negotiation.observe(None, gdk::DragAction::COPY, no_keys()),
            DropAction::Copy
        );
        assert_eq!(intent(&negotiation).action(), DropAction::Copy);
    }

    #[test]
    fn drop_negotiation_external_narrowed_copy_can_prefer_the_default() {
        // The external COPY view may be narrowed from COPY|MOVE. Ask for the
        // same-filesystem Move default, but do not treat it as selected until
        // the compositor answers. A genuine copy-only source stays at COPY.
        let mut negotiation =
            Negotiation::enter(Capability::External, gdk::DragAction::COPY, None, no_keys());
        assert_eq!(
            negotiation.filesystem_result(true, no_keys()),
            DropAction::Move
        );
        assert_eq!(negotiation.selected(), Some(DropAction::Copy));
        assert_eq!(intent(&negotiation).action(), DropAction::Copy);
        // The compositor accepted Move; now the drop may move.
        assert_eq!(
            negotiation.observe(None, gdk::DragAction::MOVE, no_keys()),
            DropAction::Move
        );
        assert_eq!(intent(&negotiation).action(), DropAction::Move);
    }

    #[test]
    fn drop_negotiation_copy_only_external_stays_copy() {
        let mut negotiation =
            Negotiation::enter(Capability::External, gdk::DragAction::COPY, None, no_keys());
        // A same-filesystem default asks for Move as a hint, but a genuine
        // copy-only offer keeps its selected action at Copy, so the final
        // action is Copy and no unadvertised move is executed.
        assert_eq!(
            negotiation.filesystem_result(true, no_keys()),
            DropAction::Move
        );
        assert_eq!(
            negotiation.observe(None, gdk::DragAction::COPY, no_keys()),
            DropAction::Move
        );
        assert_eq!(negotiation.selected(), Some(DropAction::Copy));
        assert_eq!(intent(&negotiation).action(), DropAction::Copy);
    }

    #[test]
    fn drop_negotiation_refuses_a_move_only_unsettled_offer() {
        let negotiation = Negotiation::enter(
            Capability::Local(gdk::DragAction::MOVE),
            gdk::DragAction::empty(),
            None,
            no_keys(),
        );
        assert_eq!(
            DropIntent::capture(&negotiation, ModifierEvidence::test(no_keys())),
            Err("drag action is not settled")
        );
    }

    #[test]
    fn drop_negotiation_modifier_release_keeps_filesystem_preference() {
        // Model a held key, then its release while the pointer is stationary.
        // The target's preference returns to the filesystem default, and the
        // old selected echo must not keep the override alive.
        for same in [true, false] {
            let default = action_for(same, DropModifiers::None);
            let (held, held_modifiers) = if same {
                (DropAction::Copy, DropModifiers::Ctrl)
            } else {
                (DropAction::Move, DropModifiers::Shift)
            };
            let mut negotiation =
                Negotiation::enter(local(), both(), Some(held), Some(held_modifiers));
            assert_eq!(
                negotiation.filesystem_result(same, Some(held_modifiers)),
                held
            );
            assert_eq!(
                negotiation.observe(Some(held), gdk_action(held), Some(held_modifiers)),
                held
            );
            assert_eq!(
                negotiation.observe(Some(held), gdk_action(held), no_keys()),
                default
            );
            assert_eq!(intent(&negotiation).action(), default);
        }
    }

    #[test]
    fn modifier_keys_held_before_prepare_survive_a_cleared_seat() {
        // prepare sees the seat's positive state before GDK starts the drag.
        let keys = TrackedKeys::UNKNOWN.at_drag_start(Some(DropModifiers::Ctrl));
        assert_eq!(keys.modifiers(), Some(DropModifiers::Ctrl));
        assert_eq!(keys.evidence().origin(), "drag start");
        // GDK may clear the seat after drag begin; the start observation stays
        // in the tracker until a live key event or the drag's end.
        assert_eq!(keys.evidence().modifiers(), Some(DropModifiers::Ctrl));

        let mut negotiation = Negotiation::enter(
            local(),
            gdk::DragAction::COPY,
            Some(DropAction::Move),
            keys.modifiers(),
        );
        assert_eq!(negotiation.preferred(), DropAction::Copy);
        assert_eq!(
            negotiation.filesystem_result(true, keys.modifiers()),
            DropAction::Copy
        );
        assert_eq!(
            intent_with(&negotiation, keys.modifiers()).action(),
            DropAction::Copy
        );
    }

    #[test]
    fn modifier_keys_pressed_during_drag_change_preference_and_final_action() {
        // No key at prepare: unknown, not "none".
        let keys = TrackedKeys::UNKNOWN.at_drag_start(None);
        assert_eq!(keys.modifiers(), None);
        let mut negotiation = Negotiation::enter(
            local(),
            gdk::DragAction::COPY,
            Some(DropAction::Copy),
            keys.modifiers(),
        );
        assert_eq!(
            negotiation.filesystem_result(true, keys.modifiers()),
            DropAction::Move
        );
        // The window key controller observes Ctrl being pressed mid-drag.
        let keys = keys.key(gdk::Key::Control_L, true).expect("modifier key");
        assert_eq!(keys.modifiers(), Some(DropModifiers::Ctrl));
        assert_eq!(
            negotiation.observe(
                Some(DropAction::Move),
                gdk::DragAction::MOVE,
                keys.modifiers()
            ),
            DropAction::Copy
        );
        assert_eq!(
            intent_with(&negotiation, keys.modifiers()).action(),
            DropAction::Copy
        );
    }

    #[test]
    fn modifier_keys_released_while_stationary_restore_the_filesystem_default() {
        for (same, key, held, held_modifiers) in [
            (
                true,
                gdk::Key::Control_L,
                DropAction::Copy,
                DropModifiers::Ctrl,
            ),
            (
                false,
                gdk::Key::Shift_L,
                DropAction::Move,
                DropModifiers::Shift,
            ),
        ] {
            let keys = TrackedKeys::UNKNOWN.at_drag_start(Some(held_modifiers));
            let mut negotiation = Negotiation::enter(local(), both(), Some(held), keys.modifiers());
            assert_eq!(negotiation.filesystem_result(same, keys.modifiers()), held);
            assert_eq!(negotiation.preferred(), held);
            let keys = keys.key(key, false).expect("modifier key");
            assert_eq!(keys.modifiers(), Some(DropModifiers::None));
            let default = action_for(same, DropModifiers::None);
            // Stationary release: the listener re-observes with the same live
            // compositor view and sends the new preference.
            assert_eq!(
                negotiation.observe(Some(held), gdk_action(held), keys.modifiers()),
                default
            );
            assert_eq!(
                intent_with(&negotiation, keys.modifiers()).action(),
                default
            );
        }
    }

    #[test]
    fn ctrl_wins_when_ctrl_and_shift_are_both_held() {
        let keys = TrackedKeys::UNKNOWN
            .key(gdk::Key::Control_L, true)
            .expect("modifier key")
            .key(gdk::Key::Shift_L, true)
            .expect("modifier key");
        assert_eq!(keys.modifiers(), Some(DropModifiers::Ctrl));
        let mut negotiation = Negotiation::enter(
            local(),
            gdk::DragAction::COPY,
            Some(DropAction::Move),
            keys.modifiers(),
        );
        assert_eq!(
            negotiation.filesystem_result(false, keys.modifiers()),
            DropAction::Copy
        );
        assert_eq!(
            intent_with(&negotiation, keys.modifiers()).action(),
            DropAction::Copy
        );

        // Releasing Ctrl leaves Shift held: Move wins now.
        let keys = keys.key(gdk::Key::Control_L, false).expect("modifier key");
        assert_eq!(keys.modifiers(), Some(DropModifiers::Shift));
        assert_eq!(
            negotiation.observe(
                Some(DropAction::Copy),
                gdk::DragAction::COPY,
                keys.modifiers()
            ),
            DropAction::Move
        );
        assert_eq!(
            intent_with(&negotiation, keys.modifiers()).action(),
            DropAction::Move
        );
    }

    #[test]
    fn unsupported_observed_action_is_refused() {
        // Shift on a copy-only source: Move is not offered, so refuse rather
        // than silently move or silently copy.
        let mut copy_only = Negotiation::enter(
            Capability::Local(gdk::DragAction::COPY),
            gdk::DragAction::COPY,
            Some(DropAction::Copy),
            Some(DropModifiers::Shift),
        );
        copy_only.filesystem_result(false, Some(DropModifiers::Shift));
        assert_eq!(
            DropIntent::capture(
                &copy_only,
                ModifierEvidence::test(Some(DropModifiers::Shift))
            ),
            Err("this drag does not offer Move")
        );

        // Ctrl on a move-only source: Copy is not offered.
        let mut move_only = Negotiation::enter(
            Capability::Local(gdk::DragAction::MOVE),
            gdk::DragAction::MOVE,
            Some(DropAction::Move),
            Some(DropModifiers::Ctrl),
        );
        move_only.filesystem_result(true, Some(DropModifiers::Ctrl));
        assert_eq!(
            DropIntent::capture(
                &move_only,
                ModifierEvidence::test(Some(DropModifiers::Ctrl))
            ),
            Err("this drag does not offer Copy")
        );
    }

    #[test]
    fn missing_modifier_evidence_stays_unknown_and_uses_the_compositor_action() {
        let mut negotiation =
            Negotiation::enter(local(), gdk::DragAction::COPY, Some(DropAction::Copy), None);
        negotiation.filesystem_result(true, None);
        let unknown = ModifierEvidence::unknown();
        assert_eq!(unknown.modifiers(), None);
        assert!(unknown.text().contains("unknown"));
        // Unknown is not "no key": the compositor's accepted Copy is kept.
        assert_eq!(
            DropIntent::capture(&negotiation, unknown).unwrap().action(),
            DropAction::Copy
        );
    }

    #[test]
    fn external_drag_observed_ctrl_copies_and_shift_moves() {
        // Ctrl on a narrowed MOVE view asks for Copy, but until the source
        // answers, the drop refuses rather than silently moving.
        let mut ctrl = Negotiation::enter(
            Capability::External,
            gdk::DragAction::MOVE,
            None,
            Some(DropModifiers::Ctrl),
        );
        assert_eq!(ctrl.preferred(), DropAction::Copy);
        assert_eq!(
            DropIntent::capture(&ctrl, ModifierEvidence::test(Some(DropModifiers::Ctrl))),
            Err("this drag does not offer Copy")
        );
        assert_eq!(
            ctrl.observe(None, gdk::DragAction::COPY, Some(DropModifiers::Ctrl)),
            DropAction::Copy
        );
        assert_eq!(
            intent_with(&ctrl, Some(DropModifiers::Ctrl)).action(),
            DropAction::Copy
        );

        // Shift on a narrowed COPY view asks for Move, then the source
        // answers MOVE.
        let mut shift = Negotiation::enter(
            Capability::External,
            gdk::DragAction::COPY,
            None,
            Some(DropModifiers::Shift),
        );
        assert_eq!(shift.preferred(), DropAction::Move);
        assert_eq!(
            DropIntent::capture(&shift, ModifierEvidence::test(Some(DropModifiers::Shift))),
            Err("this drag does not offer Move")
        );
        assert_eq!(
            shift.observe(None, gdk::DragAction::MOVE, Some(DropModifiers::Shift)),
            DropAction::Move
        );
        assert_eq!(
            intent_with(&shift, Some(DropModifiers::Shift)).action(),
            DropAction::Move
        );
    }

    #[test]
    fn a_drag_start_seed_does_not_survive_the_drag() {
        let seeded = TrackedKeys::UNKNOWN
            .at_drag_start(Some(DropModifiers::Ctrl))
            .end_drag();
        assert_eq!(seeded.modifiers(), None);
        // A prepare that never became a drag must not carry its seed into the
        // next empty Wayland seat reading.
        let stale = TrackedKeys::UNKNOWN
            .at_drag_start(Some(DropModifiers::Ctrl))
            .at_drag_start(None);
        assert_eq!(stale.modifiers(), None);
        // A live key observation is not a drag-start seed and survives.
        let live = TrackedKeys::UNKNOWN
            .key(gdk::Key::Shift_L, true)
            .expect("modifier key")
            .end_drag();
        assert_eq!(live.modifiers(), Some(DropModifiers::Shift));
    }

    #[test]
    fn drop_intent_schedules_the_negotiated_operation() {
        let mut copy = Negotiation::enter(
            local(),
            gdk::DragAction::COPY,
            Some(DropAction::Copy),
            no_keys(),
        );
        copy.filesystem_result(true, no_keys());
        let copy = DropIntent::capture(&copy, ModifierEvidence::test(no_keys())).unwrap();
        assert_eq!(
            operation_for_action(
                &Target::Grid(path("/tmp/b")),
                &paths(&["/tmp/a/one.txt"]),
                copy.action()
            ),
            Ok(Operation::Copy { to: path("/tmp/b") })
        );

        let move_now = Negotiation::enter(
            local(),
            gdk::DragAction::MOVE,
            Some(DropAction::Move),
            no_keys(),
        );
        let move_now = DropIntent::capture(&move_now, ModifierEvidence::test(no_keys())).unwrap();
        assert_eq!(
            operation_for_action(
                &Target::Grid(path("/tmp/b")),
                &paths(&["/tmp/a/one.txt"]),
                move_now.action()
            ),
            Ok(Operation::Move { to: path("/tmp/b") })
        );
    }

    #[test]
    fn drop_intent_survives_async_filesystem_query() {
        // The same event sequence as the callbacks: the intent captured at
        // drop does not change when the filesystem answer was already known.
        let mut negotiation = Negotiation::enter(
            local(),
            gdk::DragAction::COPY,
            Some(DropAction::Copy),
            no_keys(),
        );
        negotiation.filesystem_result(true, no_keys());
        let captured = intent(&negotiation);
        assert_eq!(captured.action(), DropAction::Copy);
        assert_eq!(captured.filesystem(), Some(true));
        assert_eq!(captured.selected(), Some(DropAction::Copy));
    }

    #[test]
    fn background_drop_resolves_padding_and_empty_view() {
        let cells = [DropRect {
            x: 32.0,
            y: 40.0,
            width: 80.0,
            height: 110.0,
        }];
        // Left and right padding around the cell.
        assert!(background_owns(10.0, 60.0, &cells));
        assert!(background_owns(120.0, 60.0, &cells));
        // The blank viewport beyond the last row.
        assert!(background_owns(60.0, 200.0, &cells));
        // An empty folder is all background.
        assert!(background_owns(0.0, 0.0, &[]));
        // The cell's own point is not background, including its edges.
        assert!(!background_owns(60.0, 60.0, &cells));
        assert!(!background_owns(32.0, 40.0, &cells));
        assert!(!background_owns(111.0, 149.0, &cells));
        assert!(background_owns(112.0, 150.0, &cells));
    }

    #[test]
    fn background_drop_preserves_cell_refusal_and_tile_destination() {
        let cells = [DropRect {
            x: 32.0,
            y: 40.0,
            width: 80.0,
            height: 110.0,
        }];
        // Padding in one tile stays that tile's folder, not another tile's.
        assert!(background_owns(10.0, 60.0, &cells));
        assert_eq!(
            destination(&Target::Grid(path("/tiles/left"))),
            Ok(Destination::Folder(path("/tiles/left")))
        );
        // A folder cell keeps its own destination.
        assert!(!background_owns(60.0, 60.0, &cells));
        assert_eq!(
            destination(&Target::FolderCell(path("/tiles/left/sub"))),
            Ok(Destination::Folder(path("/tiles/left/sub")))
        );
        // A file cell stays refused; the ancestor does not absorb it.
        assert_eq!(
            destination(&Target::FileCell),
            Err("only folders are drop targets".to_string())
        );
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
        let same = action_for(true, DropModifiers::None);
        assert_eq!(
            operation_for_action(
                &Target::Grid(path("/tmp/b")),
                &paths(&["/tmp/a/one.txt"]),
                same
            ),
            Ok(Operation::Move { to: path("/tmp/b") })
        );
        let across = action_for(false, DropModifiers::None);
        assert_eq!(
            operation_for_action(
                &Target::FolderCell(path("/tmp/b")),
                &paths(&["/tmp/a/one.txt"]),
                across
            ),
            Ok(Operation::Copy { to: path("/tmp/b") })
        );
    }

    #[test]
    fn trash_and_pins_ignore_modifiers() {
        for action in [DropAction::Copy, DropAction::Move] {
            assert_eq!(
                operation_for_action(&Target::Trash, &paths(&["/tmp/a/one"]), action),
                Ok(Operation::Trash)
            );
            assert_eq!(
                operation_for_action(&Target::PinList, &paths(&["/tmp/a/one"]), action),
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
        assert_eq!(badge_text(12), Some("12".to_string()));
        assert_eq!(badge_text(100), Some("100".to_string()));
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
