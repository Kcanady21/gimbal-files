// SPDX-License-Identifier: GPL-3.0-or-later
//! The folder grid's sort comparison and the attributes the listing asks for.
//!
//! The comparison is a pure function over the values the grid binds. Names use
//! the spike's order: case-insensitive and natural, so `file2` comes before
//! `file10`. Folders come first when the rule says so, whatever the direction;
//! the direction reverses the chosen key only, with a natural name tie-break.

use std::cmp::Ordering;

use fm_core::config::{SortBy, SortOrder, SortRule};

/// The values a comparison needs from one `gio::FileInfo`.
#[derive(Clone, Copy, Debug)]
pub struct EntryKey<'a> {
    pub name: &'a str,
    pub is_dir: bool,
    /// Unix seconds; 0 when the attribute is missing.
    pub modified: i64,
    pub size: i64,
    /// The content type for `type` sorts.
    pub kind: &'a str,
}

/// Compare two entries under `rule`.
pub fn compare(a: &EntryKey<'_>, b: &EntryKey<'_>, rule: SortRule) -> Ordering {
    if rule.folders_first {
        match (a.is_dir, b.is_dir) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
    }

    let key = match rule.by {
        SortBy::Name => natural_cmp(a.name, b.name),
        SortBy::Modified => a.modified.cmp(&b.modified),
        SortBy::Size => a.size.cmp(&b.size),
        SortBy::Type => natural_cmp(a.kind, b.kind),
    };
    let key = if rule.order == SortOrder::Descending {
        key.reverse()
    } else {
        key
    };
    if key != Ordering::Equal {
        return key;
    }
    natural_cmp(a.name, b.name)
}

/// Case-insensitive natural comparison. Runs of ASCII digits compare by their
/// number, so `file2` < `file10`; all other characters compare lowercased.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let a = a.to_lowercase();
    let b = b.to_lowercase();
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut ai, mut bi) = (0, 0);
    while ai < ab.len() && bi < bb.len() {
        if ab[ai].is_ascii_digit() && bb[bi].is_ascii_digit() {
            let (a_start, a_end) = number_run(ab, ai);
            let (b_start, b_end) = number_run(bb, bi);
            let a_digits = &ab[a_start..a_end];
            let b_digits = &bb[b_start..b_end];
            let order = a_digits
                .len()
                .cmp(&b_digits.len())
                .then_with(|| a_digits.cmp(b_digits));
            if order != Ordering::Equal {
                return order;
            }
            ai = a_end;
            bi = b_end;
        } else {
            if ab[ai] != bb[bi] {
                return ab[ai].cmp(&bb[bi]);
            }
            ai += 1;
            bi += 1;
        }
    }
    (ab.len() - ai).cmp(&(bb.len() - bi))
}

/// The digits' significant part: leading zeros are skipped so `02` and `2`
/// have the same value.
fn number_run(bytes: &[u8], start: usize) -> (usize, usize) {
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    let mut first = start;
    while first < end && bytes[first] == b'0' {
        first += 1;
    }
    (first, end)
}

/// The attributes a listing needs: name, display name, type and icon, plus the
/// one the current sort reads. `standard::name` is also prepended by GTK
/// itself, but keeping it here makes the list explicit.
pub fn attributes(rule: SortRule) -> String {
    let mut attributes =
        String::from("standard::name,standard::display-name,standard::type,standard::icon");
    match rule.by {
        SortBy::Name => {}
        SortBy::Modified => attributes.push_str(",time::modified"),
        SortBy::Size => attributes.push_str(",standard::size"),
        SortBy::Type => attributes.push_str(",standard::content-type"),
    }
    attributes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(by: SortBy, order: SortOrder, folders_first: bool) -> SortRule {
        SortRule {
            by,
            order,
            folders_first,
        }
    }

    fn entry<'a>(
        name: &'a str,
        is_dir: bool,
        modified: i64,
        size: i64,
        kind: &'a str,
    ) -> EntryKey<'a> {
        EntryKey {
            name,
            is_dir,
            modified,
            size,
            kind,
        }
    }

    #[test]
    fn names_are_case_insensitive_and_natural() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("File2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("alpha", "Alpha"), Ordering::Equal);
        assert_eq!(natural_cmp("a1b2", "a1b10"), Ordering::Less);
        assert_eq!(natural_cmp("a02", "a2"), Ordering::Equal);
        assert_eq!(natural_cmp("Report-9", "report-10"), Ordering::Less);
    }

    #[test]
    fn every_key_works_in_both_orders() {
        let alpha = entry("alpha", false, 10, 100, "image/png");
        let beta = entry("beta", false, 20, 200, "text/plain");

        let asc = rule(SortBy::Name, SortOrder::Ascending, false);
        assert_eq!(compare(&alpha, &beta, asc), Ordering::Less);
        let desc = rule(SortBy::Name, SortOrder::Descending, false);
        assert_eq!(compare(&alpha, &beta, desc), Ordering::Greater);

        let asc = rule(SortBy::Modified, SortOrder::Ascending, false);
        assert_eq!(compare(&alpha, &beta, asc), Ordering::Less);
        let desc = rule(SortBy::Modified, SortOrder::Descending, false);
        assert_eq!(compare(&alpha, &beta, desc), Ordering::Greater);

        let asc = rule(SortBy::Size, SortOrder::Ascending, false);
        assert_eq!(compare(&alpha, &beta, asc), Ordering::Less);
        let desc = rule(SortBy::Size, SortOrder::Descending, false);
        assert_eq!(compare(&alpha, &beta, desc), Ordering::Greater);

        let asc = rule(SortBy::Type, SortOrder::Ascending, false);
        assert_eq!(compare(&alpha, &beta, asc), Ordering::Less);
        let desc = rule(SortBy::Type, SortOrder::Descending, false);
        assert_eq!(compare(&alpha, &beta, desc), Ordering::Greater);
    }

    #[test]
    fn folders_come_first_in_both_orders_and_a_name_breaks_ties() {
        let folder = entry("zulu", true, 0, 0, "");
        let file = entry("alpha", false, 0, 999, "text/plain");
        for order in [SortOrder::Ascending, SortOrder::Descending] {
            for by in [SortBy::Name, SortBy::Modified, SortBy::Size, SortBy::Type] {
                let r = rule(by, order, true);
                assert_eq!(
                    compare(&folder, &file, r),
                    Ordering::Less,
                    "{by:?} {order:?}"
                );
                assert_eq!(compare(&file, &folder, r), Ordering::Greater);
            }
            let no_folders = rule(SortBy::Size, order, false);
            let expected = if order == SortOrder::Ascending {
                Ordering::Less
            } else {
                Ordering::Greater
            };
            assert_eq!(compare(&folder, &file, no_folders), expected);
        }

        let a = entry("Alpha", false, 5, 10, "text/plain");
        let b = entry("beta", false, 5, 10, "text/plain");
        assert_eq!(
            compare(&a, &b, rule(SortBy::Size, SortOrder::Ascending, true)),
            Ordering::Less
        );
        assert_eq!(
            compare(&a, &b, rule(SortBy::Size, SortOrder::Descending, true)),
            Ordering::Less,
            "the name tie-break stays ascending"
        );
    }

    #[test]
    fn only_the_sort_attribute_is_added() {
        let base = attributes(rule(SortBy::Name, SortOrder::Ascending, true));
        assert!(!base.contains("size"));
        assert!(!base.contains("time::modified"));
        assert!(!base.contains("content-type"));

        let size = attributes(rule(SortBy::Size, SortOrder::Ascending, true));
        assert!(size.contains("standard::size"));
        let modified = attributes(rule(SortBy::Modified, SortOrder::Ascending, true));
        assert!(modified.contains("time::modified"));
        let kind = attributes(rule(SortBy::Type, SortOrder::Ascending, true));
        assert!(kind.contains("standard::content-type"));
    }
}
