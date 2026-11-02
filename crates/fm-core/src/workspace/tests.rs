// SPDX-License-Identifier: GPL-3.0-or-later
//! Unit tests for the workspace model.

use super::*;
use std::ffi::OsString;
use std::path::PathBuf;

fn home() -> PathBuf {
    PathBuf::from("/home/kyle")
}

fn workspace() -> Workspace {
    let mut workspace = Workspace::with_home(home());
    workspace
        .window_new(Some(PathBuf::from("/tmp/start")))
        .unwrap();
    workspace
}

fn os(name: &str) -> OsString {
    OsString::from(name)
}

fn folder(name: &str) -> PathBuf {
    PathBuf::from(name)
}

/// Split the first `n` single-tab pills into one group at strip position 0.
fn split_first(workspace: &mut Workspace, n: usize) -> (GroupId, Vec<TabId>) {
    while workspace.focused().unwrap().tab_count() < n {
        workspace
            .tab_new(Some(PathBuf::from("/tmp/extra")))
            .unwrap();
    }
    for index in 0..n {
        workspace.pill_ctrl_click(index).unwrap();
    }
    workspace.group_split().unwrap();
    let window = workspace.focused().unwrap();
    let group_id = window.pills[0];
    let tabs = window.groups[&group_id]
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect();
    (group_id, tabs)
}

#[test]
fn ids_are_stable_and_never_reused() {
    let mut workspace = workspace();
    let first_window = workspace.focused().unwrap().id;
    let first_group = workspace.focused().unwrap().pills[0];
    let first_tab = workspace.focused().unwrap().focused_tab;

    assert_eq!(first_window.raw(), 1);
    assert_eq!(first_group.raw(), 2);
    assert_eq!(first_tab.raw(), 3);

    let added = workspace.tab_new(None).unwrap();
    let added_tab = workspace.focused().unwrap().focused_tab;
    assert!(matches!(
        added.as_slice(),
        [Event::TabAdded { tab, .. }, Event::FocusChanged { .. }] if *tab == added_tab
    ));
    assert!(added_tab.raw() > first_tab.raw());

    workspace.focus_tab(first_tab).unwrap();
    workspace.tab_close().unwrap();
    let replacement = workspace.tab_new(None).unwrap();
    let replacement_tab = workspace.focused().unwrap().focused_tab;
    assert!(replacement_tab.raw() > added_tab.raw());
    assert!(matches!(
        replacement.first(),
        Some(Event::TabAdded { tab, .. }) if *tab == replacement_tab
    ));

    let second_window = workspace.window_new(None).unwrap();
    assert!(matches!(
        second_window.first(),
        Some(Event::WindowOpened(_))
    ));
    workspace.check_invariants().unwrap();
}

#[test]
fn a_fresh_workspace_has_one_focused_tab_and_holds_the_invariants() {
    let workspace = workspace();
    workspace.check_invariants().unwrap();
    let window = workspace.focused().unwrap();
    assert_eq!(window.pills.len(), 1);
    assert_eq!(window.tab_count(), 1);
    assert_eq!(window.groups[&window.pills[0]].tabs.len(), 1);
    assert_eq!(
        window.groups[&window.pills[0]].label(),
        "Group 1".to_string()
    );
}

#[test]
fn window_new_opens_a_tab_and_focuses_it() {
    let mut workspace = Workspace::with_home(home());
    let events = workspace
        .window_new(Some(PathBuf::from("/tmp/opened")))
        .unwrap();
    assert!(matches!(events[0], Event::WindowOpened(_)));
    assert!(matches!(events[1], Event::TabAdded { index: 0, .. }));
    assert!(matches!(events[2], Event::FocusChanged { .. }));

    let window = workspace.focused().unwrap();
    assert_eq!(window.tab_count(), 1);
    assert_eq!(
        window.groups[&window.pills[0]].tabs[0].folder,
        folder("/tmp/opened")
    );
}

#[test]
fn closing_a_window_focuses_another_and_the_last_says_so() {
    let mut workspace = workspace();
    workspace.window_new(None).unwrap();
    let first = workspace.windows[0].id;

    let events = workspace.window_close().unwrap();
    assert!(matches!(events[0], Event::WindowClosed(_)));
    assert!(matches!(events[1], Event::WindowFocused(id) if id == first));
    assert!(matches!(events[2], Event::FocusChanged { window, .. } if window == first));
    assert_eq!(workspace.focused_window, Some(first));

    let events = workspace.window_close().unwrap();
    assert!(matches!(events[0], Event::WindowClosed(_)));
    assert_eq!(events[1], Event::LastWindowClosed);
    assert!(workspace.windows.is_empty());
    assert_eq!(workspace.focused_window, None);
}

#[test]
fn tab_new_opens_after_the_focused_pill_and_inherits_its_folder() {
    let mut workspace = workspace();
    workspace.tab_new(Some(folder("/tmp/a"))).unwrap();
    workspace.tab_new(Some(folder("/tmp/b"))).unwrap();

    workspace.pill_click(0).unwrap();
    let events = workspace.tab_new(None).unwrap();
    let window = workspace.focused().unwrap();
    assert_eq!(window.pills.len(), 4);
    assert!(matches!(events[0], Event::TabAdded { index: 1, .. }));
    let opened = window.pills[1];
    assert_eq!(window.groups[&opened].tabs[0].folder, folder("/tmp/start"));
    assert_eq!(window.groups[&opened].tabs[0].id, window.focused_tab);
}

#[test]
fn tab_new_in_an_empty_workspace_uses_home() {
    let mut workspace = Workspace::with_home(home());
    let events = workspace.tab_new(None).unwrap();
    assert!(matches!(events[0], Event::WindowOpened(_)));
    let window = workspace.focused().unwrap();
    assert_eq!(window.groups[&window.pills[0]].tabs[0].folder, home());
}

#[test]
fn tab_focus_rejects_an_index_past_the_end_and_changes_nothing() {
    let mut workspace = workspace();
    workspace.tab_new(None).unwrap();
    let before = workspace.clone();
    let error = workspace.tab_focus(2).unwrap_err();
    assert_eq!(error, WorkspaceError::IndexOutOfRange { index: 2, len: 2 });
    assert_eq!(workspace, before);
}

#[test]
fn tab_move_reorders_pills_and_rejects_bad_indices() {
    let mut workspace = workspace();
    workspace.tab_new(Some(folder("/tmp/a"))).unwrap();
    workspace.tab_new(Some(folder("/tmp/b"))).unwrap();
    let ids: Vec<TabId> = workspace
        .focused()
        .unwrap()
        .pills
        .iter()
        .map(|group| workspace.focused().unwrap().groups[group].tabs[0].id)
        .collect();

    let events = workspace.tab_move(0, 2).unwrap();
    assert!(matches!(events[0], Event::PillMoved { from: 0, to: 2, .. }));
    let window = workspace.focused().unwrap();
    let moved: Vec<TabId> = window
        .pills
        .iter()
        .map(|group| window.groups[group].tabs[0].id)
        .collect();
    assert_eq!(moved, vec![ids[1], ids[2], ids[0]]);
    assert_eq!(window.focused_tab, ids[0]);

    let before = workspace.clone();
    assert_eq!(
        workspace.tab_move(0, 3).unwrap_err(),
        WorkspaceError::IndexOutOfRange { index: 3, len: 3 }
    );
    assert_eq!(workspace, before);
}

#[test]
fn group_split_inserts_at_the_first_selected_pill() {
    let mut workspace = workspace();
    workspace.tab_new(Some(folder("/tmp/a"))).unwrap();
    workspace.tab_new(Some(folder("/tmp/b"))).unwrap();
    workspace.tab_new(Some(folder("/tmp/c"))).unwrap();
    let before = {
        let window = workspace.focused().unwrap();
        vec![
            window.groups[&window.pills[0]].tabs[0].id,
            window.groups[&window.pills[1]].tabs[0].id,
            window.groups[&window.pills[2]].tabs[0].id,
            window.groups[&window.pills[3]].tabs[0].id,
        ]
    };

    workspace.pill_ctrl_click(1).unwrap();
    workspace.pill_ctrl_click(2).unwrap();
    workspace.group_split().unwrap();

    let window = workspace.focused().unwrap();
    assert_eq!(window.pills.len(), 3);
    assert_eq!(window.groups[&window.pills[0]].tabs[0].id, before[0]);
    let split = &window.groups[&window.pills[1]];
    let split_tabs: Vec<TabId> = split.tabs.iter().map(|tab| tab.id).collect();
    assert_eq!(split_tabs, vec![before[1], before[2]]);
    assert_eq!(window.groups[&window.pills[2]].tabs[0].id, before[3]);
    assert_eq!(window.focused_tab, before[1]);
}

#[test]
fn tab_new_opens_after_a_group_pill_and_not_inside_it() {
    let mut workspace = workspace();
    let (group_id, tabs) = split_first(&mut workspace, 2);
    let tab_count = workspace.focused().unwrap().tab_count();

    workspace.tab_new(Some(folder("/tmp/after-group"))).unwrap();
    let window = workspace.focused().unwrap();
    assert_eq!(window.pills.len(), 2);
    assert_eq!(window.pills[0], group_id);
    assert_eq!(window.groups[&group_id].tabs.len(), 2);
    assert_eq!(window.tab_count(), tab_count + 1);
    assert_eq!(window.groups[&group_id].tabs[0].id, tabs[0]);
    assert_eq!(
        window.groups[&window.pills[1]].tabs[0].folder,
        folder("/tmp/after-group")
    );
    assert_eq!(
        window.groups[&window.pills[1]].tabs[0].id,
        window.focused_tab
    );
}

#[test]
fn closing_the_last_pill_closes_the_window() {
    let mut workspace = workspace();
    let events = workspace.tab_close().unwrap();
    assert!(matches!(events[0], Event::TabClosed { .. }));
    assert!(matches!(events[1], Event::WindowClosed(_)));
    assert_eq!(events[2], Event::LastWindowClosed);
    assert!(workspace.windows.is_empty());
}

#[test]
fn focus_after_a_close_goes_right_else_left() {
    let mut workspace = workspace();
    workspace.tab_new(Some(folder("/tmp/a"))).unwrap();
    workspace.tab_new(Some(folder("/tmp/b"))).unwrap();
    workspace.pill_click(1).unwrap();

    workspace.tab_close().unwrap();
    let window = workspace.focused().unwrap();
    assert_eq!(window.pills.len(), 2);
    assert_eq!(
        window.groups[&window.pills[1]].tabs[0].folder,
        folder("/tmp/b")
    );

    workspace.tab_close().unwrap();
    let window = workspace.focused().unwrap();
    assert_eq!(window.pills.len(), 1);
    assert_eq!(
        window.groups[&window.pills[0]].tabs[0].folder,
        folder("/tmp/start")
    );
}

#[test]
fn closing_a_tab_inside_a_group_focuses_the_next_tile_else_the_previous() {
    let mut workspace = workspace();
    let (_, tabs) = split_first(&mut workspace, 4);
    // The group's tabs are in column order: left [0, 2], right [1, 3].

    workspace.focus_tab(tabs[1]).unwrap();
    workspace.tab_close().unwrap();
    let group = &workspace.focused().unwrap().groups[&workspace.focused().unwrap().pills[0]];
    assert_eq!(group.tabs.len(), 3);
    assert_eq!(workspace.focused().unwrap().focused_tab, tabs[2]);

    workspace.focus_tab(tabs[3]).unwrap();
    workspace.tab_close().unwrap();
    let window = workspace.focused().unwrap();
    let group = &window.groups[&window.pills[0]];
    assert_eq!(group.tabs.len(), 2);
    assert_eq!(window.focused_tab, tabs[2]);
}

#[test]
fn closing_two_tiles_from_a_three_plus_three_group_rebalances_to_two_plus_two() {
    let mut workspace = workspace();
    let (_, tabs) = split_first(&mut workspace, 6);
    // Column order for six: left [0, 2, 4], right [1, 3, 5].

    workspace.focus_tab(tabs[5]).unwrap();
    workspace.tab_close().unwrap();
    let window = workspace.focused().unwrap();
    let group = &window.groups[&window.pills[0]];
    assert_eq!(column_counts(&group.layout.columns), (3, 2));

    workspace.focus_tab(tabs[3]).unwrap();
    workspace.tab_close().unwrap();
    let window = workspace.focused().unwrap();
    let group = &window.groups[&window.pills[0]];
    assert_eq!(column_counts(&group.layout.columns), (2, 2));
    assert_eq!(
        group.layout.columns,
        [vec![tabs[0], tabs[1]], vec![tabs[4], tabs[2]]]
    );
}

#[test]
fn a_hand_made_three_plus_one_survives_a_close_from_the_long_column() {
    let mut workspace = workspace();
    let (group_id, tabs) = split_first(&mut workspace, 4);
    // 2 + 2 from the split: left [0, 2], right [1, 3]. Move 3 into the left
    // column's bottom gap to make a hand layout of 3 + 1.
    workspace
        .apply_tile_drop(tabs[3], Drop::Insert { column: 0, row: 2 })
        .unwrap();
    assert_eq!(
        column_counts(
            &workspace.focused().unwrap().groups[&group_id]
                .layout
                .columns
        ),
        (3, 1)
    );

    // Close from the long column: 2 + 1 is only one apart, so it stays.
    workspace.focus_tab(tabs[1]).unwrap();
    workspace.tab_close().unwrap();
    assert_eq!(
        column_counts(
            &workspace.focused().unwrap().groups[&group_id]
                .layout
                .columns
        ),
        (2, 1)
    );
}

#[test]
fn group_split_merges_selected_groups_and_needs_two_tabs() {
    let mut workspace = workspace();
    workspace.tab_new(Some(folder("/tmp/a"))).unwrap();
    workspace.tab_new(Some(folder("/tmp/b"))).unwrap();
    workspace.tab_new(Some(folder("/tmp/c"))).unwrap();
    let (first_group, first_tabs) = split_first(&mut workspace, 2);
    let window = workspace.focused().unwrap();
    assert_eq!(window.pills.len(), 3);
    assert!(window.groups[&first_group].is_split());
    let single_tab = window.groups[&window.pills[1]].tabs[0].id;
    let last_tab = window.groups[&window.pills[2]].tabs[0].id;

    workspace.pill_ctrl_click(0).unwrap();
    workspace.pill_ctrl_click(1).unwrap();
    let events = workspace.group_split().unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::GroupFormed { .. }))
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::GroupDissolved { .. }))
            .count(),
        2
    );

    let window = workspace.focused().unwrap();
    assert_eq!(window.pills.len(), 2);
    let merged = &window.groups[&window.pills[0]];
    let merged_tabs: Vec<TabId> = merged.tabs.iter().map(|tab| tab.id).collect();
    let merged_order = vec![first_tabs[0], first_tabs[1], single_tab];
    let expected_columns = initial_columns(&merged_order);
    assert_eq!(
        merged_tabs,
        expected_columns[0]
            .iter()
            .chain(expected_columns[1].iter())
            .copied()
            .collect::<Vec<_>>()
    );
    assert_eq!(window.focused_tab, first_tabs[0]);
    assert_eq!(merged.layout.columns, expected_columns);
    assert!(window.selection.selected.iter().all(|selected| !selected));

    // One selected tab is not a split.
    workspace.pill_ctrl_click(1).unwrap();
    let before = workspace.clone();
    assert_eq!(
        workspace.group_split().unwrap_err(),
        WorkspaceError::NotEnoughTabs { selected: 1 }
    );
    assert_eq!(workspace, before);
    let _ = last_tab;
}

#[test]
fn group_split_uses_the_lowest_free_number() {
    let mut workspace = workspace();
    workspace.tab_new(None).unwrap();
    workspace.tab_new(None).unwrap();
    let window = workspace.focused().unwrap();
    assert_eq!(
        window
            .groups
            .values()
            .map(|group| group.number)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );

    workspace.pill_ctrl_click(0).unwrap();
    workspace.pill_ctrl_click(1).unwrap();
    workspace.group_split().unwrap();
    let window = workspace.focused().unwrap();
    let group = &window.groups[&window.pills[0]];
    assert_eq!(group.number, 1);
    assert_eq!(group.title(), "Group 1");
}

#[test]
fn group_close_closes_every_tab_and_focuses_the_neighbour() {
    let mut workspace = workspace();
    let (_, tabs) = split_first(&mut workspace, 3);
    workspace.tab_new(Some(folder("/tmp/after"))).unwrap();
    workspace.pill_click(0).unwrap();

    let events = workspace.group_close().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::TabClosed { .. }))
            .count(),
        3
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::GroupDissolved { .. }))
    );
    let window = workspace.focused().unwrap();
    assert_eq!(window.pills.len(), 1);
    assert_eq!(
        window.groups[&window.pills[0]].tabs[0].folder,
        folder("/tmp/after")
    );
    assert_eq!(
        window.focused_tab,
        window.groups[&window.pills[0]].tabs[0].id
    );
    let _ = tabs;
}

#[test]
fn group_rename_trims_and_rejects_an_empty_name() {
    let mut workspace = workspace();
    let group_id = workspace.focused().unwrap().pills[0];
    let events = workspace.group_rename("  Work  ").unwrap();
    assert!(matches!(events[0], Event::GroupRenamed { .. }));
    assert_eq!(
        workspace.focused().unwrap().groups[&group_id].name,
        Some("Work".to_string())
    );
    assert_eq!(
        workspace.focused().unwrap().groups[&group_id].label(),
        "Work".to_string()
    );

    let before = workspace.clone();
    assert_eq!(
        workspace.group_rename("   ").unwrap_err(),
        WorkspaceError::EmptyGroupName
    );
    assert_eq!(workspace, before);
}

#[test]
fn group_add_tab_joins_a_single_pill_into_a_new_split() {
    let mut workspace = workspace();
    workspace.tab_new(Some(folder("/tmp/second"))).unwrap();
    let tabs = {
        let window = workspace.focused().unwrap();
        vec![
            window.groups[&window.pills[0]].tabs[0].id,
            window.groups[&window.pills[1]].tabs[0].id,
        ]
    };
    workspace.focus_tab(tabs[1]).unwrap();

    let events = workspace.group_add_tab(0).unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::TabMoved { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::GroupFormed { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::GroupDissolved { .. }))
    );

    let window = workspace.focused().unwrap();
    assert_eq!(window.pills.len(), 1);
    let group = &window.groups[&window.pills[0]];
    assert_eq!(group.tabs.len(), 2);
    assert_eq!(group.layout.columns, [vec![tabs[0]], vec![tabs[1]]]);
    assert_eq!(window.focused_tab, tabs[1]);
}

#[test]
fn group_add_tab_uses_the_shorter_column_and_resets_that_columns_heights() {
    let mut workspace = workspace();
    let (group_id, tabs) = split_first(&mut workspace, 4);
    workspace.tab_new(Some(folder("/tmp/outside"))).unwrap();
    let outside = workspace.focused().unwrap().focused_tab;

    workspace.pill_click(0).unwrap();
    workspace
        .set_column_heights(0, Heights::Hand(vec![100, 200]))
        .unwrap();
    workspace.focus_tab(outside).unwrap();

    let events = workspace.group_add_tab(0).unwrap();
    let window = workspace.focused().unwrap();
    let group = &window.groups[&group_id];
    assert_eq!(column_counts(&group.layout.columns), (3, 2));
    assert_eq!(group.layout.heights[0], Heights::Default);
    assert!(group.layout.columns[0].contains(&outside));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::LayoutChanged { group, .. } if *group == group_id))
    );
    let _ = tabs;
}

#[test]
fn tab_close_selected_closes_the_selected_pills_and_focuses_right_else_left() {
    let mut workspace = workspace();
    workspace.tab_new(Some(folder("/tmp/a"))).unwrap();
    workspace.tab_new(Some(folder("/tmp/b"))).unwrap();
    workspace.tab_new(Some(folder("/tmp/c"))).unwrap();
    workspace.pill_ctrl_click(1).unwrap();
    workspace.pill_ctrl_click(2).unwrap();

    let events = workspace.tab_close_selected().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::TabClosed { .. }))
            .count(),
        2
    );
    let window = workspace.focused().unwrap();
    assert_eq!(window.pills.len(), 2);
    assert_eq!(
        window.groups[&window.pills[1]].tabs[0].folder,
        folder("/tmp/c")
    );
    assert_eq!(
        window.focused_tab,
        window.groups[&window.pills[1]].tabs[0].id
    );
    assert!(window.selection.selected.iter().all(|selected| !selected));
}

#[test]
fn tab_open_in_new_window_moves_the_tab_out() {
    let mut workspace = workspace();
    let (group_id, tabs) = split_first(&mut workspace, 2);
    let source_window = workspace.focused().unwrap().id;
    workspace.focus_tab(tabs[0]).unwrap();
    workspace.record_scroll_anchor(Some(os("top"))).unwrap();

    let events = workspace.tab_open_in_new_window().unwrap();
    assert_eq!(workspace.windows.len(), 2);
    let source = workspace
        .windows
        .iter()
        .find(|window| window.id == source_window)
        .unwrap();
    assert_eq!(source.groups[&group_id].tabs.len(), 1);
    assert_eq!(source.groups[&group_id].tabs[0].id, tabs[1]);
    assert_eq!(source.focused_tab, tabs[1]);

    let focused = workspace.focused().unwrap();
    assert_ne!(focused.id, source_window);
    assert_eq!(focused.groups[&focused.pills[0]].tabs[0].id, tabs[0]);
    assert_eq!(
        focused.groups[&focused.pills[0]].tabs[0].scroll_anchor,
        Some(os("top"))
    );
    assert!(matches!(events[0], Event::TabMoved { tab, .. } if tab == tabs[0]));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::WindowOpened(_)))
    );
    assert_eq!(focused.focused_tab, tabs[0]);
    workspace.check_invariants().unwrap();
}

#[test]
fn nav_go_pushes_history_and_clears_selection_and_anchor() {
    let mut workspace = workspace();
    workspace.file_click(os("keep")).unwrap();
    workspace.file_click(os("keep")).unwrap();
    workspace.record_scroll_anchor(Some(os("keep"))).unwrap();

    let events = workspace.nav_go(folder("/tmp/a"), None).unwrap();
    assert!(matches!(events[0], Event::Navigated { .. }));
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.folder, folder("/tmp/a"));
    assert_eq!(tab.back, vec![folder("/tmp/start")]);
    assert!(tab.forward.is_empty());
    assert!(tab.selection.names.is_empty());
    assert_eq!(tab.selection.focused, None);
    assert_eq!(tab.selection.anchor, None);
    assert_eq!(tab.scroll_anchor, None);

    let before = workspace.clone();
    assert_eq!(
        workspace.nav_go(folder("/tmp/a"), None).unwrap(),
        Vec::new()
    );
    assert_eq!(workspace, before);
}

#[test]
fn nav_go_to_a_file_goes_to_its_folder_and_selects_the_name() {
    let mut workspace = workspace();
    workspace
        .nav_go(folder("/tmp/files"), Some(os("report.pdf")))
        .unwrap();
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.folder, folder("/tmp/files"));
    assert_eq!(tab.selection.names, BTreeSet::from([os("report.pdf")]));
    assert_eq!(tab.selection.focused, Some(os("report.pdf")));
    assert_eq!(tab.selection.anchor, Some(os("report.pdf")));
}

#[test]
fn nav_back_and_forward_move_between_folders_and_do_nothing_on_empty_stacks() {
    let mut workspace = workspace();
    workspace.nav_go(folder("/tmp/a"), None).unwrap();
    workspace.nav_go(folder("/tmp/b"), None).unwrap();

    workspace.nav_back().unwrap();
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.folder, folder("/tmp/a"));
    assert_eq!(tab.forward, vec![folder("/tmp/b")]);

    workspace.nav_back().unwrap();
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.folder, folder("/tmp/start"));
    let before = workspace.clone();
    assert_eq!(workspace.nav_back().unwrap(), Vec::new());
    assert_eq!(workspace, before);

    workspace.nav_forward().unwrap();
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.folder, folder("/tmp/a"));
}

#[test]
fn history_stacks_are_capped_at_one_hundred_dropping_the_oldest() {
    let mut workspace = workspace();
    for index in 0..105 {
        workspace
            .nav_go(folder(&format!("/tmp/{index}")), None)
            .unwrap();
    }
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.back.len(), HISTORY_LIMIT);
    assert_eq!(tab.back.first(), Some(&folder("/tmp/4")));
    assert_eq!(tab.back.last(), Some(&folder("/tmp/103")));

    for _ in 0..105 {
        workspace.nav_back().unwrap();
    }
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.forward.len(), HISTORY_LIMIT);
    assert_eq!(tab.back.len(), 0);
}

#[test]
fn nav_up_stops_at_the_root() {
    let mut workspace = workspace();
    workspace.nav_go(folder("/tmp/a/b"), None).unwrap();
    workspace.nav_up().unwrap();
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.folder, folder("/tmp/a"));

    workspace.nav_up().unwrap();
    workspace.nav_up().unwrap();
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.folder, folder("/"));

    let before = workspace.clone();
    assert_eq!(workspace.nav_up().unwrap(), Vec::new());
    assert_eq!(workspace, before);
}

#[test]
fn selection_all_uses_the_handlers_names_and_none_clears_them() {
    let mut workspace = workspace();
    let names = [os("a"), os("b"), os("c")];
    let events = workspace.selection_all(&names).unwrap();
    assert!(matches!(events[0], Event::SelectionChanged { .. }));
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.selection.names, names.iter().cloned().collect());
    assert_eq!(tab.selection.focused, None);

    workspace.selection_none().unwrap();
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert!(tab.selection.names.is_empty());
    assert_eq!(tab.selection.anchor, None);
}

#[test]
fn file_clicks_follow_the_strip_rules() {
    let mut workspace = workspace();
    let order = [os("a"), os("b"), os("c"), os("d")];

    workspace.file_click(os("b")).unwrap();
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.selection.names, BTreeSet::from([os("b")]));
    assert_eq!(tab.selection.focused, Some(os("b")));
    assert_eq!(tab.selection.anchor, Some(os("b")));

    workspace.file_ctrl_click(os("d")).unwrap();
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.selection.names, BTreeSet::from([os("b"), os("d")]));
    assert_eq!(tab.selection.anchor, Some(os("b")));

    workspace.file_shift_click(os("c"), &order).unwrap();
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.selection.names, BTreeSet::from([os("b"), os("c")]));
    assert_eq!(tab.selection.focused, Some(os("c")));
    assert_eq!(tab.selection.anchor, Some(os("b")));

    workspace.file_click(os("d")).unwrap();
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.selection.names, BTreeSet::from([os("d")]));
    assert_eq!(tab.selection.anchor, Some(os("d")));
}

#[test]
fn pill_clicks_follow_the_strip_rules() {
    let mut workspace = workspace();
    workspace.tab_new(None).unwrap();
    workspace.tab_new(None).unwrap();
    workspace.tab_new(None).unwrap();
    let window = workspace.focused().unwrap();
    let tabs: Vec<TabId> = window
        .pills
        .iter()
        .map(|group| window.groups[group].tabs[0].id)
        .collect();

    workspace.pill_click(0).unwrap();
    workspace.pill_ctrl_click(2).unwrap();
    let window = workspace.focused().unwrap();
    assert_eq!(window.selection.selected, [false, false, true, false]);
    assert_eq!(window.selection.anchor, 0);
    assert_eq!(window.focused_tab, tabs[2]);

    workspace.pill_shift_click(3).unwrap();
    let window = workspace.focused().unwrap();
    assert_eq!(window.selection.selected, [true, true, true, true]);
    assert_eq!(window.selection.anchor, 0);
    assert_eq!(window.focused_tab, tabs[3]);

    workspace.pill_click(1).unwrap();
    let window = workspace.focused().unwrap();
    assert!(window.selection.selected.iter().all(|selected| !selected));
    assert_eq!(window.selection.anchor, 1);
    assert_eq!(window.focused_tab, tabs[1]);
}

#[test]
fn the_scroll_anchor_is_recorded_and_a_repeat_is_a_no_op() {
    let mut workspace = workspace();
    let events = workspace.record_scroll_anchor(Some(os("first"))).unwrap();
    assert!(matches!(events[0], Event::ScrollAnchorChanged { .. }));
    let tab = workspace
        .focused()
        .unwrap()
        .tab(workspace.focused().unwrap().focused_tab)
        .unwrap();
    assert_eq!(tab.scroll_anchor, Some(os("first")));

    let before = workspace.clone();
    assert_eq!(
        workspace.record_scroll_anchor(Some(os("first"))).unwrap(),
        Vec::new()
    );
    assert_eq!(workspace, before);
}

#[test]
fn a_tile_drop_swaps_reorders_and_inserts() {
    let mut workspace = workspace();
    let (group_id, tabs) = split_first(&mut workspace, 4);
    let window = workspace.focused().unwrap();
    assert_eq!(
        window.groups[&group_id].layout.columns,
        [vec![tabs[0], tabs[1]], vec![tabs[2], tabs[3]]]
    );

    let events = workspace
        .apply_tile_drop(tabs[0], Drop::Swap(tabs[2]))
        .unwrap();
    assert!(matches!(events[0], Event::LayoutChanged { .. }));
    let group = &workspace.focused().unwrap().groups[&group_id];
    assert_eq!(
        group.layout.columns,
        [vec![tabs[2], tabs[1]], vec![tabs[0], tabs[3]]]
    );
    let order: Vec<TabId> = group.tabs.iter().map(|tab| tab.id).collect();
    assert_eq!(order, vec![tabs[2], tabs[1], tabs[0], tabs[3]]);

    workspace
        .apply_tile_drop(tabs[1], Drop::Insert { column: 1, row: 2 })
        .unwrap();
    let group = &workspace.focused().unwrap().groups[&group_id];
    assert_eq!(column_counts(&group.layout.columns), (1, 3));
    assert_eq!(
        group.layout.columns,
        [vec![tabs[2]], vec![tabs[0], tabs[3], tabs[1]]]
    );
    assert_eq!(group.layout.heights, [Heights::Default, Heights::Default]);
}

#[test]
fn a_drop_that_would_empty_a_column_is_refused_and_changes_nothing() {
    let mut workspace = workspace();
    let (group_id, tabs) = split_first(&mut workspace, 4);
    workspace
        .apply_tile_drop(tabs[3], Drop::Insert { column: 0, row: 2 })
        .unwrap();
    assert_eq!(
        column_counts(
            &workspace.focused().unwrap().groups[&group_id]
                .layout
                .columns
        ),
        (3, 1)
    );

    let before = workspace.clone();
    assert_eq!(
        workspace
            .apply_tile_drop(tabs[1], Drop::Insert { column: 0, row: 1 })
            .unwrap_err(),
        WorkspaceError::DropRefused
    );
    assert_eq!(workspace, before);

    assert_eq!(
        workspace
            .apply_tile_drop(tabs[0], Drop::Swap(tabs[0]))
            .unwrap_err(),
        WorkspaceError::DropRefused
    );
    assert_eq!(
        workspace
            .apply_tile_drop(tabs[0], Drop::Insert { column: 2, row: 0 })
            .unwrap_err(),
        WorkspaceError::DropRefused
    );
}

#[test]
fn hand_heights_need_one_entry_per_tile_and_the_width_share_is_permille() {
    let mut workspace = workspace();
    let (group_id, tabs) = split_first(&mut workspace, 4);
    let window = workspace.focused().unwrap();
    assert_eq!(window.groups[&group_id].layout.columns[0].len(), 2);

    let events = workspace
        .set_column_heights(0, Heights::Hand(vec![400, 600]))
        .unwrap();
    assert!(matches!(events[0], Event::LayoutChanged { .. }));
    assert_eq!(
        workspace.focused().unwrap().groups[&group_id]
            .layout
            .heights[0],
        Heights::Hand(vec![400, 600])
    );

    let before = workspace.clone();
    assert_eq!(
        workspace
            .set_column_heights(0, Heights::Hand(vec![400]))
            .unwrap_err(),
        WorkspaceError::HeightsLength {
            expected: 2,
            got: 1
        }
    );
    assert_eq!(workspace, before);
    assert_eq!(
        workspace
            .set_column_heights(2, Heights::Default)
            .unwrap_err(),
        WorkspaceError::ColumnOutOfRange(2)
    );
    assert_eq!(
        workspace.set_width_share(1001).unwrap_err(),
        WorkspaceError::WidthShareOutOfRange(1001)
    );

    workspace.set_width_share(750).unwrap();
    assert_eq!(
        workspace.focused().unwrap().groups[&group_id]
            .layout
            .width_permille,
        750
    );
    let _ = tabs;
}

#[test]
fn the_second_launch_with_no_paths_opens_one_window_at_home() {
    let mut workspace = Workspace::with_home(home());
    let events = workspace.open_request(&[]).unwrap();
    assert!(matches!(events[0], Event::WindowOpened(_)));
    assert_eq!(workspace.windows.len(), 1);
    let window = workspace.focused().unwrap();
    assert_eq!(window.tab_count(), 1);
    assert_eq!(window.groups[&window.pills[0]].tabs[0].folder, home());
}

#[test]
fn the_second_launch_with_folders_opens_one_tab_each_in_the_focused_window() {
    let mut workspace = Workspace::with_home(home());
    let events = workspace
        .open_request(&[folder("/tmp/a"), folder("/tmp/b"), folder("/tmp/c")])
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, Event::TabAdded { .. }))
            .count(),
        3
    );
    assert_eq!(workspace.windows.len(), 1);
    let window = workspace.focused().unwrap();
    assert_eq!(window.pills.len(), 3);
    let folders: Vec<PathBuf> = window
        .pills
        .iter()
        .map(|group| window.groups[group].tabs[0].folder.clone())
        .collect();
    assert_eq!(
        folders,
        vec![folder("/tmp/a"), folder("/tmp/b"), folder("/tmp/c")]
    );
    assert_eq!(
        window.groups[&window.pills[2]].tabs[0].id,
        window.focused_tab
    );

    // With a window already open, the tabs still land in that window.
    let window_id = window.id;
    workspace
        .open_request(&[folder("/tmp/d"), folder("/tmp/e")])
        .unwrap();
    assert_eq!(workspace.windows.len(), 1);
    let window = workspace.focused().unwrap();
    assert_eq!(window.id, window_id);
    assert_eq!(window.pills.len(), 5);
    assert_eq!(
        window.groups[&window.pills[4]].tabs[0].folder,
        folder("/tmp/e")
    );
}

#[test]
fn a_failed_operation_changes_nothing() {
    let mut workspace = workspace();
    workspace.tab_new(None).unwrap();

    let before = workspace.clone();
    assert!(workspace.tab_focus(99).is_err());
    assert_eq!(workspace, before);

    assert!(workspace.tab_move(0, 99).is_err());
    assert_eq!(workspace, before);

    assert!(workspace.group_add_tab(99).is_err());
    assert_eq!(workspace, before);

    assert!(workspace.group_rename("").is_err());
    assert_eq!(workspace, before);

    assert!(workspace.set_width_share(1001).is_err());
    assert_eq!(workspace, before);
}

struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            self.next_u32() as usize % n
        }
    }
}

#[test]
fn a_long_random_sequence_keeps_the_invariants() {
    let mut workspace = Workspace::with_home(home());
    workspace
        .window_new(Some(PathBuf::from("/tmp/random")))
        .unwrap();
    let mut lcg = Lcg::new(0x5eed_1234_abcd_0001);
    let folders = [folder("/tmp/a"), folder("/tmp/b"), folder("/tmp/c")];
    let names = [os("alpha"), os("beta"), os("gamma"), os("delta")];

    for step in 0..1500 {
        let window = workspace.focused().cloned();
        let pill_count = window.as_ref().map_or(0, |window| window.pills.len());
        let tabs: Vec<TabId> = window
            .as_ref()
            .map(|window| {
                window
                    .groups
                    .values()
                    .flat_map(|group| group.tabs.iter().map(|tab| tab.id))
                    .collect()
            })
            .unwrap_or_default();
        let first_group = window
            .as_ref()
            .and_then(|window| window.pills.first().copied());
        let action = lcg.below(26);

        match action {
            0 => {
                workspace
                    .window_new(Some(folders[lcg.below(folders.len())].clone()))
                    .ok();
            }
            1 => {
                workspace.window_close().ok();
            }
            2 => {
                workspace
                    .tab_new(Some(folders[lcg.below(folders.len())].clone()))
                    .ok();
            }
            3 => {
                workspace.tab_new(None).ok();
            }
            4 => {
                workspace.tab_close().ok();
            }
            5 if pill_count > 0 => {
                workspace.tab_focus(lcg.below(pill_count)).ok();
            }
            6 if pill_count > 0 => {
                workspace
                    .tab_move(lcg.below(pill_count), lcg.below(pill_count))
                    .ok();
            }
            7 if pill_count > 0 => {
                workspace.group_add_tab(lcg.below(pill_count)).ok();
            }
            8 => {
                workspace.group_close().ok();
            }
            9 => {
                workspace
                    .group_rename(if lcg.below(4) == 0 { " " } else { "Work" })
                    .ok();
            }
            10 => {
                workspace.group_split().ok();
            }
            11 => {
                workspace.tab_close_selected().ok();
            }
            12 => {
                workspace
                    .nav_go(folders[lcg.below(folders.len())].clone(), None)
                    .ok();
            }
            13 => {
                workspace
                    .nav_go(
                        folders[lcg.below(folders.len())].clone(),
                        Some(names[lcg.below(names.len())].clone()),
                    )
                    .ok();
            }
            14 => {
                workspace.nav_back().ok();
            }
            15 => {
                workspace.nav_forward().ok();
            }
            16 => {
                workspace.nav_up().ok();
            }
            17 => {
                workspace.selection_all(&names).ok();
            }
            18 => {
                workspace.selection_none().ok();
            }
            19 if pill_count > 0 => {
                workspace.pill_click(lcg.below(pill_count)).ok();
            }
            20 if pill_count > 0 => {
                workspace.pill_ctrl_click(lcg.below(pill_count)).ok();
            }
            21 if pill_count > 0 => {
                workspace.pill_shift_click(lcg.below(pill_count)).ok();
            }
            22 => {
                workspace.tab_open_in_new_window().ok();
            }
            23 if !tabs.is_empty() => {
                let from = tabs[lcg.below(tabs.len())];
                let drop = if lcg.below(2) == 0 {
                    Drop::Swap(tabs[lcg.below(tabs.len())])
                } else {
                    Drop::Insert {
                        column: lcg.below(2),
                        row: lcg.below(5),
                    }
                };
                workspace.apply_tile_drop(from, drop).ok();
            }
            24 => {
                workspace
                    .open_request(&folders[..=lcg.below(folders.len())])
                    .ok();
            }
            25 => {
                if let Some(group) = first_group {
                    let column = lcg.below(2);
                    let len = window
                        .as_ref()
                        .and_then(|window| window.groups.get(&group))
                        .map(|group| group.layout.columns[column].len())
                        .unwrap_or(0);
                    let heights = if lcg.below(2) == 0 {
                        Heights::Default
                    } else {
                        Heights::Hand(vec![400; if lcg.below(2) == 0 { len } else { len + 1 }])
                    };
                    workspace.set_column_heights(column, heights).ok();
                }
                workspace.set_width_share(lcg.below(1001) as u16).ok();
            }
            _ => {}
        }

        workspace.check_invariants().unwrap_or_else(|error| {
            panic!("step {step} (action {action}) broke an invariant: {error}\n{workspace:#?}");
        });
    }

    assert!(!workspace.windows.is_empty());
}
