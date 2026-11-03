// SPDX-License-Identifier: GPL-3.0-or-later
//! The conflict menu as a pure state machine.
//!
//! `fm-core`'s [`Decisions`] already knows how to answer one item, how to
//! answer every remaining one, and when the run is cancelled. This wraps it
//! with the dialog's order and the "Apply this decision to all files" rule so
//! the flow can be tested without a display.

use std::path::PathBuf;

use fm_core::fileops::{Decision, Decisions, ItemKey, Plan};

/// One dialog's worth of information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictQuestion {
    /// The unique item key used to record the answer. For a restore this is
    /// its trash URI, so two versions of one original path differ.
    pub item: ItemKey,
    /// The destination the dialog names, kept separately from the key.
    pub destination: Option<PathBuf>,
    /// Just the name, for the dialog's title.
    pub name: String,
    /// 1-based position in the plan's order.
    pub position: usize,
    /// How many conflicts need a decision in this plan.
    pub total: usize,
    /// The checkbox is only useful when more than one conflict remains.
    pub apply_all: bool,
}

/// A `Decisions` value plus the dialog's progress through it.
#[derive(Debug, Clone)]
pub struct ConflictFlow {
    decisions: Decisions,
    total: usize,
    answered: usize,
    /// The human-readable destination for each key. It never replaces the key.
    destinations: Vec<(ItemKey, Option<PathBuf>)>,
}

impl ConflictFlow {
    pub fn from_plan(plan: &Plan) -> Self {
        let mut destinations = Vec::new();
        let mut keys = Vec::new();
        for step in plan.decisions() {
            let Some(key) = step.decision_key() else {
                continue;
            };
            if !keys.contains(&key) {
                keys.push(key.clone());
                destinations.push((key, step.target.clone()));
            }
        }
        Self {
            decisions: Decisions::new(keys),
            total: destinations.len(),
            answered: 0,
            destinations,
        }
    }

    /// For tests and for a plan the UI has already turned into path items.
    #[cfg(test)]
    pub fn from_items(items: impl IntoIterator<Item = PathBuf>) -> Self {
        let pairs: Vec<(ItemKey, Option<PathBuf>)> = items
            .into_iter()
            .map(|path| (ItemKey::Path(path.clone()), Some(path)))
            .collect();
        Self::from_pairs(pairs)
    }

    #[cfg(test)]
    fn from_pairs(items: impl IntoIterator<Item = (ItemKey, Option<PathBuf>)>) -> Self {
        let mut unique = Vec::new();
        for (key, destination) in items {
            if !unique.iter().any(|(seen, _)| seen == &key) {
                unique.push((key, destination));
            }
        }
        let keys: Vec<ItemKey> = unique.iter().map(|(key, _)| key.clone()).collect();
        Self {
            total: unique.len(),
            decisions: Decisions::new(keys),
            answered: 0,
            destinations: unique,
        }
    }

    #[cfg(test)]
    pub fn total(&self) -> usize {
        self.total
    }

    fn destination(&self, item: &ItemKey) -> Option<PathBuf> {
        self.destinations
            .iter()
            .find(|(key, _)| key == item)
            .and_then(|(_, destination)| destination.clone())
    }

    /// The dialog to show now, or `None` when the flow is finished.
    pub fn question(&self) -> Option<ConflictQuestion> {
        let item = self.decisions.next_unanswered()?.clone();
        let destination = self.destination(&item);
        let name = destination
            .as_deref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| item.display_name());
        Some(ConflictQuestion {
            item,
            destination,
            name,
            position: self.answered + 1,
            total: self.total,
            apply_all: self.total > 1,
        })
    }

    /// Record one answer and return the next question, if any.
    ///
    /// `apply_to_all` follows `Decisions::answer`: earlier, different answers
    /// stay, and every conflict with no answer gets this one. `Cancel` ends
    /// the questions and makes the whole run run nothing.
    pub fn answer(
        &mut self,
        item: &ItemKey,
        decision: Decision,
        apply_to_all: bool,
    ) -> Option<ConflictQuestion> {
        self.decisions.answer(item, decision, apply_to_all);
        if decision == Decision::Cancel || apply_to_all || self.decisions.is_complete() {
            self.answered = self.total;
        } else {
            self.answered += 1;
        }
        self.question()
    }

    pub fn decisions(&self) -> Decisions {
        self.decisions.clone()
    }

    #[cfg(test)]
    pub fn cancelled(&self) -> bool {
        self.decisions.is_cancelled()
    }

    #[cfg(test)]
    pub fn is_complete(&self) -> bool {
        self.decisions.is_complete()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<PathBuf> {
        ["a.txt", "b.txt", "c.txt"]
            .iter()
            .map(PathBuf::from)
            .collect()
    }

    #[test]
    fn three_conflicts_are_answered_one_by_one() {
        let mut flow = ConflictFlow::from_items(items());
        assert_eq!(flow.total(), 3);
        let first = flow.question().unwrap();
        assert_eq!(first.name, "a.txt");
        assert_eq!((first.position, first.total), (1, 3));
        assert!(first.apply_all);

        let second = flow.answer(&first.item, Decision::Skip, false).unwrap();
        assert_eq!(second.name, "b.txt");
        assert_eq!(second.position, 2);
        assert_eq!(
            flow.decisions().decision_for(&first.item),
            Some(Decision::Skip)
        );

        let third = flow
            .answer(&second.item, Decision::KeepBoth, false)
            .unwrap();
        assert_eq!(third.name, "c.txt");
        assert_eq!(third.position, 3);
        assert!(flow.answer(&third.item, Decision::Replace, false).is_none());
        assert!(flow.is_complete());
        assert!(!flow.cancelled());
        assert_eq!(
            flow.decisions().decision_for(&ItemKey::path("b.txt")),
            Some(Decision::KeepBoth)
        );
        assert_eq!(
            flow.decisions().decision_for(&ItemKey::path("c.txt")),
            Some(Decision::Replace)
        );
    }

    #[test]
    fn apply_to_all_partway_answers_the_rest_but_keeps_earlier_answers() {
        let mut flow = ConflictFlow::from_items(items());
        let first = flow.question().unwrap();
        assert_eq!(
            flow.answer(&first.item, Decision::Skip, false)
                .unwrap()
                .name,
            "b.txt"
        );
        let second = flow.question().unwrap();
        assert!(
            flow.answer(&second.item, Decision::KeepBoth, true)
                .is_none()
        );
        let decisions = flow.decisions();
        assert_eq!(
            decisions.decision_for(&ItemKey::path("a.txt")),
            Some(Decision::Skip)
        );
        assert_eq!(
            decisions.decision_for(&ItemKey::path("b.txt")),
            Some(Decision::KeepBoth)
        );
        assert_eq!(
            decisions.decision_for(&ItemKey::path("c.txt")),
            Some(Decision::KeepBoth)
        );
        assert!(flow.is_complete());
    }

    #[test]
    fn cancel_runs_nothing_and_ends_the_questions() {
        let mut flow = ConflictFlow::from_items(items());
        let first = flow.question().unwrap();
        assert!(flow.answer(&first.item, Decision::Cancel, false).is_none());
        assert!(flow.cancelled());
        assert!(flow.is_complete());
        assert!(flow.decisions().is_cancelled());
    }

    #[test]
    fn a_single_conflict_has_no_apply_to_all_checkbox() {
        let mut flow = ConflictFlow::from_items([PathBuf::from("only.txt")]);
        let question = flow.question().unwrap();
        assert!(!question.apply_all);
        assert_eq!((question.position, question.total), (1, 1));
        assert!(
            flow.answer(&question.item, Decision::KeepBoth, false)
                .is_none()
        );
    }

    #[test]
    fn two_trashed_versions_of_one_original_ask_two_questions() {
        let first =
            ItemKey::trash(fm_core::fileops::TrashUri::parse("trash:///first.txt").unwrap());
        let second =
            ItemKey::trash(fm_core::fileops::TrashUri::parse("trash:///second.txt").unwrap());
        let destination = Some(PathBuf::from("/home/k/same.txt"));
        let mut flow = ConflictFlow::from_pairs([
            (first.clone(), destination.clone()),
            (second.clone(), destination.clone()),
        ]);

        let question = flow.question().unwrap();
        assert_eq!(question.name, "same.txt");
        assert_eq!((question.position, question.total), (1, 2));
        assert_eq!(question.destination, destination);

        let next = flow.answer(&question.item, Decision::Skip, false).unwrap();
        assert_eq!(next.name, "same.txt");
        assert_eq!((next.position, next.total), (2, 2));
        assert_eq!(next.item, second);
        assert_eq!(flow.decisions().decision_for(&first), Some(Decision::Skip));
        assert_eq!(flow.decisions().decision_for(&second), None);
        assert!(flow.answer(&next.item, Decision::KeepBoth, false).is_none());
    }
}
