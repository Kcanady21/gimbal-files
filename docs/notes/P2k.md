# P2k: file operations in the UI

`[observed]` unless a line says otherwise. Tags are `[read]` for source or crate
facts, `[observed]` for a command or a run, and `[inferred]` for anything that
needs a real pointer, a screenshot or a live feel.

## What I built

All changes are under `crates/fm-ui/src/`; `fm-core` was not changed. No new
dependency and no `Cargo.toml` change.

- **One worker owns `FileOperations`** — `fileops/worker.rs`, `fileops/mod.rs`.
  - `fm-ui` starts one `gimbal-fileops` thread with `FileOperations::new()`.
    The client sends `Start { id, request, Cancellable }` on one channel and
    answers/cancels on a second, so a queued `Start` is never mistaken for a
    conflict answer.
  - The worker plans, sends `Planned { plan }`, waits for `Answer`, then calls
    `FileOperations::execute` with the `Decisions` and a progress callback. It
    sends `Progress`, then `UndoState`, then `Finished`/`Refused`.
  - `FileOps` (the main-thread client) queues a second request behind the
    first. Its poll is a `glib` main-context timeout (`timeout_add_local`) every
    30 ms; it drains the worker's channel and stops when the queue and current
    job are empty. No main-thread wait, no file I/O on the main thread for
    execution.
  - The progress callback is throttled to one event per 50 ms (plus the final
    item), so a large copy does not flood the main context.
  - `FileOps::submit` returns the job id. The 500 ms reveal is checked by the
    same poll against `Instant::now()`; an operation shorter than that never
    shows the row.
- **Every `Files` command goes through it** — `app.rs`.
  - `file.trash`, `file.copy`, `file.move`, `file.rename`, `file.new-folder`
    and `file.undo` are rebuilt as a `CheckedCommand` with the context paths
    filled (`filled_invocation`), then mapped with `request_for`.
  - `dry-run=true` returns the plan's lines as `Outcome::DryRun` and submits
    nothing. Non-undo dry runs are planned read-only with a fresh
    `FileOperations`; undo uses the worker's latest record, which arrives with
    `UndoState`.
  - A non-UTF-8 path cannot be represented in the registry's string arguments;
    the handler logs it and runs nothing rather than running on a lossy name.
  - The folder view is not refreshed after an operation: `DirectoryList`
    already receives the live changes.
- **Conflict menu** — `fileops/conflict.rs` (pure state machine) and
  `fileops/dialogs.rs` (the AlertDialog).
  - `ConflictFlow` wraps `Decisions` and exposes `question()` and
    `answer(item, decision, apply_to_all)`.
  - One dialog per conflict, in `Decisions::next_unanswered` order. The title
    names the item and the body says the name already exists in the target
    folder.
  - Skip, Keep Both, Replace (destructive) and Cancel. Cancel answers
    `Decision::Cancel`, so `execute` writes nothing.
  - "Apply this decision to all files" is a `GtkCheckButton` in
    `AlertDialog::extra_child`, shown when the plan has more than one
    conflict. It reaches `Decisions::answer(..., apply_to_all = true)`.
  - Non-overridable conflicts never reach the dialog: `Plan::decisions()`
    only lists `TargetExists`, and `execute` reports the rest as `Refused`.
- **Progress and Cancel** — `fileops/progress.rs`, `content.rs`.
  - The row sits between the rule and the status line: label, bar, Cancel.
    The label is `Copying 3 of 120`, with `— 1 waiting` when the client has a
    queued request.
  - The bar uses `bytes_done / bytes_total` when GIO reported a total; otherwise
    `items_done / items_total`.
  - Cancel calls the job's `gio::Cancellable` and sends `Cancel`; the core stops
    before the next item and records what finished.
  - The row is hidden until 500 ms (`row_visible`) and is removed on finish.
  - A finished operation holds the status line for 4 s (`SUMMARY_MS`) with the
    short summary, then returns to the counts. The status label has a click
    gesture; when the report had failures it opens
    `FileOps::show_details` with every failed/refused path and message.
  - The full report is logged through `GIMBAL_LOG` in the worker, the client
    and `Process::fileop_finished`.
- **Delete and the right-click menus** — `fileops/menu.rs`,
  `fileops/dialogs.rs`, `folder/view.rs`.
  - Delete is the existing `file.trash` hotkey and now reaches the worker.
    There is no Shift+Delete.
  - Item menu: Open, Open in New Tab (one folder), Rename… (one item), Cut,
    Copy, Copy To…, Move To…, Move to Trash, Pin (one folder). A right press on
    an unselected item selects it first; a tile is focused first.
  - Background menu: Undo <summary> (when there is one), New Folder…, Paste,
    Pin This Folder, Refresh, Disk Usage.
  - Rename… and New Folder… are entry popovers in a `gl-menu`. Rename
    pre-selects `file_stem` (the name without its extension).
  - Copy To…/Move To… use `gtk::FileDialog::select_folder` and dispatch
    `file.copy`/`file.move` with `to=`.
  - Disk Usage is insensitive when `baobab` is not on `PATH`. When it is, the
    handler runs `baobab <folder>`; no test or agent run launches it.
  - Every entry carries a registry command name; a test asserts the whole menu
    is in `COMMANDS`.
- **Undo** — `file.undo` (Ctrl+Z and the top background item) submits
  `Request::Undo` to the worker. The status line uses `Undid: <summary>` from
  the undo plan, lists any item not undone with its reason, and says
  "Nothing to undo" for an empty stack. The background item is read from the
  `UndoState` event sent after every operation and undo.
- **Cut, copy, paste** — `fileops/clipboard.rs`.
  - `file.cut` and `file.copy-to-clipboard` write a `gdk::FileList` plus
    `x-special/gnome-copied-files` plus `application/x-kde-cutselection` in one
    `ContentProvider` union.
  - `file.paste` reads the clipboard asynchronously, prefers GNOME's
    `cut`/`copy`, then Dolphin's `1`, then treats `text/uri-list` as a copy,
    and submits a `Request::Move` or `Request::Copy` into `in` through the same
    worker, conflict dialog, progress and undo. A cut's successful paste clears
    the clipboard.
  - The pure format builders and parsers are tested; no test or agent run
    touches the live clipboard.
- **Text entries keep their keys** — `fileops/keys.rs`, `commands.rs`.
  - `entry_owns_key` is the pure decision: Ctrl+A/C/X/V/Z; Delete, Left, Right
    and Up with or without Alt; Escape.
  - The hotkey callback returns `Propagation::Proceed` for those keys while a
    path bar, search field, rename popover or new-folder popover has focus, so
    the entry gets them; every other hotkey still runs.

## Choices and notes for review

- `file.paste`'s `dry-run=true` reads the clipboard asynchronously. GDK has no
  synchronous clipboard read, so the handler logs the plan's lines instead of
  returning them from `dispatch`; the six core `Files` commands do return
  `Outcome::DryRun`. `[read]` for the API, `[inferred]` for the trade-off.
- The item menu stores paths as strings, so a non-UTF-8 selection is logged and
  not run, as with any command invocation.
- `FileOps::plan_lines` builds a non-undo plan on the main thread. This is
  read-only (`Plan` never writes; P2f) and avoids a second `FileOperations`
  ever owning an undo stack. `[inferred]`
- The conflict flow is tested over `Decisions`, not over real dialogs. The
  actual AlertDialog sequence is in the "Not verified live" list.
- `FileOps` is created before `restore_session`; it only holds a weak reference
  to `Process`, so there is no cycle.
- `fm-core` needed no change. `request_for` already returned
  `Result<Option<Request>, RequestError>` and `Decisions::answer` already
  handled "apply to all".

## API citations

Read in the pinned sources:

- `gtk4-0.11.5:src/auto/file_dialog.rs:571` `select_folder`; `:1070`
  `initial_folder`; `:1088` `modal`.
- `gdk4-0.11.5:src/clipboard.rs:11` `read_async`; `src/auto/clipboard.rs:38`
  `formats`; `:247` `set_content`.
- `gdk4-0.11.5:src/auto/content_provider.rs:28` `for_bytes`; `:40` `for_value`;
  `:50` `new_union`.
- `gdk4-0.11.5:src/auto/file_list.rs:29` `from_array`; `:37` `files`.
- `gio-0.22.10:src/auto/input_stream.rs:155` `read_bytes_async`;
  `src/auto/cancellable.rs:38` `unsafe impl Send for Cancellable`.
- `libadwaita-0.9.2:src/auto/alert_dialog.rs:441` `add_response`; `:624`
  `set_extra_child`; `:704` `connect_response`; the widget also implements
  `gtk::Widget`, so `add_css_class("gimbal-look")` is valid.
- `gtk-4.22.4:gtk/gtkshortcutcontroller.c:428` only a shortcut action that
  returns handled stops the event, which is why `Propagation::Proceed` gives
  the key back to the entry.
- `gtk-4.22.4:gdk/gdkcontentserializer.c:1045-1050` registers
  `GDK_TYPE_FILE_LIST` for `text/uri-list`.
- `nautilus-50.2.2:src/nautilus-clipboard.c:54-66` builds
  `x-special/gnome-copied-files`; `:85-110` parses it (first line `cut` or
  `copy`, one URI per line, empty lines invalid).
- `kio-6.24.0:src/widgets/paste.cpp:256-263` writes
  `application/x-kde-cutselection` as `1`/`0`; `:264-267` reads it.
- Phase 0's captures agree: Cosmic offers
  `x-special/gnome-copied-files`
  (`docs/research/phase0/captures/20261031-070317-cosmic-outside/summary.md:9`)
  and Dolphin offers `text/uri-list`
  (`docs/research/phase0/captures/20261031-070328-dolphin-downloads/summary.md:9`).
- `fm-core` `fileops`: `Request`, `request_for`, `Plan`, `Conflict`,
  `Decision`/`Decisions`, `execute`, `Progress`, `ItemOutcome`, `Report`.

## Verified

### `scripts/check.sh`

Run from `$PROJ` with the 1.95.0 toolchain on `PATH`; exit 0. The whole output
follows this section.

`fm-ui` has 127 tests, 27 of them new in `fileops` (clipboard 8, conflict 4,
keys 3, menu 8, progress 4, plus the existing suite). `fm-core` has 175, the
CLI 7, the agent 1, and the trash binary 6 parents plus 6 ignored children.
316 tests ran in all. `[observed]`

```text
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.03s
==> cargo test --workspace
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.03s
     Running unittests src/lib.rs (target/debug/deps/fm_agent-d3e0ba7a5f0ed207)

running 1 test
test tests::socket_lives_under_the_runtime_directory ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/main.rs (target/debug/deps/gimbal_files_cli-7fb62e9220e6d530)

running 7 tests
test tests::check_without_a_command_is_a_usage_error ... ok
test tests::anything_else_is_a_usage_error ... ok
test tests::check_keeps_the_command_and_its_arguments ... ok
test tests::no_arguments_shows_usage ... ok
test tests::help_shows_usage ... ok
test tests::commands_lists_the_registry ... ok
test tests::version_shows_the_version ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (target/debug/deps/fm_core-71b31b2576b0b33f)

running 175 tests
test command::tests::a_checked_command_prints_in_catalogue_order ... ok
test command::tests::a_forbidden_call_never_reaches_the_handler ... ok
test command::tests::a_files_command_defaults_dry_run_to_false ... ok
test command::tests::a_valid_call_reaches_the_handler_once ... ok
test command::tests::a_known_command_is_not_an_unknown_command ... ok
test command::tests::a_token_without_equals_is_an_error ... ok
test command::tests::a_single_valued_argument_may_not_repeat ... ok
test command::tests::a_value_may_contain_equals_and_spaces ... ok
test command::tests::an_empty_argument_name_is_an_error ... ok
test command::tests::an_invalid_call_never_reaches_the_handler ... ok
test command::tests::a_distant_typo_gets_no_suggestion ... ok
test command::tests::argument_kinds_are_sane ... ok
test command::tests::argument_names_are_unique_within_a_command ... ok
test command::tests::bool_accepts_true_and_false_only ... ok
test command::tests::choice_must_be_one_of_the_fixed_list ... ok
test command::tests::cli_and_agent_may_not_fill_files_paths_from_context ... ok
test command::tests::context_rules_are_pure_functions_over_every_caller ... ok
test command::tests::cli_and_agent_must_give_every_files_context_path ... ok
test command::tests::every_files_command_gets_exactly_one_dry_run_added_by_the_registry ... ok
test command::tests::every_caller_may_give_explicit_files_paths ... ok
test command::tests::int_accepts_the_range_and_rejects_bad_and_out_of_range_values ... ok
test command::tests::missing_required_argument_is_named ... ok
test command::tests::names_are_area_dot_verb_in_lowercase_kebab_case ... ok
test command::tests::no_command_deletes_permanently ... ok
test command::tests::no_tokens_is_a_missing_command ... ok
test command::tests::names_are_unique ... ok
test command::tests::number_values_are_hundredths_and_print_with_two_places ... ok
test command::tests::opacity_is_a_number_without_a_step ... ok
test command::tests::path_must_be_absolute_and_stays_as_given ... ok
test command::tests::repeated_keys_are_kept_in_order ... ok
test command::tests::paths_print_once_per_key ... ok
test command::tests::repeated_paths_keys_make_a_list_in_input_order ... ok
test command::tests::summaries_are_one_non_empty_line ... ok
test command::tests::text_must_not_be_empty ... ok
test command::tests::scale_accepts_the_step_edges_and_refuses_off_step_and_out_of_range ... ok
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
test config::tests::an_unknown_key_is_a_warning_with_its_position ... ok
test config::tests::a_bad_value_falls_back_and_the_good_values_still_load ... ok
test config::tests::a_hotkeys_table_replaces_listed_keys_and_an_empty_value_unbinds ... ok
test config::tests::a_sidebar_and_section_save_keep_their_inline_formatting ... ok
test config::tests::saving_creates_the_folder_and_the_changed_key ... ok
test config::tests::a_save_rewrites_only_the_changed_key_on_a_commented_file ... ok
test config::tests::default_pins_include_home_then_only_existing_special_folders ... ok
test config::tests::sort_for_expands_home_and_normalizes_dot_dot_and_trailing_slashes ... ok
test config::tests::a_hotkey_with_an_unknown_command_or_bad_arguments_keeps_the_default ... ok
test config::tests::unknown_keys_and_tables_are_warnings_and_stay_in_the_file ... ok
test config::tests::an_unparsable_file_reports_one_problem_with_a_position ... ok
test config::tests::commands_that_need_the_current_folder_use_the_argument ... ok
test fileops::tests::a_cancel_partway_still_runs_nothing ... ok
test fileops::tests::a_dry_run_changes_nothing_on_disk ... ok
test fileops::tests::a_cancel_decision_runs_nothing ... ok
test fileops::tests::a_folder_into_itself_is_refused ... ok
test fileops::tests::a_new_folder_with_a_taken_or_invalid_name_is_a_conflict ... ok
test config::tests::every_default_matches_the_brief ... ok
test fileops::tests::cancel_ends_the_questions_at_once ... ok
test config::tests::sort_changes_write_one_global_setting_or_one_folder_rule ... ok
test fileops::tests::apply_to_all_does_not_overwrite_an_earlier_answer ... ok
test config::tests::config_path_is_a_pure_function_of_xdg_config_home_and_home ... ok
test config::tests::zoom_stops_at_the_scale_limits_and_toggles_flip ... ok
test fileops::tests::copy_names_add_copy_then_numbers ... ok
test config::tests::set_scale_and_sort_map_to_the_expected_changes ... ok
test fileops::tests::a_command_outside_the_file_operations_is_not_a_request ... ok
test config::tests::every_settings_command_has_a_change_for ... ok
test fileops::tests::decisions_cancel_with_apply_to_all_marks_the_run_cancelled ... ok
test config::tests::an_unparsable_file_is_never_written ... ok
test fileops::tests::every_files_command_maps_to_its_request ... ok
test fileops::tests::a_copy_with_a_conflict_lists_the_target ... ok
test fileops::tests::missing_sources_are_refused_for_their_item ... ok
test fileops::tests::names_are_checked_against_the_rules ... ok
test config::tests::saving_is_atomic_and_keeps_permissions ... ok
test config::tests::every_problem_kind_reports_its_line_and_column ... ok
test fileops::tests::moving_into_the_folder_an_item_is_already_in_is_refused ... ok
test fileops::tests::request_mapping_refuses_unfilled_context_paths ... ok
test fileops::tests::renaming_to_an_invalid_or_taken_name_is_a_conflict ... ok
test fileops::tests::the_first_answer_can_apply_to_all ... ok
test config::tests::pin_changes_without_a_pin_list_keep_the_defaults ... ok
test config::tests::pin_changes_write_the_pins_list ... ok
test config::tests::prints_a_sample_problem_list ... ok
test fileops::tests::three_conflicts_are_answered_one_by_one ... ok
test fileops::tests::trashinfo_keeps_the_first_occurrence_of_a_key ... ok
test fileops::tests::trashinfo_percent_decodes_non_utf8_paths ... ok
test fileops::tests::trashinfo_percent_decodes_the_path ... ok
test fileops::tests::trashinfo_reads_both_date_forms ... ok
test fileops::tests::trashinfo_reports_a_missing_key_header_or_bad_value ... ok
test fileops::tests::undo_with_nothing_on_the_stack_is_an_error ... ok
test fileops::tests::unanswered_conflicts_refuse_the_whole_operation_and_name_the_items ... ok
test session::tests::a_missing_file_gives_one_window_at_home_without_a_problem ... ok
test session::tests::an_empty_workspace_is_never_saved ... ok
test session::tests::session_path_is_a_pure_function_of_xdg_state_home_and_home ... ok
test session::tests::a_version_2_file_gives_one_window_at_home_and_a_problem ... ok
test tests::app_id_matches_the_niri_rule_and_is_a_valid_gapplication_id ... ok
test workspace::split::tests::a_close_rebalances_two_or_more_apart ... ok
test session::tests::an_unparsable_file_gives_one_window_at_home_and_a_problem ... ok
test session::tests::preserve_bad_replaces_an_older_copy ... ok
test workspace::split::tests::a_drop_never_empties_a_column ... ok
test workspace::split::tests::a_slot_past_the_end_changes_nothing ... ok
test workspace::split::tests::a_hand_made_three_plus_one_survives_a_close_from_the_long_column ... ok
test workspace::split::tests::a_swap_keeps_heights_while_an_insert_resets_the_changed_column ... ok
test workspace::split::tests::closing_the_lone_tile_rebalances ... ok
test workspace::split::tests::ctrl_click_toggles_without_moving_the_anchor ... ok
test workspace::split::tests::dropping_in_a_gap_inserts_and_the_column_gets_longer ... ok
test workspace::split::tests::dropping_on_a_tile_swaps_across_and_reorders_within ... ok
test session::tests::a_missing_folder_falls_back_to_its_parent_then_to_home ... ok
test workspace::split::tests::initial_columns_fill_the_shorter_column ... ok
test workspace::split::tests::plain_click_clears_and_moves_the_anchor ... ok
test workspace::split::tests::shift_click_selects_the_range_from_the_anchor ... ok
test workspace::tests::a_failed_operation_changes_nothing ... ok
test workspace::tests::a_fresh_workspace_has_one_focused_tab_and_holds_the_invariants ... ok
test workspace::tests::a_drop_that_would_empty_a_column_is_refused_and_changes_nothing ... ok
test workspace::tests::a_hand_made_three_plus_one_survives_a_close_from_the_long_column ... ok
test workspace::tests::closing_a_window_focuses_another_and_the_last_says_so ... ok
test session::tests::a_non_utf8_folder_and_file_name_round_trip ... ok
test workspace::tests::closing_a_tab_inside_a_group_focuses_the_next_tile_else_the_previous ... ok
test workspace::tests::a_tile_drop_swaps_reorders_and_inserts ... ok
test workspace::tests::closing_the_last_pill_closes_the_window ... ok
test workspace::tests::focus_window_moves_the_focus_and_refuses_an_unknown_id ... ok
test workspace::tests::file_clicks_follow_the_strip_rules ... ok
test workspace::tests::focus_after_a_close_goes_right_else_left ... ok
test workspace::tests::group_add_tab_joins_a_single_pill_into_a_new_split ... ok
test workspace::tests::group_add_tab_uses_the_shorter_column_and_resets_that_columns_heights ... ok
test workspace::tests::closing_two_tiles_from_a_three_plus_three_group_rebalances_to_two_plus_two ... ok
test workspace::tests::group_close_closes_every_tab_and_focuses_the_neighbour ... ok
test workspace::tests::group_split_uses_the_lowest_free_number ... ok
test workspace::tests::group_rename_trims_and_rejects_an_empty_name ... ok
test session::tests::a_rich_workspace_round_trips ... ok
test workspace::tests::group_split_inserts_at_the_first_selected_pill ... ok
test workspace::tests::hand_heights_need_one_entry_per_tile_and_the_width_share_is_permille ... ok
test workspace::tests::group_split_merges_selected_groups_and_needs_two_tabs ... ok
test workspace::tests::nav_back_and_forward_move_between_folders_and_do_nothing_on_empty_stacks ... ok
test workspace::tests::ids_are_stable_and_never_reused ... ok
test workspace::tests::nav_go_pushes_history_and_clears_selection_and_anchor ... ok
test workspace::tests::history_stacks_are_capped_at_one_hundred_dropping_the_oldest ... ok
test workspace::tests::nav_go_to_a_file_goes_to_its_folder_and_selects_the_name ... ok
test workspace::tests::restore_repairs_a_focused_tab_out_of_range ... ok
test workspace::tests::restore_repairs_a_width_share_over_1000 ... ok
test workspace::tests::selection_all_uses_the_handlers_names_and_none_clears_them ... ok
test workspace::tests::restore_replaces_a_column_list_that_does_not_match_its_tabs ... ok
test workspace::tests::restore_repairs_hand_heights_of_the_wrong_length ... ok
test workspace::tests::nav_up_stops_at_the_root ... ok
test workspace::tests::only_split_groups_take_group_numbers ... ok
test workspace::tests::pill_clicks_follow_the_strip_rules ... ok
test workspace::tests::restore_drops_an_empty_group_and_an_empty_window ... ok
test workspace::tests::restore_trims_history_from_the_oldest_end ... ok
test workspace::tests::tab_close_selected_closes_the_selected_pills_and_focuses_right_else_left ... ok
test workspace::tests::tab_focus_rejects_an_index_past_the_end_and_changes_nothing ... ok
test workspace::tests::tab_new_in_an_empty_workspace_uses_home ... ok
test workspace::tests::tab_move_reorders_pills_and_rejects_bad_indices ... ok
test workspace::tests::tab_new_opens_after_a_group_pill_and_not_inside_it ... ok
test workspace::tests::tab_new_opens_after_the_focused_pill_and_inherits_its_folder ... ok
test workspace::tests::the_scroll_anchor_is_recorded_and_a_repeat_is_a_no_op ... ok
test workspace::tests::tab_open_in_new_window_moves_the_tab_out ... ok
test workspace::tests::the_second_launch_with_folders_opens_one_tab_each_in_the_focused_window ... ok
test workspace::tests::the_second_launch_with_no_paths_opens_one_window_at_home ... ok
test workspace::tests::window_new_opens_a_tab_and_focuses_it ... ok
test fileops::tests::cancelling_stops_after_the_current_item ... ok
test fileops::tests::skip_and_keep_both_are_applied_per_item ... ok
test fileops::tests::a_failed_item_does_not_stop_the_others ... ok
test fileops::tests::copying_into_its_own_folder_uses_copy_and_copy_2 ... ok
test fileops::tests::a_folder_tree_with_a_symlink_is_copied_faithfully ... ok
test fileops::tests::new_folder_and_rename_run_and_the_plan_lists_them ... ok
test fileops::tests::the_undo_stack_keeps_at_most_fifty_records ... ok
test fileops::tests::undo_a_rename_renames_it_back ... ok
test fileops::tests::undo_a_move_moves_the_item_back ... ok
test fileops::tests::undo_refuses_only_the_item_that_changed ... ok
test workspace::tests::a_long_random_sequence_keeps_the_invariants ... ok

test result: ok. 175 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running tests/trash.rs (target/debug/deps/trash-5dc9b4a126eac558)

running 12 tests
test child_cross_filesystem_move ... ignored
test child_mixed_decisions ... ignored
test child_new_folder_undo ... ignored
test child_replace_copy_and_undo ... ignored
test child_trash_and_undo ... ignored
test child_untrashable_source ... ignored, runs only as a child of an_untrashable_source_keeps_both_copies
test an_untrashable_source_keeps_both_copies ... ok
test new_folder_undo_trashes_only_an_empty_folder ... ok
test a_cross_filesystem_folder_move_trashes_its_source ... ok
test every_decision_runs_together_in_one_mix ... ok
test trash_and_its_undo_use_the_scratch_trash ... ok
test replace_trashes_the_old_target_and_undo_restores_it ... ok

test result: ok. 6 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/main.rs (target/debug/deps/gimbal_files-d8e87882e1328c49)

running 127 tests
test events::tests::a_window_event_opens_or_closes_the_one_window ... ok
test events::tests::only_group_formation_and_dissolution_fade ... ok
test events::tests::tab_group_and_focus_events_touch_the_header_and_content ... ok
test commands::tests::hotkey_mapping_keeps_the_command_and_its_arguments ... ok
test events::tests::the_other_events_touch_only_what_they_can_change ... ok
test fileops::clipboard::tests::a_cut_clears_the_clipboard_after_its_paste ... ok
test fileops::clipboard::tests::a_dolphin_cut_parses_from_its_flag_and_uri_list ... ok
test fileops::clipboard::tests::a_nautilus_copy_parses ... ok
test fileops::clipboard::tests::a_nautilus_cut_parses ... ok
test fileops::clipboard::tests::a_plain_uri_list_is_a_copy ... ok
test fileops::clipboard::tests::cut_and_copy_build_the_nautilus_and_dolphin_formats ... ok
test fileops::clipboard::tests::files_are_recognised_from_the_advertised_formats ... ok
test fileops::clipboard::tests::gnome_wins_over_kde_and_a_bad_gnome_string_is_an_error ... ok
test fileops::conflict::tests::a_single_conflict_has_no_apply_to_all_checkbox ... ok
test fileops::conflict::tests::cancel_runs_nothing_and_ends_the_questions ... ok
test fileops::conflict::tests::apply_to_all_partway_answers_the_rest_but_keeps_earlier_answers ... ok
test fileops::keys::tests::delete_and_the_arrows_belong_to_the_entry_with_or_without_alt ... ok
test fileops::conflict::tests::three_conflicts_are_answered_one_by_one ... ok
test fileops::keys::tests::editing_chords_belong_to_the_entry ... ok
test fileops::keys::tests::escape_belongs_to_the_entry_and_other_hotkeys_do_not ... ok
test fileops::menu::tests::a_mix_has_no_single_item_entries ... ok
test fileops::menu::tests::disk_usage_is_insensitive_when_baobab_is_missing ... ok
test fileops::menu::tests::empty_space_gets_the_background_menu ... ok
test fileops::menu::tests::one_file_has_no_folder_only_entries ... ok
test fileops::menu::tests::every_entry_is_a_registry_command ... ok
test fileops::menu::tests::one_folder_gets_open_in_new_tab_and_pin ... ok
test fileops::menu::tests::several_files_cannot_be_renamed_or_pinned ... ok
test fileops::menu::tests::undo_is_first_when_there_is_something_to_undo ... ok
test fileops::progress::tests::failures_list_the_path_and_the_message ... ok
test fileops::progress::tests::the_bar_prefers_bytes_and_falls_back_to_items ... ok
test fileops::progress::tests::summaries_cover_every_outcome_mix ... ok
test fileops::progress::tests::the_row_waits_half_a_second_and_then_shows_the_operation ... ok
test folder::sort::tests::every_key_works_in_both_orders ... ok
test folder::sort::tests::folders_come_first_in_both_orders_and_a_name_breaks_ties ... ok
test folder::sort::tests::names_are_case_insensitive_and_natural ... ok
test folder::sort::tests::only_the_sort_attribute_is_added ... ok
test folder::tests::dot_names_are_hidden ... ok
test folder::tests::shift_gets_the_visible_order_exactly ... ok
test folder::tests::status_counts_and_selection_read_naturally ... ok
test folder::view::tests::the_column_cap_follows_the_viewport_and_never_is_g_maxuint ... ok
test header::tests::a_root_folder_cannot_go_up ... ok
test layout::tests::a_retarget_uses_the_new_end_width ... ok
test header::tests::a_single_tab_shows_its_folder_name ... ok
test layout::tests::a_reverse_restarts_from_the_current_width ... ok
test layout::tests::collapsed_sidebars_are_strips ... ok
test layout::tests::defaults_match_the_sketch ... ok
test header::tests::the_view_marks_the_focused_pill_active_and_keeps_selection ... ok
test header::tests::a_split_group_shows_its_title_and_count_then_its_name ... ok
test layout::tests::giving_way_never_touches_the_saved_widths ... ok
test layout::tests::insets_and_positions_agree ... ok
test layout::tests::narrowing_drops_columns_then_sidebars_give_way ... ok
test layout::tests::one_column_counts_the_two_split_columns ... ok
test layout::tests::sidebar_motion_starts_mid_and_ends_on_both_sides ... ok
test layout::tests::the_button_glides_from_the_panel_to_the_strip_centre ... ok
test layout::tests::the_outline_fades_over_the_last_part_only ... ok
test layout::tests::the_strip_centres_the_expand_button ... ok
test look::blur::tests::region_rects_at_half_scale ... ok
test look::blur::tests::region_rects_degenerate_sizes ... ok
test look::blur::tests::region_rects_stay_inside_and_cut_the_corners ... ok
test look::css::tests::drag_arithmetic ... ok
test look::css::tests::drive_classes ... ok
test look::css::tests::a_selected_grid_cell_is_filled_after_the_transparent_rule ... ok
test look::css::tests::close_icon_is_symbolic_with_foreground_strokes ... ok
test look::css::tests::float_icon_is_symbolic_and_maps_the_front_fill ... ok
test look::css::tests::scaling ... ok
test look::css::tests::nov_1_outlines_and_contrast ... ok
test look::css::tests::opacity_follows_the_config_range ... ok
test look::css::tests::header_fits_stretches_then_collapses ... ok
test look::icons::tests::all_six_mockup_icons_are_embedded ... ok
test look::css::tests::pinned_list_has_no_fill_and_collapsed_pills_sit_six_apart ... ok
test look::css::tests::the_focused_tile_ring_is_drawn_inside_the_frame ... ok
test look::icons::tests::palettes_stay_on_theme_colours ... ok
test look::motion::tests::durations_scale_by_the_slowdown ... ok
test look::tokens::tests::scaled_tokens_keep_a_visible_pixel ... ok
test look::css::tests::icon_palettes_map_the_spec_table ... ok
test look::tokens::tests::tokens_lost_nothing_in_the_merge ... ok
test open::tests::a_folder_navigates_and_a_file_launches ... ok
test platform::tests::niri_floats_through_ipc_and_has_no_minimize ... ok
test look::theme::tests::imports_are_watched_once_each ... ok
test platform::tests::other_desktops_maximize_and_show_minimize ... ok
test save::tests::a_change_at_the_deadline_is_still_a_full_second_away ... ok
test save::tests::the_debounce_restarts_on_every_change_with_a_fake_clock ... ok
test sidebar::tests::drive_fullness_classes_at_the_edges ... ok
test sidebar::tests::drag_layout_uses_original_row_indices ... ok
test sidebar::tests::only_a_past_threshold_press_reorders ... ok
test sidebar::tests::recent_list_dedupes_caps_and_puts_the_newest_first ... ok
test split::tests::a_close_rebalances_two_or_more_apart ... ok
test split::tests::a_column_that_grows_past_the_view_stops_being_pinned ... ok
test look::css::tests::spec_rules_and_css_shape ... ok
test look::css::tests::spec_rules ... ok
test split::tests::a_drag_respects_the_grab_offset ... ok
test split::tests::a_drop_never_empties_a_column ... ok
test sidebar::tests::every_pin_reorder_moves_exactly_one_row ... ok
test split::tests::a_dragged_tile_keeps_its_pill_at_its_own_top ... ok
test split::tests::a_hand_drag_reaches_the_minimum_below_the_half_floor ... ok
test split::tests::a_pills_wrap_sits_with_its_tile_when_not_stuck ... ok
test split::tests::a_hand_made_layout_survives_a_close_that_leaves_one_gap ... ok
test split::tests::a_pinned_column_never_moves_its_pills ... ok
test split::tests::a_short_column_is_pinned_and_a_tall_one_scrolls ... ok
test split::tests::a_reset_returns_to_the_equal_default ... ok
test split::tests::a_slot_top_above_the_view_sticks_the_pill_and_the_frame_top ... ok
test split::tests::a_stuck_pills_wrap_holds_the_view_top_with_its_frame ... ok
test split::tests::a_visible_slot_leaves_its_pill_at_the_slot_top ... ok
test split::tests::below_the_minimum_the_whole_tile_scrolls_off ... ok
test split::tests::closing_from_either_column_leaves_the_other_taller ... ok
test split::tests::closing_the_lone_tile_rebalances ... ok
test split::tests::a_stuck_tiles_strip_follows_its_frame_top ... ok
test split::tests::dragging_down_grows_the_tile_above_and_moves_the_rest_down ... ok
test split::tests::drag_follows_the_pointer_and_previews_the_drop ... ok
test split::tests::dragging_up_shrinks_to_the_minimum_and_moves_the_rest_up ... ok
test split::tests::dropping_in_a_gap_inserts_and_the_column_gets_longer ... ok
test split::tests::dropping_on_a_tile_swaps_across_and_reorders_within ... ok
test split::tests::border_hit_test_covers_the_whole_strip_except_the_pill ... ok
test split::tests::hand_heights_keep_their_design_px_through_resizes_and_scale_changes ... ok
test split::tests::hit_test ... ok
test split::tests::pill_hit_test ... ok
test split::tests::the_column_border_still_trades_width ... ok
test split::tests::the_drop_line_spans_the_gap ... ok
test split::tests::the_frame_shrinks_to_one_grid_row_at_the_minimum ... ok
test split::tests::the_drop_zone_is_the_gap_plus_a_fifth_on_each_side ... ok
test split::tests::the_last_tile_fills_a_column_shorter_than_the_view ... ok
test split::tests::the_model_layout_converts_to_tile_rects_at_scale ... ok
test split::tests::the_state_keeps_the_model_permille ... ok
test split::tests::tiles_fill_the_height_and_split_the_width ... ok
test split::tests::uneven_columns_make_their_tiles_taller ... ok
test split::tests::which_tile_a_click_focuses ... ok
test split::tests::rects_never_overlap ... ok

test result: ok. 127 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests fm_agent

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests fm_core

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

==> cargo build --workspace --release
   Compiling fm-ui v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-ui)
    Finished `release` profile [optimized] target(s) in 1.81s
==> cargo deny check
warning[duplicate]: found 2 duplicate entries for crate 'system-deps'
   ┌─ /home/kyle/Documents/03.) Code-Projects/gimbal-files/Cargo.lock:72:1
   │  
72 │ ╭ system-deps 7.0.8 registry+https://github.com/rust-lang/crates.io-index
73 │ │ system-deps 9.0.1 registry+https://github.com/rust-lang/crates.io-index
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
     │   │   │   ├── gdk4-wayland v0.11.5
     │   │   │   │   └── fm-ui v0.1.0
     │   │   │   ├── gsk4 v0.11.5
     │   │   │   │   └── gtk4 v0.11.5
     │   │   │   │       ├── fm-ui v0.1.0 (*)
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
     ├── (build) gdk4-wayland-sys v0.11.5
     │   └── gdk4-wayland v0.11.5 (*)
     ├── (build) gio-sys v0.22.9
     │   ├── gdk-pixbuf-sys v0.22.9 (*)
     │   ├── gdk4-sys v0.11.5 (*)
     │   ├── gio v0.22.10
     │   │   ├── fm-core v0.1.0
     │   │   │   ├── fm-agent v0.1.0
     │   │   │   │   └── fm-cli v0.1.0
     │   │   │   ├── fm-cli v0.1.0 (*)
     │   │   │   └── fm-ui v0.1.0 (*)
     │   │   ├── gdk-pixbuf v0.22.0 (*)
     │   │   ├── gdk4 v0.11.5 (*)
     │   │   ├── gdk4-wayland v0.11.5 (*)
     │   │   ├── gtk4 v0.11.5 (*)
     │   │   ├── libadwaita v0.9.2 (*)
     │   │   └── pango v0.22.9
     │   │       ├── gdk4 v0.11.5 (*)
     │   │       ├── gsk4 v0.11.5 (*)
     │   │       ├── gtk4 v0.11.5 (*)
     │   │       └── libadwaita v0.9.2 (*)
     │   ├── glib v0.22.10
     │   │   ├── cairo-rs v0.22.9 (*)
     │   │   ├── fm-core v0.1.0 (*)
     │   │   ├── gdk-pixbuf v0.22.0 (*)
     │   │   ├── gdk4 v0.11.5 (*)
     │   │   ├── gdk4-wayland v0.11.5 (*)
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
     │   ├── gdk4-wayland-sys v0.11.5 (*)
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

### Live runs

Both runs used a temporary `GIMBAL_P2K_TIMER=1` harness in `app.rs` that
dispatched the same `Invocation`s through `Process::dispatch_for_window`, and a
temporary auto-answer hook so a conflict plan could be answered without pointer
injection. Both were removed before delivery:
`grep -rn 'P2K_TIMER\|auto_answer\|timer:' crates/fm-ui/src` prints nothing.
`[observed]`

`pgrep -a gimbal-files` was empty before and after both runs. Each app was
started with scratch `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `XDG_DATA_HOME`
under the run's folder, and stopped by PID with SIGTERM after 8-9 s. `[observed]`

**1. Copy, move, rename, new folder, undo** — `/tmp/gimbal-p2k-live1`. Three
source files were copied into a folder holding two of the same names; the
conflict was answered **Keep Both** with **apply to all**. Then a folder was
moved, a file renamed, a folder created, and undo ran three times. The new
folder's undo failed on `/tmp` because GIO refuses to trash there (the P2f rule);
the rename and move undos succeeded. The `fresh` folder therefore remains;
that is the expected `/tmp` limitation, not a UI failure. `[observed]`

```text
[gimbal 1.555] timer: ---- step 0: file.copy ----
[gimbal 1.555] fileops: started job 1: copy 3 item(s) to /tmp/gimbal-p2k-live1/dst
[gimbal 1.555] fileops: plan 1 (copy 3 item(s) to /tmp/gimbal-p2k-live1/dst): dry run: copy 3 item(s) to /tmp/gimbal-p2k-live1/dst | copy /tmp/gimbal-p2k-live1/src/one.txt -> /tmp/gimbal-p2k-live1/dst/one.txt [the target exists] | copy /tmp/gimbal-p2k-live1/src/two.txt -> /tmp/gimbal-p2k-live1/dst/two.txt [the target exists] | copy /tmp/gimbal-p2k-live1/src/three.txt -> /tmp/gimbal-p2k-live1/dst/three.txt
[gimbal 1.585] fileops: decision 1 one.txt: KeepBoth (apply to all: true)
[gimbal 1.585] fileops: answer job 1: decisions ready
[gimbal 1.585] fileops: undo state: trashed 3 items
[gimbal 1.585] fileops: report copy 3 item(s) to /tmp/gimbal-p2k-live1/dst: copied /tmp/gimbal-p2k-live1/src/one.txt -> /tmp/gimbal-p2k-live1/dst/one (copy).txt | copied /tmp/gimbal-p2k-live1/src/two.txt -> /tmp/gimbal-p2k-live1/dst/two (copy).txt | copied /tmp/gimbal-p2k-live1/src/three.txt -> /tmp/gimbal-p2k-live1/dst/three.txt
[gimbal 1.615] fileops: progress 3/3 items, 14/14 bytes
[gimbal 1.615] fileops: summary: Copied 3 items
[gimbal 2.256] fileops: plan 2 (move 1 item(s) to /tmp/gimbal-p2k-live1/dst): dry run: move 1 item(s) to /tmp/gimbal-p2k-live1/dst | move /tmp/gimbal-p2k-live1/src/move-me -> /tmp/gimbal-p2k-live1/dst/move-me
[gimbal 2.286] fileops: undo state: moved 1 item back
[gimbal 2.316] fileops: summary: Moved 1 item
[gimbal 2.956] fileops: plan 3 (rename /tmp/gimbal-p2k-live1/src/rename-me.txt to renamed.txt): dry run: rename /tmp/gimbal-p2k-live1/src/rename-me.txt to renamed.txt | rename /tmp/gimbal-p2k-live1/src/rename-me.txt -> /tmp/gimbal-p2k-live1/src/renamed.txt
[gimbal 2.987] fileops: undo state: renamed 1 item back
[gimbal 3.016] fileops: summary: Renamed 1 item
[gimbal 3.657] fileops: plan 4 (create folder fresh in /tmp/gimbal-p2k-live1/src): dry run: create folder fresh in /tmp/gimbal-p2k-live1/src | new-folder /tmp/gimbal-p2k-live1/src -> /tmp/gimbal-p2k-live1/src/fresh
[gimbal 3.687] fileops: undo state: trashed 1 item
[gimbal 3.717] fileops: summary: Created folder “fresh”
[gimbal 4.388] fileops: report undo the last file operation: failed /tmp/gimbal-p2k-live1/src/fresh: Trashing on system internal mounts is not supported
[gimbal 4.417] fileops: summary: Undid: trashed 1 item; 1 not undone: fresh (Trashing on system internal mounts is not supported)
[gimbal 5.088] fileops: report undo the last file operation: renamed /tmp/gimbal-p2k-live1/src/renamed.txt -> /tmp/gimbal-p2k-live1/src/rename-me.txt
[gimbal 5.118] fileops: summary: Undid: renamed 1 item back
[gimbal 5.788] fileops: report undo the last file operation: moved /tmp/gimbal-p2k-live1/dst/move-me -> /tmp/gimbal-p2k-live1/src/move-me
[gimbal 5.818] fileops: progress 1/1 items, 0/6 bytes
[gimbal 5.818] fileops: summary: Undid: moved 1 item back
[gimbal 6.458] timer: ---- done ----
```

**2. Trash and its undo** — `$HOME/.cache/gimbal-tests/p2k-live2`, with
`XDG_DATA_HOME` inside that folder. A folder was created and undone, then two
files were trashed and undone. The scratch trash received the undone folder and
the two files, then the two files were restored. Kyle's real
`~/.local/share/Trash` hash was identical before and after:
`89918530475c251a2e5a82a5a9b3cc81826ae134f6f39af468fb59671979ec20`. `[observed]`

```text
[gimbal 1.557] timer: ---- step 0: file.new-folder ----
[gimbal 1.557] fileops: plan 1 (create folder cache-folder in /home/kyle/.cache/gimbal-tests/p2k-live2/src): dry run: create folder cache-folder in /home/kyle/.cache/gimbal-tests/p2k-live2/src | new-folder /home/kyle/.cache/gimbal-tests/p2k-live2/src -> /home/kyle/.cache/gimbal-tests/p2k-live2/src/cache-folder
[gimbal 1.617] fileops: summary: Created folder “cache-folder”
[gimbal 2.257] timer: ---- step 1: file.undo ----
[gimbal 2.257] fileops: plan 2 (undo the last file operation): dry run: undo the last file operation | trash /home/kyle/.cache/gimbal-tests/p2k-live2/src/cache-folder
[gimbal 2.289] fileops: report undo the last file operation: trashed /home/kyle/.cache/gimbal-tests/p2k-live2/src/cache-folder
[gimbal 2.318] fileops: summary: Undid: trashed 1 item
[gimbal 2.958] timer: ---- step 2: file.trash ----
[gimbal 2.958] fileops: plan 3 (trash 2 item(s)): dry run: trash 2 item(s) | trash /home/kyle/.cache/gimbal-tests/p2k-live2/src/one.txt | trash /home/kyle/.cache/gimbal-tests/p2k-live2/src/two.txt
[gimbal 2.989] fileops: undo state: restored 2 items
[gimbal 2.989] fileops: report trash 2 item(s): trashed /home/kyle/.cache/gimbal-tests/p2k-live2/src/one.txt | trashed /home/kyle/.cache/gimbal-tests/p2k-live2/src/two.txt
[gimbal 3.018] fileops: progress 2/2 items, 0/? bytes
[gimbal 3.018] fileops: summary: Moved 2 items to the trash
[gimbal 3.659] timer: ---- step 3: file.undo ----
[gimbal 3.659] fileops: plan 4 (undo the last file operation): dry run: undo the last file operation | restore /home/kyle/.cache/gimbal-tests/p2k-live2/data/Trash/files/one.txt -> /home/kyle/.cache/gimbal-tests/p2k-live2/src/one.txt | restore /home/kyle/.cache/gimbal-tests/p2k-live2/data/Trash/files/two.txt -> /home/kyle/.cache/gimbal-tests/p2k-live2/src/two.txt
[gimbal 3.689] fileops: report undo the last file operation: restored /home/kyle/.cache/gimbal-tests/p2k-live2/data/Trash/files/one.txt -> /home/kyle/.cache/gimbal-tests/p2k-live2/src/one.txt | restored /home/kyle/.cache/gimbal-tests/p2k-live2/data/Trash/files/two.txt -> /home/kyle/.cache/gimbal-tests/p2k-live2/src/two.txt
[gimbal 3.719] fileops: summary: Undid: restored 2 items
[gimbal 4.359] timer: ---- done ----
```

### Cleanup

After both runs:

```text
$ find /tmp -maxdepth 1 -name 'gimbal-p2k-*' -print
$ test -e ~/.cache/gimbal-tests && echo present || echo absent
absent
$ find ~/.local/share/Trash -mindepth 1 -maxdepth 1 -printf '%P\n' | sort | sha256sum
89918530475c251a2e5a82a5a9b3cc81826ae134f6f39af468fb59671979ec20  -
```

Both scratch folders and the temporary timer code were removed before the
final `scripts/check.sh` above. `[observed]`

## Not verified live (Claude checks these)

- The item and background menus with real right-clicks, including selecting an
  unselected item, focusing a split tile, and the empty-space gesture.
- The conflict dialog with real clicks, including the "Apply this decision to
  all files" checkbox and Cancel.
- The progress row on a large copy, and Cancel.
- The Delete key.
- Copy To… and Move To… through `gtk::FileDialog::select_folder`.
- Undo of a trash through the UI (the worker path was exercised in the timer
  run, not a click).
- Cut, copy and paste, including between Gimbal and Dolphin/Nautilus. No agent
  run wrote to the live clipboard.
- Ctrl+C and Ctrl+V inside the path bar, and the other editing keys in the
  rename/new-folder popovers.

## Anything outside my files

Nothing needs changing. `fm-core` already had every operation, plan, decision,
progress callback and undo record this brief needed; no core edit was made and
`spike/` was read only. `[observed]`

The one design choice worth a second look is the dry run of `file.paste`: GDK's
clipboard has only an async read, so it logs its plan lines instead of returning
`Outcome::DryRun`. If that must be returned synchronously, it would need a core
or clipboard API change outside this brief's files. `[inferred]`

## Review and live check (Claude, Nov 2, 2026): passes, with two fixes by Claude

`scripts/check.sh` from `/tmp`: exit 0. The tests are 127 `fm-ui`, 175 `fm-core`, the 6 trash
parents, 7 CLI and 1 agent, and deny is clean. The UI code has no permanent-delete call; the one
`remove_dir_all` is in `theme.rs`'s test module. Kyle's real Trash was snapshotted before the live
check and was identical after, timestamps included. His config and session are untouched since
10:23. All scratch folders are gone.

**Live** (release build, niri-tiled on DP-2 at 2256×1504). Files were under
`~/.cache/gimbal-tests/p2k-review` with `XDG_DATA_HOME` inside it, so the app's trash was the
scratch one; config and state were in `/tmp`.
- **Item menu** with three files selected: Open, Cut, Copy, Copy To…, Move To… and Move to Trash.
  Rename and Pin are correctly absent.
- **Copy:** logged `copy put 3 item(s) on the clipboard`.
- **Empty-space menu** (after fix 1): New Folder…, Paste (enabled), Pin This Folder, Refresh and
  Disk Usage (`baobab` is installed). Undo appeared at the top once there was a record.
- **Paste into a folder holding `a.txt` and `b.txt`** gave "2 conflict(s) need a decision".
  - The dialog read "“a.txt” already exists … (1 of 2)", with the "Apply this decision to all
    files" checkbox and Cancel, Replace (destructive), Keep Both and Skip.
  - After fix 2, ticking the box and choosing Keep Both copied `a (copy).txt`, `b (copy).txt` and
    `c.txt`, and left the originals unchanged.
- **Undo of that copy** trashed the three copies into the scratch trash ("Undid: trashed 3
  items").
- **Delete** on `c.txt` ran `file.trash via Hotkey` and moved it to the trash as `c.2.txt` (a
  `c.txt` was already there).
- **Undo of the trash** restored the right entry: `src/c.txt` came back with its own content, and
  `c.2.txt` and its `.trashinfo` left the trash while the older `c.txt` stayed.
- **800 MB copy:** done in 60 ms (btrfs clones) and under a second into `/tmp`. The progress row
  correctly stayed hidden, and the summary read "Copied 20 items".
- Claude's screenshots go through niri, which copies each one to the clipboard. So Copy and Paste
  were tested with no screenshot in between.

**Fixes (Claude):**
1. **The empty-space menu never appeared.**
   - The cause: it was a popover parented to the whole grid scroller with no `pointing-to`, so GTK
     placed it against the scroller's edge, outside the window.
   - The fix: the right-click position now goes through `background_click` and
     `right_click_background` to `dialogs::show_menu(…, Some((x, y)))`, which sets `pointing-to` at
     the click. The item menu keeps its cell anchor (`None`).
2. **Every conflict answer became Cancel.**
   - The cause: libadwaita emits `closed` before `response` when a button is clicked
     (`adw-alert-dialog.c:278-279`). The `closed` handler saw no answer yet, cancelled the
     question and sent the decisions to the worker; the real answer came too late. The log showed
     "closed without an answer; cancelling a.txt" right before "decision 1 a.txt: KeepBoth".
   - The fix: the closed handler now checks from an idle, after the response handler has run, so a
     dialog closed without an answer still cancels.
   - P2h's group-close confirmation was checked too: its `closed` handler only counts dialogs, so
     it isn't affected.

Not checked live, for Kyle: the progress row and Cancel on a long operation (both copies here
finished too fast), Copy To… and Move To… through the folder dialog, typing in the Rename and New
Folder popovers, Ctrl+C and Ctrl+V inside the path bar, Cut and Paste with real keys, and pasting to
and from Dolphin.

Cosmetic, for a later brief: the Undo menu item's label describes what undo will do, so undoing a
copy reads "Undo trashed 3 items". "Undo copy of 3 items" would read better. The status line's
"Undid: trashed 3 items" is fine.

## Fixes after Kyle's check (Claude, Nov 2, 2026)

Kyle reported that cut, copy, paste and undo with keys, Copy To… / Move To…, pasting to and from
Dolphin, and the progress bar on a large file all work. He also found two problems.

1. **"The tab pill's name doesn't change when a folder inside the tab is entered."**
   - The header pill already followed navigation (checked: "kyle" → "backups"). The split-view
     **tile** pills didn't: their label was set once in `add_tile`.
   - `sync_folder` now sets the tile pill's label to the folder's name whenever the tile's folder
     changes. Verified: double-clicking `inner` in the right tile changed its pill from "top" to
     "inner", and the left tile kept "top".
2. **"No indicator when an item is selected."** There were two causes.
   - **The style was too faint:** a translucent tint, hard to see on the blurred background.
     Kyle chose (Nov 2) a solid accent block with the name in the accent's contrasting colour. A
     selected cell is now `@accent_bg_color` with `@accent_fg_color` text. Its folder icon uses
     `SELECTED_FOLDER_PALETTE` (`@accent_fg_color`, shading mixed toward the block), so it doesn't
     vanish into the accent. The test was updated. Verified in the single grid and in a tile.
   - **Split tiles dropped every selection:**
     - `sync_folder` cloned `tile.displayed_folder`, a plain `RefCell`, so it wrote to a copy. Each
       tile always believed it showed nothing, so every model event (a click, a selection, a focus
       change) re-listed every tile's folder.
     - The re-list cleared the selection and reset the view, so the grid never showed the
       selection.
     - `displayed_folder` is now `Rc<RefCell<…>>`. Verified: after the fix each tile lists its
       folder once at start, a selection no longer re-lists anything, and both selected items show
       the accent block.

Also from this check:
- Claude's first test launch handed its folder to Kyle's running window (GApplication), adding a
  tab for `/tmp/gimbal-p2x/top`. `CLAUDE.md` now says to run test copies under `dbus-run-session`
  when Kyle's instance is running, which is how the rest of this check ran.
- Kyle's WD-Blue-SSD, now mounted, shows under Devices with its bar at 68% of 1.0 TB.

**Selection, revised (Kyle, Nov 2):** Kyle didn't want the folder icons recoloured. A selected
cell is now `alpha(@accent_bg_color, 0.45)`, and the icon and the name keep their normal colours.
`SELECTED_FOLDER_PALETTE` and the label and icon overrides are gone. Measured in a split tile:
selected (90, 50, 154) against unselected (44, 13, 87).

Claude's mistake during this check: the relaunch helper picked Gimbal windows by excluding Kyle's
*old* process ID. Kyle had restarted the app, so the helper moved and resized his window (to
workspace 5) and two test clicks selected items in it. Nothing was opened or changed on disk, and
the screenshot of his window was deleted. The helper now records every window before launching
and only takes the one new window, aborting otherwise.
