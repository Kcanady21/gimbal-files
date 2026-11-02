# P2d: the workspace model

## What I built

- `crates/fm-core/src/workspace/` (three files):
  - `mod.rs`: the data (`WindowId`, `GroupId`, `TabId`, `Workspace`, `Window`, `Group`, `Tab`,
    `SplitLayout`, `FileSelection`, `Selection`), every `View` operation, the UI-only operations,
    the events, the errors, the second-launch policy and the invariant checker.
  - `split.rs`: the spike's pure split rules with `TabId`s: `Heights`, `Drop`, `Selection`,
    `add_to_shorter`, `initial_columns`, `column_counts`, `columns_without`,
    `columns_after_swap`, `columns_after_insert`, `apply_drop`, `heights_after_change` and
    `selection_after`.
  - `tests.rs`: the model tests, the ported split cases that belong to the model, and the random
    sequence.
- `crates/fm-core/src/lib.rs`: `pub mod workspace;` only.
- `docs/notes/P2d.md`: this file.

No dependency was added and `Cargo.toml` was not touched. `[observed]`

## The data

- **One counter.** `WindowId`, `GroupId` and `TabId` are newtypes over `u64`; every value comes
  from the private `IdSource` in the workspace. Numbers are never reused. `from_raw`/`raw` exist for
  the later session restore. `[observed]`
- **`Workspace`** holds `windows: Vec<Window>` (open order), the most recently focused
  `focused_window`, `$HOME` as `home`, and the counter.
- **`Window`** holds `pills: Vec<GroupId>` in strip order, `groups: BTreeMap<GroupId, Group>`,
  `focused_tab`, and `selection: Selection { selected: Vec<bool>, anchor: usize }`.
- **`Group`** holds `tabs: Vec<Tab>` in column order (left top-to-bottom, then right), a
  `SplitLayout { columns: [Vec<TabId>; 2], heights: [Heights; 2], width_permille: u16 }`, an
  optional `name` and a `number`. `is_split()` is `tabs.len() >= 2`; a one-tab group is a normal
  pill again.
- **`Tab`** holds an absolute `PathBuf` folder, `back`/`forward` `Vec<PathBuf>` capped at
  `HISTORY_LIMIT = 100` from the oldest end, a `FileSelection` (`BTreeSet<OsString>` names plus
  focused name and anchor), and `scroll_anchor: Option<OsString>`.
- Every file name is `OsString`; no path or name is converted to UTF-8 anywhere.
- `Group::number` is chosen when the group is made as the lowest positive number no other group in
  that window uses. Closing or merging a group frees its number; existing groups are not
  renumbered.

## Operations and events

Each command is a method on `Workspace`, taking already-checked values:

```
window_new, window_close
tab_new, tab_close, tab_close_selected, tab_focus, tab_move, tab_open_in_new_window
group_split, group_close, group_rename, group_add_tab
nav_go, nav_back, nav_forward, nav_up
selection_all, selection_none
```

UI-only operations:

```
pill_click, pill_ctrl_click, pill_shift_click
focus_tab
file_click, file_ctrl_click, file_shift_click
record_scroll_anchor
apply_tile_drop
set_column_heights, set_width_share
open_request
```

`focus_tab` is the extra operation a split tile's own pill needs; the brief lists header pill
clicks but not a tile click. It only changes `focused_tab`. `[inferred]`

Every method returns `Result<Vec<Event>, WorkspaceError>`. A refused operation is checked before
mutation, so it changes nothing and returns no events. `Event` has `WindowOpened`, `WindowClosed`,
`WindowFocused`, `LastWindowClosed`, `TabAdded`, `TabClosed`, `TabMoved`, `GroupFormed`,
`GroupDissolved`, `GroupRenamed`, `FocusChanged`, `PillMoved`, `LayoutChanged`,
`SelectionChanged`, `Navigated` and `ScrollAnchorChanged`, in the order the operation caused them.

Choices where the brief leaves room:

- `GroupFormed` means a group now shows as the split view. `GroupDissolved` means a group stopped
  being a split or stopped existing (it fell back to one tab, merged, or closed). This gives the UI
  the fade into and out of the split view. `[inferred]`
- `group.add-tab` keeps the target pill's `GroupId` when a single target becomes a split, so the
  UI's widget binding stays stable. It emits `GroupFormed` for that target. `[inferred]`
- `tab_close_selected` closes every selected pill; a selected split pill therefore closes all its
  tabs. This follows the strip's "selected pills are entries" rule. `[inferred]`
- `nav_back`, `nav_forward` and `nav_up` clear the file selection and scroll anchor as well as
  `nav_go` does: the names belonged to the folder being left. The brief only states the
  `nav_go` rule. `[inferred]`
- `selection_all` leaves the focused name and anchor empty: there is no one focused item in an
  all-selection. `[inferred]`
- `tab.open-in-new-window` on a tab that is its group's last one removes the empty source window,
  emits `WindowClosed`, then opens the new focused window; it is not `LastWindowClosed` because a
  window remains. `[inferred]`
- `set_column_heights` takes a `Heights` and refuses a `Hand` list whose length is not the
  column's tile count. `Default` is always valid. `[observed]`
- `set_width_share` takes permille as `u16` and refuses values above 1000.

## The split rules, ported

`split.rs` keeps the spike's rules and tests, with one deliberate difference: the spike's ids were
contiguous `usize`s and `columns_without` renumbered the survivors. `TabId`s are stable and are
never renumbered, so that loop is gone; the rebalancing rules themselves are unchanged:

1. an empty column is repaired from the other column when the other has two or more tiles;
2. columns two or more apart are rebalanced from the longer column's bottom until they differ by at
   most one;
3. otherwise a hand-made layout is left alone (3 + 1 stays through a close from the long column).

`heights_after_change` resets only a column whose length changed; a swap or same-column reorder
keeps both columns' heights. `apply_drop` refuses a move that would empty a column while two or
more tiles exist. The model uses these functions directly, so there is no second copy of the
rules. `[observed]`

No pixel, rect, scroll-offset or GTK code is in `fm-core`. `Heights::Hand` is in design px, and
the width share is permille, as the brief asks. `[observed]`

## A second launch

`Workspace::open_request(paths)` is the whole policy:

- no paths: `window_new(None)`, a new window at home;
- one or more folders: one `tab_new(Some(folder))` per folder in the most recently focused window,
  so the last folder is focused. If no window is open, the first folder opens the window and the
  rest become tabs in it.

Kyle can change this later by changing this one function; tests pin both branches. `[observed]`

## Invariants and the random test

`Workspace::check_invariants()` checks:

- every present window, group and tab id is unique (one counter);
- each window's `pills` and selected flags have the same length, and its anchor is in range;
- `groups` contains exactly the ids in `pills`, with no duplicate pill;
- each group has tabs, its two columns hold exactly its tabs, and no column is empty when the group
  has two or more tabs;
- a `Heights::Hand` list has one entry per tile in its column;
- each group's width share is 0–1000;
- each window's focused tab belongs to it, and a non-empty workspace has a valid focused window;
- group numbers are unique within a window and history stacks are at most 100.

`a_long_random_sequence_keeps_the_invariants` runs 1,500 operations from a fixed seed through a
tiny LCG (the standard 64-bit multiply/add constants, `wrapping_mul`, `wrapping_add`), mixing
window, tab, group, navigation, selection, drop, height and width operations, valid and invalid,
and calls `check_invariants()` after every step. `[observed]`

## Verification

From `$PROJ`, with `PATH="$T:$PATH"`:

- `scripts/check.sh`: exit 0.
- Workspace tests: 51 (12 in `split.rs`, 39 in `tests.rs`).
- `fm-core`: 120 tests; workspace total: 128 (120 `fm-core`, 7 CLI, 1 agent); 0 failed.
- `cargo fmt --all --check`: no output.
- `cargo clippy --workspace --all-targets -- -D warnings`: finished with no warnings.
- `cargo build --workspace --release`: finished.
- `cargo deny check`: all checks passed; the only warning is the pre-existing duplicate
  `system-deps` 7.0.8/9.0.1 that P2a also had (`multiple-versions = "warn"`).

The full `scripts/check.sh` output follows. The 120 individual `fm-core` test-name lines are
included; the `cargo deny` dependency tree is long, so its lines are the tool's own output too.

```text
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.02s
==> cargo test --workspace
   Compiling fm-core v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-core)
   Compiling fm-agent v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-agent)
   Compiling fm-cli v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-cli)
   Compiling fm-ui v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-ui)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.49s
     Running unittests src/lib.rs (target/debug/deps/fm_agent-cc4ee4bf374e51f2)

running 1 test
test tests::socket_lives_under_the_runtime_directory ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/main.rs (target/debug/deps/gimbal_files_cli-4295b4be5da538bb)

running 7 tests
test tests::anything_else_is_a_usage_error ... ok
test tests::check_keeps_the_command_and_its_arguments ... ok
test tests::commands_lists_the_registry ... ok
test tests::check_without_a_command_is_a_usage_error ... ok
test tests::help_shows_usage ... ok
test tests::version_shows_the_version ... ok
test tests::no_arguments_shows_usage ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (target/debug/deps/fm_core-408d0e2ff853beac)

running 120 tests
test command::tests::a_checked_command_prints_in_catalogue_order ... ok
test command::tests::a_files_command_defaults_dry_run_to_false ... ok
test command::tests::a_forbidden_call_never_reaches_the_handler ... ok
test command::tests::a_token_without_equals_is_an_error ... ok
test command::tests::a_known_command_is_not_an_unknown_command ... ok
test command::tests::a_valid_call_reaches_the_handler_once ... ok
test command::tests::a_single_valued_argument_may_not_repeat ... ok
test command::tests::a_value_may_contain_equals_and_spaces ... ok
test command::tests::an_empty_argument_name_is_an_error ... ok
test command::tests::an_invalid_call_never_reaches_the_handler ... ok
test command::tests::argument_kinds_are_sane ... ok
test command::tests::a_distant_typo_gets_no_suggestion ... ok
test command::tests::argument_names_are_unique_within_a_command ... ok
test command::tests::bool_accepts_true_and_false_only ... ok
test command::tests::choice_must_be_one_of_the_fixed_list ... ok
test command::tests::cli_and_agent_may_not_fill_files_paths_from_context ... ok
test command::tests::context_rules_are_pure_functions_over_every_caller ... ok
test command::tests::cli_and_agent_must_give_every_files_context_path ... ok
test command::tests::every_caller_may_give_explicit_files_paths ... ok
test command::tests::every_files_command_gets_exactly_one_dry_run_added_by_the_registry ... ok
test command::tests::int_accepts_the_range_and_rejects_bad_and_out_of_range_values ... ok
test command::tests::missing_required_argument_is_named ... ok
test command::tests::no_command_deletes_permanently ... ok
test command::tests::names_are_area_dot_verb_in_lowercase_kebab_case ... ok
test command::tests::names_are_unique ... ok
test command::tests::no_tokens_is_a_missing_command ... ok
test command::tests::number_values_are_hundredths_and_print_with_two_places ... ok
test command::tests::opacity_is_a_number_without_a_step ... ok
test command::tests::path_must_be_absolute_and_stays_as_given ... ok
test command::tests::paths_print_once_per_key ... ok
test command::tests::repeated_keys_are_kept_in_order ... ok
test command::tests::repeated_paths_keys_make_a_list_in_input_order ... ok
test command::tests::summaries_are_one_non_empty_line ... ok
test command::tests::scale_accepts_the_step_edges_and_refuses_off_step_and_out_of_range ... ok
test command::tests::text_must_not_be_empty ... ok
test command::tests::the_context_rule_applies_only_to_files_commands ... ok
test command::tests::the_handler_outcome_is_returned_unchanged ... ok
test command::tests::the_sort_scope_defaults_to_folder ... ok
test command::tests::the_v1_catalogue_has_every_reviewed_command_in_order ... ok
test command::tests::tokens_parse_into_a_command_and_arguments ... ok
test command::tests::ui_and_hotkey_may_fill_files_paths_from_context ... ok
test command::tests::undo_is_only_offered_on_files_commands ... ok
test command::tests::unknown_argument_is_named ... ok
test command::tests::usage_lists_arguments_in_catalogue_order ... ok
test command::tests::unknown_command_suggests_the_closest_name ... ok
test config::tests::a_read_error_reports_the_file_and_position ... ok
test config::tests::config_path_is_a_pure_function_of_xdg_config_home_and_home ... ok
test config::tests::every_default_matches_the_brief ... ok
test config::tests::a_hotkeys_table_replaces_listed_keys_and_an_empty_value_unbinds ... ok
test config::tests::an_unparsable_file_is_never_written ... ok
test config::tests::an_unknown_key_is_a_warning_with_its_position ... ok
test config::tests::a_bad_value_falls_back_and_the_good_values_still_load ... ok
test config::tests::every_settings_command_has_a_change_for ... ok
test config::tests::default_pins_include_home_then_only_existing_special_folders ... ok
test config::tests::an_unparsable_file_reports_one_problem_with_a_position ... ok
test config::tests::a_sidebar_and_section_save_keep_their_inline_formatting ... ok
test config::tests::set_scale_and_sort_map_to_the_expected_changes ... ok
test config::tests::a_save_rewrites_only_the_changed_key_on_a_commented_file ... ok
test config::tests::zoom_stops_at_the_scale_limits_and_toggles_flip ... ok
test tests::app_id_matches_the_niri_rule_and_is_a_valid_gapplication_id ... ok
test workspace::split::tests::a_close_rebalances_two_or_more_apart ... ok
test workspace::split::tests::a_drop_never_empties_a_column ... ok
test config::tests::a_hotkey_with_an_unknown_command_or_bad_arguments_keeps_the_default ... ok
test workspace::split::tests::a_hand_made_three_plus_one_survives_a_close_from_the_long_column ... ok
test config::tests::saving_creates_the_folder_and_the_changed_key ... ok
test workspace::split::tests::a_slot_past_the_end_changes_nothing ... ok
test config::tests::saving_is_atomic_and_keeps_permissions ... ok
test workspace::split::tests::a_swap_keeps_heights_while_an_insert_resets_the_changed_column ... ok
test config::tests::pin_changes_write_the_pins_list ... ok
test workspace::split::tests::closing_the_lone_tile_rebalances ... ok
test config::tests::sort_for_expands_home_and_normalizes_dot_dot_and_trailing_slashes ... ok
test config::tests::unknown_keys_and_tables_are_warnings_and_stay_in_the_file ... ok
test workspace::split::tests::dropping_in_a_gap_inserts_and_the_column_gets_longer ... ok
test config::tests::sort_changes_write_one_global_setting_or_one_folder_rule ... ok
test workspace::split::tests::ctrl_click_toggles_without_moving_the_anchor ... ok
test workspace::split::tests::dropping_on_a_tile_swaps_across_and_reorders_within ... ok
test workspace::split::tests::initial_columns_fill_the_shorter_column ... ok
test workspace::split::tests::plain_click_clears_and_moves_the_anchor ... ok
test workspace::split::tests::shift_click_selects_the_range_from_the_anchor ... ok
test workspace::tests::a_fresh_workspace_has_one_focused_tab_and_holds_the_invariants ... ok
test config::tests::every_problem_kind_reports_its_line_and_column ... ok
test workspace::tests::a_failed_operation_changes_nothing ... ok
test config::tests::prints_a_sample_problem_list ... ok
test workspace::tests::a_hand_made_three_plus_one_survives_a_close_from_the_long_column ... ok
test workspace::tests::a_drop_that_would_empty_a_column_is_refused_and_changes_nothing ... ok
test workspace::tests::a_tile_drop_swaps_reorders_and_inserts ... ok
test workspace::tests::closing_a_window_focuses_another_and_the_last_says_so ... ok
test workspace::tests::closing_the_last_pill_closes_the_window ... ok
test workspace::tests::closing_a_tab_inside_a_group_focuses_the_next_tile_else_the_previous ... ok
test workspace::tests::file_clicks_follow_the_strip_rules ... ok
test workspace::tests::focus_after_a_close_goes_right_else_left ... ok
test workspace::tests::closing_two_tiles_from_a_three_plus_three_group_rebalances_to_two_plus_two ... ok
test workspace::tests::group_add_tab_joins_a_single_pill_into_a_new_split ... ok
test workspace::tests::group_rename_trims_and_rejects_an_empty_name ... ok
test workspace::tests::group_add_tab_uses_the_shorter_column_and_resets_that_columns_heights ... ok
test workspace::tests::group_close_closes_every_tab_and_focuses_the_neighbour ... ok
test workspace::tests::group_split_inserts_at_the_first_selected_pill ... ok
test workspace::tests::group_split_merges_selected_groups_and_needs_two_tabs ... ok
test workspace::tests::group_split_uses_the_lowest_free_number ... ok
test workspace::tests::nav_back_and_forward_move_between_folders_and_do_nothing_on_empty_stacks ... ok
test workspace::tests::nav_go_pushes_history_and_clears_selection_and_anchor ... ok
test workspace::tests::hand_heights_need_one_entry_per_tile_and_the_width_share_is_permille ... ok
test workspace::tests::nav_go_to_a_file_goes_to_its_folder_and_selects_the_name ... ok
test workspace::tests::ids_are_stable_and_never_reused ... ok
test workspace::tests::nav_up_stops_at_the_root ... ok
test workspace::tests::history_stacks_are_capped_at_one_hundred_dropping_the_oldest ... ok
test workspace::tests::pill_clicks_follow_the_strip_rules ... ok
test workspace::tests::selection_all_uses_the_handlers_names_and_none_clears_them ... ok
test workspace::tests::tab_close_selected_closes_the_selected_pills_and_focuses_right_else_left ... ok
test workspace::tests::tab_focus_rejects_an_index_past_the_end_and_changes_nothing ... ok
test workspace::tests::tab_new_in_an_empty_workspace_uses_home ... ok
test workspace::tests::tab_move_reorders_pills_and_rejects_bad_indices ... ok
test workspace::tests::tab_new_opens_after_a_group_pill_and_not_inside_it ... ok
test workspace::tests::tab_new_opens_after_the_focused_pill_and_inherits_its_folder ... ok
test workspace::tests::the_scroll_anchor_is_recorded_and_a_repeat_is_a_no_op ... ok
test workspace::tests::tab_open_in_new_window_moves_the_tab_out ... ok
test workspace::tests::the_second_launch_with_no_paths_opens_one_window_at_home ... ok
test workspace::tests::the_second_launch_with_folders_opens_one_tab_each_in_the_focused_window ... ok
test workspace::tests::window_new_opens_a_tab_and_focuses_it ... ok
test workspace::tests::a_long_random_sequence_keeps_the_invariants ... ok

test result: ok. 120 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running unittests src/main.rs (target/debug/deps/gimbal_files-218cc8a9c52eb1b9)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests fm_agent

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests fm_core

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

==> cargo build --workspace --release
   Compiling fm-core v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-core)
   Compiling fm-agent v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-agent)
   Compiling fm-cli v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-cli)
   Compiling fm-ui v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-ui)
    Finished `release` profile [optimized] target(s) in 1.14s
==> cargo deny check
warning[duplicate]: found 2 duplicate entries for crate 'system-deps'
   ┌─ /home/kyle/Documents/03.) Code-Projects/gimbal-files/Cargo.lock:57:1
   │  
57 │ ╭ system-deps 7.0.8 registry+https://github.com/rust-lang/crates.io-index
58 │ │ system-deps 9.0.1 registry+https://github.com/rust-lang/crates.io-index
   │ ╰───────────────────────────────────────────────────────────────────────┘ lock entries
   │  
   ├ system-deps v7.0.8
     └── (build) libadwaita-sys v0.9.2
         └── libadwaita v0.9.2
             └── fm-ui v0.1.0
   ├ system-deps v9.0.1
     ├── (build) cairo-sys-rs v0.22.9
     │   ├── cairo-rs v0.22.9
     │   │   ├── gdk4 v0.11.5
     │   │   │   ├── gsk4 v0.11.5
     │   │   │   │   └── gtk4 v0.11.5
     │   │   │   │       ├── fm-ui v0.1.0
     │   │   │   │       └── libadwaita v0.9.2
     │   │   │   │           └── fm-ui v0.1.0 (*)
     │   │   │   ├── gtk4 v0.11.5 (*)
     │   │   │   └── libadwaita v0.9.2 (*)
     │   │   ├── gsk4 v0.11.5 (*)
     │   │   └── gtk4 v0.11.5 (*)
     │   ├── gdk4-sys v0.11.5
     │   │   ├── gdk4 v0.11.5 (*)
     │   │   ├── gsk4-sys v0.11.5
     │   │   │   ├── gsk4 v0.11.5 (*)
     │   │   │   └── gtk4-sys v0.11.5
     │   │   │       ├── gtk4 v0.11.5 (*)
     │   │   │       └── libadwaita-sys v0.9.2
     │   │   │           └── libadwaita v0.9.2 (*)
     │   │   ├── gtk4-sys v0.11.5 (*)
     │   │   └── libadwaita-sys v0.9.2 (*)
     │   ├── gsk4-sys v0.11.5 (*)
     │   └── gtk4-sys v0.11.5 (*)
     ├── (build) gdk-pixbuf-sys v0.22.9
     │   ├── gdk-pixbuf v0.22.0
     │   │   ├── gdk4 v0.11.5 (*)
     │   │   └── gtk4 v0.11.5 (*)
     │   ├── gdk4-sys v0.11.5 (*)
     │   └── gtk4-sys v0.11.5 (*)
     ├── (build) gdk4-sys v0.11.5 (*)
     ├── (build) gio-sys v0.22.9
     │   ├── gdk-pixbuf-sys v0.22.9 (*)
     │   ├── gdk4-sys v0.11.5 (*)
     │   ├── gio v0.22.10
     │   │   ├── gdk-pixbuf v0.22.0 (*)
     │   │   ├── gdk4 v0.11.5 (*)
     │   │   ├── gtk4 v0.11.5 (*)
     │   │   ├── libadwaita v0.9.2 (*)
     │   │   └── pango v0.22.9
     │   │       ├── gdk4 v0.11.5 (*)
     │   │       ├── gsk4 v0.11.5 (*)
     │   │       ├── gtk4 v0.11.5 (*)
     │   │       └── libadwaita v0.9.2 (*)
     │   ├── glib v0.22.10
     │   │   ├── cairo-rs v0.22.9 (*)
     │   │   ├── fm-core v0.1.0
     │   │   │   ├── fm-agent v0.1.0
     │   │   │   │   └── fm-cli v0.1.0
     │   │   │   ├── fm-cli v0.1.0 (*)
     │   │   │   └── fm-ui v0.1.0 (*)
     │   │   ├── gdk-pixbuf v0.22.0 (*)
     │   │   ├── gdk4 v0.11.5 (*)
     │   │   ├── gio v0.22.10 (*)
     │   │   ├── graphene-rs v0.22.8
     │   │   │   ├── gsk4 v0.11.5 (*)
     │   │   │   └── gtk4 v0.11.5 (*)
     │   │   ├── gsk4 v0.11.5 (*)
     │   │   ├── gtk4 v0.11.5 (*)
     │   │   ├── libadwaita v0.9.2 (*)
     │   │   └── pango v0.22.9 (*)
     │   ├── gtk4-sys v0.11.5 (*)
     │   └── libadwaita-sys v0.9.2 (*)
     ├── (build) glib-sys v0.22.9
     │   ├── cairo-sys-rs v0.22.9 (*)
     │   ├── gdk-pixbuf-sys v0.22.9 (*)
     │   ├── gdk4-sys v0.11.5 (*)
     │   ├── gio-sys v0.22.9 (*)
     │   ├── glib v0.22.10 (*)
     │   ├── gobject-sys v0.22.9
     │   │   ├── gdk-pixbuf-sys v0.22.9 (*)
     │   │   ├── gdk4-sys v0.11.5 (*)
     │   │   ├── gio-sys v0.22.9 (*)
     │   │   ├── glib v0.22.10 (*)
     │   │   ├── gsk4-sys v0.11.5 (*)
     │   │   ├── gtk4-sys v0.11.5 (*)
     │   │   ├── libadwaita-sys v0.9.2 (*)
     │   │   └── pango-sys v0.22.9
     │   │       ├── gdk4-sys v0.11.5 (*)
     │   │       ├── gsk4-sys v0.11.5 (*)
     │   │       ├── gtk4-sys v0.11.5 (*)
     │   │       ├── libadwaita-sys v0.9.2 (*)
     │   │       └── pango v0.22.9 (*)
     │   ├── graphene-sys v0.22.9
     │   │   ├── graphene-rs v0.22.8 (*)
     │   │   ├── gsk4-sys v0.11.5 (*)
     │   │   └── gtk4-sys v0.11.5 (*)
     │   ├── gsk4-sys v0.11.5 (*)
     │   ├── gtk4-sys v0.11.5 (*)
     │   ├── libadwaita-sys v0.9.2 (*)
     │   └── pango-sys v0.22.9 (*)
     ├── (build) gobject-sys v0.22.9 (*)
     ├── (build) graphene-sys v0.22.9 (*)
     ├── (build) gsk4-sys v0.11.5 (*)
     ├── (build) gtk4-sys v0.11.5 (*)
     └── (build) pango-sys v0.22.9 (*)

advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

## API citations

No new crate is used. Everything is `std`:

- `std::ffi::OsString` and `std::collections::BTreeSet<OsString>` keep names without assuming
  UTF-8: <https://doc.rust-lang.org/std/ffi/struct.OsString.html>,
  <https://doc.rust-lang.org/std/collections/struct.BTreeSet.html>. `[read]`
- `std::path::Path::parent` for `nav_up`, and `PathBuf` equality for "same folder":
  <https://doc.rust-lang.org/std/path/struct.Path.html#method.parent>. `[read]`
- `u64::wrapping_mul` / `u64::wrapping_add` for the test LCG:
  <https://doc.rust-lang.org/std/primitive.u64.html#method.wrapping_mul>. `[read]`
- `std::env::var_os("HOME")` for the fallback home; the test suite uses
  `Workspace::with_home` so no test depends on the environment. `[read]`
- `BTreeMap::from([...])` and `let ... else` are stable in the pinned 1.95.0 / edition 2024.
  `[observed]`

No GTK, GIO or other crate name appears in `workspace/`; `fm-core`'s existing `glib` dependency is
untouched. `[observed]`

## What needs a live session

Nothing. The workspace model is plain data and pure functions; every rule is covered by unit tests
and the random invariant test. There is no widget, no event loop and no filesystem access in this
module, so there is nothing to click. `[inferred]`

## Anything outside my files

Nothing needs changing now. Two notes for later briefs:

- **P2e (session restore)** will need to seed the workspace's id counter after rebuilding windows,
  groups and tabs from stored ids. Today `IdSource` is private and `from_raw` is public; P2e should
  add a restore constructor or a `Workspace` method that sets the counter past the largest stored
  id. That is deliberately not guessed here. `[inferred]`
- **The command handler** should map `tab.focus`/`tab.move`/`group.add-tab`'s `int` arguments to
  `usize` (the registry already rejects negatives) and pass `nav.go`'s parent folder plus file name
  as `(folder, Some(name))`. The model does not look at the filesystem. `[inferred]`

No file outside the brief's Output list was created or changed. `spike/` was read only, and
`Cargo.toml` was not edited. `[observed]`

## Review (Claude, Nov 2, 2026): passes

The model is ids-first, every operation returns events or an error without mutating on refusal,
and the split rules are the spike's, with the renumbering loop removed because ids are stable. The
1,500-step random sequence checks the invariants after every step.

Claude drove the public API from a scratch program outside the repo:
- **Grouping:** `open_request([])`, then five `tab_new`, a pill click plus Shift-click across all
  six, and `group_split`. Result: one pill, "Group 1 · 6 tabs", columns 3 + 3. The events: six
  `GroupDissolved` for the merged single pills, then `GroupFormed` and `FocusChanged`.
- **Rebalance:** closing the top-left tile twice took the group from 3 + 3 to 2 + 2, with
  `TabClosed`, `LayoutChanged` and `FocusChanged` (to the next tile in column order) each time.
  The invariants held.
- **Second launch:** `open_request` with two folders added two tabs to the focused window (pills
  1 → 3).

For later briefs (not blocking):
- P2e needs a restore constructor that seeds the id counter past the largest stored id, as the
  notes say.
- `GroupDissolved` for single-tab pills merging into a split isn't a "split view ending". If the UI
  only fades on real splits, it should check whether the group was a split; worth a look when
  `fm-ui` animates from events.
