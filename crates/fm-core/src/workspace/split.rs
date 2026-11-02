// SPDX-License-Identifier: GPL-3.0-or-later
//! The split view's pure rules, ported from the spike's
//! `look-body/split.rs`.
//!
//! Nothing here knows about pixels, rectangles or GTK. It decides which
//! [`TabId`]s each of the two columns holds, which heights a column remembers,
//! where a dragged tile may land, and how a strip or tile selection changes.
//! The spike's rules are kept; only the tile type changed from a contiguous
//! `usize` to a stable [`TabId`], so the spike's renumbering step is gone.

use super::TabId;

/// The two columns, left first, each top to bottom.
pub type Columns = [Vec<TabId>; 2];

/// One column's remembered slot heights.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Heights {
    /// Equal shares of the visible height. The UI works out the pixels.
    #[default]
    Default,
    /// Hand-set heights in unscaled design pixels, one per tile in the
    /// column, top to bottom. The UI scales them.
    Hand(Vec<i32>),
}

/// Where a tile drag would land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drop {
    /// On a tile: across columns the two swap slots, within a column it
    /// reorders (the dragged tile takes the target's row).
    Swap(TabId),
    /// Into a gap: the tile becomes row `row` of `column`.
    Insert { column: usize, row: usize },
}

/// A strip or tile selection: the selected flags and the Shift anchor.
///
/// The focused pill or tile lives elsewhere in the model; this type owns only
/// the flags `selection_after` changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub selected: Vec<bool>,
    pub anchor: usize,
}

impl Selection {
    pub fn new(len: usize) -> Self {
        Self {
            selected: vec![false; len],
            anchor: 0,
        }
    }
}

/// New tiles go to the shorter column (the left one on a tie), so the first
/// distribution is 1 = 1+0, 2 = 1+1, 3 = 2+1, 4 = 2+2, 5 = 3+2, 6 = 3+3.
pub fn add_to_shorter(columns: &mut Columns, tab: TabId) -> usize {
    let column = if columns[0].len() <= columns[1].len() {
        0
    } else {
        1
    };
    columns[column].push(tab);
    column
}

/// The initial distribution of `tabs` by [`add_to_shorter`].
pub fn initial_columns(tabs: &[TabId]) -> Columns {
    let mut columns = [Vec::new(), Vec::new()];
    for tab in tabs {
        add_to_shorter(&mut columns, *tab);
    }
    columns
}

/// The column counts, left first.
pub fn column_counts(columns: &Columns) -> (usize, usize) {
    (columns[0].len(), columns[1].len())
}

/// Remove `tab` from its column and keep both columns populated, with the
/// spike's three rules in order:
///
/// 1. if the close empties a column while the other still has two or more
///    tiles, the other column's last tile moves across;
/// 2. if the columns end two or more tiles apart, the longer column's bottom
///    tile moves across, repeatedly, until they differ by at most one — so
///    3 + 3 closed twice from one column becomes 2 + 2, not 1 + 3;
/// 3. a layout made by hand is left alone otherwise: 3 + 1 stays 3 + 1
///    through a close that only leaves one tile between the columns.
///
/// Unlike the spike, ids are not renumbered: [`TabId`]s are stable.
pub fn columns_without(columns: &Columns, tab: TabId) -> Columns {
    let mut out = columns.clone();
    for column in &mut out {
        column.retain(|id| *id != tab);
    }

    for empty in 0..2 {
        let other = 1 - empty;
        if out[empty].is_empty() && out[other].len() >= 2 {
            let moved = out[other].pop().expect("the other column has two or more");
            out[empty].push(moved);
            break;
        }
    }

    loop {
        let (left, right) = (out[0].len(), out[1].len());
        if left.abs_diff(right) < 2 {
            break;
        }
        let long = if left > right { 0 } else { 1 };
        let short = 1 - long;
        if out[long].len() < 2 {
            break;
        }
        let moved = out[long].pop().expect("the longer column has two or more");
        out[short].push(moved);
    }

    out
}

/// The columns after dropping the tile `from` on the tile `to`.
pub fn columns_after_swap(columns: &Columns, from: TabId, to: TabId) -> Option<Columns> {
    if from == to {
        return None;
    }

    let locate = |tile: TabId| {
        columns.iter().enumerate().find_map(|(column, tiles)| {
            tiles
                .iter()
                .position(|id| *id == tile)
                .map(|row| (column, row))
        })
    };

    let (from_column, from_row) = locate(from)?;
    let (to_column, to_row) = locate(to)?;
    let mut out = columns.clone();
    if from_column == to_column {
        out[from_column].remove(from_row);
        out[from_column].insert(to_row.min(out[from_column].len()), from);
    } else {
        out[from_column][from_row] = to;
        out[to_column][to_row] = from;
    }
    Some(out)
}

/// The columns after inserting the tile `from` at `row` (0..=len) of
/// `column`. Refused (`None`) when the move would leave a column empty while
/// two or more tiles exist, or when it changes nothing.
pub fn columns_after_insert(
    columns: &Columns,
    from: TabId,
    column: usize,
    row: usize,
) -> Option<Columns> {
    if column > 1 {
        return None;
    }

    let total: usize = columns[0].len() + columns[1].len();
    let from_column = (0..2).find(|&c| columns[c].contains(&from))?;
    let pos = columns[from_column].iter().position(|id| *id == from)?;
    if from_column != column && columns[from_column].len() == 1 && total >= 2 {
        return None;
    }

    let mut out = columns.clone();
    out[from_column].remove(pos);

    let mut row = row;
    if from_column == column && pos < row {
        row -= 1;
    }
    let row = row.min(out[column].len());
    out[column].insert(row, from);

    if out[from_column].is_empty() && total >= 2 {
        return None;
    }
    if out == *columns {
        return None;
    }
    Some(out)
}

/// The columns after `drop`; `None` means the drop is refused and the tile
/// slides back.
pub fn apply_drop(columns: &Columns, from: TabId, drop: Drop) -> Option<Columns> {
    match drop {
        Drop::Swap(to) => columns_after_swap(columns, from, to),
        Drop::Insert { column, row } => columns_after_insert(columns, from, column, row),
    }
}

/// The new heights after the columns change: a column that gained or lost a
/// tile goes back to the default; the others keep theirs. A swap or a reorder
/// changes no length, so each column keeps its heights.
pub fn heights_after_change(
    old_columns: &Columns,
    old_heights: &[Heights; 2],
    new_columns: &Columns,
) -> [Heights; 2] {
    [0, 1].map(|column| {
        if old_columns[column].len() == new_columns[column].len() {
            old_heights[column].clone()
        } else {
            Heights::Default
        }
    })
}

/// The spike's selection rules: a plain click clears the selection and moves
/// the anchor, Ctrl toggles one flag without moving the anchor, and Shift
/// selects the range from the anchor to the clicked slot.
pub fn selection_after(state: &Selection, slot: usize, ctrl: bool, shift: bool) -> Selection {
    let mut next = state.clone();
    if slot >= next.selected.len() {
        return next;
    }

    if ctrl {
        next.selected[slot] = !next.selected[slot];
    } else if shift {
        let (from, to) = (state.anchor.min(slot), state.anchor.max(slot));
        next.selected.fill(false);
        for selected in next.selected.iter_mut().take(to + 1).skip(from) {
            *selected = true;
        }
    } else {
        next.selected.fill(false);
        next.anchor = slot;
    }

    next
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(raw: u64) -> TabId {
        TabId::from_raw(raw)
    }

    fn tabs(ids: &[u64]) -> Vec<TabId> {
        ids.iter().map(|raw| tab(*raw)).collect()
    }

    #[test]
    fn initial_columns_fill_the_shorter_column() {
        assert_eq!(initial_columns(&[]), [vec![], vec![]]);
        assert_eq!(initial_columns(&tabs(&[0])), [tabs(&[0]), vec![]]);
        assert_eq!(initial_columns(&tabs(&[0, 1])), [tabs(&[0]), tabs(&[1])]);
        assert_eq!(
            initial_columns(&tabs(&[0, 1, 2])),
            [tabs(&[0, 2]), tabs(&[1])]
        );
        assert_eq!(
            initial_columns(&tabs(&[0, 1, 2, 3])),
            [tabs(&[0, 2]), tabs(&[1, 3])]
        );
        assert_eq!(
            initial_columns(&tabs(&[0, 1, 2, 3, 4])),
            [tabs(&[0, 2, 4]), tabs(&[1, 3])]
        );
        assert_eq!(
            initial_columns(&tabs(&[0, 1, 2, 3, 4, 5])),
            [tabs(&[0, 2, 4]), tabs(&[1, 3, 5])]
        );

        let mut columns = [tabs(&[0]), tabs(&[1, 2])];
        assert_eq!(add_to_shorter(&mut columns, tab(3)), 0);
        assert_eq!(columns, [tabs(&[0, 3]), tabs(&[1, 2])]);

        let mut columns = [tabs(&[0, 1]), tabs(&[2])];
        assert_eq!(add_to_shorter(&mut columns, tab(3)), 1);
        assert_eq!(columns, [tabs(&[0, 1]), tabs(&[2, 3])]);
    }

    #[test]
    fn a_close_rebalances_two_or_more_apart() {
        let six = initial_columns(&tabs(&[0, 1, 2, 3, 4, 5]));
        assert_eq!(column_counts(&six), (3, 3));

        let five = columns_without(&six, tab(5));
        assert_eq!(five, [tabs(&[0, 2, 4]), tabs(&[1, 3])]);
        assert_eq!(column_counts(&five), (3, 2));

        let four = columns_without(&five, tab(3));
        assert_eq!(column_counts(&four), (2, 2));
        assert_eq!(four, [tabs(&[0, 2]), tabs(&[1, 4])]);

        let five = columns_without(&six, tab(4));
        assert_eq!(five, [tabs(&[0, 2]), tabs(&[1, 3, 5])]);
        let four = columns_without(&five, tab(2));
        assert_eq!(column_counts(&four), (2, 2));
    }

    #[test]
    fn a_hand_made_three_plus_one_survives_a_close_from_the_long_column() {
        let hand = [tabs(&[0, 1, 2]), tabs(&[3])];
        let after = columns_without(&hand, tab(2));
        assert_eq!(after, [tabs(&[0, 1]), tabs(&[3])]);
        assert_eq!(column_counts(&after), (2, 1));
    }

    #[test]
    fn closing_the_lone_tile_rebalances() {
        let hand = [tabs(&[0, 1, 2]), tabs(&[3])];
        let after = columns_without(&hand, tab(3));
        assert_eq!(column_counts(&after), (2, 1));
        assert_eq!(after, [tabs(&[0, 1]), tabs(&[2])]);

        let two = columns_without(&initial_columns(&tabs(&[0, 1, 2])), tab(1));
        assert_eq!(two, [tabs(&[0]), tabs(&[2])]);
    }

    #[test]
    fn dropping_on_a_tile_swaps_across_and_reorders_within() {
        let four = initial_columns(&tabs(&[0, 1, 2, 3]));
        assert_eq!(
            columns_after_swap(&four, tab(0), tab(1)).unwrap(),
            [tabs(&[1, 2]), tabs(&[0, 3])]
        );
        assert_eq!(
            columns_after_swap(&four, tab(0), tab(2)).unwrap(),
            [tabs(&[2, 0]), tabs(&[1, 3])]
        );
        assert_eq!(
            columns_after_swap(&four, tab(3), tab(1)).unwrap(),
            [tabs(&[0, 2]), tabs(&[3, 1])]
        );
        assert_eq!(columns_after_swap(&four, tab(2), tab(2)), None);
    }

    #[test]
    fn dropping_in_a_gap_inserts_and_the_column_gets_longer() {
        let four = initial_columns(&tabs(&[0, 1, 2, 3]));
        assert_eq!(
            columns_after_insert(&four, tab(0), 0, 2).unwrap(),
            [tabs(&[2, 0]), tabs(&[1, 3])]
        );
        assert_eq!(columns_after_insert(&four, tab(0), 0, 1), None);

        let moved = columns_after_insert(&four, tab(0), 1, 2).unwrap();
        assert_eq!(moved, [tabs(&[2]), tabs(&[1, 3, 0])]);
        assert_eq!(column_counts(&moved), (1, 3));

        let moved = columns_after_insert(&four, tab(3), 0, 2).unwrap();
        assert_eq!(moved, [tabs(&[0, 2, 3]), tabs(&[1])]);
    }

    #[test]
    fn a_drop_never_empties_a_column() {
        let hand = [tabs(&[0, 1, 2]), tabs(&[3])];
        assert_eq!(columns_after_insert(&hand, tab(3), 0, 1), None);
        assert_eq!(
            apply_drop(&hand, tab(3), Drop::Insert { column: 0, row: 2 }),
            None
        );

        let two = [tabs(&[0, 1]), tabs(&[2])];
        assert!(columns_after_insert(&two, tab(0), 1, 1).is_some());

        let one = [tabs(&[0]), vec![]];
        assert!(columns_after_insert(&one, tab(0), 0, 1).is_none());
    }

    #[test]
    fn a_swap_keeps_heights_while_an_insert_resets_the_changed_column() {
        let four = initial_columns(&tabs(&[0, 1, 2, 3]));
        let hand = [Heights::Hand(vec![700, 300]), Heights::Hand(vec![500, 500])];
        let swapped = columns_after_swap(&four, tab(0), tab(1)).unwrap();
        let kept = heights_after_change(&four, &hand, &swapped);
        assert_eq!(kept[0], hand[0]);
        assert_eq!(kept[1], hand[1]);

        let moved = columns_after_insert(&four, tab(0), 1, 2).unwrap();
        let reset = heights_after_change(&four, &hand, &moved);
        assert_eq!(reset[0], Heights::Default);
        assert_eq!(reset[1], Heights::Default);
    }

    #[test]
    fn plain_click_clears_and_moves_the_anchor() {
        let mut state = Selection::new(4);
        state.selected = vec![true, true, true, true];
        state.anchor = 1;

        let next = selection_after(&state, 2, false, false);
        assert_eq!(next.selected, [false, false, false, false]);
        assert_eq!(next.anchor, 2);
    }

    #[test]
    fn ctrl_click_toggles_without_moving_the_anchor() {
        let mut state = Selection::new(4);
        state.anchor = 0;

        let added = selection_after(&state, 2, true, false);
        assert_eq!(added.selected, [false, false, true, false]);
        assert_eq!(added.anchor, 0);

        let removed = selection_after(&added, 2, true, false);
        assert_eq!(removed.selected, [false, false, false, false]);
        assert_eq!(removed.anchor, 0);
    }

    #[test]
    fn shift_click_selects_the_range_from_the_anchor() {
        let mut state = Selection::new(5);
        state = selection_after(&state, 0, false, false);
        let range = selection_after(&state, 3, false, true);
        assert_eq!(range.selected, [true, true, true, true, false]);
        assert_eq!(range.anchor, 0);

        let back = selection_after(&range, 1, false, true);
        assert_eq!(back.selected, [true, true, false, false, false]);
        assert_eq!(back.anchor, 0);
    }

    #[test]
    fn a_slot_past_the_end_changes_nothing() {
        let mut state = Selection::new(2);
        state.selected = vec![true, false];
        assert_eq!(selection_after(&state, 9, false, false), state);
    }
}
