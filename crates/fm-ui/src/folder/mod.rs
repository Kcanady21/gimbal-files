// SPDX-License-Identifier: GPL-3.0-or-later
//! The folder view: the GIO listing, the virtualized grid and its status line.

pub mod size;
mod sort;
mod view;

use std::cell::RefCell;
use std::ffi::OsString;
use std::path::Path;
use std::rc::Rc;

use gtk::prelude::*;

pub use view::FolderView;

/// The non-widget half of the status line. A selection update always caches
/// the next status, but a held operation summary stays on screen until it is
/// released.
#[derive(Default)]
pub struct StatusCache {
    counts: StatusText,
    held: bool,
}

impl StatusCache {
    pub fn set(&mut self, text: StatusText) -> Option<StatusText> {
        self.counts = text;
        (!self.held).then(|| self.counts.clone())
    }

    pub fn hold(&mut self) {
        self.held = true;
    }

    pub fn release(&mut self) -> StatusText {
        self.held = false;
        self.counts.clone()
    }
}

/// The one status line. A folder view writes its counts/selection here; a
/// finished file operation temporarily holds the line for its summary, so a
/// live listing cannot overwrite the summary before it has been read.
pub struct StatusLine {
    root: gtk::Box,
    primary: gtk::Label,
    /// The dash between the name and the size/counts, shown only with a suffix.
    separator: gtk::Label,
    suffix: gtk::Label,
    cache: RefCell<StatusCache>,
    /// Which pulse class the last message used. GTK restarts a CSS animation
    /// only when the class changes, so back-to-back messages alternate.
    pulse_b: std::cell::Cell<bool>,
}

impl StatusLine {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("gl-status");
        root.set_hexpand(true);
        // The name, a dash and the size/counts sit together as one centered
        // group (Kyle, Nov 6). The name once filled the width and pushed the
        // size to the far right, leaving a wide gap between them.
        let group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        group.set_hexpand(true);
        group.set_halign(gtk::Align::Center);
        let primary = gtk::Label::new(Some(&status_text(0, 0, 0)));
        primary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let separator = gtk::Label::new(Some("—"));
        separator.add_css_class("gl-status-sep");
        separator.set_visible(false);
        let suffix = gtk::Label::new(None);
        // The size is the first suffix field, so End ellipsis keeps it before
        // the counts when the window is narrow.
        suffix.set_ellipsize(gtk::pango::EllipsizeMode::End);
        group.append(&primary);
        group.append(&separator);
        group.append(&suffix);
        root.append(&group);
        Rc::new(Self {
            root,
            primary,
            separator,
            suffix,
            cache: RefCell::new(StatusCache::default()),
            pulse_b: std::cell::Cell::new(false),
        })
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn set_status(&self, text: StatusText) {
        if let Some(show) = self.cache.borrow_mut().set(text) {
            self.apply(&show);
        }
    }

    pub fn set_counts(&self, text: String) {
        self.set_status(StatusText {
            primary: text,
            suffix: String::new(),
            tooltip: None,
        });
    }

    /// A message for the user. The bar pulses (the look module's
    /// `gl-status-pulse`, timed by the motion setting) to draw the eye.
    pub fn show_summary(&self, text: &str) {
        self.cache.borrow_mut().hold();
        self.primary.set_text(text);
        self.primary.set_tooltip_text(None);
        self.separator.set_visible(false);
        self.suffix.set_text("");
        self.suffix.set_visible(false);
        let next_b = !self.pulse_b.get();
        self.pulse_b.set(next_b);
        let (on, off) = if next_b {
            ("notify-b", "notify-a")
        } else {
            ("notify-a", "notify-b")
        };
        self.root.remove_css_class(off);
        self.root.add_css_class(on);
    }

    pub fn end_summary(&self) {
        let status = self.cache.borrow_mut().release();
        self.root.remove_css_class("notify-a");
        self.root.remove_css_class("notify-b");
        self.apply(&status);
    }

    fn apply(&self, text: &StatusText) {
        self.primary.set_text(&text.primary);
        self.primary.set_tooltip_text(
            text.tooltip
                .as_deref()
                .or_else(|| (!text.primary.is_empty()).then_some(text.primary.as_str())),
        );
        self.suffix.set_text(&text.suffix);
        self.suffix.set_visible(!text.suffix.is_empty());
        self.separator.set_visible(!text.suffix.is_empty());
    }
}

/// The status line under the grid. `selected` is the model's count, so it is
/// shown even when a live listing has not realised every name yet.
pub fn status_text(files: u64, folders: u64, selected: usize) -> String {
    let mut text = format!("Files: {files}, Folders: {folders}");
    if selected > 0 {
        text.push_str(&format!(", {selected} selected"));
    }
    text
}

/// One selected entry as the status line and tooltip know it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionEntry {
    pub name: String,
    pub is_folder: bool,
    pub size: Option<u64>,
}

/// The status line's two parts. The primary name ellipsizes; the suffix keeps
/// the size and counts visible at narrow widths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusText {
    pub primary: String,
    pub suffix: String,
    pub tooltip: Option<String>,
}

impl Default for StatusText {
    fn default() -> Self {
        Self::counts(0, 0, 0)
    }
}

impl StatusText {
    pub fn counts(files: u64, folders: u64, selected: usize) -> Self {
        Self {
            primary: status_text(files, folders, selected),
            suffix: String::new(),
            tooltip: None,
        }
    }
}

/// Build the selection-aware status. Unknown sizes are reported as unknown,
/// never folded into a total that looks complete. A folder is never given a
/// misleading regular-file byte count.
pub fn selection_status(files: u64, folders: u64, selected: &[SelectionEntry]) -> StatusText {
    let counts = format!("Files: {files}, Folders: {folders}");
    match selected {
        [] => StatusText {
            primary: counts,
            suffix: String::new(),
            tooltip: None,
        },
        [entry] if entry.is_folder => StatusText {
            primary: entry.name.clone(),
            suffix: format!("Folder — {counts}"),
            tooltip: Some(format!("{}\nFolder", entry.name)),
        },
        [entry] => {
            let (size, tooltip_size) = match entry.size {
                Some(size) => (
                    fm_core::preview::format_size(size),
                    fm_core::preview::format_size(size),
                ),
                None => (
                    "Size unavailable".to_string(),
                    "Size unavailable".to_string(),
                ),
            };
            StatusText {
                primary: entry.name.clone(),
                suffix: format!("{size} — {counts}"),
                tooltip: Some(format!("{}\n{tooltip_size}", entry.name)),
            }
        }
        entries => {
            let known: u64 = entries.iter().filter_map(|entry| entry.size).sum();
            let unknown = entries
                .iter()
                .filter(|entry| !entry.is_folder && entry.size.is_none())
                .count();
            let folders_selected = entries.iter().filter(|entry| entry.is_folder).count();
            let total = if unknown == 0 {
                fm_core::preview::format_size(known)
            } else {
                format!(
                    "{} known, {unknown} unavailable",
                    fm_core::preview::format_size(known)
                )
            };
            let mut suffix = total;
            if folders_selected > 0 {
                suffix.push_str(&format!(" ({folders_selected} folder(s) not sized)"));
            }
            suffix.push_str(&format!(" — {counts}"));
            StatusText {
                primary: format!("{} selected", entries.len()),
                suffix,
                tooltip: Some(format!(
                    "{} selected\n{}",
                    entries.len(),
                    if unknown == 0 {
                        "All regular-file sizes known".to_string()
                    } else {
                        format!("{unknown} size(s) unavailable")
                    }
                )),
            }
        }
    }
}

/// One item's hover details, in plain text. `None` fields are explicitly
/// unavailable rather than silently omitted.
pub fn details_text(name: &str, kind: &str, size: Option<u64>, modified: Option<&str>) -> String {
    let size = size
        .map(fm_core::preview::format_size)
        .unwrap_or_else(|| "Unavailable".to_string());
    format!(
        "{name}\nType: {kind}\nSize: {size}\nModified: {}",
        modified.unwrap_or("Unavailable")
    )
}

/// One keyboard move in the visible sorted/filtered order. Returns `None` when
/// the grid is empty; positions are clamped rather than wrapping so the
/// preview and scroll stay near the user's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridKey {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
}

pub fn keyboard_target(
    position: Option<u32>,
    columns: u32,
    count: u32,
    key: GridKey,
    page_rows: u32,
) -> Option<u32> {
    if count == 0 {
        return None;
    }
    let columns = columns.max(1) as i64;
    let last = i64::from(count - 1);
    let current = i64::from(position.unwrap_or(0)).clamp(0, last);
    let page = columns * i64::from(page_rows.max(1));
    let target = match key {
        GridKey::Left => current - 1,
        GridKey::Right => current + 1,
        GridKey::Up => current - columns,
        GridKey::Down => current + columns,
        GridKey::Home => 0,
        GridKey::End => last,
        GridKey::PageUp => current - page,
        GridKey::PageDown => current + page,
    }
    .clamp(0, last);
    Some(target as u32)
}

/// How one navigation key should reach the model. Ctrl moves focus only; a
/// shift move extends/contracts from the model's existing anchor; a plain move
/// selects the target exactly as a plain click does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavMode {
    Plain,
    Extend,
    Focus,
}

pub fn nav_mode(ctrl: bool, shift: bool) -> NavMode {
    if shift {
        NavMode::Extend
    } else if ctrl {
        NavMode::Focus
    } else {
        NavMode::Plain
    }
}

/// Visible rows in the viewport. The scroller's page size, not the grid's
/// full scrollable height, is the page distance; a row height measured from
/// realised cells keeps multi-line selected names correct.
pub fn visible_page_rows(viewport_height: f64, row_height: f64) -> u32 {
    if !viewport_height.is_finite() || !row_height.is_finite() || row_height <= 0.0 {
        return 1;
    }
    (viewport_height / row_height).floor().max(1.0) as u32
}

/// The production keyboard-to-selection name: one key move in the actual
/// visible order. Only the target entry is read; Shift builds the range order
/// separately because that is the only operation that needs it.
pub fn keyboard_move(
    position: Option<u32>,
    columns: u32,
    count: u32,
    key: GridKey,
    page_rows: u32,
) -> Option<u32> {
    keyboard_target(position, columns, count, key, page_rows)
}

/// The keyboard's focus, separate from the model's selection anchor. It is a
/// file name, not a position, so re-sorting or a listing update cannot move it
/// to a different file; only removal/filtering falls back to the selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyboardFocus {
    name: Option<OsString>,
}

impl KeyboardFocus {
    pub fn name(&self) -> Option<&std::ffi::OsStr> {
        self.name.as_deref()
    }

    pub fn set(&mut self, name: OsString) {
        self.name = Some(name);
    }

    pub fn reset(&mut self) {
        self.name = None;
    }

    /// A passive model selection must not reset an explicit keyboard focus.
    /// Keep the cursor while its name is still listed; only a removed or
    /// filtered-away item falls back to the selection anchor.
    pub fn reapply(
        &mut self,
        listed: impl Fn(&std::ffi::OsStr) -> bool,
        fallback: Option<OsString>,
    ) {
        if self
            .name
            .as_ref()
            .is_some_and(|name| listed(name.as_os_str()))
        {
            return;
        }
        self.name = fallback;
    }
}

/// A rectangle in the folder viewport's coordinates. Rubber-band geometry is
/// pure so the press, the overlay and the final hits all agree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RubberRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl RubberRect {
    pub fn contains(self, x: f64, y: f64) -> bool {
        self.width > 0.0
            && self.height > 0.0
            && x >= self.x
            && x < self.x + self.width
            && y >= self.y
            && y < self.y + self.height
    }
}

/// The rectangle between two points, in either direction.
pub fn normalize_rect(ax: f64, ay: f64, bx: f64, by: f64) -> RubberRect {
    RubberRect {
        x: ax.min(bx),
        y: ay.min(by),
        width: (bx - ax).abs(),
        height: (by - ay).abs(),
    }
}

/// The visible part of a rectangle inside a viewport. A drag that leaves the
/// tile therefore cannot draw or select beyond it.
pub fn clip_rect(rect: RubberRect, width: f64, height: f64) -> RubberRect {
    let left = rect.x.max(0.0);
    let top = rect.y.max(0.0);
    let right = (rect.x + rect.width).min(width.max(0.0));
    let bottom = (rect.y + rect.height).min(height.max(0.0));
    RubberRect {
        x: left,
        y: top,
        width: (right - left).max(0.0),
        height: (bottom - top).max(0.0),
    }
}

/// Positive-area intersection. An edge or a corner that merely touches is not
/// a hit, so a rectangle exactly at a cell's edge selects nothing from it.
pub fn rects_intersect(a: RubberRect, b: RubberRect) -> bool {
    a.width > 0.0
        && a.height > 0.0
        && b.width > 0.0
        && b.height > 0.0
        && a.x < b.x + b.width
        && b.x < a.x + a.width
        && a.y < b.y + b.height
        && b.y < a.y + a.height
}

/// Which realized cell owns a press. Gaps between cells and the space below
/// the last row own no cell, so a press there belongs to the viewport.
pub fn cell_at(x: f64, y: f64, cells: &[(u32, RubberRect)]) -> Option<u32> {
    cells
        .iter()
        .find(|(_, rect)| rect.contains(x, y))
        .map(|(position, _)| *position)
}

/// Every realized cell the rectangle intersects.
pub fn rubber_hits(rect: RubberRect, cells: &[(u32, RubberRect)]) -> Vec<u32> {
    cells
        .iter()
        .filter(|(_, cell)| rects_intersect(rect, *cell))
        .map(|(position, _)| *position)
        .collect()
}

/// GTK's own drag threshold test. A movement exactly equal to the threshold is
/// still a press, matching `gtk_drag_check_threshold_double`'s strict `>`.
pub fn drag_passes_threshold(dx: f64, dy: f64, threshold: f64) -> bool {
    dx.abs() > threshold || dy.abs() > threshold
}

/// One click's meaning, before it reaches the model. Shift carries the visible
/// order because the model has no listing of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClickRequest {
    Plain(OsString),
    Ctrl(OsString),
    Shift {
        name: OsString,
        order: Vec<OsString>,
    },
}

/// The pure click mapping. Ctrl wins if both modifiers are down, as the strip's
/// clicks do.
pub fn click_request(ctrl: bool, shift: bool, name: OsString, order: &[OsString]) -> ClickRequest {
    if ctrl {
        ClickRequest::Ctrl(name)
    } else if shift {
        ClickRequest::Shift {
            name,
            order: order.to_vec(),
        }
    } else {
        ClickRequest::Plain(name)
    }
}

/// A name starting with `.` is hidden and left out of the listing.
pub fn is_hidden_name(name: &Path) -> bool {
    name.file_name()
        .is_some_and(|name| name.as_encoded_bytes().first() == Some(&b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn status_counts_and_selection_read_naturally() {
        assert_eq!(status_text(12, 4, 0), "Files: 12, Folders: 4");
        assert_eq!(status_text(12, 4, 2), "Files: 12, Folders: 4, 2 selected");
        assert_eq!(status_text(0, 0, 1), "Files: 0, Folders: 0, 1 selected");
    }

    #[test]
    fn keyboard_moves_clamp_to_the_visible_grid() {
        assert_eq!(keyboard_target(Some(0), 2, 5, GridKey::Down, 1), Some(2));
        assert_eq!(keyboard_target(Some(4), 2, 5, GridKey::Down, 1), Some(4));
        assert_eq!(keyboard_target(Some(3), 2, 5, GridKey::Up, 1), Some(1));
        assert_eq!(keyboard_target(Some(2), 2, 5, GridKey::Left, 1), Some(1));
        assert_eq!(keyboard_target(Some(2), 2, 5, GridKey::Right, 1), Some(3));
        assert_eq!(keyboard_target(Some(3), 2, 5, GridKey::Home, 1), Some(0));
        assert_eq!(keyboard_target(Some(0), 2, 5, GridKey::End, 1), Some(4));
        assert_eq!(
            keyboard_target(Some(0), 2, 5, GridKey::PageDown, 2),
            Some(4)
        );
        assert_eq!(keyboard_target(Some(4), 2, 5, GridKey::PageUp, 2), Some(0));
        assert_eq!(keyboard_target(Some(0), 2, 0, GridKey::Down, 1), None);
    }

    #[test]
    fn status_keeps_the_size_and_unknown_totals_honest() {
        let file = SelectionEntry {
            name: "a very long report name.pdf".to_string(),
            is_folder: false,
            size: Some(1536),
        };
        let status = selection_status(12, 4, std::slice::from_ref(&file));
        assert_eq!(status.primary, "a very long report name.pdf");
        assert_eq!(status.suffix, "1.5 KiB — Files: 12, Folders: 4");

        let folder = SelectionEntry {
            name: "Documents".to_string(),
            is_folder: true,
            size: None,
        };
        let status = selection_status(12, 4, &[folder]);
        assert_eq!(status.suffix, "Folder — Files: 12, Folders: 4");

        let unknown = SelectionEntry {
            name: "remote.bin".to_string(),
            is_folder: false,
            size: None,
        };
        let status = selection_status(0, 0, &[file.clone(), unknown]);
        assert!(status.suffix.contains("1.5 KiB known, 1 unavailable"));
        assert!(!status.suffix.contains("complete"));
        assert_eq!(status.primary, "2 selected");

        let counts = selection_status(12, 4, &[]);
        assert_eq!(counts.primary, "Files: 12, Folders: 4");

        // A late listing's new size replaces the old one; the status never
        // keeps a stale byte count.
        let newer = SelectionEntry {
            name: "a very long report name.pdf".to_string(),
            is_folder: false,
            size: Some(2048),
        };
        assert_eq!(
            selection_status(12, 4, &[newer]).suffix,
            "2.0 KiB — Files: 12, Folders: 4"
        );
    }

    #[test]
    fn a_held_operation_summary_restores_the_latest_selection_status() {
        let mut cache = StatusCache::default();
        assert_eq!(
            cache
                .set(StatusText::counts(12, 4, 0))
                .map(|status| status.primary),
            Some("Files: 12, Folders: 4".to_string())
        );
        cache.hold();
        assert!(cache.set(StatusText::counts(1, 2, 1)).is_none());
        let restored = cache.release();
        assert_eq!(restored.primary, "Files: 1, Folders: 2, 1 selected");
        assert!(cache.release().primary == "Files: 1, Folders: 2, 1 selected");
    }

    #[test]
    fn hover_details_are_plain_text_with_explicit_unknowns() {
        assert_eq!(
            details_text("a.txt", "text/plain", Some(1536), Some("2026-11-03 12:00")),
            "a.txt\nType: text/plain\nSize: 1.5 KiB\nModified: 2026-11-03 12:00"
        );
        assert_eq!(
            details_text("remote", "File", None, None),
            "remote\nType: File\nSize: Unavailable\nModified: Unavailable"
        );
    }
    #[test]
    fn shift_gets_the_visible_order_exactly() {
        let order: Vec<OsString> = ["b", "a", "c"]
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        assert_eq!(
            click_request(false, true, OsString::from("a"), &order),
            ClickRequest::Shift {
                name: OsString::from("a"),
                order,
            }
        );
        assert_eq!(
            click_request(false, false, OsString::from("b"), &[]),
            ClickRequest::Plain(OsString::from("b"))
        );
        assert_eq!(
            click_request(true, true, OsString::from("c"), &[]),
            ClickRequest::Ctrl(OsString::from("c"))
        );
    }

    #[test]
    fn empty_space_and_item_bounds_decide_the_press_owner() {
        let cells = [
            (
                0,
                RubberRect {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: 80.0,
                },
            ),
            (
                1,
                RubberRect {
                    x: 120.0,
                    y: 0.0,
                    width: 100.0,
                    height: 80.0,
                },
            ),
            (
                2,
                RubberRect {
                    x: 0.0,
                    y: 100.0,
                    width: 100.0,
                    height: 80.0,
                },
            ),
        ];
        assert_eq!(cell_at(50.0, 50.0, &cells), Some(0));
        // A gap between cells owns no item.
        assert_eq!(cell_at(110.0, 50.0, &cells), None);
        // The gap between rows owns no item.
        assert_eq!(cell_at(50.0, 90.0, &cells), None);
        // The space below the last row owns no item.
        assert_eq!(cell_at(50.0, 200.0, &cells), None);
        // A point exactly on a cell's right edge belongs to the viewport.
        assert_eq!(cell_at(100.0, 50.0, &cells), None);
        assert_eq!(cell_at(0.0, 0.0, &cells), Some(0));
    }

    #[test]
    fn drag_rectangles_normalize_in_every_direction() {
        let expected = RubberRect {
            x: 10.0,
            y: 20.0,
            width: 90.0,
            height: 60.0,
        };
        assert_eq!(normalize_rect(10.0, 20.0, 100.0, 80.0), expected);
        assert_eq!(normalize_rect(100.0, 20.0, 10.0, 80.0), expected);
        assert_eq!(normalize_rect(10.0, 80.0, 100.0, 20.0), expected);
        assert_eq!(normalize_rect(100.0, 80.0, 10.0, 20.0), expected);
        let point = normalize_rect(7.0, 9.0, 7.0, 9.0);
        assert_eq!((point.width, point.height), (0.0, 0.0));
    }

    #[test]
    fn intersection_stops_at_cell_and_viewport_edges() {
        let cell = RubberRect {
            x: 100.0,
            y: 50.0,
            width: 80.0,
            height: 60.0,
        };
        // A rectangle ending exactly at the cell's left edge does not hit it.
        assert!(!rects_intersect(
            RubberRect {
                x: 20.0,
                y: 50.0,
                width: 80.0,
                height: 60.0,
            },
            cell
        ));
        // One pixel into the cell does.
        assert!(rects_intersect(
            RubberRect {
                x: 20.0,
                y: 50.0,
                width: 81.0,
                height: 60.0,
            },
            cell
        ));
        // A drag past the viewport is clipped to its edge, and a cell that
        // begins exactly there stays untouched.
        let clipped = clip_rect(
            RubberRect {
                x: -20.0,
                y: -10.0,
                width: 140.0,
                height: 80.0,
            },
            120.0,
            70.0,
        );
        assert_eq!(
            clipped,
            RubberRect {
                x: 0.0,
                y: 0.0,
                width: 120.0,
                height: 70.0,
            }
        );
        let at_edge = RubberRect {
            x: 120.0,
            y: 0.0,
            width: 30.0,
            height: 30.0,
        };
        assert!(!rects_intersect(clipped, at_edge));
        // The rectangle is clipped, not discarded, when the pointer leaves.
        assert_eq!(
            clip_rect(
                RubberRect {
                    x: 50.0,
                    y: 50.0,
                    width: 300.0,
                    height: 300.0,
                },
                120.0,
                70.0
            ),
            RubberRect {
                x: 50.0,
                y: 50.0,
                width: 70.0,
                height: 20.0,
            }
        );
        assert!(rects_intersect(
            clipped,
            RubberRect {
                x: 119.0,
                y: 0.0,
                width: 30.0,
                height: 30.0,
            }
        ));
    }

    #[test]
    fn the_drag_threshold_is_strict_like_gtk() {
        assert!(!drag_passes_threshold(8.0, 8.0, 8.0));
        assert!(!drag_passes_threshold(-8.0, 0.0, 8.0));
        assert!(drag_passes_threshold(8.1, 0.0, 8.0));
        assert!(drag_passes_threshold(0.0, -8.1, 8.0));
    }

    #[test]
    fn dot_names_are_hidden() {
        assert!(is_hidden_name(Path::new(".git")));
        assert!(is_hidden_name(Path::new("notes/.hidden")));
        assert!(!is_hidden_name(Path::new("notes")));
        assert!(!is_hidden_name(PathBuf::from("file.txt").as_path()));
    }

    #[test]
    fn keyboard_page_distance_uses_the_viewport_not_the_whole_grid() {
        assert_eq!(visible_page_rows(600.0, 100.0), 6);
        assert_eq!(visible_page_rows(650.0, 120.0), 5);
        assert_eq!(visible_page_rows(10.0, 200.0), 1);
        assert_eq!(visible_page_rows(f64::NAN, 20.0), 1);
        assert_eq!(visible_page_rows(400.0, 0.0), 1);
    }

    #[test]
    fn the_keyboard_adapter_keeps_focus_across_model_refreshes_and_range_moves() {
        use fm_core::preview::{
            FileRevision, LoadTicket, Loaded, PreviewContent, PreviewDocument, PreviewSession,
            Selection, TextDocument, TextEncoding, ViewState, WriteAccess,
        };
        use fm_core::workspace::Workspace;

        fn document(ticket: &LoadTicket) -> PreviewDocument {
            PreviewDocument {
                path: ticket.path.clone(),
                name: ticket
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                size: 1,
                revision: FileRevision {
                    device: 1,
                    inode: 2,
                    size: 1,
                    modified_sec: 0,
                    modified_nsec: 0,
                    changed_sec: 0,
                    changed_nsec: 0,
                    etag: None,
                },
                access: WriteAccess {
                    file_writable: true,
                    parent_writable: true,
                    reason: None,
                },
                content: PreviewContent::Text(TextDocument {
                    text: "x".to_string(),
                    encoding: TextEncoding {
                        bom: false,
                        line_ending: Some(fm_core::preview::LineEnding::Lf),
                    },
                }),
            }
        }

        /// The production sequence: choose the target from the explicit cursor,
        /// set the cursor before the synchronous model refresh, apply the model's
        /// selection to the preview, then reapply the cursor to the refreshed
        /// listing.
        fn press(
            focus: &mut KeyboardFocus,
            workspace: &mut Workspace,
            session: &mut PreviewSession,
            names: &[OsString],
            key: GridKey,
            ctrl: bool,
            shift: bool,
        ) -> (u32, NavMode) {
            let window = workspace.focused().unwrap();
            let tab = window.tab(window.focused_tab).unwrap();
            let cursor_position = focus
                .name()
                .and_then(|name| {
                    names
                        .iter()
                        .position(|candidate| candidate.as_os_str() == name)
                })
                .map(|position| position as u32);
            let selected_position = tab
                .selection
                .names
                .iter()
                .find_map(|name| names.iter().position(|candidate| candidate == name))
                .map(|position| position as u32);
            let position = cursor_position.or(selected_position).unwrap_or(0);
            let target = keyboard_move(Some(position), 2, names.len() as u32, key, 2).unwrap();
            let name = names[target as usize].clone();
            focus.set(name.clone());
            let mode = nav_mode(ctrl, shift);
            match mode {
                NavMode::Plain => {
                    workspace.file_click(name).unwrap();
                }
                NavMode::Extend => {
                    workspace.file_shift_click(name, names).unwrap();
                }
                NavMode::Focus => {}
            }
            let window = workspace.focused().unwrap();
            let tab = window.tab(window.focused_tab).unwrap();
            let selection = Selection::from_names(&tab.folder, &tab.selection.names);
            if let Some(ticket) = session.select_user(selection) {
                session.load_finished(&ticket, Loaded::Document(document(&ticket)));
            }
            focus.reapply(
                |listed| {
                    names
                        .iter()
                        .any(|candidate| candidate.as_os_str() == listed)
                },
                tab.selection.names.iter().next().cloned(),
            );
            (target, mode)
        }

        let names: Vec<OsString> = ["a", "b", "c", "d", "e"]
            .iter()
            .map(OsString::from)
            .collect();
        let mut workspace = Workspace::with_home(PathBuf::from("/home/kyle"));
        workspace
            .window_new(Some(PathBuf::from("/tmp/gimbal-preview-keyboard")))
            .unwrap();
        let mut session = PreviewSession::new();
        let mut focus = KeyboardFocus::default();

        // Plain Right selects b and previews it. The synchronous refresh must
        // keep the cursor on b, not reset it to the first selected name.
        assert_eq!(
            press(
                &mut focus,
                &mut workspace,
                &mut session,
                &names,
                GridKey::Right,
                false,
                false
            ),
            (1, NavMode::Plain)
        );
        assert_eq!(focus.name(), Some(std::ffi::OsStr::new("b")));

        // Three consecutive Shift moves extend b..c, b..d, b..e. The cursor
        // advances each time; it is not reset to the anchor b.
        for (expected, name) in [(2, "c"), (3, "d"), (4, "e")] {
            assert_eq!(
                press(
                    &mut focus,
                    &mut workspace,
                    &mut session,
                    &names,
                    GridKey::Right,
                    false,
                    true
                ),
                (expected, NavMode::Extend)
            );
            assert_eq!(focus.name(), Some(std::ffi::OsStr::new(name)));
            let window = workspace.focused().unwrap();
            let tab = window.tab(window.focused_tab).unwrap();
            assert_eq!(tab.selection.names.len(), expected as usize);
        }
        assert!(matches!(session.state(), ViewState::Multiple { count: 4 }));

        // Contraction from e returns to d and selects b..d from the same anchor.
        assert_eq!(
            press(
                &mut focus,
                &mut workspace,
                &mut session,
                &names,
                GridKey::Left,
                false,
                true
            ),
            (3, NavMode::Extend)
        );
        assert_eq!(focus.name(), Some(std::ffi::OsStr::new("d")));
        let window = workspace.focused().unwrap();
        let tab = window.tab(window.focused_tab).unwrap();
        assert_eq!(tab.selection.names.len(), 3);
        assert!(tab.selection.names.contains(&OsString::from("b")));
        assert!(!tab.selection.names.contains(&OsString::from("e")));

        // Ctrl+Right moves focus only: the cursor advances to e while the
        // selection and preview stay on the b..d range.
        assert_eq!(
            press(
                &mut focus,
                &mut workspace,
                &mut session,
                &names,
                GridKey::Right,
                true,
                false
            ),
            (4, NavMode::Focus)
        );
        assert_eq!(focus.name(), Some(std::ffi::OsStr::new("e")));
        let window = workspace.focused().unwrap();
        let tab = window.tab(window.focused_tab).unwrap();
        assert_eq!(tab.selection.names.len(), 3);
        assert!(!tab.selection.names.contains(&OsString::from("e")));

        // A re-sort keeps the cursor by name, not by an old position.
        let sorted: Vec<OsString> = ["e", "d", "c", "b", "a"]
            .iter()
            .map(OsString::from)
            .collect();
        let window = workspace.focused().unwrap();
        let tab = window.tab(window.focused_tab).unwrap();
        focus.reapply(
            |listed| {
                sorted
                    .iter()
                    .any(|candidate| candidate.as_os_str() == listed)
            },
            tab.selection.names.iter().next().cloned(),
        );
        assert_eq!(focus.name(), Some(std::ffi::OsStr::new("e")));

        // Removing the focused file falls back deliberately to the selection.
        let without_e: Vec<OsString> = ["a", "b", "c", "d"].iter().map(OsString::from).collect();
        let window = workspace.focused().unwrap();
        let tab = window.tab(window.focused_tab).unwrap();
        focus.reapply(
            |listed| {
                without_e
                    .iter()
                    .any(|candidate| candidate.as_os_str() == listed)
            },
            tab.selection.names.iter().next().cloned(),
        );
        assert_eq!(focus.name(), Some(std::ffi::OsStr::new("b")));

        // A second view (single/split tiles) has its own focus; moving one does
        // not move the other.
        let mut other = KeyboardFocus::default();
        other.set(OsString::from("a"));
        assert_eq!(focus.name(), Some(std::ffi::OsStr::new("b")));
        assert_eq!(other.name(), Some(std::ffi::OsStr::new("a")));
        assert_eq!(nav_mode(false, false), NavMode::Plain);
        assert_eq!(nav_mode(true, false), NavMode::Focus);
        assert_eq!(nav_mode(false, true), NavMode::Extend);
    }
}
