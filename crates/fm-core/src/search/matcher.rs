// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic filename matching and ranking for the search walker.
//!
//! The matcher has no locale, model or external process. Valid UTF-8 names are
//! compared through `to_lowercase` (simple Unicode lowercase, not full case
//! folding or normalisation). Invalid UTF-8 names keep their native bytes and
//! use ASCII case folding only. Matching never looks at file contents or the
//! parent path.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

/// The most query scalars that may take part in fuzzy matching. Longer queries
/// still get exact/prefix/substring checks over their full text.
pub const MAX_FUZZY_QUERY_SCALARS: usize = 64;
/// The most scalars one fuzzy candidate (name, stem or token) may contain.
pub const MAX_FUZZY_CANDIDATE_SCALARS: usize = 256;
/// A per-name budget for fuzzy scalar work, so a pathological name with many
/// tokens cannot make one entry dominate the scan. Literal matching is never
/// limited by this budget.
const MAX_FUZZY_SCALARS_PER_NAME: usize = 8_192;

/// The confidence class of one match. Declaration order is rank order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchClass {
    ExactName,
    ExactStem,
    Prefix,
    Substring,
    Fuzzy,
}

/// The stable, inspectable score tuple. It is ordered lexicographically:
/// `(class, edits, gaps, start)`. The walker resolves remaining ties by the
/// native relative path, never by discovery order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MatchScore {
    pub class: MatchClass,
    /// Edit count for typo matches; zero for literal and subsequence matches.
    pub edits: u16,
    /// Unmatched scalars inside a subsequence's tightest observed span.
    pub gaps: u32,
    /// Scalar offset of the match (byte offset for native invalid UTF-8).
    pub start: u32,
}

impl MatchScore {
    fn literal(class: MatchClass, start: usize) -> Self {
        Self {
            class,
            edits: 0,
            gaps: 0,
            start: u32::try_from(start).unwrap_or(u32::MAX),
        }
    }
}

/// One submitted query, prepared once for every candidate in a scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    text: String,
    lowered: String,
    bytes: Vec<u8>,
    chars: Vec<char>,
    fuzzy: bool,
}

impl SearchQuery {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let lowered = text.to_lowercase();
        let chars: Vec<char> = lowered.chars().collect();
        let whitespace_only = !text.is_empty() && text.chars().all(char::is_whitespace);
        let fuzzy = chars.len() >= 3 && chars.len() <= MAX_FUZZY_QUERY_SCALARS && !whitespace_only;
        Self {
            bytes: text.as_bytes().to_vec(),
            text,
            lowered,
            chars,
            fuzzy,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// A literal or fuzzy match test without cancellation. This is the
    /// convenient API for unit tests and pure callers.
    pub fn matches_name(&self, name: &OsStr) -> bool {
        self.score_name(name, &|| false).is_some()
    }

    /// Score one basename. `cancel` is checked before candidate work and
    /// periodically inside the bounded fuzzy loops; a cancelled score is
    /// `None`, indistinguishable from "no match" to the walker, which then
    /// stops on its own cancellation check.
    pub fn score_name(&self, name: &OsStr, cancel: &dyn Fn() -> bool) -> Option<MatchScore> {
        if cancel() {
            return None;
        }
        match name.to_str() {
            Some(valid) => self.score_utf8(valid, cancel),
            None => self.score_native_bytes(name.as_bytes()),
        }
    }

    fn score_utf8(&self, name: &str, cancel: &dyn Fn() -> bool) -> Option<MatchScore> {
        if self.text.is_empty() {
            return Some(MatchScore::literal(MatchClass::ExactName, 0));
        }
        let name_lower = name.to_lowercase();
        if name_lower == self.lowered {
            return Some(MatchScore::literal(MatchClass::ExactName, 0));
        }
        let (stem, stem_start) = stem_with_start(&name_lower);
        if stem == self.lowered {
            return Some(MatchScore::literal(MatchClass::ExactStem, 0));
        }
        if name_lower.starts_with(&self.lowered) {
            return Some(MatchScore::literal(MatchClass::Prefix, 0));
        }
        if let Some(byte_start) = name_lower.find(&self.lowered) {
            let start = name_lower[..byte_start].chars().count();
            return Some(MatchScore::literal(MatchClass::Substring, start));
        }
        if !self.fuzzy {
            return None;
        }

        let mut best: Option<MatchScore> = None;
        let mut budget = MAX_FUZZY_SCALARS_PER_NAME;
        if !self.consider_candidate(&name_lower, 0, cancel, &mut best, &mut budget) {
            return None;
        }
        if !self.consider_candidate(stem, stem_start, cancel, &mut best, &mut budget) {
            return None;
        }
        if !self.consider_tokens(&name_lower, cancel, &mut best, &mut budget) {
            return None;
        }
        best
    }

    fn score_native_bytes(&self, name: &[u8]) -> Option<MatchScore> {
        if self.bytes.is_empty() {
            return Some(MatchScore::literal(MatchClass::ExactName, 0));
        }
        if bytes_eq_fold(name, &self.bytes) {
            return Some(MatchScore::literal(MatchClass::ExactName, 0));
        }
        let (stem, _stem_start) = stem_bytes_with_start(name);
        if bytes_eq_fold(stem, &self.bytes) {
            return Some(MatchScore::literal(MatchClass::ExactStem, 0));
        }
        if bytes_prefix_fold(name, &self.bytes) {
            return Some(MatchScore::literal(MatchClass::Prefix, 0));
        }
        bytes_find_fold(name, &self.bytes)
            .map(|start| MatchScore::literal(MatchClass::Substring, start))
    }

    /// Consider one name/stem/token candidate. Returns `false` only when
    /// cancellation was observed; "no match" is `true`.
    fn consider_candidate(
        &self,
        candidate: &str,
        origin: usize,
        cancel: &dyn Fn() -> bool,
        best: &mut Option<MatchScore>,
        budget: &mut usize,
    ) -> bool {
        if cancel() {
            return false;
        }
        let scalar_len = candidate.chars().count();
        if scalar_len > MAX_FUZZY_CANDIDATE_SCALARS {
            return true;
        }
        let cost = scalar_len.max(self.chars.len());
        if cost > *budget {
            return true;
        }
        *budget -= cost;
        if let Some((gaps, start)) = subsequence_span(candidate, &self.chars, cancel) {
            let score = MatchScore {
                class: MatchClass::Fuzzy,
                edits: 0,
                gaps,
                start: u32::try_from(origin.saturating_add(start)).unwrap_or(u32::MAX),
            };
            offer(best, score);
            return true;
        }
        if cancel() {
            return false;
        }
        let max = typo_budget(self.chars.len());
        if let Some(edits) = bounded_osa(candidate, &self.chars, max, cancel) {
            let score = MatchScore {
                class: MatchClass::Fuzzy,
                edits: u16::try_from(edits).unwrap_or(u16::MAX),
                gaps: 0,
                start: u32::try_from(origin).unwrap_or(u32::MAX),
            };
            offer(best, score);
        }
        true
    }

    fn consider_tokens(
        &self,
        name: &str,
        cancel: &dyn Fn() -> bool,
        best: &mut Option<MatchScore>,
        budget: &mut usize,
    ) -> bool {
        let mut token_start_byte: Option<usize> = None;
        let mut token_start_char = 0usize;
        for (char_index, (byte_index, ch)) in name.char_indices().enumerate() {
            if ch.is_alphanumeric() {
                if token_start_byte.is_none() {
                    token_start_byte = Some(byte_index);
                    token_start_char = char_index;
                }
            } else if let Some(start) = token_start_byte.take()
                && !self.consider_candidate(
                    &name[start..byte_index],
                    token_start_char,
                    cancel,
                    best,
                    budget,
                )
            {
                return false;
            }
        }
        if let Some(start) = token_start_byte {
            let _ = self.consider_candidate(&name[start..], token_start_char, cancel, best, budget);
            if cancel() {
                return false;
            }
        }
        true
    }
}

fn offer(best: &mut Option<MatchScore>, score: MatchScore) {
    if best.is_none_or(|current| score < current) {
        *best = Some(score);
    }
}

fn typo_budget(query_len: usize) -> usize {
    if query_len >= 6 { 2 } else { 1 }
}

/// Find the tightest ordered-character span, then the earliest start on ties.
/// Keeping the latest start for each prefix finds later, tighter matches too.
/// Reverse updates prevent one candidate character from filling two slots.
fn subsequence_span(
    candidate: &str,
    query: &[char],
    cancel: &dyn Fn() -> bool,
) -> Option<(u32, usize)> {
    if query.is_empty() {
        return None;
    }
    let mut starts = vec![None; query.len()];
    let mut best = None;
    for (index, ch) in candidate.chars().enumerate() {
        if index % 32 == 0 && cancel() {
            return None;
        }
        for slot in (0..query.len()).rev() {
            if ch == query[slot] {
                if slot == 0 {
                    starts[slot] = Some(index);
                } else if let Some(start) = starts[slot - 1] {
                    starts[slot] = Some(start);
                }
            }
        }
        if ch == query[query.len() - 1]
            && let Some(start) = starts[query.len() - 1]
        {
            let gaps = (index - start + 1).saturating_sub(query.len());
            let score = (u32::try_from(gaps).unwrap_or(u32::MAX), start);
            if best.is_none_or(|current| score < current) {
                best = Some(score);
            }
        }
    }
    best
}

/// Bounded optimal-string-alignment distance: insertion, deletion,
/// substitution and adjacent transposition, with an early length rejection and
/// a diagonal band of `max` cells. Returns `None` above `max` without doing
/// unbounded work.
fn bounded_osa(
    candidate: &str,
    query: &[char],
    max: usize,
    cancel: &dyn Fn() -> bool,
) -> Option<usize> {
    let candidate: Vec<char> = candidate.chars().collect();
    let n = candidate.len();
    let m = query.len();
    if n.abs_diff(m) > max {
        return None;
    }
    if m == 0 {
        return (n <= max).then_some(n);
    }
    let unreachable = max + 1;
    let mut older = vec![unreachable; m + 1];
    let mut previous: Vec<usize> = (0..=m)
        .map(|index| if index <= max { index } else { unreachable })
        .collect();
    let mut current = vec![unreachable; m + 1];
    for row in 1..=n {
        if row % 16 == 0 && cancel() {
            return None;
        }
        current.fill(unreachable);
        if row <= max {
            current[0] = row;
        }
        let low = row.saturating_sub(max).max(1);
        let high = (row + max).min(m);
        for column in low..=high {
            let substitution =
                previous[column - 1] + usize::from(candidate[row - 1] != query[column - 1]);
            let mut value = (previous[column] + 1)
                .min(current[column - 1] + 1)
                .min(substitution);
            if row > 1
                && column > 1
                && candidate[row - 1] == query[column - 2]
                && candidate[row - 2] == query[column - 1]
            {
                value = value.min(older[column - 2] + 1);
            }
            current[column] = value;
        }
        std::mem::swap(&mut older, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    (previous[m] <= max).then_some(previous[m])
}

/// The stem is the name before the last extension dot. A leading dot does not
/// divide an extension (`/.bashrc` has the whole name as its stem).
fn stem_with_start(name: &str) -> (&str, usize) {
    let Some((byte, _)) = name.char_indices().rev().find(|(_, ch)| *ch == '.') else {
        return (name, 0);
    };
    if byte == 0 {
        return (name, 0);
    }
    (&name[..byte], 0)
}

fn stem_bytes_with_start(name: &[u8]) -> (&[u8], usize) {
    let Some(byte) = name.iter().rposition(|ch| *ch == b'.') else {
        return (name, 0);
    };
    if byte == 0 {
        return (name, 0);
    }
    (&name[..byte], byte)
}

fn bytes_eq_fold(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

fn bytes_prefix_fold(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len()
        && haystack[..needle.len()]
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

fn bytes_find_fold(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}
