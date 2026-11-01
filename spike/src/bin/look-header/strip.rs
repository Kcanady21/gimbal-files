// SPDX-License-Identifier: GPL-3.0-or-later
//! The fake tab/group model behind the pill strip.
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub enum Entry {
    Tab(String),
    Group { name: String, tabs: Vec<String> },
}

impl Entry {
    pub fn label(&self) -> String {
        match self {
            Entry::Tab(name) => name.clone(),
            Entry::Group { name, tabs } => format!("{name} · {} tabs", tabs.len()),
        }
    }

    pub fn tabs(&self) -> usize {
        match self {
            Entry::Tab(_) => 1,
            Entry::Group { tabs, .. } => tabs.len(),
        }
    }

    pub fn is_group(&self) -> bool {
        matches!(self, Entry::Group { .. })
    }
}

#[derive(Debug)]
pub struct Model {
    pub entries: Vec<Entry>,
    pub focused: usize,
    pub selection: BTreeSet<usize>,
    groups: usize,
    next: usize,
}

impl Model {
    pub fn new(tabs: usize) -> Self {
        let mut model = Self {
            entries: Vec::new(),
            focused: 0,
            selection: BTreeSet::new(),
            groups: 0,
            next: 1,
        };
        for _ in 0..tabs {
            model.add_tab();
        }
        model
    }

    pub fn add_tab(&mut self) -> usize {
        let index = self.entries.len();
        self.entries.push(Entry::Tab(format!("Tab {}", self.next)));
        self.next += 1;
        index
    }

    pub fn count(&self) -> usize {
        self.entries.iter().map(Entry::tabs).sum()
    }

    pub fn focus(&mut self, index: usize, extend: bool) {
        if index >= self.entries.len() {
            return;
        }
        if !extend {
            self.focused = index;
            self.selection.clear();
            return;
        }
        let (first, last) = if self.focused <= index {
            (self.focused, index)
        } else {
            (index, self.focused)
        };
        self.selection = (first..=last).collect();
    }

    pub fn toggle(&mut self, index: usize) {
        if index < self.entries.len() && !self.selection.remove(&index) {
            self.selection.insert(index);
        }
    }

    pub fn selected_tabs(&self) -> usize {
        self.selection
            .iter()
            .filter(|index| matches!(self.entries.get(**index), Some(Entry::Tab(_))))
            .count()
    }

    /// Close entries, keeping focus on a neighbour. True if the window is now empty.
    pub fn close(&mut self, indices: &[usize]) -> bool {
        let mut indices: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|index| *index < self.entries.len())
            .collect();
        indices.sort_unstable();
        indices.dedup();
        let focused = self.focused;
        let before = indices.iter().filter(|index| **index < focused).count();
        for index in indices.iter().rev() {
            self.entries.remove(*index);
        }
        self.selection.clear();
        if self.entries.is_empty() {
            return true;
        }
        self.focused = focused.saturating_sub(before).min(self.entries.len() - 1);
        false
    }

    /// Turn selected separate tabs into one group at the first one's slot.
    pub fn group_selected(&mut self) -> Option<usize> {
        let mut indices: Vec<usize> = self
            .selection
            .iter()
            .copied()
            .filter(|index| matches!(self.entries.get(*index), Some(Entry::Tab(_))))
            .collect();
        if indices.len() < 2 {
            return None;
        }
        indices.sort_unstable();
        let first = indices[0];
        let mut tabs: Vec<String> = indices
            .iter()
            .rev()
            .filter_map(|index| match self.entries.remove(*index) {
                Entry::Tab(name) => Some(name),
                _ => None,
            })
            .collect();
        tabs.reverse();
        self.groups += 1;
        self.entries.insert(
            first,
            Entry::Group {
                name: format!("Group {}", self.groups),
                tabs,
            },
        );
        self.selection.clear();
        self.focused = first;
        Some(first)
    }

    pub fn ungroup(&mut self, index: usize) {
        let Some(Entry::Group { tabs, .. }) = self.entries.get(index).cloned() else {
            return;
        };
        self.entries
            .splice(index..=index, tabs.into_iter().map(Entry::Tab));
        self.selection.clear();
        self.focused = index;
    }

    pub fn rename(&mut self, index: usize, name: &str) {
        if let Some(Entry::Group { name: old, .. }) = self.entries.get_mut(index)
            && !name.trim().is_empty()
        {
            *old = name.trim().to_string();
        }
    }

    pub fn move_entry(&mut self, from: usize, shift: isize) {
        if from >= self.entries.len() {
            return;
        }
        let to = (from as isize + shift).clamp(0, self.entries.len() as isize - 1) as usize;
        let entry = self.entries.remove(from);
        self.entries.insert(to, entry);
        self.focused = to;
        self.selection.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_close_and_move() {
        let mut model = Model::new(3);
        model.selection.extend([0, 2]);
        assert_eq!(model.group_selected(), Some(0));
        assert_eq!((model.entries.len(), model.count()), (2, 3));
        assert_eq!(model.entries[0].label(), "Group 1 · 2 tabs");
        model.rename(0, "Work");
        assert_eq!(model.entries[0].label(), "Work · 2 tabs");
        model.ungroup(0);
        assert_eq!((model.entries.len(), model.count()), (3, 3));
        model.focused = 1;
        assert!(!model.close(&[0]) && model.focused == 0);
        model.move_entry(0, 1);
        assert_eq!(
            (model.entries[1].label(), model.focused),
            ("Tab 3".to_string(), 1)
        );
        assert!(!model.close(&[0]));
        assert!(model.close(&[0]));
    }
}
