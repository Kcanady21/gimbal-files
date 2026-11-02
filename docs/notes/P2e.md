# P2e: saving and restoring the session

## What I built

- `crates/fm-core/src/session.rs`: the session path, the TOML file format,
  `save`, `load`, `preserve_bad`, `SessionProblem`, and its tests.
- `crates/fm-core/src/fsutil.rs`: P2c's `save_atomic`, moved out of `config.rs`
  so the config and the session share one atomic writer. The body is unchanged;
  the only import is `crate::config::CONFIG_FILE` for the fallback name used
  when a path has no file name. `[observed]`
- `crates/fm-core/src/workspace/mod.rs`: `RestoreData` and its window, group and
  tab parts, `RestoreProblem`, and `Workspace::restore`.
- `crates/fm-core/src/workspace/tests.rs`: one test for each repair.
- `crates/fm-core/src/config.rs`: `Config::current_folder` is gone and
  `change_for(command, &config, current_folder: &Path)` takes the folder as an
  argument. P2c's tests were updated and one new test checks the argument.
- `crates/fm-core/src/lib.rs`: the `fsutil` and `session` module lines only.
- `docs/notes/P2e.md`: this file.

No dependency was added, and `Cargo.toml` was not touched. `[observed]`

## The file format

The app owns `$XDG_STATE_HOME/gimbal-files/session.toml`, or
`~/.local/state/gimbal-files/session.toml` when `XDG_STATE_HOME` is unset or
relative. `session_path` is a pure function; `default_session_path` reads the
environment. A save writes the whole document through `fsutil::save_atomic`.
`[observed]`

```toml
version = 1
focused_window = 1

[[window]]
pills = [0, 1]      # indices into this window's groups, in strip order
focused_tab = 2     # flat index over pills, then tabs in column order

[[window.group]]
name = "Build & test"  # omitted when the group has no name
number = 1
width_permille = 600
heights = [[420, 260], [310, 290]]  # or "default"
columns = [[0, 1], [2, 3]]          # indices into this group's tabs

[[window.group.tab]]
folder = "/home/kyle/work/a"
back = ["/home/kyle/work/two"]
forward = ["/home/kyle/work/b"]
selection_names = ["alpha", "beta"]
selection_focused = "beta"
selection_anchor = "alpha"
scroll_anchor = "beta"
```

- Groups are written in the window's `BTreeMap` order. `pills` gives the strip
  order as indices into that list. `[inferred]`
- A tab's `folder`, history entries, selection names and anchors are strings
  when they are valid UTF-8. Any other value is `{ bytes = [ … ] }`; both forms
  are read back. A name is an `OsString` and a path is a `PathBuf`, so nothing
  is lossily converted. `[observed]`
- The pill selection is not written. A restore starts with every pill
  unselected and the anchor at 0. `[observed]`
- No ids are written. `save` writes positions only, and `Workspace::restore`
  takes fresh ids from the workspace's private counter. `[observed]`
- `focused_tab` is one flat index: groups in `pills` order, each group's tabs in
  column order. This is also the index the focused-tab repair checks. `[inferred]`

## Restore and repair

`Workspace::restore(home, data)` builds the windows, groups and tabs directly,
then repairs only what is needed. Each repair is a `RestoreProblem`:

- a column list that does not hold every tab exactly once, or leaves a column
  empty with two or more tabs, is replaced by `initial_columns`;
- a `Heights::Hand` list whose length is not that column's tile count becomes
  `Heights::Default`;
- a width share over 1000 becomes 500;
- a focused tab index past the end becomes the first tab;
- a back or forward stack over 100 is trimmed from the oldest end;
- a group with no tabs is dropped, and a window with no groups left is dropped.

I also repair two cases the brief does not list, because the invariants require
it: a pill index that names no stored group is dropped, as is a repeated pill;
and duplicate group numbers are renumbered to the lowest free number. `[inferred]`

The tests call `check_invariants()` after every repair. `[observed]`

## Load and save

- `save(path, &Workspace)` is a no-op when the workspace has no windows. The
  previous file stays, so closing the last window does not erase its tabs. It
  also does not create a file when none existed. `[observed]`
- `load(path, home, exists)` takes the existence check as a closure, so tests
  never touch the real state folder. The app passes `Path::is_dir`. `[observed]`
- A missing file gives one window at home and no problem. An unparsable file,
  or a `version` other than 1, gives one window at home and one problem. A read
  error is treated the same way as an unparsable file, with a `Read` problem. `[inferred]`
- A tab whose folder no longer exists moves to its nearest existing parent, or
  home if no parent exists. History is kept as stored. Each move is a
  `MissingFolder` problem. `[observed]`
- `preserve_bad(path)` copies the file to `session.toml.bad` with `fs::copy`,
  which replaces an older copy, and returns the copy's path. The app calls it
  before its first save after a bad load. `[observed]`

## The config change

`Config` now holds only settings plus its runtime `home`. `change_for` takes the
current folder explicitly and uses it for `view.set-sort scope=folder` and for
`pins.add` without a path. The explicit `pins.add path=` still wins. No other
behaviour changed. `[observed]`

## Verified

From `$PROJ`, with `PATH="$T:$PATH"`:

- `scripts/check.sh`: exit 0, all five steps pass.
- `cargo test --workspace`: 144 tests, 0 failed. `fm-core` has 136 (16 new:
  9 session, 6 restore and 1 config), CLI has 7 and the agent has 1. `[observed]`
- The session tests ran only in `/tmp/gimbal-p2e-*` folders that each test
  created and removed; `find /tmp -maxdepth 1 -name 'gimbal-p2e-*'` printed
  nothing afterwards. Nothing touched `~/.local/state`. `[observed]`
- The rich round-trip test prints its file with `GIMBAL_P2E_PRINT=1`. The
  sample below is that test's actual output. `[observed]`

### `scripts/check.sh`

```text
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.02s
==> cargo test --workspace
   Compiling fm-core v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-core)
   Compiling fm-agent v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-agent)
   Compiling fm-ui v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-ui)
   Compiling fm-cli v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-cli)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.32s
     Running unittests src/lib.rs (target/debug/deps/fm_agent-cc4ee4bf374e51f2)

running 1 test
test tests::socket_lives_under_the_runtime_directory ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/main.rs (target/debug/deps/gimbal_files_cli-4295b4be5da538bb)

running 7 tests
test tests::anything_else_is_a_usage_error ... ok
test tests::check_keeps_the_command_and_its_arguments ... ok
test tests::check_without_a_command_is_a_usage_error ... ok
test tests::help_shows_usage ... ok
test tests::no_arguments_shows_usage ... ok
test tests::commands_lists_the_registry ... ok
test tests::version_shows_the_version ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (target/debug/deps/fm_core-408d0e2ff853beac)

running 136 tests
test command::tests::a_files_command_defaults_dry_run_to_false ... ok
test command::tests::a_checked_command_prints_in_catalogue_order ... ok
test command::tests::a_known_command_is_not_an_unknown_command ... ok
test command::tests::a_forbidden_call_never_reaches_the_handler ... ok
test command::tests::a_token_without_equals_is_an_error ... ok
test command::tests::a_value_may_contain_equals_and_spaces ... ok
test command::tests::a_single_valued_argument_may_not_repeat ... ok
test command::tests::an_empty_argument_name_is_an_error ... ok
test command::tests::a_valid_call_reaches_the_handler_once ... ok
test command::tests::an_invalid_call_never_reaches_the_handler ... ok
test command::tests::argument_kinds_are_sane ... ok
test command::tests::a_distant_typo_gets_no_suggestion ... ok
test command::tests::argument_names_are_unique_within_a_command ... ok
test command::tests::bool_accepts_true_and_false_only ... ok
test command::tests::choice_must_be_one_of_the_fixed_list ... ok
test command::tests::cli_and_agent_may_not_fill_files_paths_from_context ... ok
test command::tests::context_rules_are_pure_functions_over_every_caller ... ok
test command::tests::cli_and_agent_must_give_every_files_context_path ... ok
test command::tests::every_files_command_gets_exactly_one_dry_run_added_by_the_registry ... ok
test command::tests::missing_required_argument_is_named ... ok
test command::tests::int_accepts_the_range_and_rejects_bad_and_out_of_range_values ... ok
test command::tests::every_caller_may_give_explicit_files_paths ... ok
test command::tests::no_command_deletes_permanently ... ok
test command::tests::no_tokens_is_a_missing_command ... ok
test command::tests::names_are_unique ... ok
test command::tests::names_are_area_dot_verb_in_lowercase_kebab_case ... ok
test command::tests::number_values_are_hundredths_and_print_with_two_places ... ok
test command::tests::opacity_is_a_number_without_a_step ... ok
test command::tests::path_must_be_absolute_and_stays_as_given ... ok
test command::tests::repeated_keys_are_kept_in_order ... ok
test command::tests::paths_print_once_per_key ... ok
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
test config::tests::a_read_error_reports_the_file_and_position ... ok
test command::tests::unknown_command_suggests_the_closest_name ... ok
test config::tests::every_default_matches_the_brief ... ok
test config::tests::an_unknown_key_is_a_warning_with_its_position ... ok
test config::tests::default_pins_include_home_then_only_existing_special_folders ... ok
test config::tests::a_hotkeys_table_replaces_listed_keys_and_an_empty_value_unbinds ... ok
test config::tests::a_bad_value_falls_back_and_the_good_values_still_load ... ok
test config::tests::every_settings_command_has_a_change_for ... ok
test config::tests::an_unparsable_file_is_never_written ... ok
test config::tests::a_save_rewrites_only_the_changed_key_on_a_commented_file ... ok
test config::tests::a_sidebar_and_section_save_keep_their_inline_formatting ... ok
test config::tests::zoom_stops_at_the_scale_limits_and_toggles_flip ... ok
test config::tests::saving_is_atomic_and_keeps_permissions ... ok
test config::tests::saving_creates_the_folder_and_the_changed_key ... ok
test config::tests::set_scale_and_sort_map_to_the_expected_changes ... ok
test config::tests::config_path_is_a_pure_function_of_xdg_config_home_and_home ... ok
test config::tests::a_hotkey_with_an_unknown_command_or_bad_arguments_keeps_the_default ... ok
test config::tests::commands_that_need_the_current_folder_use_the_argument ... ok
test config::tests::pin_changes_write_the_pins_list ... ok
test session::tests::a_missing_file_gives_one_window_at_home_without_a_problem ... ok
test session::tests::an_empty_workspace_is_never_saved ... ok
test config::tests::an_unparsable_file_reports_one_problem_with_a_position ... ok
test config::tests::sort_for_expands_home_and_normalizes_dot_dot_and_trailing_slashes ... ok
test session::tests::session_path_is_a_pure_function_of_xdg_state_home_and_home ... ok
test config::tests::unknown_keys_and_tables_are_warnings_and_stay_in_the_file ... ok
test config::tests::sort_changes_write_one_global_setting_or_one_folder_rule ... ok
test tests::app_id_matches_the_niri_rule_and_is_a_valid_gapplication_id ... ok
test workspace::split::tests::a_close_rebalances_two_or_more_apart ... ok
test session::tests::a_version_2_file_gives_one_window_at_home_and_a_problem ... ok
test session::tests::preserve_bad_replaces_an_older_copy ... ok
test workspace::split::tests::a_hand_made_three_plus_one_survives_a_close_from_the_long_column ... ok
test workspace::split::tests::a_drop_never_empties_a_column ... ok
test session::tests::an_unparsable_file_gives_one_window_at_home_and_a_problem ... ok
test workspace::split::tests::a_slot_past_the_end_changes_nothing ... ok
test workspace::split::tests::closing_the_lone_tile_rebalances ... ok
test workspace::split::tests::a_swap_keeps_heights_while_an_insert_resets_the_changed_column ... ok
test workspace::split::tests::ctrl_click_toggles_without_moving_the_anchor ... ok
test config::tests::every_problem_kind_reports_its_line_and_column ... ok
test workspace::split::tests::dropping_in_a_gap_inserts_and_the_column_gets_longer ... ok
test workspace::split::tests::dropping_on_a_tile_swaps_across_and_reorders_within ... ok
test workspace::split::tests::initial_columns_fill_the_shorter_column ... ok
test workspace::split::tests::plain_click_clears_and_moves_the_anchor ... ok
test workspace::split::tests::shift_click_selects_the_range_from_the_anchor ... ok
test workspace::tests::a_fresh_workspace_has_one_focused_tab_and_holds_the_invariants ... ok
test workspace::tests::a_failed_operation_changes_nothing ... ok
test config::tests::prints_a_sample_problem_list ... ok
test workspace::tests::a_drop_that_would_empty_a_column_is_refused_and_changes_nothing ... ok
test workspace::tests::a_hand_made_three_plus_one_survives_a_close_from_the_long_column ... ok
test workspace::tests::closing_the_last_pill_closes_the_window ... ok
test workspace::tests::a_tile_drop_swaps_reorders_and_inserts ... ok
test workspace::tests::closing_a_window_focuses_another_and_the_last_says_so ... ok
test session::tests::a_missing_folder_falls_back_to_its_parent_then_to_home ... ok
test workspace::tests::focus_after_a_close_goes_right_else_left ... ok
test workspace::tests::file_clicks_follow_the_strip_rules ... ok
test workspace::tests::group_add_tab_joins_a_single_pill_into_a_new_split ... ok
test workspace::tests::closing_a_tab_inside_a_group_focuses_the_next_tile_else_the_previous ... ok
test workspace::tests::closing_two_tiles_from_a_three_plus_three_group_rebalances_to_two_plus_two ... ok
test session::tests::a_non_utf8_folder_and_file_name_round_trip ... ok
test workspace::tests::group_close_closes_every_tab_and_focuses_the_neighbour ... ok
test workspace::tests::group_add_tab_uses_the_shorter_column_and_resets_that_columns_heights ... ok
test workspace::tests::group_rename_trims_and_rejects_an_empty_name ... ok
test workspace::tests::group_split_inserts_at_the_first_selected_pill ... ok
test workspace::tests::group_split_merges_selected_groups_and_needs_two_tabs ... ok
test workspace::tests::hand_heights_need_one_entry_per_tile_and_the_width_share_is_permille ... ok
test workspace::tests::group_split_uses_the_lowest_free_number ... ok
test workspace::tests::ids_are_stable_and_never_reused ... ok
test workspace::tests::nav_go_pushes_history_and_clears_selection_and_anchor ... ok
test workspace::tests::nav_back_and_forward_move_between_folders_and_do_nothing_on_empty_stacks ... ok
test workspace::tests::nav_go_to_a_file_goes_to_its_folder_and_selects_the_name ... ok
test workspace::tests::nav_up_stops_at_the_root ... ok
test workspace::tests::pill_clicks_follow_the_strip_rules ... ok
test workspace::tests::restore_drops_an_empty_group_and_an_empty_window ... ok
test workspace::tests::history_stacks_are_capped_at_one_hundred_dropping_the_oldest ... ok
test workspace::tests::restore_repairs_a_focused_tab_out_of_range ... ok
test workspace::tests::restore_repairs_a_width_share_over_1000 ... ok
test workspace::tests::restore_replaces_a_column_list_that_does_not_match_its_tabs ... ok
test workspace::tests::restore_repairs_hand_heights_of_the_wrong_length ... ok
test workspace::tests::selection_all_uses_the_handlers_names_and_none_clears_them ... ok
test workspace::tests::tab_close_selected_closes_the_selected_pills_and_focuses_right_else_left ... ok
test workspace::tests::tab_focus_rejects_an_index_past_the_end_and_changes_nothing ... ok
test workspace::tests::restore_trims_history_from_the_oldest_end ... ok
test workspace::tests::tab_new_in_an_empty_workspace_uses_home ... ok
test workspace::tests::tab_move_reorders_pills_and_rejects_bad_indices ... ok
test session::tests::a_rich_workspace_round_trips ... ok
test workspace::tests::tab_new_opens_after_the_focused_pill_and_inherits_its_folder ... ok
test workspace::tests::tab_new_opens_after_a_group_pill_and_not_inside_it ... ok
test workspace::tests::the_scroll_anchor_is_recorded_and_a_repeat_is_a_no_op ... ok
test workspace::tests::tab_open_in_new_window_moves_the_tab_out ... ok
test workspace::tests::window_new_opens_a_tab_and_focuses_it ... ok
test workspace::tests::the_second_launch_with_folders_opens_one_tab_each_in_the_focused_window ... ok
test workspace::tests::the_second_launch_with_no_paths_opens_one_window_at_home ... ok
test workspace::tests::a_long_random_sequence_keeps_the_invariants ... ok

test result: ok. 136 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

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
   Compiling fm-ui v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-ui)
   Compiling fm-cli v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-cli)
    Finished `release` profile [optimized] target(s) in 1.33s
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

### Saved session from the rich round trip

```toml
version = 1
focused_window = 1

[[window]]
pills = [0, 1]
focused_tab = 4

[[window.group]]
name = "Build & test"
number = 1
width_permille = 600
heights = [[420, 260], [310, 290]]
columns = [[0, 1], [2, 3]]

[[window.group.tab]]
folder = "/home/kyle/work/one"
back = []
forward = []
selection_names = []

[[window.group.tab]]
folder = "/home/kyle/work/three"
back = []
forward = []
selection_names = []

[[window.group.tab]]
folder = "/home/kyle/work/two"
back = []
forward = []
selection_names = []

[[window.group.tab]]
folder = "/home/kyle/work/four"
back = []
forward = []
selection_names = []

[[window.group]]
number = 2
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle/work/a"
back = ["/home/kyle/work/notes"]
forward = ["/home/kyle/work/b"]
selection_names = ["alpha", "beta"]
selection_focused = "beta"
selection_anchor = "alpha"
scroll_anchor = "beta"

[[window]]
pills = [0]
focused_tab = 0

[[window.group]]
number = 1
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle/work/other"
back = []
forward = []
selection_names = []
```

## API citations

All `toml_edit` names were read in the pinned source under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`:

- `Document::parse`: `toml_edit-0.25.17+spec-1.1.0:src/document.rs:25`.
- `DocumentMut`: `toml_edit-0.25.17+spec-1.1.0:src/document.rs:133`.
- `ArrayOfTables` and `ArrayOfTables::push`:
  `src/array_of_tables.rs:7` and `:92`.
- `Array::push`: `src/array.rs:180`.
- `Table::insert` and `InlineTable::insert`: `src/table.rs:429`,
  `src/inline_table.rs:396`.
- `Item::as_array_of_tables`, `as_integer`, `as_str`, `as_array`,
  `as_inline_table`: `src/item.rs:96`, `:224`, `:254`, `:274`, `:289`.
- `Value::as_str`, `as_array`, `as_inline_table`: `src/value.rs:46`, `:111`,
  `:132`.

The non-UTF-8 conversion is `std::os::unix::ffi::OsStrExt::as_bytes` and
`OsStringExt::from_vec`:
<https://doc.rust-lang.org/std/os/unix/ffi/trait.OsStrExt.html> and
<https://doc.rust-lang.org/std/os/unix/ffi/trait.OsStringExt.html>. `[read]`
`Path::ancestors` for the missing-folder fallback:
<https://doc.rust-lang.org/std/path/struct.Path.html#method.ancestors>. `[read]`
`std::fs::copy`, `std::fs::rename`, `OpenOptions::create_new` and
`File::sync_all` are the pinned standard library:
<https://doc.rust-lang.org/std/fs/fn.copy.html>,
<https://doc.rust-lang.org/std/fs/fn.rename.html>,
<https://doc.rust-lang.org/std/fs/struct.OpenOptions.html#method.create_new>,
<https://doc.rust-lang.org/std/fs/struct.File.html#method.sync_all>. `[read]`

## What needs a live session

Nothing in this brief. The module is pure data plus file I/O, and every file
test used a `/tmp/gimbal-p2e-*` folder. The app is not wired to it yet, so the
actual save-on-exit and restore-on-start path is still for Claude to check once
the UI calls `save`, `load` and `preserve_bad`. `[inferred]`

## Anything outside my files

Nothing needs changing now. For the UI brief that wires the session:

- On exit (or whenever the app saves), call `session::save` only after the
  workspace has been updated; an empty workspace is a deliberate no-op.
- On start, call `session::load(path, home, Path::is_dir)`. If any problem is a
  parse or version problem, call `session::preserve_bad(path)` before the first
  save so the bad file is kept.
- When a `Settings` command needs the current folder, pass the focused tab's
  folder to `config::change_for` rather than storing it on `Config`.

No file outside the brief's Output list was created or changed. `spike/` was
read only. `[observed]`

## Review (Claude, Nov 2, 2026): passes

`scripts/check.sh` from `/tmp`: exit 0 (136 + 7 + 1 tests, deny clean). Nothing was left in
`/tmp`, and `~/.local/state/gimbal-files` was never created. `save_atomic` moved to `fsutil.rs`
with its body unchanged, and `Config` no longer carries the current folder.

Claude ran a round trip from a scratch program outside the repo, built through real model
operations rather than hand-made data:
- `open_request`, two `nav_go`s and a `nav_back`;
- four `tab_new`s, including the non-UTF-8 folder `/tmp/caf\xE9`;
- a Shift-selected `group_split` of three tabs, renamed "Work", with share 620 and left-column hand
  heights `[500, 300]`;
- a second window.

Results:
- **Round trip:** after `save` and `load`, every window, pill order, group name, number, share,
  height list, column and tab (folder, history, selection, scroll anchor) printed identically,
  with no problems, and the invariants held.
- **Missing folders**, with only home, `/tmp` and `/` existing: 6 problems.
  `~/Documents` and `~/Music` fell back to home; `/tmp/a`, `/tmp/b`, `/tmp/c` and the non-UTF-8
  folder fell back to `/tmp`.
- **An empty workspace** didn't touch the file (same mtime).
- **A `version = 2` file** gave `UnsupportedVersion` and one fresh window. `preserve_bad` copied
  it to `session.toml.bad` unchanged.

Nits (not blocking):
- `fsutil.rs` imports `config::CONFIG_FILE` as its fallback temp name when a path has no file
  name, so a session save could name its temp file after the config. A neutral name in `fsutil`
  would drop the dependency on `config`.
