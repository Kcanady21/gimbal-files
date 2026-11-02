# P2i: the folder view and the left sidebar

## What I built

`[observed]` unless a line says otherwise.

- **The folder view** -- `crates/fm-ui/src/folder/`:
  - `view.rs` is the pipeline `DirectoryList` -> `FilterListModel` (hidden) ->
    `SortListModel` -> `MultiSelection` -> `GridView`. `DirectoryList` loads
    asynchronously and watches the folder, so created, removed and renamed
    files arrive live. Every `set_file` cancels the old load and clears its
    items in one change (P1e's observation).
  - The attributes are `standard::name`, `standard::display-name`,
    `standard::type`, `standard::icon`, plus exactly one sort attribute:
    `standard::size`, `time::modified` or `standard::content-type`. Changing
    the sort key changes this list, which starts a fresh enumeration; an
    order-only change calls `Sorter::changed(Different)` in place.
  - `SortListModel::incremental` is off, as P1e-r1 delivered it: each batch is
    sorted before the next arrives, so a position is final when `loading`
    goes false. The comparison is pure in `folder/sort.rs`: case-insensitive
    natural names (`file2` before `file10`), folders first when configured,
    then name / modified / size / type, reversed only for the chosen key,
    with a natural-name tie-break.
  - Names starting with `.` are filtered by a `CustomFilter` on
    `standard::name`; the model and the counts never see them.
  - Cells are the spike's `gl-cell`: fixed `CELL_W` width from the token
    table, the mockup folder as `gtk::Svg`, and `info.icon()` from the theme
    for files. The label has `width_chars`/`max_width_chars` 9,
    `WrapMode::WordChar`, two lines and end ellipsis, so no long name can
    widen a column (P1e's widest-name trap).
  - `max_columns` is `(viewport_width + gap) / (cell + gap) + 1`, clamped to
    `1..=64`; it is recomputed from the h-adjustment's `page-size` notify in
    an idle (`glib::idle_add_local_once`), never from inside allocation
    (`gtkwidget.c:3709`). `G_MAXUINT` is never used.
  - The status line is `Files: N, Folders: M` plus `, K selected` when the
    model's selection is non-empty. Counts are recomputed in an idle after
    live changes.
  - A folder that cannot be read shows the GIO error message centred over
    the grid, logs it once (`folder: cannot read ...`), and leaves navigation
    working. The error overlay is cleared as soon as another folder starts.
- **Selection and opening, through the model**:
  - Each cell has a `GestureClick`. A click builds a pure `ClickRequest`:
    plain -> `file_click`, Ctrl -> `file_ctrl_click`, Shift ->
    `file_shift_click` with the visible order from the `MultiSelection`.
    `Process` then calls the model; the grid is re-synced from the model's
    `FileSelection` every time the content refreshes.
  - `selection.all` passes `FolderView::listed_names()` (the filtered, sorted
    model), and `selection.none` calls `Workspace::selection_none`. Ctrl+A
    reaches the first through the existing hotkey.
  - `connect_activate` handles double-click and Enter. `open.rs` is the pure
    decision: a folder -> `nav.go`, a file -> `file.open`'s launch path,
    missing -> a log line. The filesystem test is
    `path.is_dir()`/`path.is_file()`, both of which follow a symlink, so a
    symlink opens its target. `Process::launch_file` uses
    `gio::AppInfo::launch_default_for_uri` on the file's URI and logs a
    failure; it is never called by a test or by these runs.
  - New folder: top. Tab switch or restore: the tab's `scroll_anchor`, else
    the view's remembered per-folder anchor. Back/forward: the app arms one
    restore flag, and the view uses its remembered anchor because the model
    clears `scroll_anchor` on navigation. `nav.refresh` re-reads the current
    folder and keeps the selection and top item.
  - The scroll anchor is the first visible item found from the realised cell
    bounds, recorded 300 ms after the v-adjustment changes. It goes to
    `Workspace::record_scroll_anchor` and to a per-folder map in the view.
- **The left sidebar** -- `crates/fm-ui/src/sidebar.rs`:
  - Pinned Folders come from `config.pins` in order, with the spike's
    `PinnedRows` + `PinnedRowLayout` (44 px rows at scale 1). A click selects,
    a double-click runs `nav.go`, the focused tab's folder gets `sel`, and a
    press only reorders after the drag threshold. Release dispatches
    `pins.move`; the config's `after_change` rebuilds the sidebars from the
    saved list. Right-click opens a one-item `Unpin` popover that dispatches
    `pins.remove`. No add menu yet (P2k).
  - Recent Directories is the last five distinct folders from this window's
    `Navigated` events, newest first, deduped. It is only in memory; saving it
    is a later session change. A click runs `nav.go`.
  - Devices lists every `gio::VolumeMonitor` mount except the root filesystem:
    mount name, total size from `filesystem::size`, and a drive bar whose fill
    is `(size - free) / size` from `filesystem::free` through
    `query_filesystem_info`. It uses the spike's `bar_class`: contrast rises
    through `bar-lvN` to 94 %, and `bar-full` / the error label from 95 %.
    Mount added/removed/changed rebuild the rows. A click navigates to the
    mount point.
  - Remote says `No remotes` for now.
  - Recent/Devices/Remote are `Revealer`s with `SlideDown` and
    `motion::ms(200)`; their pills dispatch `view.toggle-section`, whose
    config change updates every window's revealers. The CSS keeps the
    collapsed 6 px gap and the expanded `gap-m` tail.
  - The main drive at the bottom is the filesystem holding `/`, with the same
    bar. Pinned/recent/device sections live in one vertical scroller, so the
    sidebar scrolls when it overflows; the main drive stays at the bottom.
- **Commands**:
  - `file.open` now launches the paths from the checked command, or the
    focused tab's selection when the context supplies none.
  - `selection.all`, `selection.none` and `nav.refresh` are handled; F5 and
    the path bar's refresh button now reach the view.
  - `pins.add`, `pins.remove` and `pins.move` reach the config as before; the
    sidebars follow every change live. `Change::SetSection` updates revealers
    and `Change::SetSort` re-sorts every window's current folder.
  - The `Settings` path still calls `change_for` with the focused tab's
    folder as the current folder (P2h already did this; P2i keeps it).
  - A split group still shows P2h's placeholder until P2j.

Choices where the brief leaves room:

- `type` sorting compares `standard::content-type`, so it is a MIME-type
  order with the natural-name tie-break. `standard::type` is still queried
  for folders-first and the folder/file cell icon.
- A UTF-8 grid item opens through the named `nav.go` or `file.open` command.
  A non-UTF-8 name falls back to the same model navigation or
  `launch_default_for_uri` code directly, because an `Invocation` argument is
  a string and would otherwise lose the name.
- `max_columns` is clamped in the pure helper as well as in the idle, so a
  test cannot ask for `G_MAXUINT`.
- A split group's status line is `Files: 0, Folders: 0`; P2j owns the real
  split status.

## Verified

### `scripts/check.sh`

Run from `$PROJ` with the rustup 1.95.0 toolchain on `PATH`; exit 0.
Workspace: 248 passed, 6 ignored. `fm-ui`: 60 tests (46 from P2g/P2h plus 14
new); `fm-core`: 174. `[observed]`

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
test tests::anything_else_is_a_usage_error ... ok
test tests::check_without_a_command_is_a_usage_error ... ok
test tests::check_keeps_the_command_and_its_arguments ... ok
test tests::help_shows_usage ... ok
test tests::commands_lists_the_registry ... ok
test tests::no_arguments_shows_usage ... ok
test tests::version_shows_the_version ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (target/debug/deps/fm_core-71b31b2576b0b33f)

running 174 tests
test command::tests::a_known_command_is_not_an_unknown_command ... ok
test command::tests::a_forbidden_call_never_reaches_the_handler ... ok
test command::tests::a_files_command_defaults_dry_run_to_false ... ok
test command::tests::a_checked_command_prints_in_catalogue_order ... ok
test command::tests::a_token_without_equals_is_an_error ... ok
test command::tests::a_single_valued_argument_may_not_repeat ... ok
test command::tests::a_valid_call_reaches_the_handler_once ... ok
test command::tests::an_invalid_call_never_reaches_the_handler ... ok
test command::tests::an_empty_argument_name_is_an_error ... ok
test command::tests::a_value_may_contain_equals_and_spaces ... ok
test command::tests::argument_kinds_are_sane ... ok
test command::tests::a_distant_typo_gets_no_suggestion ... ok
test command::tests::bool_accepts_true_and_false_only ... ok
test command::tests::argument_names_are_unique_within_a_command ... ok
test command::tests::choice_must_be_one_of_the_fixed_list ... ok
test command::tests::cli_and_agent_may_not_fill_files_paths_from_context ... ok
test command::tests::cli_and_agent_must_give_every_files_context_path ... ok
test command::tests::context_rules_are_pure_functions_over_every_caller ... ok
test command::tests::every_files_command_gets_exactly_one_dry_run_added_by_the_registry ... ok
test command::tests::every_caller_may_give_explicit_files_paths ... ok
test command::tests::missing_required_argument_is_named ... ok
test command::tests::int_accepts_the_range_and_rejects_bad_and_out_of_range_values ... ok
test command::tests::no_command_deletes_permanently ... ok
test command::tests::names_are_area_dot_verb_in_lowercase_kebab_case ... ok
test command::tests::no_tokens_is_a_missing_command ... ok
test command::tests::number_values_are_hundredths_and_print_with_two_places ... ok
test command::tests::names_are_unique ... ok
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
test config::tests::commands_that_need_the_current_folder_use_the_argument ... ok
test config::tests::an_unparsable_file_reports_one_problem_with_a_position ... ok
test config::tests::an_unknown_key_is_a_warning_with_its_position ... ok
test config::tests::every_default_matches_the_brief ... ok
test config::tests::a_bad_value_falls_back_and_the_good_values_still_load ... ok
test config::tests::every_settings_command_has_a_change_for ... ok
test config::tests::a_hotkeys_table_replaces_listed_keys_and_an_empty_value_unbinds ... ok
test config::tests::default_pins_include_home_then_only_existing_special_folders ... ok
test config::tests::an_unparsable_file_is_never_written ... ok
test config::tests::a_sidebar_and_section_save_keep_their_inline_formatting ... ok
test config::tests::a_save_rewrites_only_the_changed_key_on_a_commented_file ... ok
test config::tests::a_hotkey_with_an_unknown_command_or_bad_arguments_keeps_the_default ... ok
test config::tests::every_problem_kind_reports_its_line_and_column ... ok
test config::tests::set_scale_and_sort_map_to_the_expected_changes ... ok
test fileops::tests::cancel_ends_the_questions_at_once ... ok
test config::tests::saving_is_atomic_and_keeps_permissions ... ok
test config::tests::zoom_stops_at_the_scale_limits_and_toggles_flip ... ok
test config::tests::saving_creates_the_folder_and_the_changed_key ... ok
test config::tests::pin_changes_write_the_pins_list ... ok
test fileops::tests::a_command_outside_the_file_operations_is_not_a_request ... ok
test config::tests::sort_for_expands_home_and_normalizes_dot_dot_and_trailing_slashes ... ok
test fileops::tests::a_cancel_decision_runs_nothing ... ok
test fileops::tests::a_cancel_partway_still_runs_nothing ... ok
test fileops::tests::apply_to_all_does_not_overwrite_an_earlier_answer ... ok
test config::tests::sort_changes_write_one_global_setting_or_one_folder_rule ... ok
test fileops::tests::copy_names_add_copy_then_numbers ... ok
test config::tests::unknown_keys_and_tables_are_warnings_and_stay_in_the_file ... ok
test fileops::tests::a_folder_into_itself_is_refused ... ok
test config::tests::prints_a_sample_problem_list ... ok
test fileops::tests::names_are_checked_against_the_rules ... ok
test fileops::tests::missing_sources_are_refused_for_their_item ... ok
test fileops::tests::a_copy_with_a_conflict_lists_the_target ... ok
test fileops::tests::moving_into_the_folder_an_item_is_already_in_is_refused ... ok
test fileops::tests::a_new_folder_with_a_taken_or_invalid_name_is_a_conflict ... ok
test fileops::tests::request_mapping_refuses_unfilled_context_paths ... ok
test fileops::tests::a_dry_run_changes_nothing_on_disk ... ok
test fileops::tests::the_first_answer_can_apply_to_all ... ok
test fileops::tests::decisions_cancel_with_apply_to_all_marks_the_run_cancelled ... ok
test fileops::tests::three_conflicts_are_answered_one_by_one ... ok
test fileops::tests::trashinfo_keeps_the_first_occurrence_of_a_key ... ok
test fileops::tests::trashinfo_percent_decodes_non_utf8_paths ... ok
test fileops::tests::trashinfo_percent_decodes_the_path ... ok
test fileops::tests::every_files_command_maps_to_its_request ... ok
test fileops::tests::trashinfo_reads_both_date_forms ... ok
test fileops::tests::trashinfo_reports_a_missing_key_header_or_bad_value ... ok
test fileops::tests::renaming_to_an_invalid_or_taken_name_is_a_conflict ... ok
test fileops::tests::undo_with_nothing_on_the_stack_is_an_error ... ok
test fileops::tests::unanswered_conflicts_refuse_the_whole_operation_and_name_the_items ... ok
test session::tests::a_missing_file_gives_one_window_at_home_without_a_problem ... ok
test session::tests::session_path_is_a_pure_function_of_xdg_state_home_and_home ... ok
test session::tests::a_version_2_file_gives_one_window_at_home_and_a_problem ... ok
test tests::app_id_matches_the_niri_rule_and_is_a_valid_gapplication_id ... ok
test session::tests::an_empty_workspace_is_never_saved ... ok
test session::tests::preserve_bad_replaces_an_older_copy ... ok
test workspace::split::tests::a_close_rebalances_two_or_more_apart ... ok
test workspace::split::tests::a_drop_never_empties_a_column ... ok
test workspace::split::tests::a_hand_made_three_plus_one_survives_a_close_from_the_long_column ... ok
test workspace::split::tests::a_slot_past_the_end_changes_nothing ... ok
test session::tests::an_unparsable_file_gives_one_window_at_home_and_a_problem ... ok
test session::tests::a_missing_folder_falls_back_to_its_parent_then_to_home ... ok
test workspace::split::tests::a_swap_keeps_heights_while_an_insert_resets_the_changed_column ... ok
test workspace::split::tests::closing_the_lone_tile_rebalances ... ok
test workspace::split::tests::ctrl_click_toggles_without_moving_the_anchor ... ok
test workspace::split::tests::dropping_in_a_gap_inserts_and_the_column_gets_longer ... ok
test workspace::split::tests::dropping_on_a_tile_swaps_across_and_reorders_within ... ok
test workspace::split::tests::plain_click_clears_and_moves_the_anchor ... ok
test workspace::split::tests::initial_columns_fill_the_shorter_column ... ok
test workspace::split::tests::shift_click_selects_the_range_from_the_anchor ... ok
test session::tests::a_non_utf8_folder_and_file_name_round_trip ... ok
test workspace::tests::a_failed_operation_changes_nothing ... ok
test workspace::tests::a_fresh_workspace_has_one_focused_tab_and_holds_the_invariants ... ok
test workspace::tests::a_drop_that_would_empty_a_column_is_refused_and_changes_nothing ... ok
test workspace::tests::closing_a_window_focuses_another_and_the_last_says_so ... ok
test workspace::tests::a_hand_made_three_plus_one_survives_a_close_from_the_long_column ... ok
test workspace::tests::closing_a_tab_inside_a_group_focuses_the_next_tile_else_the_previous ... ok
test workspace::tests::closing_the_last_pill_closes_the_window ... ok
test workspace::tests::a_tile_drop_swaps_reorders_and_inserts ... ok
test session::tests::a_rich_workspace_round_trips ... ok
test workspace::tests::file_clicks_follow_the_strip_rules ... ok
test workspace::tests::focus_after_a_close_goes_right_else_left ... ok
test workspace::tests::closing_two_tiles_from_a_three_plus_three_group_rebalances_to_two_plus_two ... ok
test workspace::tests::focus_window_moves_the_focus_and_refuses_an_unknown_id ... ok
test workspace::tests::group_add_tab_joins_a_single_pill_into_a_new_split ... ok
test workspace::tests::group_add_tab_uses_the_shorter_column_and_resets_that_columns_heights ... ok
test workspace::tests::group_close_closes_every_tab_and_focuses_the_neighbour ... ok
test workspace::tests::group_rename_trims_and_rejects_an_empty_name ... ok
test workspace::tests::group_split_inserts_at_the_first_selected_pill ... ok
test workspace::tests::ids_are_stable_and_never_reused ... ok
test workspace::tests::group_split_merges_selected_groups_and_needs_two_tabs ... ok
test workspace::tests::group_split_uses_the_lowest_free_number ... ok
test workspace::tests::nav_back_and_forward_move_between_folders_and_do_nothing_on_empty_stacks ... ok
test workspace::tests::nav_go_to_a_file_goes_to_its_folder_and_selects_the_name ... ok
test workspace::tests::hand_heights_need_one_entry_per_tile_and_the_width_share_is_permille ... ok
test workspace::tests::nav_go_pushes_history_and_clears_selection_and_anchor ... ok
test workspace::tests::history_stacks_are_capped_at_one_hundred_dropping_the_oldest ... ok
test workspace::tests::nav_up_stops_at_the_root ... ok
test workspace::tests::pill_clicks_follow_the_strip_rules ... ok
test workspace::tests::only_split_groups_take_group_numbers ... ok
test workspace::tests::restore_repairs_a_focused_tab_out_of_range ... ok
test workspace::tests::restore_drops_an_empty_group_and_an_empty_window ... ok
test workspace::tests::restore_repairs_a_width_share_over_1000 ... ok
test workspace::tests::restore_repairs_hand_heights_of_the_wrong_length ... ok
test workspace::tests::restore_replaces_a_column_list_that_does_not_match_its_tabs ... ok
test workspace::tests::selection_all_uses_the_handlers_names_and_none_clears_them ... ok
test workspace::tests::tab_close_selected_closes_the_selected_pills_and_focuses_right_else_left ... ok
test workspace::tests::tab_focus_rejects_an_index_past_the_end_and_changes_nothing ... ok
test workspace::tests::restore_trims_history_from_the_oldest_end ... ok
test workspace::tests::tab_move_reorders_pills_and_rejects_bad_indices ... ok
test workspace::tests::tab_new_in_an_empty_workspace_uses_home ... ok
test workspace::tests::tab_new_opens_after_the_focused_pill_and_inherits_its_folder ... ok
test workspace::tests::tab_new_opens_after_a_group_pill_and_not_inside_it ... ok
test workspace::tests::the_scroll_anchor_is_recorded_and_a_repeat_is_a_no_op ... ok
test workspace::tests::tab_open_in_new_window_moves_the_tab_out ... ok
test workspace::tests::the_second_launch_with_no_paths_opens_one_window_at_home ... ok
test workspace::tests::the_second_launch_with_folders_opens_one_tab_each_in_the_focused_window ... ok
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

test result: ok. 174 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

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

running 60 tests
test events::tests::only_group_formation_and_dissolution_fade ... ok
test events::tests::a_window_event_opens_or_closes_the_one_window ... ok
test commands::tests::hotkey_mapping_keeps_the_command_and_its_arguments ... ok
test events::tests::tab_group_and_focus_events_touch_the_header_and_content ... ok
test events::tests::the_other_events_touch_only_what_they_can_change ... ok
test folder::sort::tests::every_key_works_in_both_orders ... ok
test folder::sort::tests::folders_come_first_in_both_orders_and_a_name_breaks_ties ... ok
test folder::sort::tests::only_the_sort_attribute_is_added ... ok
test folder::sort::tests::names_are_case_insensitive_and_natural ... ok
test folder::tests::dot_names_are_hidden ... ok
test folder::tests::shift_gets_the_visible_order_exactly ... ok
test folder::tests::status_counts_and_selection_read_naturally ... ok
test folder::view::tests::the_column_cap_follows_the_viewport_and_never_is_g_maxuint ... ok
test header::tests::a_root_folder_cannot_go_up ... ok
test header::tests::a_single_tab_shows_its_folder_name ... ok
test layout::tests::a_reverse_restarts_from_the_current_width ... ok
test layout::tests::collapsed_sidebars_are_strips ... ok
test header::tests::the_view_marks_the_focused_pill_active_and_keeps_selection ... ok
test layout::tests::a_retarget_uses_the_new_end_width ... ok
test layout::tests::defaults_match_the_sketch ... ok
test header::tests::a_split_group_shows_its_title_and_count_then_its_name ... ok
test layout::tests::insets_and_positions_agree ... ok
test layout::tests::giving_way_never_touches_the_saved_widths ... ok
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
test look::css::tests::close_icon_is_symbolic_with_foreground_strokes ... ok
test look::css::tests::scaling ... ok
test look::css::tests::float_icon_is_symbolic_and_maps_the_front_fill ... ok
test look::css::tests::opacity_follows_the_config_range ... ok
test look::css::tests::nov_1_outlines_and_contrast ... ok
test look::css::tests::header_fits_stretches_then_collapses ... ok
test look::css::tests::pinned_list_has_no_fill_and_collapsed_pills_sit_six_apart ... ok
test look::motion::tests::durations_scale_by_the_slowdown ... ok
test look::icons::tests::all_six_mockup_icons_are_embedded ... ok
test look::icons::tests::palettes_stay_on_theme_colours ... ok
test look::tokens::tests::scaled_tokens_keep_a_visible_pixel ... ok
test look::tokens::tests::tokens_lost_nothing_in_the_merge ... ok
test look::css::tests::icon_palettes_map_the_spec_table ... ok
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
test look::css::tests::spec_rules ... ok
test look::css::tests::spec_rules_and_css_shape ... ok
test sidebar::tests::every_pin_reorder_moves_exactly_one_row ... ok

test result: ok. 60 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests fm_agent

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests fm_core

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

==> cargo build --workspace --release
   Compiling fm-ui v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-ui)
    Finished `release` profile [optimized] target(s) in 1.20s
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

### Unit tests added

`[observed]`

- `folder::sort`: every key in both orders, folders first in both orders,
  natural/case-insensitive names, the one-attribute rule.
- `folder`: status text with and without a selection; the Shift-click request
  carries the exact visible order; dot names are hidden.
- `folder::view`: the width-derived cap, its 64 ceiling and the 0-width case.
- `open`: folder -> navigate, file -> launch, missing -> missing (the symlink
  cases are the same booleans because `is_dir`/`is_file` follow links).
- `sidebar`: recent dedupe/cap/order; the drive fullness classes at 0, 50, 94,
  95 and 100 %; pin order, drag layout and the threshold-only reorder.

### 10-second runs

`pgrep -a gimbal-files` was empty before and after every run. Each run used a
fresh `XDG_CONFIG_HOME` and `XDG_STATE_HOME` under `/tmp/gimbal-p2i-run/` and
was stopped by PID with SIGTERM after 9 s. No GTK/CSS criticals appeared. The
100k runs print the allowed intermittent `VK_SUBOPTIMAL_KHR` notice.
`[observed]`

**1. The 30-item folder** (`/tmp/gimbal-p2i-run/small30`: 6 folders, 24 files,
one hidden name).

```text
[gimbal 0.079] config: /tmp/gimbal-p2i-run/config-final3/gimbal-files/config.toml does not exist; defaults apply
[gimbal 0.080] config: scale 1.00 (base 1.00), opacity 0.85, font "", left collapsed false width 348, right collapsed false width 648
[gimbal 0.080] theme: reloaded /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia-extensions.css
[gimbal 0.080] session: restored 1 window(s)
[gimbal 0.088] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal 0.088] blur: capabilities Capability(Blur) (blur true)
[gimbal 0.088] hotkey: bound "<Alt>Left" to nav.back
[gimbal 0.088] hotkey: bound "<Alt>Right" to nav.forward
[gimbal 0.088] hotkey: bound "<Alt>Up" to nav.up
[gimbal 0.088] hotkey: bound "<Control>a" to selection.all
[gimbal 0.088] hotkey: bound "<Control>equal" to view.zoom-in
[gimbal 0.088] hotkey: bound "<Control>f" to search.open
[gimbal 0.088] hotkey: bound "<Control>l" to nav.focus-path
[gimbal 0.088] hotkey: bound "<Control>minus" to view.zoom-out
[gimbal 0.088] hotkey: bound "<Control>n" to window.new
[gimbal 0.088] hotkey: bound "<Control>q" to app.quit
[gimbal 0.088] hotkey: bound "<Control>t" to tab.new
[gimbal 0.088] hotkey: bound "<Control>w" to tab.close
[gimbal 0.088] hotkey: bound "<Control>z" to file.undo
[gimbal 0.088] hotkey: bound "Delete" to file.trash
[gimbal 0.088] hotkey: bound "F5" to nav.refresh
[gimbal 0.088] header: window 1 pills ["kyle"] (active 0, overflow false)
[gimbal 0.088] layout: scale 1.00: left 348, content 0, preview 648 (inset 8)
[gimbal 0.088] folder: listing /home/kyle (sort name ascending, folders first true)
[gimbal 0.088] window: built model window 1 (0×0)
[gimbal 0.847] window: opened model window 1
[gimbal 0.847] fm_ui: open () with 1 folder argument(s): ["/tmp/gimbal-p2i-run/small30"]
[gimbal 0.847] fm_ui: first open; the session windows are restored first
[gimbal 0.847] launch: open request: ["/tmp/gimbal-p2i-run/small30"]
[gimbal 0.847] model: TabAdded { window: WindowId(1), group: GroupId(5), tab: TabId(4), index: 1 }
[gimbal 0.847] header: window 1 pills ["kyle", "small30"] (active 1, overflow false)
[gimbal 0.847] folder: listing /tmp/gimbal-p2i-run/small30 (sort name ascending, folders first true)
[gimbal 0.848] model: FocusChanged { window: WindowId(1), tab: TabId(4) }
[gimbal 0.848] header: window 1 pills ["kyle", "small30"] (active 1, overflow false)
[gimbal 0.856] header: window 1 pills ["kyle", "small30"] (active 1, overflow true)
[gimbal 0.856] blur: surface layout changed to 1170×1512
[gimbal 0.856] blur: opacity 0.85: visible 1120×1462 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1120, h: 1438 }, Rect { x: 37, y: 25, w: 1096, h: 3 }, Rect { x: 37, y: 1484, w: 1096, h: 3 }, Rect { x: 30, y: 28, w: 1110, h: 3 }, Rect { x: 30, y: 1481, w: 1110, h: 3 }, Rect { x: 27, y: 31, w: 1116, h: 3 }, Rect { x: 27, y: 1478, w: 1116, h: 3 }, Rect { x: 26, y: 34, w: 1118, h: 3 }, Rect { x: 26, y: 1475, w: 1118, h: 3 }]
[gimbal 0.860] folder: first items in model: +31 (31 so far) in 12 ms
[gimbal 0.860] folder: listing complete: 31 items (30 visible) in 12 ms
[gimbal 0.860] folder: viewport 154 px at scale 1.00 -> cap 2
[gimbal 1.848] session: saved /tmp/gimbal-p2i-run/state-final3/gimbal-files/session.toml
```

**2. The 100,000-file folder** (`/tmp/gimbal-p2i-run/big`). Three runs:

| run | first items in model | complete listing | peak RSS |
|---|---|---|---|
| 1 | 33 ms | 319 ms | not measured |
| 2 | 33 ms | 329 ms | not measured |
| 3 | 32 ms | 334 ms | not measured |

P1e-r1 delivered 0.149-0.152 s to first items, 0.713-0.732 s to complete, and
238.6 MB peak RSS on the same class of tmpfs folder. The current listing is
about 4.5x faster to first items and 2.2x faster to complete; the smaller
pool (2 columns in this tiled window rather than P1e's 9-10) and the trimmed
attribute list are the likely reasons. I did not measure RSS. `[observed]`
for the times; `[inferred]` for the reasons.

Left in place for the live check: `/tmp/gimbal-p2i-run/small30` (30 visible
items), `/tmp/gimbal-p2i-run/big` (100,000 files) and
`/tmp/gimbal-p2i-run/unreadable` (restored to mode 700; `chmod 000` to retest
the error). These are all scratch folders I created. `[observed]`

Run 1 full stderr:

```text
[gimbal 0.080] config: /tmp/gimbal-p2i-run/config-fbig1/gimbal-files/config.toml does not exist; defaults apply
[gimbal 0.080] config: scale 1.00 (base 1.00), opacity 0.85, font "", left collapsed false width 348, right collapsed false width 648
[gimbal 0.080] theme: reloaded /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia-extensions.css
[gimbal 0.080] session: restored 1 window(s)
[gimbal 0.088] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal 0.088] blur: capabilities Capability(Blur) (blur true)
[gimbal 0.088] hotkey: bound "<Alt>Left" to nav.back
[gimbal 0.088] hotkey: bound "<Alt>Right" to nav.forward
[gimbal 0.088] hotkey: bound "<Alt>Up" to nav.up
[gimbal 0.088] hotkey: bound "<Control>a" to selection.all
[gimbal 0.088] hotkey: bound "<Control>equal" to view.zoom-in
[gimbal 0.088] hotkey: bound "<Control>f" to search.open
[gimbal 0.088] hotkey: bound "<Control>l" to nav.focus-path
[gimbal 0.088] hotkey: bound "<Control>minus" to view.zoom-out
[gimbal 0.088] hotkey: bound "<Control>n" to window.new
[gimbal 0.088] hotkey: bound "<Control>q" to app.quit
[gimbal 0.088] hotkey: bound "<Control>t" to tab.new
[gimbal 0.088] hotkey: bound "<Control>w" to tab.close
[gimbal 0.088] hotkey: bound "<Control>z" to file.undo
[gimbal 0.088] hotkey: bound "Delete" to file.trash
[gimbal 0.088] hotkey: bound "F5" to nav.refresh
[gimbal 0.088] header: window 1 pills ["kyle"] (active 0, overflow false)
[gimbal 0.088] layout: scale 1.00: left 348, content 0, preview 648 (inset 8)
[gimbal 0.088] folder: listing /home/kyle (sort name ascending, folders first true)
[gimbal 0.088] window: built model window 1 (0×0)
[gimbal 0.133] window: opened model window 1
[gimbal 0.133] fm_ui: open () with 1 folder argument(s): ["/tmp/gimbal-p2i-run/big"]
[gimbal 0.133] fm_ui: first open; the session windows are restored first
[gimbal 0.133] launch: open request: ["/tmp/gimbal-p2i-run/big"]
[gimbal 0.133] model: TabAdded { window: WindowId(1), group: GroupId(5), tab: TabId(4), index: 1 }
[gimbal 0.134] header: window 1 pills ["kyle", "big"] (active 1, overflow false)
[gimbal 0.134] folder: listing /tmp/gimbal-p2i-run/big (sort name ascending, folders first true)
[gimbal 0.142] model: FocusChanged { window: WindowId(1), tab: TabId(4) }
[gimbal 0.143] header: window 1 pills ["kyle", "big"] (active 1, overflow false)
[gimbal 0.150] header: window 1 pills ["kyle", "big"] (active 1, overflow true)
[gimbal 0.151] blur: surface layout changed to 1170×1512
[gimbal 0.151] blur: opacity 0.85: visible 1120×1462 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1120, h: 1438 }, Rect { x: 37, y: 25, w: 1096, h: 3 }, Rect { x: 37, y: 1484, w: 1096, h: 3 }, Rect { x: 30, y: 28, w: 1110, h: 3 }, Rect { x: 30, y: 1481, w: 1110, h: 3 }, Rect { x: 27, y: 31, w: 1116, h: 3 }, Rect { x: 27, y: 1478, w: 1116, h: 3 }, Rect { x: 26, y: 34, w: 1118, h: 3 }, Rect { x: 26, y: 1475, w: 1118, h: 3 }]
[gimbal 0.154] folder: viewport 154 px at scale 1.00 -> cap 2
[gimbal 0.168] folder: first items in model: +5000 (5000 so far) in 33 ms

(gimbal-files:684031): Gdk-WARNING **: 10:37:58.934: vkAcquireNextImageKHR(): A swapchain no longer matches the surface properties exactly, but can still be used to present to the surface successfully. (VK_SUBOPTIMAL_KHR) (1000001003)
[gimbal 0.453] folder: listing complete: 100000 items (100000 visible) in 319 ms
[gimbal 0.909] model: ScrollAnchorChanged { window: WindowId(1), tab: TabId(4) }
[gimbal 0.909] header: window 1 pills ["kyle", "big"] (active 1, overflow true)
[gimbal 1.909] session: saved /tmp/gimbal-p2i-run/state-fbig1/gimbal-files/session.toml
```

The other two runs' listing lines:

```text
[run 2]
[gimbal 0.127] folder: listing /tmp/gimbal-p2i-run/big (sort name ascending, folders first true)
[gimbal 0.161] folder: first items in model: +5000 (5000 so far) in 33 ms
[gimbal 0.456] folder: listing complete: 100000 items (100000 visible) in 329 ms

[run 3]
[gimbal 0.124] folder: listing /tmp/gimbal-p2i-run/big (sort name ascending, folders first true)
[gimbal 0.157] folder: first items in model: +5000 (5000 so far) in 32 ms
[gimbal 0.459] folder: listing complete: 100000 items (100000 visible) in 334 ms
```

Their remaining lines are the same startup, hotkey and `VK_SUBOPTIMAL_KHR`
notice as run 1.

**3. The unreadable folder** (`chmod 000` on a folder I created; restored to
`chmod 700` after the run). Navigation still works; the error is shown centred
and logged once.

```text
[gimbal 0.080] config: /tmp/gimbal-p2i-run/config-funread/gimbal-files/config.toml does not exist; defaults apply
[gimbal 0.080] config: scale 1.00 (base 1.00), opacity 0.85, font "", left collapsed false width 348, right collapsed false width 648
[gimbal 0.080] theme: reloaded /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia-extensions.css
[gimbal 0.081] session: restored 1 window(s)
[gimbal 0.088] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal 0.088] blur: capabilities Capability(Blur) (blur true)
[gimbal 0.088] hotkey: bound "<Alt>Left" to nav.back
[gimbal 0.088] hotkey: bound "<Alt>Right" to nav.forward
[gimbal 0.088] hotkey: bound "<Alt>Up" to nav.up
[gimbal 0.088] hotkey: bound "<Control>a" to selection.all
[gimbal 0.088] hotkey: bound "<Control>equal" to view.zoom-in
[gimbal 0.088] hotkey: bound "<Control>f" to search.open
[gimbal 0.088] hotkey: bound "<Control>l" to nav.focus-path
[gimbal 0.088] hotkey: bound "<Control>minus" to view.zoom-out
[gimbal 0.088] hotkey: bound "<Control>n" to window.new
[gimbal 0.088] hotkey: bound "<Control>q" to app.quit
[gimbal 0.088] hotkey: bound "<Control>t" to tab.new
[gimbal 0.088] hotkey: bound "<Control>w" to tab.close
[gimbal 0.088] hotkey: bound "<Control>z" to file.undo
[gimbal 0.088] hotkey: bound "Delete" to file.trash
[gimbal 0.088] hotkey: bound "F5" to nav.refresh
[gimbal 0.088] header: window 1 pills ["kyle"] (active 0, overflow false)
[gimbal 0.088] layout: scale 1.00: left 348, content 0, preview 648 (inset 8)
[gimbal 0.088] folder: listing /home/kyle (sort name ascending, folders first true)
[gimbal 0.088] window: built model window 1 (0×0)
[gimbal 0.131] window: opened model window 1
[gimbal 0.131] fm_ui: open () with 1 folder argument(s): ["/tmp/gimbal-p2i-run/unreadable"]
[gimbal 0.131] fm_ui: first open; the session windows are restored first
[gimbal 0.131] launch: open request: ["/tmp/gimbal-p2i-run/unreadable"]
[gimbal 0.131] model: TabAdded { window: WindowId(1), group: GroupId(5), tab: TabId(4), index: 1 }
[gimbal 0.131] header: window 1 pills ["kyle", "unreadable"] (active 1, overflow false)
[gimbal 0.131] folder: listing /tmp/gimbal-p2i-run/unreadable (sort name ascending, folders first true)
[gimbal 0.131] model: FocusChanged { window: WindowId(1), tab: TabId(4) }
[gimbal 0.131] header: window 1 pills ["kyle", "unreadable"] (active 1, overflow false)
[gimbal 0.132] folder: cannot read /tmp/gimbal-p2i-run/unreadable: Error opening directory '/tmp/gimbal-p2i-run/unreadable': Permission denied
[gimbal 0.139] header: window 1 pills ["kyle", "unreadable"] (active 1, overflow true)
[gimbal 0.140] blur: surface layout changed to 1170×1512
[gimbal 0.140] blur: opacity 0.85: visible 1120×1462 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1120, h: 1438 }, Rect { x: 37, y: 25, w: 1096, h: 3 }, Rect { x: 37, y: 1484, w: 1096, h: 3 }, Rect { x: 30, y: 28, w: 1110, h: 3 }, Rect { x: 30, y: 1481, w: 1110, h: 3 }, Rect { x: 27, y: 31, w: 1116, h: 3 }, Rect { x: 27, y: 1478, w: 1116, h: 3 }, Rect { x: 26, y: 34, w: 1118, h: 3 }, Rect { x: 26, y: 1475, w: 1118, h: 3 }]
[gimbal 0.144] folder: viewport 154 px at scale 1.00 -> cap 2
[gimbal 1.132] session: saved /tmp/gimbal-p2i-run/state-funread/gimbal-files/session.toml
```

### Temporary timer log

The harness was `GIMBAL_P2I_TIMER=1`: a 850 ms repeating timeout that selected
two names, ran `selection.all`, ran `view.set-sort by=size order=descending`,
created `live-new.txt` in the folder, scrolled to the last item, ran
`nav.go` into `sub-a`, `nav.back`, `pins.add`, and `pins.move from=0 to=2`.
Each tick logged the listing order, the model's folder/selection/anchor/first
visible item, and the config file. The log below is from the 30-item folder
with a scratch config that already had four pins (see *Outside my files* for
why). The harness is removed before delivery: `grep -rn
"GIMBAL_P2I_TIMER\|timer:" crates/fm-ui/src` prints nothing. `[observed]`

```text
[gimbal 1.688] timer: ---- after step 0 ----
[gimbal 1.688] timer: listing order: ["Folder 1", "Folder 2", "Folder 3", "sub-a", "sub-b", "Zeta Folder", "file1.txt", "file2.txt", "file3.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "file10.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file20.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt"]
[gimbal 1.688] timer: folder /tmp/gimbal-p2i-run/small30 selection {} anchor None first visible Some("Folder 1")
[gimbal 1.688] timer: config /tmp/gimbal-p2i-run/config-timer30/gimbal-files/config.toml:
# scratch P2i timer config
[pins]
folders = ["/tmp/gimbal-p2i-run/pin-a", "/tmp/gimbal-p2i-run/pin-b", "/tmp/gimbal-p2i-run/pin-c", "/tmp/gimbal-p2i-run/pin-d"]

[gimbal 1.688] model: SelectionChanged { window: WindowId(1) }
[gimbal 1.689] header: window 1 pills ["kyle", "small30"] (active 1, overflow true)
[gimbal 1.689] model: SelectionChanged { window: WindowId(1) }
[gimbal 1.689] header: window 1 pills ["kyle", "small30"] (active 1, overflow true)
[gimbal 1.690] folder: viewport 194 px at scale 1.00 -> cap 2
[gimbal 2.539] timer: ---- after step 1 ----
[gimbal 2.539] timer: listing order: ["Folder 1", "Folder 2", "Folder 3", "sub-a", "sub-b", "Zeta Folder", "file1.txt", "file2.txt", "file3.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "file10.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file20.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt"]
[gimbal 2.539] timer: folder /tmp/gimbal-p2i-run/small30 selection {"Folder 1", "Folder 3"} anchor None first visible Some("Folder 1")
[gimbal 2.539] timer: config /tmp/gimbal-p2i-run/config-timer30/gimbal-files/config.toml:
# scratch P2i timer config
[pins]
folders = ["/tmp/gimbal-p2i-run/pin-a", "/tmp/gimbal-p2i-run/pin-b", "/tmp/gimbal-p2i-run/pin-c", "/tmp/gimbal-p2i-run/pin-d"]

[gimbal 2.539] command: selection.all via Ui (view)
[gimbal 2.539] model: SelectionChanged { window: WindowId(1) }
[gimbal 2.540] header: window 1 pills ["kyle", "small30"] (active 1, overflow true)
[gimbal 2.541] folder: viewport 208 px at scale 1.00 -> cap 2

(gimbal-files:683636): Gdk-WARNING **: 10:37:28.432: vkAcquireNextImageKHR(): A swapchain no longer matches the surface properties exactly, but can still be used to present to the surface successfully. (VK_SUBOPTIMAL_KHR) (1000001003)
[gimbal 3.389] timer: ---- after step 2 ----
[gimbal 3.389] timer: listing order: ["Folder 1", "Folder 2", "Folder 3", "sub-a", "sub-b", "Zeta Folder", "file1.txt", "file2.txt", "file3.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "file10.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file20.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt"]
[gimbal 3.389] timer: folder /tmp/gimbal-p2i-run/small30 selection {"Folder 1", "Folder 2", "Folder 3", "Zeta Folder", "file1.txt", "file10.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file2.txt", "file20.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt", "file3.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "sub-a", "sub-b"} anchor None first visible Some("Folder 1")
[gimbal 3.389] timer: config /tmp/gimbal-p2i-run/config-timer30/gimbal-files/config.toml:
# scratch P2i timer config
[pins]
folders = ["/tmp/gimbal-p2i-run/pin-a", "/tmp/gimbal-p2i-run/pin-b", "/tmp/gimbal-p2i-run/pin-c", "/tmp/gimbal-p2i-run/pin-d"]

[gimbal 3.389] command: view.set-sort via Ui (settings)
[gimbal 3.389] command: view.set-sort applied
[gimbal 3.390] folder: re-listing for sort size descending (the attribute list changed)
[gimbal 3.392] folder: first items in model: +31 (31 so far) in 2 ms
[gimbal 3.392] folder: listing complete: 31 items (30 visible) in 2 ms
[gimbal 3.392] folder: restoring scroll anchor "Folder 1" at item 1
[gimbal 3.540] session: saved /tmp/gimbal-p2i-run/state-timer30/gimbal-files/session.toml
[gimbal 3.708] model: ScrollAnchorChanged { window: WindowId(1), tab: TabId(4) }
[gimbal 3.708] header: window 1 pills ["kyle", "small30"] (active 1, overflow true)
[gimbal 4.240] timer: ---- after step 3 ----
[gimbal 4.240] timer: listing order: ["sub-a", "Folder 1", "Folder 2", "Folder 3", "sub-b", "Zeta Folder", "file20.txt", "file3.txt", "file2.txt", "file10.txt", "file1.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt"]
[gimbal 4.240] timer: folder /tmp/gimbal-p2i-run/small30 selection {"Folder 1", "Folder 2", "Folder 3", "Zeta Folder", "file1.txt", "file10.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file2.txt", "file20.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt", "file3.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "sub-a", "sub-b"} anchor Some("Folder 1") first visible Some("Folder 1")
[gimbal 4.240] timer: config /tmp/gimbal-p2i-run/config-timer30/gimbal-files/config.toml:
# scratch P2i timer config
[pins]
folders = ["/tmp/gimbal-p2i-run/pin-a", "/tmp/gimbal-p2i-run/pin-b", "/tmp/gimbal-p2i-run/pin-c", "/tmp/gimbal-p2i-run/pin-d"]

[sort]

[[sort.folder]]
path = "/tmp/gimbal-p2i-run/small30"
by = "size"
order = "descending"

[gimbal 4.240] timer: created live-new.txt in /tmp/gimbal-p2i-run/small30
[gimbal 4.709] session: saved /tmp/gimbal-p2i-run/state-timer30/gimbal-files/session.toml
[gimbal 5.090] timer: ---- after step 4 ----
[gimbal 5.090] timer: listing order: ["sub-a", "Folder 1", "Folder 2", "Folder 3", "sub-b", "Zeta Folder", "file20.txt", "file3.txt", "file2.txt", "file10.txt", "file1.txt", "live-new.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt"]
[gimbal 5.090] timer: folder /tmp/gimbal-p2i-run/small30 selection {"Folder 1", "Folder 2", "Folder 3", "Zeta Folder", "file1.txt", "file10.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file2.txt", "file20.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt", "file3.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "sub-a", "sub-b"} anchor Some("Folder 1") first visible Some("Folder 1")
[gimbal 5.090] timer: config /tmp/gimbal-p2i-run/config-timer30/gimbal-files/config.toml:
# scratch P2i timer config
[pins]
folders = ["/tmp/gimbal-p2i-run/pin-a", "/tmp/gimbal-p2i-run/pin-b", "/tmp/gimbal-p2i-run/pin-c", "/tmp/gimbal-p2i-run/pin-d"]

[sort]

[[sort.folder]]
path = "/tmp/gimbal-p2i-run/small30"
by = "size"
order = "descending"

[gimbal 5.090] timer: scrolled to item 30
[gimbal 5.390] model: ScrollAnchorChanged { window: WindowId(1), tab: TabId(4) }
[gimbal 5.391] header: window 1 pills ["kyle", "small30"] (active 1, overflow true)
[gimbal 5.940] timer: ---- after step 5 ----
[gimbal 5.940] timer: listing order: ["sub-a", "Folder 1", "Folder 2", "Folder 3", "sub-b", "Zeta Folder", "file20.txt", "file3.txt", "file2.txt", "file10.txt", "file1.txt", "live-new.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt"]
[gimbal 5.940] timer: folder /tmp/gimbal-p2i-run/small30 selection {"Folder 1", "Folder 2", "Folder 3", "Zeta Folder", "file1.txt", "file10.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file2.txt", "file20.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt", "file3.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "sub-a", "sub-b"} anchor Some("file15.txt") first visible Some("file15.txt")
[gimbal 5.940] timer: config /tmp/gimbal-p2i-run/config-timer30/gimbal-files/config.toml:
# scratch P2i timer config
[pins]
folders = ["/tmp/gimbal-p2i-run/pin-a", "/tmp/gimbal-p2i-run/pin-b", "/tmp/gimbal-p2i-run/pin-c", "/tmp/gimbal-p2i-run/pin-d"]

[sort]

[[sort.folder]]
path = "/tmp/gimbal-p2i-run/small30"
by = "size"
order = "descending"

[gimbal 5.940] command: nav.go via Ui (view)
[gimbal 5.940] model: Navigated { window: WindowId(1), tab: TabId(4), folder: "/tmp/gimbal-p2i-run/small30/sub-a" }
[gimbal 5.940] header: window 1 pills ["kyle", "sub-a"] (active 1, overflow true)
[gimbal 5.941] folder: listing /tmp/gimbal-p2i-run/small30/sub-a (sort name ascending, folders first true)
[gimbal 5.943] folder: first items in model: +1 (1 so far) in 2 ms
[gimbal 5.943] folder: listing complete: 1 items (1 visible) in 2 ms
[gimbal 5.943] folder: viewport 154 px at scale 1.00 -> cap 2
[gimbal 6.242] model: ScrollAnchorChanged { window: WindowId(1), tab: TabId(4) }
[gimbal 6.243] header: window 1 pills ["kyle", "sub-a"] (active 1, overflow true)
[gimbal 6.790] timer: ---- after step 6 ----
[gimbal 6.790] timer: listing order: ["inside-a.txt"]
[gimbal 6.790] timer: folder /tmp/gimbal-p2i-run/small30/sub-a selection {} anchor Some("inside-a.txt") first visible Some("inside-a.txt")
[gimbal 6.790] timer: config /tmp/gimbal-p2i-run/config-timer30/gimbal-files/config.toml:
# scratch P2i timer config
[pins]
folders = ["/tmp/gimbal-p2i-run/pin-a", "/tmp/gimbal-p2i-run/pin-b", "/tmp/gimbal-p2i-run/pin-c", "/tmp/gimbal-p2i-run/pin-d"]

[sort]

[[sort.folder]]
path = "/tmp/gimbal-p2i-run/small30"
by = "size"
order = "descending"

[gimbal 6.790] command: nav.back via Ui (view)
[gimbal 6.790] model: Navigated { window: WindowId(1), tab: TabId(4), folder: "/tmp/gimbal-p2i-run/small30" }
[gimbal 6.791] header: window 1 pills ["kyle", "small30"] (active 1, overflow true)
[gimbal 6.791] folder: listing /tmp/gimbal-p2i-run/small30 (sort size descending, folders first true)
[gimbal 6.793] folder: first items in model: +32 (32 so far) in 1 ms
[gimbal 6.793] folder: listing complete: 32 items (31 visible) in 1 ms
[gimbal 6.793] folder: restoring scroll anchor "file15.txt" at item 22
[gimbal 7.095] model: ScrollAnchorChanged { window: WindowId(1), tab: TabId(4) }
[gimbal 7.096] header: window 1 pills ["kyle", "small30"] (active 1, overflow true)
[gimbal 7.640] timer: ---- after step 7 ----
[gimbal 7.640] timer: listing order: ["sub-a", "Folder 1", "Folder 2", "Folder 3", "sub-b", "Zeta Folder", "file20.txt", "file3.txt", "file2.txt", "file10.txt", "file1.txt", "live-new.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt"]
[gimbal 7.641] timer: folder /tmp/gimbal-p2i-run/small30 selection {} anchor Some("file15.txt") first visible Some("file15.txt")
[gimbal 7.641] timer: config /tmp/gimbal-p2i-run/config-timer30/gimbal-files/config.toml:
# scratch P2i timer config
[pins]
folders = ["/tmp/gimbal-p2i-run/pin-a", "/tmp/gimbal-p2i-run/pin-b", "/tmp/gimbal-p2i-run/pin-c", "/tmp/gimbal-p2i-run/pin-d"]

[sort]

[[sort.folder]]
path = "/tmp/gimbal-p2i-run/small30"
by = "size"
order = "descending"

[gimbal 7.641] command: pins.add via Ui (settings)
[gimbal 7.641] command: pins.add applied
[gimbal 7.641] pins: the sidebars follow the config
[gimbal 8.096] session: saved /tmp/gimbal-p2i-run/state-timer30/gimbal-files/session.toml
[gimbal 8.490] timer: ---- after step 8 ----
[gimbal 8.490] timer: listing order: ["sub-a", "Folder 1", "Folder 2", "Folder 3", "sub-b", "Zeta Folder", "file20.txt", "file3.txt", "file2.txt", "file10.txt", "file1.txt", "live-new.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt"]
[gimbal 8.491] timer: folder /tmp/gimbal-p2i-run/small30 selection {} anchor Some("file15.txt") first visible Some("file15.txt")
[gimbal 8.491] timer: config /tmp/gimbal-p2i-run/config-timer30/gimbal-files/config.toml:
# scratch P2i timer config
[pins]
folders = ["/tmp/gimbal-p2i-run/pin-a", "/tmp/gimbal-p2i-run/pin-b", "/tmp/gimbal-p2i-run/pin-c", "/tmp/gimbal-p2i-run/pin-d", "/tmp/gimbal-p2i-run/small30"]

[sort]

[[sort.folder]]
path = "/tmp/gimbal-p2i-run/small30"
by = "size"
order = "descending"

[gimbal 8.491] command: pins.move via Ui (settings)
[gimbal 8.491] command: pins.move applied
[gimbal 8.491] pins: the sidebars follow the config
[gimbal 9.341] timer: ---- after step 9 ----
[gimbal 9.341] timer: listing order: ["sub-a", "Folder 1", "Folder 2", "Folder 3", "sub-b", "Zeta Folder", "file20.txt", "file3.txt", "file2.txt", "file10.txt", "file1.txt", "live-new.txt", "file4.txt", "file5.txt", "file6.txt", "file7.txt", "file8.txt", "file9.txt", "file11.txt", "file12.txt", "file13.txt", "file14.txt", "file15.txt", "file16.txt", "file17.txt", "file18.txt", "file19.txt", "file21.txt", "file22.txt", "file23.txt", "file24.txt"]
[gimbal 9.341] timer: folder /tmp/gimbal-p2i-run/small30 selection {} anchor Some("file15.txt") first visible Some("file15.txt")
[gimbal 9.341] timer: config /tmp/gimbal-p2i-run/config-timer30/gimbal-files/config.toml:
# scratch P2i timer config
[pins]
folders = [ "/tmp/gimbal-p2i-run/pin-b", "/tmp/gimbal-p2i-run/pin-c","/tmp/gimbal-p2i-run/pin-a", "/tmp/gimbal-p2i-run/pin-d", "/tmp/gimbal-p2i-run/small30"]

[sort]

[[sort.folder]]
path = "/tmp/gimbal-p2i-run/small30"
by = "size"
order = "descending"

```

The log shows: two selected names -> 30 selected after `selection.all`;
the size-descending order on the next tick; `live-new.txt` appearing without
a reload; `anchor Some("file15.txt")` and the first visible item at
`file15.txt`; `nav.go` -> `sub-a`; `nav.back` restoring `file15.txt` at item
22 and showing it first; the pin added at the end of the config's list; and
`pins.move 0 -> 2` changing the saved list to `pin-b, pin-c, pin-a, pin-d,
small30`. `[observed]`

## API citations

Read in the pinned crate sources under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. `[read]`

- `gtk4-0.11.5:src/auto/directory_list.rs:24` `new(attributes, file)`, `:36`
  `attributes()`, `:46` `error()`, `:82` `set_file()`, `:125`
  `connect_error_notify`, `:200` `connect_loading_notify`.
- `gio-0.22.10:src/auto/list_model.rs:68` `connect_items_changed`.
- `gtk4-0.11.5:src/auto/filter_list_model.rs:39` `new(model, filter)`;
  `src/custom_filter.rs:11` `CustomFilter::new`.
- `gtk4-0.11.5:src/custom_sorter.rs:11` `CustomSorter::new`;
  `src/auto/sorter.rs:29` `Sorter::changed(SorterChange::Different)`.
- `gtk4-0.11.5:src/auto/sort_list_model.rs:39` `new(model, sorter)`, `:104`
  `set_incremental`.
- `gtk4-0.11.5:src/auto/multi_selection.rs:39` `new(model)`.
- `gtk4-0.11.5:src/auto/grid_view.rs:89` `max_columns()`, `:129`
  `scroll_to(pos, ListScrollFlags, Option<ScrollInfo>)`, `:164`
  `set_max_columns(u32)`, `:211` `connect_activate`.
- `gtk4-0.11.5:src/auto/widget.rs:166` `compute_bounds`, `:1035`
  `observe_children`, `:250` `drag_check_threshold`, `:1382`
  `set_size_request`; `src/auto/scrollable.rs:42`/`:63` the scrollable
  adjustments; `src/auto/scrolled_window.rs:64`/`:196` the scrolled window's
  own adjustments (the h-adjustment's `page-size` notify drives the cap).
- `gdk4-0.11.5:src/auto/event.rs:126` `modifier_state()` for the click
  modifiers; `gtk4-0.11.5:src/auto/gesture.rs:198` `set_state`.
- `gtk4-0.11.5:src/auto/label.rs:293`/`:328`/`:350`/`:438`/`:454` the label's
  ellipsize, lines, max/width chars and wrap mode; `pango-0.22.9` has
  `WrapMode::WordChar` at `src/auto/enums.rs:3230`.
- `gio-0.22.10:src/auto/file.rs:1655` `query_filesystem_info`;
  `src/auto/volume_monitor.rs:56` `mounts`, `:241`/`:272`/`:334`
  mount-added/changed/removed; `src/auto/app_info.rs:247`
  `launch_default_for_uri`.
- `glib-0.22.10:src/auto/functions.rs:317` `format_size`;
  `src/source.rs:353` `idle_add_local_once`, `:532`
  `timeout_add_local_once`. `SourceId::remove` panics if the source already
  fired (`:31`); the anchor debounce therefore keeps no `SourceId` at all,
  and a generation number makes older 300 ms timers harmless no-ops. This was
  the one runtime panic the timer found and fixed.
- `libadwaita-0.9.2:src/auto/timed_animation.rs:25` `TimedAnimation::new` for
  the pinned-row slide.
- The mockup folder uses the existing `crate::look::icons::svg` path from
  `gtk4-0.11.5` `GtkSvg`, as P2g/P1m recorded.

## Not verified live (Claude)

- The grid's look against the spike's: cell width, the mockup folder, the
  theme file icons, two-line ellipsis, spacing and the status line.
- Scrolling the 100k folder by wheel and scrollbar, and the selection after
  the scroll.
- Double-click opening a file and a folder, and Enter; and that a symlink
  opens its target. I never triggered `file.open` or `launch_default_for_uri`.
- The pinned drag and its slide, the right-click Unpin, and the selected pin
  following the focused folder.
- The drive bars against real mounts, including a 95 %+ filesystem, and a
  mount appearing/disappearing.
- Section collapse/expand and the 6 px collapsed gap.
- The content width in a tiled window: the 1170 px test window left the
  content 154 px wide (2 grid columns) because the default 348 + 648 px
  sidebars take most of it. That is P2g's layout, not a P2i change; Claude
  can widen or collapse the preview to see the intended grid.

## Anything outside my files

- **Config pin bug (P2c, not touched):** with no `[pins]` table in the file,
  `pins.add` writes only the newly added pin, while the in-memory list still
  has the default pins. A following `pins.move from=0 to=2` therefore moves
  the in-memory list but the saved array has one entry and the move is
  ignored by `apply_to_document`. I saw this in the first timer run. The
  timer log above uses a scratch config with four existing pins so the move
  is visible. The fix belongs in `config.rs` (`pins_array` should start from
  `config.pins` when the table/key is absent), not in `fm-ui`.
- `events::touch` still records `SelectionChanged` as content-false even
  though the grid now re-syncs on it. It is only a test/map table; I left it
  alone to keep this brief's changes inside the UI path.
- No dependency, `Cargo.toml`, `CLAUDE.md`, core, agent or CLI file was
  changed. `spike/` was read only.

## Review and live check (Claude, Nov 2, 2026): passes, with four fixes by Claude

`scripts/check.sh` from `/tmp`: exit 0. The tests are 61 `fm-ui`, 175 `fm-core`, the 6 trash
parents, 7 CLI and 1 agent, and deny is clean.

The real `~/.config/gimbal-files/config.toml` and `~/.local/state/gimbal-files/session.toml` that
exist now were written at 10:23, about 10 minutes before this brief's runs started (10:33–10:42).
They hold Kyle's own changes from trying the app. The agent's runs used scratch folders as
`CLAUDE.md` requires. It left `/tmp/gimbal-p2i-run` behind; Claude reused its 100k folder, then
deleted it.

**Live** (release build, niri-tiled on DP-2, now 2256×1504 with DP-3 off; scratch config with the
preview collapsed):
- **The grid:** folders first in natural order (Alpha, beta, Gamma, the symlink to a folder, then
  the long name ellipsized on two lines), then files in natural order (file1, file2, file10,
  file20). `.hidden` is filtered out. The status reads "Files: 6, Folders: 5".
- **Opening:**
  - double-clicking Alpha ran `nav.go`, and Back returned;
  - Recent Directories then listed "mixed", "Alpha";
  - double-clicking `notes.txt` ran `file.open` and Kate opened it. Claude then closed that one
    unedited Kate window.
- **Selection** (after the fixes below):
  - a click and two Ctrl-clicks filled exactly those three cells;
  - a Shift-click from file1 to notes filled the natural-order range, and `big.bin` before it
    stayed clear;
  - the status showed ", N selected".
- **Pinned drag:** dragging Desktop above Home ran `pins.move`. With the pin fix the scratch config
  saved `["/home/kyle/Desktop", "/home/kyle"]`.
- **Recent pill:** collapsing it saved `recent = "collapsed"`.
- **The main drive bar** reads 73% of 964.2 GB. **Devices** is empty because Kyle's two other
  volumes (Nvme-Storage-1, WD-Blue-SSD) aren't mounted. Showing unmounted volumes belongs with the
  mount feature.
- **The 100k folder:** first items in 114 ms, complete in 372–411 ms. Wheel scrolling was smooth,
  the names stayed in order, and the cells stayed uniform.
- With a scratch `XDG_CONFIG_HOME` there's no `user-dirs.dirs`, so the default pins were only Home
  and Desktop. That's a test artefact.

**Fixes (Claude):**
1. **Selected files didn't look selected.**
   - The cause: the transparent `window.gimbal-look .gl-frame gridview > child` rule outranks the
     theme's `:selected` background.
   - The fix: a later `gridview > child:selected` rule fills selected cells with the `selected`
     role at the window opacity, with the standard radius.
   - Test: `a_selected_grid_cell_is_filled_after_the_transparent_rule`.
2. **Ctrl-click didn't add to the selection on screen** (the model had it).
   - The cause: GTK's own list-item click selection ran after the cell's gesture and toggled the
     item back.
   - The fix: list items are `set_selectable(false)`, so the grid only shows the model's
     selection. Activation still opens.
3. **Opening an empty folder logged a Gtk-CRITICAL** (`gtk_grid_view_scroll_to: assertion 'pos <
   n_items'`).
   - The fix: every scroll now goes through `scroll_to_item`, which skips positions outside the
     list. Verified: two empty folders opened with no critical.
4. **A new tab opened at another tab's place.**
   - The cause: a second tab on the 100k folder opened at item 5668. `set_folder` and
     `restore_anchor` fell back to the view's per-folder memory on a tab switch.
   - The fix: that memory is for Back and Forward only. A tab switch or a new tab uses the tab's
     own anchor or the top. Verified: a new tab on the same folder opened at `file000001`.

**And in `fm-core` (the agent's finding):**
- **The bug:** with no `[pins]` table in the file, `pins.add` saved only the new pin, so the
  defaults were lost on the next load, and a `pins.move` was ignored.
- **The fix:** a pin change on a file without a pin list writes the whole in-memory list.
- **Test:** `pin_changes_without_a_pin_list_keep_the_defaults` reloads the file after an add and
  after a move.

Agents have left other scratch files in `/tmp` over earlier briefs (`/tmp/gimbal-app.err`,
`/tmp/gimbal-check-*.log`, `/tmp/gimbal-p2a`, …). They're harmless, but later briefs should remove
what they create.
