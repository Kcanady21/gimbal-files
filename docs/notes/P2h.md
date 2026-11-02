# P2h: the header, real windows, and the session

## What I built

`[observed]` unless a line says otherwise.

- **The process/window split.**
  - `crates/fm-ui/src/app.rs` is the process state: the one `ConfigFile`, the
    `Workspace`, the session path and saver, the one `Look` (stylesheet
    provider and theme watcher) and a `BTreeMap<WindowId, Rc<WindowUi>>`.
  - `crates/fm-ui/src/frame.rs` is now one model window's UI: P2g's frame,
    sidebars and status line, plus the header and the content placeholder.
    It holds its own `Blur`; the stylesheet is not rebuilt per window.
  - `crates/fm-ui/src/main.rs` owns the thread-local `Rc<Process>` and
    implements GApplication's `activate` and `open`. The application has
    `HANDLES_OPEN`, so a later launch hands its folders to the running
    instance.
  - `crates/fm-ui/src/events.rs` is the pure event-to-UI map (`touch`) and the
    group-fade predicate; the process has one `on_*` function per `Event`
    variant.
  - `crates/fm-ui/src/header.rs` is the P1i-P1l header bound to the model.
  - `crates/fm-ui/src/platform.rs` is the pure niri/other-desktop decision and
    the niri IPC adapter.
  - `crates/fm-ui/src/save.rs` is the one-second debounce and the background
    writer. The UI thread never waits for a normal save; quitting flushes.
  - `crates/fm-ui/src/look/mod.rs` lost the per-window blur and now builds
    the one provider and watcher once.
  - `crates/fm-ui/src/look/css.rs` gained the one `.gl-path.invalid` rule.
- **`fm-core`.** `Workspace::focus_window(id)` sets `focused_window`, returns
  `WindowFocused(id)` only when it changed, and refuses an unknown id with
  `NoSuchWindow`. One test covers the move, the no-op and the refusal.
- **The header.** Menu (`window.new`, `app.quit`); back/forward/up with
  sensitivity; the crossfading search field; the pill strip in a `GtkFixed`
  with drag reorder, Ctrl/Shift selection and the ✕; the accordion pill and
  its hover list with a ✕ per entry; the right-click menu from the command
  registry (Split View, Close, Close Selected Tabs, Open in New Window,
  Rename Group..., Close Group with confirmation, Add to Group); group pill
  crossfades; the stretching editable path bar with Ctrl+L and the red
  non-existent state; Trash; Float and Close as `gtk::Svg`; Minimize only off
  niri.
- **The content placeholder.** The focused tab's folder, centred in the
  `text-title` role; a split group shows each tile's folder on its own line
  in column order. The status line is still `Files: 0, Folders: 0`.
- **Start-up and saving.** The first activation calls
  `session::load(path, home, Path::is_dir)`, logs every problem, opens the
  stored windows (or `open_request([])` if there were none), and schedules
  the first save. A parse or version problem makes the first save call
  `session::preserve_bad` before writing. Later activations run
  `open_request([])`; `open` folders run `open_request(paths)` and a file
  argument is replaced by its parent folder. Each model change restarts one
  one-second timeout; closing operations flush the pending save first.
- **Commands.** Every `View` command the model has is handled: windows, tabs,
  groups, navigation, `search.open`, `nav.focus-path`, `app.quit` and the
  still-deferred selection, refresh, Trash and preview commands log their
  deferral. Model errors are logged, never panics. The P2g `Settings` path is
  unchanged apart from now applying scale/opacity to every window.

## Verified

`scripts/check.sh` from `$PROJ` with rustup 1.95.0 on `PATH`: exit 0.
Workspace total: 233 passed, 6 ignored. `fm-ui` has 46 tests (34 from P2g,
12 new: 4 event mapping, 4 header label/view, 2 platform, 2 debounce);
`fm-core` has 173 (172 + `focus_window`). `[observed]`

### `scripts/check.sh`

```text
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s
==> cargo test --workspace
   Compiling fm-ui v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-ui)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.35s
     Running unittests src/lib.rs (target/debug/deps/fm_agent-d3e0ba7a5f0ed207)

running 1 test
test tests::socket_lives_under_the_runtime_directory ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/main.rs (target/debug/deps/gimbal_files_cli-7fb62e9220e6d530)

running 7 tests
test tests::anything_else_is_a_usage_error ... ok
test tests::check_keeps_the_command_and_its_arguments ... ok
test tests::check_without_a_command_is_a_usage_error ... ok
test tests::help_shows_usage ... ok
test tests::commands_lists_the_registry ... ok
test tests::no_arguments_shows_usage ... ok
test tests::version_shows_the_version ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (target/debug/deps/fm_core-71b31b2576b0b33f)

running 173 tests
test command::tests::a_checked_command_prints_in_catalogue_order ... ok
test command::tests::a_token_without_equals_is_an_error ... ok
test command::tests::a_forbidden_call_never_reaches_the_handler ... ok
test command::tests::a_valid_call_reaches_the_handler_once ... ok
test command::tests::a_files_command_defaults_dry_run_to_false ... ok
test command::tests::an_empty_argument_name_is_an_error ... ok
test command::tests::a_known_command_is_not_an_unknown_command ... ok
test command::tests::a_value_may_contain_equals_and_spaces ... ok
test command::tests::a_single_valued_argument_may_not_repeat ... ok
test command::tests::an_invalid_call_never_reaches_the_handler ... ok
test command::tests::choice_must_be_one_of_the_fixed_list ... ok
test command::tests::argument_names_are_unique_within_a_command ... ok
test command::tests::bool_accepts_true_and_false_only ... ok
test command::tests::argument_kinds_are_sane ... ok
test command::tests::context_rules_are_pure_functions_over_every_caller ... ok
test command::tests::cli_and_agent_must_give_every_files_context_path ... ok
test command::tests::cli_and_agent_may_not_fill_files_paths_from_context ... ok
test command::tests::every_files_command_gets_exactly_one_dry_run_added_by_the_registry ... ok
test command::tests::every_caller_may_give_explicit_files_paths ... ok
test command::tests::a_distant_typo_gets_no_suggestion ... ok
test command::tests::int_accepts_the_range_and_rejects_bad_and_out_of_range_values ... ok
test command::tests::missing_required_argument_is_named ... ok
test command::tests::names_are_area_dot_verb_in_lowercase_kebab_case ... ok
test command::tests::no_tokens_is_a_missing_command ... ok
test command::tests::no_command_deletes_permanently ... ok
test command::tests::names_are_unique ... ok
test command::tests::number_values_are_hundredths_and_print_with_two_places ... ok
test command::tests::opacity_is_a_number_without_a_step ... ok
test command::tests::repeated_keys_are_kept_in_order ... ok
test command::tests::path_must_be_absolute_and_stays_as_given ... ok
test command::tests::paths_print_once_per_key ... ok
test command::tests::repeated_paths_keys_make_a_list_in_input_order ... ok
test command::tests::summaries_are_one_non_empty_line ... ok
test command::tests::scale_accepts_the_step_edges_and_refuses_off_step_and_out_of_range ... ok
test command::tests::the_context_rule_applies_only_to_files_commands ... ok
test command::tests::text_must_not_be_empty ... ok
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
test config::tests::an_unknown_key_is_a_warning_with_its_position ... ok
test config::tests::every_default_matches_the_brief ... ok
test config::tests::commands_that_need_the_current_folder_use_the_argument ... ok
test config::tests::a_bad_value_falls_back_and_the_good_values_still_load ... ok
test config::tests::a_hotkeys_table_replaces_listed_keys_and_an_empty_value_unbinds ... ok
test config::tests::an_unparsable_file_is_never_written ... ok
test config::tests::config_path_is_a_pure_function_of_xdg_config_home_and_home ... ok
test config::tests::an_unparsable_file_reports_one_problem_with_a_position ... ok
test config::tests::default_pins_include_home_then_only_existing_special_folders ... ok
test config::tests::a_save_rewrites_only_the_changed_key_on_a_commented_file ... ok
test config::tests::a_sidebar_and_section_save_keep_their_inline_formatting ... ok
test config::tests::saving_creates_the_folder_and_the_changed_key ... ok
test config::tests::every_settings_command_has_a_change_for ... ok
test config::tests::zoom_stops_at_the_scale_limits_and_toggles_flip ... ok
test config::tests::a_hotkey_with_an_unknown_command_or_bad_arguments_keeps_the_default ... ok
test config::tests::sort_for_expands_home_and_normalizes_dot_dot_and_trailing_slashes ... ok
test fileops::tests::a_new_folder_with_a_taken_or_invalid_name_is_a_conflict ... ok
test config::tests::set_scale_and_sort_map_to_the_expected_changes ... ok
test fileops::tests::apply_to_all_does_not_overwrite_an_earlier_answer ... ok
test fileops::tests::a_folder_into_itself_is_refused ... ok
test fileops::tests::a_cancel_decision_runs_nothing ... ok
test config::tests::saving_is_atomic_and_keeps_permissions ... ok
test fileops::tests::a_copy_with_a_conflict_lists_the_target ... ok
test fileops::tests::a_command_outside_the_file_operations_is_not_a_request ... ok
test fileops::tests::decisions_cancel_with_apply_to_all_marks_the_run_cancelled ... ok
test fileops::tests::a_dry_run_changes_nothing_on_disk ... ok
test config::tests::pin_changes_write_the_pins_list ... ok
test fileops::tests::cancel_ends_the_questions_at_once ... ok
test fileops::tests::a_cancel_partway_still_runs_nothing ... ok
test fileops::tests::every_files_command_maps_to_its_request ... ok
test fileops::tests::names_are_checked_against_the_rules ... ok
test fileops::tests::missing_sources_are_refused_for_their_item ... ok
test fileops::tests::moving_into_the_folder_an_item_is_already_in_is_refused ... ok
test config::tests::prints_a_sample_problem_list ... ok
test fileops::tests::copy_names_add_copy_then_numbers ... ok
test fileops::tests::renaming_to_an_invalid_or_taken_name_is_a_conflict ... ok
test config::tests::unknown_keys_and_tables_are_warnings_and_stay_in_the_file ... ok
test config::tests::sort_changes_write_one_global_setting_or_one_folder_rule ... ok
test config::tests::every_problem_kind_reports_its_line_and_column ... ok
test fileops::tests::request_mapping_refuses_unfilled_context_paths ... ok
test fileops::tests::the_first_answer_can_apply_to_all ... ok
test fileops::tests::three_conflicts_are_answered_one_by_one ... ok
test fileops::tests::trashinfo_keeps_the_first_occurrence_of_a_key ... ok
test fileops::tests::trashinfo_percent_decodes_non_utf8_paths ... ok
test fileops::tests::trashinfo_percent_decodes_the_path ... ok
test fileops::tests::trashinfo_reads_both_date_forms ... ok
test fileops::tests::trashinfo_reports_a_missing_key_header_or_bad_value ... ok
test fileops::tests::undo_with_nothing_on_the_stack_is_an_error ... ok
test fileops::tests::unanswered_conflicts_refuse_the_whole_operation_and_name_the_items ... ok
test session::tests::an_empty_workspace_is_never_saved ... ok
test session::tests::a_missing_file_gives_one_window_at_home_without_a_problem ... ok
test session::tests::session_path_is_a_pure_function_of_xdg_state_home_and_home ... ok
test tests::app_id_matches_the_niri_rule_and_is_a_valid_gapplication_id ... ok
test session::tests::a_version_2_file_gives_one_window_at_home_and_a_problem ... ok
test session::tests::preserve_bad_replaces_an_older_copy ... ok
test workspace::split::tests::a_close_rebalances_two_or_more_apart ... ok
test workspace::split::tests::a_drop_never_empties_a_column ... ok
test workspace::split::tests::a_hand_made_three_plus_one_survives_a_close_from_the_long_column ... ok
test workspace::split::tests::a_slot_past_the_end_changes_nothing ... ok
test session::tests::an_unparsable_file_gives_one_window_at_home_and_a_problem ... ok
test workspace::split::tests::a_swap_keeps_heights_while_an_insert_resets_the_changed_column ... ok
test workspace::split::tests::ctrl_click_toggles_without_moving_the_anchor ... ok
test workspace::split::tests::closing_the_lone_tile_rebalances ... ok
test workspace::split::tests::dropping_in_a_gap_inserts_and_the_column_gets_longer ... ok
test workspace::split::tests::dropping_on_a_tile_swaps_across_and_reorders_within ... ok
test workspace::split::tests::initial_columns_fill_the_shorter_column ... ok
test workspace::split::tests::plain_click_clears_and_moves_the_anchor ... ok
test workspace::split::tests::shift_click_selects_the_range_from_the_anchor ... ok
test workspace::tests::a_failed_operation_changes_nothing ... ok
test workspace::tests::a_fresh_workspace_has_one_focused_tab_and_holds_the_invariants ... ok
test workspace::tests::a_drop_that_would_empty_a_column_is_refused_and_changes_nothing ... ok
test workspace::tests::a_hand_made_three_plus_one_survives_a_close_from_the_long_column ... ok
test session::tests::a_missing_folder_falls_back_to_its_parent_then_to_home ... ok
test workspace::tests::closing_a_tab_inside_a_group_focuses_the_next_tile_else_the_previous ... ok
test workspace::tests::closing_the_last_pill_closes_the_window ... ok
test workspace::tests::closing_a_window_focuses_another_and_the_last_says_so ... ok
test workspace::tests::a_tile_drop_swaps_reorders_and_inserts ... ok
test session::tests::a_non_utf8_folder_and_file_name_round_trip ... ok
test workspace::tests::closing_two_tiles_from_a_three_plus_three_group_rebalances_to_two_plus_two ... ok
test workspace::tests::focus_window_moves_the_focus_and_refuses_an_unknown_id ... ok
test workspace::tests::focus_after_a_close_goes_right_else_left ... ok
test workspace::tests::file_clicks_follow_the_strip_rules ... ok
test workspace::tests::group_add_tab_joins_a_single_pill_into_a_new_split ... ok
test workspace::tests::group_rename_trims_and_rejects_an_empty_name ... ok
test workspace::tests::group_close_closes_every_tab_and_focuses_the_neighbour ... ok
test workspace::tests::group_split_inserts_at_the_first_selected_pill ... ok
test workspace::tests::group_add_tab_uses_the_shorter_column_and_resets_that_columns_heights ... ok
test workspace::tests::group_split_uses_the_lowest_free_number ... ok
test workspace::tests::hand_heights_need_one_entry_per_tile_and_the_width_share_is_permille ... ok
test workspace::tests::ids_are_stable_and_never_reused ... ok
test workspace::tests::nav_back_and_forward_move_between_folders_and_do_nothing_on_empty_stacks ... ok
test session::tests::a_rich_workspace_round_trips ... ok
test workspace::tests::group_split_merges_selected_groups_and_needs_two_tabs ... ok
test workspace::tests::nav_go_pushes_history_and_clears_selection_and_anchor ... ok
test workspace::tests::nav_go_to_a_file_goes_to_its_folder_and_selects_the_name ... ok
test workspace::tests::history_stacks_are_capped_at_one_hundred_dropping_the_oldest ... ok
test workspace::tests::nav_up_stops_at_the_root ... ok
test workspace::tests::pill_clicks_follow_the_strip_rules ... ok
test workspace::tests::restore_drops_an_empty_group_and_an_empty_window ... ok
test workspace::tests::restore_repairs_a_focused_tab_out_of_range ... ok
test workspace::tests::restore_repairs_a_width_share_over_1000 ... ok
test workspace::tests::restore_repairs_hand_heights_of_the_wrong_length ... ok
test workspace::tests::restore_replaces_a_column_list_that_does_not_match_its_tabs ... ok
test workspace::tests::selection_all_uses_the_handlers_names_and_none_clears_them ... ok
test workspace::tests::tab_focus_rejects_an_index_past_the_end_and_changes_nothing ... ok
test workspace::tests::restore_trims_history_from_the_oldest_end ... ok
test workspace::tests::tab_close_selected_closes_the_selected_pills_and_focuses_right_else_left ... ok
test workspace::tests::tab_move_reorders_pills_and_rejects_bad_indices ... ok
test workspace::tests::tab_new_in_an_empty_workspace_uses_home ... ok
test workspace::tests::tab_new_opens_after_a_group_pill_and_not_inside_it ... ok
test workspace::tests::tab_new_opens_after_the_focused_pill_and_inherits_its_folder ... ok
test workspace::tests::the_scroll_anchor_is_recorded_and_a_repeat_is_a_no_op ... ok
test workspace::tests::the_second_launch_with_no_paths_opens_one_window_at_home ... ok
test workspace::tests::the_second_launch_with_folders_opens_one_tab_each_in_the_focused_window ... ok
test workspace::tests::tab_open_in_new_window_moves_the_tab_out ... ok
test workspace::tests::window_new_opens_a_tab_and_focuses_it ... ok
test fileops::tests::skip_and_keep_both_are_applied_per_item ... ok
test fileops::tests::cancelling_stops_after_the_current_item ... ok
test fileops::tests::a_failed_item_does_not_stop_the_others ... ok
test fileops::tests::copying_into_its_own_folder_uses_copy_and_copy_2 ... ok
test fileops::tests::a_folder_tree_with_a_symlink_is_copied_faithfully ... ok
test fileops::tests::new_folder_and_rename_run_and_the_plan_lists_them ... ok
test fileops::tests::the_undo_stack_keeps_at_most_fifty_records ... ok
test fileops::tests::undo_a_rename_renames_it_back ... ok
test fileops::tests::undo_a_move_moves_the_item_back ... ok
test fileops::tests::undo_refuses_only_the_item_that_changed ... ok
test workspace::tests::a_long_random_sequence_keeps_the_invariants ... ok

test result: ok. 173 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running tests/trash.rs (target/debug/deps/trash-5dc9b4a126eac558)

running 12 tests
test child_cross_filesystem_move ... ignored
test child_mixed_decisions ... ignored
test child_new_folder_undo ... ignored
test child_replace_copy_and_undo ... ignored
test child_trash_and_undo ... ignored
test child_untrashable_source ... ignored, runs only as a child of an_untrashable_source_keeps_both_copies
test new_folder_undo_trashes_only_an_empty_folder ... ok
test an_untrashable_source_keeps_both_copies ... ok
test a_cross_filesystem_folder_move_trashes_its_source ... ok
test trash_and_its_undo_use_the_scratch_trash ... ok
test every_decision_runs_together_in_one_mix ... ok
test replace_trashes_the_old_target_and_undo_restores_it ... ok

test result: ok. 6 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/main.rs (target/debug/deps/gimbal_files-d8e87882e1328c49)

running 46 tests
test events::tests::a_window_event_opens_or_closes_the_one_window ... ok
test events::tests::only_group_formation_and_dissolution_fade ... ok
test commands::tests::hotkey_mapping_keeps_the_command_and_its_arguments ... ok
test events::tests::tab_group_and_focus_events_touch_the_header_and_content ... ok
test events::tests::the_other_events_touch_only_what_they_can_change ... ok
test layout::tests::a_retarget_uses_the_new_end_width ... ok
test header::tests::a_single_tab_shows_its_folder_name ... ok
test layout::tests::a_reverse_restarts_from_the_current_width ... ok
test header::tests::a_root_folder_cannot_go_up ... ok
test header::tests::a_split_group_shows_its_title_and_count_then_its_name ... ok
test layout::tests::collapsed_sidebars_are_strips ... ok
test layout::tests::defaults_match_the_sketch ... ok
test layout::tests::giving_way_never_touches_the_saved_widths ... ok
test header::tests::the_view_marks_the_focused_pill_active_and_keeps_selection ... ok
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
test look::css::tests::close_icon_is_symbolic_with_foreground_strokes ... ok
test look::css::tests::float_icon_is_symbolic_and_maps_the_front_fill ... ok
test look::css::tests::opacity_follows_the_config_range ... ok
test look::css::tests::scaling ... ok
test look::css::tests::header_fits_stretches_then_collapses ... ok
test look::css::tests::nov_1_outlines_and_contrast ... ok
test look::css::tests::pinned_list_has_no_fill_and_collapsed_pills_sit_six_apart ... ok
test look::icons::tests::all_six_mockup_icons_are_embedded ... ok
test look::icons::tests::palettes_stay_on_theme_colours ... ok
test look::css::tests::icon_palettes_map_the_spec_table ... ok
test look::motion::tests::durations_scale_by_the_slowdown ... ok
test look::tokens::tests::scaled_tokens_keep_a_visible_pixel ... ok
test look::tokens::tests::tokens_lost_nothing_in_the_merge ... ok
test platform::tests::niri_floats_through_ipc_and_has_no_minimize ... ok
test platform::tests::other_desktops_maximize_and_show_minimize ... ok
test look::theme::tests::imports_are_watched_once_each ... ok
test save::tests::a_change_at_the_deadline_is_still_a_full_second_away ... ok
test save::tests::the_debounce_restarts_on_every_change_with_a_fake_clock ... ok
test look::css::tests::spec_rules ... ok
test look::css::tests::spec_rules_and_css_shape ... ok

test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests fm_agent

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests fm_core

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

==> cargo build --workspace --release
    Finished `release` profile [optimized] target(s) in 0.02s
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

### Four 10-second runs

`pgrep -a gimbal-files` was empty before every run. Each run used its own
`XDG_CONFIG_HOME` and `XDG_STATE_HOME` under `/tmp/gimbal-p2h-final3-*` and was
stopped by PID with SIGTERM. No GTK, CSS or theme warnings appeared. Run 2
printed the allowed intermittent `VK_SUBOPTIMAL_KHR` notice. Run 1's VK
notice did not appear this time; run 3's did not either. `[observed]`

**1. No session file: one window at home.**

```text
[gimbal 0.082] config: /tmp/gimbal-p2h-final3/run1/config/gimbal-files/config.toml does not exist; defaults apply
[gimbal 0.082] config: scale 1.00 (base 1.00), opacity 0.85, font "", left collapsed false width 348, right collapsed false width 648
[gimbal 0.082] theme: reloaded /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.082] theme: watching /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.082] theme: watching /home/kyle/.config/gtk-4.0/noctalia-extensions.css
[gimbal 0.083] session: restored 1 window(s)
[gimbal 0.085] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal 0.085] blur: capabilities Capability(Blur) (blur true)
[gimbal 0.085] hotkey: bound "<Alt>Left" to nav.back
[gimbal 0.085] hotkey: bound "<Alt>Right" to nav.forward
[gimbal 0.085] hotkey: bound "<Alt>Up" to nav.up
[gimbal 0.085] hotkey: bound "<Control>a" to selection.all
[gimbal 0.085] hotkey: bound "<Control>equal" to view.zoom-in
[gimbal 0.085] hotkey: bound "<Control>f" to search.open
[gimbal 0.085] hotkey: bound "<Control>l" to nav.focus-path
[gimbal 0.085] hotkey: bound "<Control>minus" to view.zoom-out
[gimbal 0.085] hotkey: bound "<Control>n" to window.new
[gimbal 0.085] hotkey: bound "<Control>q" to app.quit
[gimbal 0.085] hotkey: bound "<Control>t" to tab.new
[gimbal 0.085] hotkey: bound "<Control>w" to tab.close
[gimbal 0.085] hotkey: bound "<Control>z" to file.undo
[gimbal 0.085] hotkey: bound "Delete" to file.trash
[gimbal 0.085] hotkey: bound "F5" to nav.refresh
[gimbal 0.085] header: window 1 pills ["kyle"] (active 0, overflow false)
[gimbal 0.085] layout: scale 1.00: left 348, content 0, preview 648 (inset 8)
[gimbal 0.085] window: built model window 1 (0×0)
[gimbal 0.845] window: opened model window 1
[gimbal 0.845] fm_ui: activated; the session is restored
[gimbal 0.850] blur: surface layout changed to 1322×1448
[gimbal 0.850] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1272, h: 1374 }, Rect { x: 37, y: 25, w: 1248, h: 3 }, Rect { x: 37, y: 1420, w: 1248, h: 3 }, Rect { x: 30, y: 28, w: 1262, h: 3 }, Rect { x: 30, y: 1417, w: 1262, h: 3 }, Rect { x: 27, y: 31, w: 1268, h: 3 }, Rect { x: 27, y: 1414, w: 1268, h: 3 }, Rect { x: 26, y: 34, w: 1270, h: 3 }, Rect { x: 26, y: 1411, w: 1270, h: 3 }]
[gimbal 1.845] session: saved /tmp/gimbal-p2h-final3/run1/state/gimbal-files/session.toml
```

**2. A hand-written session with two windows, a renamed 2 + 2 group and a tab
with history.**

The session file had `focused_window = 0`, window 1 with group `Work`,
`columns = [[0, 1], [2, 3]]`, and window 2 with `back = [a]` and
`forward = [c]`.

```text
[gimbal 0.067] config: /tmp/gimbal-p2h-final3/run2/config/gimbal-files/config.toml does not exist; defaults apply
[gimbal 0.067] config: scale 1.00 (base 1.00), opacity 0.85, font "", left collapsed false width 348, right collapsed false width 648
[gimbal 0.067] theme: reloaded /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.067] theme: watching /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.067] theme: watching /home/kyle/.config/gtk-4.0/noctalia-extensions.css
[gimbal 0.067] session: restored 2 window(s)
[gimbal 0.069] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal 0.069] blur: capabilities Capability(Blur) (blur true)
[gimbal 0.069] hotkey: bound "<Alt>Left" to nav.back
[gimbal 0.069] hotkey: bound "<Alt>Right" to nav.forward
[gimbal 0.069] hotkey: bound "<Alt>Up" to nav.up
[gimbal 0.069] hotkey: bound "<Control>a" to selection.all
[gimbal 0.069] hotkey: bound "<Control>equal" to view.zoom-in
[gimbal 0.069] hotkey: bound "<Control>f" to search.open
[gimbal 0.069] hotkey: bound "<Control>l" to nav.focus-path
[gimbal 0.069] hotkey: bound "<Control>minus" to view.zoom-out
[gimbal 0.069] hotkey: bound "<Control>n" to window.new
[gimbal 0.069] hotkey: bound "<Control>q" to app.quit
[gimbal 0.069] hotkey: bound "<Control>t" to tab.new
[gimbal 0.069] hotkey: bound "<Control>w" to tab.close
[gimbal 0.069] hotkey: bound "<Control>z" to file.undo
[gimbal 0.069] hotkey: bound "Delete" to file.trash
[gimbal 0.069] hotkey: bound "F5" to nav.refresh
[gimbal 0.069] header: window 1 pills ["Work · 4 tabs"] (active 0, overflow false)
[gimbal 0.069] layout: scale 1.00: left 348, content 0, preview 648 (inset 8)
[gimbal 0.069] window: built model window 1 (0×0)
[gimbal 0.099] window: opened model window 1
[gimbal 0.100] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal 0.100] blur: capabilities Capability(Blur) (blur true)
[gimbal 0.100] hotkey: bound "<Alt>Left" to nav.back
[gimbal 0.100] hotkey: bound "<Alt>Right" to nav.forward
[gimbal 0.100] hotkey: bound "<Alt>Up" to nav.up
[gimbal 0.100] hotkey: bound "<Control>a" to selection.all
[gimbal 0.100] hotkey: bound "<Control>equal" to view.zoom-in
[gimbal 0.100] hotkey: bound "<Control>f" to search.open
[gimbal 0.100] hotkey: bound "<Control>l" to nav.focus-path
[gimbal 0.100] hotkey: bound "<Control>minus" to view.zoom-out
[gimbal 0.100] hotkey: bound "<Control>n" to window.new
[gimbal 0.100] hotkey: bound "<Control>q" to app.quit
[gimbal 0.100] hotkey: bound "<Control>t" to tab.new
[gimbal 0.100] hotkey: bound "<Control>w" to tab.close
[gimbal 0.100] hotkey: bound "<Control>z" to file.undo
[gimbal 0.100] hotkey: bound "Delete" to file.trash
[gimbal 0.100] hotkey: bound "F5" to nav.refresh
[gimbal 0.100] header: window 7 pills ["b"] (active 0, overflow false)
[gimbal 0.100] layout: scale 1.00: left 348, content 0, preview 648 (inset 8)
[gimbal 0.100] window: built model window 7 (0×0)
[gimbal 0.102] window: opened model window 7
[gimbal 0.102] fm_ui: activated; the session is restored
[gimbal 0.106] blur: surface layout changed to 1322×1448
[gimbal 0.106] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1272, h: 1374 }, Rect { x: 37, y: 25, w: 1248, h: 3 }, Rect { x: 37, y: 1420, w: 1248, h: 3 }, Rect { x: 30, y: 28, w: 1262, h: 3 }, Rect { x: 30, y: 1417, w: 1262, h: 3 }, Rect { x: 27, y: 31, w: 1268, h: 3 }, Rect { x: 27, y: 1414, w: 1268, h: 3 }, Rect { x: 26, y: 34, w: 1270, h: 3 }, Rect { x: 26, y: 1411, w: 1270, h: 3 }]
[gimbal 0.112] blur: surface layout changed to 1322×1448
[gimbal 0.112] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1272, h: 1374 }, Rect { x: 37, y: 25, w: 1248, h: 3 }, Rect { x: 37, y: 1420, w: 1248, h: 3 }, Rect { x: 30, y: 28, w: 1262, h: 3 }, Rect { x: 30, y: 1417, w: 1262, h: 3 }, Rect { x: 27, y: 31, w: 1268, h: 3 }, Rect { x: 27, y: 1414, w: 1268, h: 3 }, Rect { x: 26, y: 34, w: 1270, h: 3 }, Rect { x: 26, y: 1411, w: 1270, h: 3 }]
[gimbal 0.112] model: WindowFocused(WindowId(7))
[gimbal 0.112] focus: model window 7 is now the focused one
[gimbal 0.113] model: WindowFocused(WindowId(1))
[gimbal 0.113] focus: model window 1 is now the focused one
[gimbal 1.114] session: saved /tmp/gimbal-p2h-final3/run2/state/gimbal-files/session.toml
```

Both windows opened, and the header logs list their pills:
`["Work · 4 tabs"]` and `["b"]`. The saved file kept the name, the 2 + 2
columns and the history. `[observed]`

**3. An unparsable session: one window at home, the problem logged, then
`session.toml.bad`.**

```text
[gimbal 0.080] config: /tmp/gimbal-p2h-final3/run3/config/gimbal-files/config.toml does not exist; defaults apply
[gimbal 0.080] config: scale 1.00 (base 1.00), opacity 0.85, font "", left collapsed false width 348, right collapsed false width 648
[gimbal 0.080] theme: reloaded /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia-extensions.css
[gimbal 0.080] session: /tmp/gimbal-p2h-final3/run3/state/gimbal-files/session.toml: could not parse the session: unclosed table, expected `]`
[gimbal 0.080] session: restored 1 window(s)
[gimbal 0.082] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal 0.082] blur: capabilities Capability(Blur) (blur true)
[gimbal 0.082] hotkey: bound "<Alt>Left" to nav.back
[gimbal 0.082] hotkey: bound "<Alt>Right" to nav.forward
[gimbal 0.082] hotkey: bound "<Alt>Up" to nav.up
[gimbal 0.082] hotkey: bound "<Control>a" to selection.all
[gimbal 0.082] hotkey: bound "<Control>equal" to view.zoom-in
[gimbal 0.082] hotkey: bound "<Control>f" to search.open
[gimbal 0.082] hotkey: bound "<Control>l" to nav.focus-path
[gimbal 0.082] hotkey: bound "<Control>minus" to view.zoom-out
[gimbal 0.082] hotkey: bound "<Control>n" to window.new
[gimbal 0.082] hotkey: bound "<Control>q" to app.quit
[gimbal 0.082] hotkey: bound "<Control>t" to tab.new
[gimbal 0.082] hotkey: bound "<Control>w" to tab.close
[gimbal 0.082] hotkey: bound "<Control>z" to file.undo
[gimbal 0.082] hotkey: bound "Delete" to file.trash
[gimbal 0.082] hotkey: bound "F5" to nav.refresh
[gimbal 0.082] header: window 1 pills ["kyle"] (active 0, overflow false)
[gimbal 0.082] layout: scale 1.00: left 348, content 0, preview 648 (inset 8)
[gimbal 0.083] window: built model window 1 (0×0)
[gimbal 0.118] window: opened model window 1
[gimbal 0.119] fm_ui: activated; the session is restored
[gimbal 0.123] blur: surface layout changed to 1322×1448
[gimbal 0.123] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1272, h: 1374 }, Rect { x: 37, y: 25, w: 1248, h: 3 }, Rect { x: 37, y: 1420, w: 1248, h: 3 }, Rect { x: 30, y: 28, w: 1262, h: 3 }, Rect { x: 30, y: 1417, w: 1262, h: 3 }, Rect { x: 27, y: 31, w: 1268, h: 3 }, Rect { x: 27, y: 1414, w: 1268, h: 3 }, Rect { x: 26, y: 34, w: 1270, h: 3 }, Rect { x: 26, y: 1411, w: 1270, h: 3 }]
[gimbal 1.119] session: kept the broken file as /tmp/gimbal-p2h-final3/run3/state/gimbal-files/session.toml.bad
[gimbal 1.119] session: saved /tmp/gimbal-p2h-final3/run3/state/gimbal-files/session.toml
```

`session.toml.bad` contains the original 8 bytes `[window\n`, and the new
`session.toml` was written by the same save. `[observed]`

**4. A second launch with two folders while run 1's instance is running.**

```text
[gimbal 0.080] config: /tmp/gimbal-p2h-final3/run4/config/gimbal-files/config.toml does not exist; defaults apply
[gimbal 0.080] config: scale 1.00 (base 1.00), opacity 0.85, font "", left collapsed false width 348, right collapsed false width 648
[gimbal 0.080] theme: reloaded /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia-extensions.css
[gimbal 0.081] session: restored 1 window(s)
[gimbal 0.083] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal 0.083] blur: capabilities Capability(Blur) (blur true)
[gimbal 0.083] hotkey: bound "<Alt>Left" to nav.back
[gimbal 0.083] hotkey: bound "<Alt>Right" to nav.forward
[gimbal 0.083] hotkey: bound "<Alt>Up" to nav.up
[gimbal 0.083] hotkey: bound "<Control>a" to selection.all
[gimbal 0.083] hotkey: bound "<Control>equal" to view.zoom-in
[gimbal 0.083] hotkey: bound "<Control>f" to search.open
[gimbal 0.083] hotkey: bound "<Control>l" to nav.focus-path
[gimbal 0.083] hotkey: bound "<Control>minus" to view.zoom-out
[gimbal 0.083] hotkey: bound "<Control>n" to window.new
[gimbal 0.083] hotkey: bound "<Control>q" to app.quit
[gimbal 0.083] hotkey: bound "<Control>t" to tab.new
[gimbal 0.083] hotkey: bound "<Control>w" to tab.close
[gimbal 0.083] hotkey: bound "<Control>z" to file.undo
[gimbal 0.083] hotkey: bound "Delete" to file.trash
[gimbal 0.083] hotkey: bound "F5" to nav.refresh
[gimbal 0.083] header: window 1 pills ["kyle"] (active 0, overflow false)
[gimbal 0.083] layout: scale 1.00: left 348, content 0, preview 648 (inset 8)
[gimbal 0.083] window: built model window 1 (0×0)
[gimbal 0.119] window: opened model window 1
[gimbal 0.119] fm_ui: activated; the session is restored
[gimbal 0.123] blur: surface layout changed to 1322×1448
[gimbal 0.123] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1272, h: 1374 }, Rect { x: 37, y: 25, w: 1248, h: 3 }, Rect { x: 37, y: 1420, w: 1248, h: 3 }, Rect { x: 30, y: 28, w: 1262, h: 3 }, Rect { x: 30, y: 1417, w: 1262, h: 3 }, Rect { x: 27, y: 31, w: 1268, h: 3 }, Rect { x: 27, y: 1414, w: 1268, h: 3 }, Rect { x: 26, y: 34, w: 1270, h: 3 }, Rect { x: 26, y: 1411, w: 1270, h: 3 }]
[gimbal 1.120] session: saved /tmp/gimbal-p2h-final3/run4/state/gimbal-files/session.toml
[gimbal 2.005] fm_ui: open () with 2 folder argument(s): ["/tmp/gimbal-p2h-final3-x/a", "/tmp/gimbal-p2h-final3-x/b"]
[gimbal 2.005] launch: open request: ["/tmp/gimbal-p2h-final3-x/a", "/tmp/gimbal-p2h-final3-x/b"]
[gimbal 2.005] model: TabAdded { window: WindowId(1), group: GroupId(5), tab: TabId(4), index: 1 }
[gimbal 2.005] header: window 1 pills ["kyle", "a", "b"] (active 2, overflow true)
[gimbal 2.005] model: FocusChanged { window: WindowId(1), tab: TabId(4) }
[gimbal 2.006] header: window 1 pills ["kyle", "a", "b"] (active 2, overflow true)
[gimbal 2.006] model: TabAdded { window: WindowId(1), group: GroupId(7), tab: TabId(6), index: 2 }
[gimbal 2.006] header: window 1 pills ["kyle", "a", "b"] (active 2, overflow true)
[gimbal 2.006] model: FocusChanged { window: WindowId(1), tab: TabId(6) }
[gimbal 2.006] header: window 1 pills ["kyle", "a", "b"] (active 2, overflow true)
[gimbal 3.007] session: saved /tmp/gimbal-p2h-final3/run4/state/gimbal-files/session.toml
```

The second process' stderr was empty (it handed off and exited). The saved
session lists three pills and folders `/home/kyle`, `.../a`, `.../b`.
`[observed]`

### Temporary timer log (harness removed before delivery)

The harness was `GIMBAL_P2H_TIMER=1`: a 1.3 s repeating timeout in one window
that logged the model pills, the header pills and the session file, then ran
`tab.new` three times, Ctrl/Shift-selected pills 1-3 and `group.split`,
`group.rename name=Work`, `nav.go` into a folder, `nav.back`, and
`tab.open-in-new-window`. The log below is from that run. The harness is gone:
`grep -rn "GIMBAL_P2H_TIMER\|timer_tick\|Temporary P2h" crates/fm-ui/src`
prints nothing. `[observed]`

```text
[gimbal 1.405] timer: ---- after step 0 ----
[gimbal 1.405] timer: model window 1 pills ["kyle"]
[gimbal 1.405] timer: header window 1 pills ["kyle"]
[gimbal 1.405] timer: session:
version = 1
focused_window = 0

[[window]]
pills = [0]
focused_tab = 0

[[window.group]]
number = 1
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[gimbal 1.405] command: tab.new via Ui (view)
[gimbal 1.405] model: TabAdded { window: WindowId(1), group: GroupId(5), tab: TabId(4), index: 1 }
[gimbal 1.406] header: window 1 pills ["kyle", "kyle"] (active 1, overflow true)
[gimbal 1.406] model: FocusChanged { window: WindowId(1), tab: TabId(4) }
[gimbal 1.406] header: window 1 pills ["kyle", "kyle"] (active 1, overflow true)
[gimbal 1.406] command: tab.new via Ui (view)
[gimbal 1.406] model: TabAdded { window: WindowId(1), group: GroupId(7), tab: TabId(6), index: 2 }
[gimbal 1.406] header: window 1 pills ["kyle", "kyle", "kyle"] (active 2, overflow true)
[gimbal 1.406] model: FocusChanged { window: WindowId(1), tab: TabId(6) }
[gimbal 1.406] header: window 1 pills ["kyle", "kyle", "kyle"] (active 2, overflow true)
[gimbal 1.406] command: tab.new via Ui (view)
[gimbal 1.406] model: TabAdded { window: WindowId(1), group: GroupId(9), tab: TabId(8), index: 3 }
[gimbal 1.407] header: window 1 pills ["kyle", "kyle", "kyle", "kyle"] (active 3, overflow true)
[gimbal 1.407] model: FocusChanged { window: WindowId(1), tab: TabId(8) }
[gimbal 1.407] header: window 1 pills ["kyle", "kyle", "kyle", "kyle"] (active 3, overflow true)
[gimbal 2.408] session: saved /tmp/gimbal-p2h-timer/state/gimbal-files/session.toml
[gimbal 2.706] timer: ---- after step 1 ----
[gimbal 2.706] timer: model window 1 pills ["kyle", "kyle", "kyle", "kyle"]
[gimbal 2.706] timer: header window 1 pills ["kyle", "kyle", "kyle", "kyle"]
[gimbal 2.706] timer: session:
version = 1
focused_window = 0

[[window]]
pills = [0, 1, 2, 3]
focused_tab = 3

[[window.group]]
number = 1
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group]]
number = 2
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group]]
number = 3
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group]]
number = 4
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[gimbal 2.706] model: FocusChanged { window: WindowId(1), tab: TabId(4) }
[gimbal 2.706] header: window 1 pills ["kyle", "kyle", "kyle", "kyle"] (active 1, overflow true)
[gimbal 2.706] model: SelectionChanged { window: WindowId(1) }
[gimbal 2.706] header: window 1 pills ["kyle", "kyle", "kyle", "kyle"] (active 2, overflow true)
[gimbal 2.706] model: FocusChanged { window: WindowId(1), tab: TabId(6) }
[gimbal 2.707] header: window 1 pills ["kyle", "kyle", "kyle", "kyle"] (active 2, overflow true)
[gimbal 2.707] model: SelectionChanged { window: WindowId(1) }
[gimbal 2.707] header: window 1 pills ["kyle", "kyle", "kyle", "kyle"] (active 3, overflow true)
[gimbal 2.707] model: FocusChanged { window: WindowId(1), tab: TabId(8) }
[gimbal 2.707] header: window 1 pills ["kyle", "kyle", "kyle", "kyle"] (active 3, overflow true)
[gimbal 2.707] command: group.split via Ui (view)
[gimbal 2.707] model: GroupDissolved { window: WindowId(1), group: GroupId(5) }
[gimbal 2.707] header: window 1 pills ["kyle", "Group 2 · 3 tabs"] (active 1, overflow true)
[gimbal 2.707] model: GroupDissolved { window: WindowId(1), group: GroupId(7) }
[gimbal 2.707] header: window 1 pills ["kyle", "Group 2 · 3 tabs"] (active 1, overflow true)
[gimbal 2.707] model: GroupDissolved { window: WindowId(1), group: GroupId(9) }
[gimbal 2.707] header: window 1 pills ["kyle", "Group 2 · 3 tabs"] (active 1, overflow true)
[gimbal 2.707] model: GroupFormed { window: WindowId(1), group: GroupId(10) }
[gimbal 2.707] header: window 1 pills ["kyle", "Group 2 · 3 tabs"] (active 1, overflow true)
[gimbal 2.707] model: FocusChanged { window: WindowId(1), tab: TabId(4) }
[gimbal 2.708] header: window 1 pills ["kyle", "Group 2 · 3 tabs"] (active 1, overflow true)
[gimbal 3.708] session: saved /tmp/gimbal-p2h-timer/state/gimbal-files/session.toml
[gimbal 4.006] timer: ---- after step 2 ----
[gimbal 4.006] timer: model window 1 pills ["kyle", "Group 2 · 3 tabs"]
[gimbal 4.006] timer: header window 1 pills ["kyle", "Group 2 · 3 tabs"]
[gimbal 4.006] timer: session:
version = 1
focused_window = 0

[[window]]
pills = [0, 1]
focused_tab = 1

[[window.group]]
number = 1
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group]]
number = 2
width_permille = 500
heights = ["default", "default"]
columns = [[0, 1], [2]]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[gimbal 4.006] command: group.rename via Ui (view)
[gimbal 4.006] model: GroupRenamed { window: WindowId(1), group: GroupId(10) }
[gimbal 4.007] header: window 1 pills ["kyle", "Work · 3 tabs"] (active 1, overflow true)
[gimbal 5.008] session: saved /tmp/gimbal-p2h-timer/state/gimbal-files/session.toml
[gimbal 5.306] timer: ---- after step 3 ----
[gimbal 5.306] timer: model window 1 pills ["kyle", "Work · 3 tabs"]
[gimbal 5.306] timer: header window 1 pills ["kyle", "Work · 3 tabs"]
[gimbal 5.306] timer: session:
version = 1
focused_window = 0

[[window]]
pills = [0, 1]
focused_tab = 1

[[window.group]]
number = 1
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group]]
name = "Work"
number = 2
width_permille = 500
heights = ["default", "default"]
columns = [[0, 1], [2]]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[gimbal 5.306] command: nav.go via Ui (view)
[gimbal 5.306] model: Navigated { window: WindowId(1), tab: TabId(4), folder: "/tmp/gimbal-p2h-timer/a" }
[gimbal 5.307] header: window 1 pills ["kyle", "Work · 3 tabs"] (active 1, overflow true)
[gimbal 6.308] session: saved /tmp/gimbal-p2h-timer/state/gimbal-files/session.toml
[gimbal 6.606] timer: ---- after step 4 ----
[gimbal 6.606] timer: model window 1 pills ["kyle", "Work · 3 tabs"]
[gimbal 6.607] timer: header window 1 pills ["kyle", "Work · 3 tabs"]
[gimbal 6.607] timer: session:
version = 1
focused_window = 0

[[window]]
pills = [0, 1]
focused_tab = 1

[[window.group]]
number = 1
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group]]
name = "Work"
number = 2
width_permille = 500
heights = ["default", "default"]
columns = [[0, 1], [2]]

[[window.group.tab]]
folder = "/tmp/gimbal-p2h-timer/a"
back = ["/home/kyle"]
forward = []
selection_names = []

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[gimbal 6.607] command: nav.back via Ui (view)
[gimbal 6.607] model: Navigated { window: WindowId(1), tab: TabId(4), folder: "/home/kyle" }
[gimbal 6.607] header: window 1 pills ["kyle", "Work · 3 tabs"] (active 1, overflow true)

(gimbal-files:651031): Gdk-WARNING **: 10:00:15.407: vkAcquireNextImageKHR(): A swapchain no longer matches the surface properties exactly, but can still be used to present to the surface successfully. (VK_SUBOPTIMAL_KHR) (1000001003)
[gimbal 7.608] session: saved /tmp/gimbal-p2h-timer/state/gimbal-files/session.toml
[gimbal 7.907] timer: ---- after step 5 ----
[gimbal 7.907] timer: model window 1 pills ["kyle", "Work · 3 tabs"]
[gimbal 7.907] timer: header window 1 pills ["kyle", "Work · 3 tabs"]
[gimbal 7.907] timer: session:
version = 1
focused_window = 0

[[window]]
pills = [0, 1]
focused_tab = 1

[[window.group]]
number = 1
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group]]
name = "Work"
number = 2
width_permille = 500
heights = ["default", "default"]
columns = [[0, 1], [2]]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = ["/tmp/gimbal-p2h-timer/a"]
selection_names = []

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[gimbal 7.907] command: tab.open-in-new-window via Ui (view)
[gimbal 7.907] model: TabMoved { window: WindowId(1), tab: TabId(4), from: GroupId(10), to: GroupId(12) }
[gimbal 7.907] session: saved /tmp/gimbal-p2h-timer/state/gimbal-files/session.toml
[gimbal 7.907] header: window 1 pills ["kyle", "Work · 2 tabs"] (active 1, overflow true)
[gimbal 7.907] model: LayoutChanged { window: WindowId(1), group: GroupId(10) }
[gimbal 7.907] header: window 1 pills ["kyle", "Work · 2 tabs"] (active 1, overflow true)
[gimbal 7.907] model: FocusChanged { window: WindowId(1), tab: TabId(8) }
[gimbal 7.907] header: window 1 pills ["kyle", "Work · 2 tabs"] (active 1, overflow true)
[gimbal 7.908] model: WindowOpened(WindowId(11))
[gimbal 7.908] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal 7.908] blur: capabilities Capability(Blur) (blur true)
[gimbal 7.909] hotkey: bound "<Alt>Left" to nav.back
[gimbal 7.909] hotkey: bound "<Alt>Right" to nav.forward
[gimbal 7.909] hotkey: bound "<Alt>Up" to nav.up
[gimbal 7.909] hotkey: bound "<Control>a" to selection.all
[gimbal 7.909] hotkey: bound "<Control>equal" to view.zoom-in
[gimbal 7.909] hotkey: bound "<Control>f" to search.open
[gimbal 7.909] hotkey: bound "<Control>l" to nav.focus-path
[gimbal 7.909] hotkey: bound "<Control>minus" to view.zoom-out
[gimbal 7.909] hotkey: bound "<Control>n" to window.new
[gimbal 7.909] hotkey: bound "<Control>q" to app.quit
[gimbal 7.909] hotkey: bound "<Control>t" to tab.new
[gimbal 7.909] hotkey: bound "<Control>w" to tab.close
[gimbal 7.909] hotkey: bound "<Control>z" to file.undo
[gimbal 7.909] hotkey: bound "Delete" to file.trash
[gimbal 7.909] hotkey: bound "F5" to nav.refresh
[gimbal 7.909] header: window 11 pills ["kyle"] (active 0, overflow false)
[gimbal 7.909] layout: scale 1.00: left 348, content 0, preview 648 (inset 8)
[gimbal 7.909] window: built model window 11 (0×0)
[gimbal 7.912] window: opened model window 11
[gimbal 7.912] model: FocusChanged { window: WindowId(11), tab: TabId(4) }
[gimbal 7.912] header: window 11 pills ["kyle"] (active 0, overflow false)
[gimbal 7.914] blur: surface layout changed to 1322×1448
[gimbal 7.914] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1272, h: 1374 }, Rect { x: 37, y: 25, w: 1248, h: 3 }, Rect { x: 37, y: 1420, w: 1248, h: 3 }, Rect { x: 30, y: 28, w: 1262, h: 3 }, Rect { x: 30, y: 1417, w: 1262, h: 3 }, Rect { x: 27, y: 31, w: 1268, h: 3 }, Rect { x: 27, y: 1414, w: 1268, h: 3 }, Rect { x: 26, y: 34, w: 1270, h: 3 }, Rect { x: 26, y: 1411, w: 1270, h: 3 }]
[gimbal 8.912] session: saved /tmp/gimbal-p2h-timer/state/gimbal-files/session.toml
[gimbal 9.207] timer: ---- after step 6 ----
[gimbal 9.207] timer: model window 1 pills ["kyle", "Work · 2 tabs"]
[gimbal 9.207] timer: header window 1 pills ["kyle", "Work · 2 tabs"]
[gimbal 9.207] timer: model window 11 pills ["kyle"]
[gimbal 9.207] timer: header window 11 pills ["kyle"]
[gimbal 9.207] timer: session:
version = 1
focused_window = 1

[[window]]
pills = [0, 1]
focused_tab = 1

[[window.group]]
number = 1
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group]]
name = "Work"
number = 2
width_permille = 500
heights = ["default", "default"]
columns = [[0], [1]]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = []
selection_names = []

[[window]]
pills = [0]
focused_tab = 0

[[window.group]]
number = 1
width_permille = 500
heights = ["default", "default"]
columns = [[0], []]

[[window.group.tab]]
folder = "/home/kyle"
back = []
forward = ["/tmp/gimbal-p2h-timer/a"]
selection_names = []

```

The final session shows window 1 with the home pill plus `Work · 2 tabs`, and
window 11 with the tab that had the `/tmp/gimbal-p2h-timer/a` forward entry.
`[observed]`

## API citations

Read in the pinned sources under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. `[read]`

- `gio-0.22.10:src/auto/flags.rs:106-107` — `ApplicationFlags::HANDLES_OPEN`.
- `gio-0.22.10:src/application.rs:52` — `connect_open`.
- `gio-0.22.10:src/auto/file.rs:754` — `File::path`.
- `gtk4-0.11.5:src/auto/editable.rs:220-223` — `select_region`;
  `src/auto/entry.rs:968` — `EntryExt::set_alignment`.
- `gtk4-0.11.5:src/auto/window_handle.rs:50,119` — `WindowHandle::builder`,
  its `child`.
- `gtk4-0.11.5:src/auto/widget.rs:922` — `insert_after`, the drag's raise.
- `glib-0.22.10:src/source.rs:455,532` — `timeout_add_local`,
  `timeout_add_local_once`.
- `libadwaita-0.9.2:src/alert_dialog.rs:31-32,78` — `choose`/`choose_future`;
  `src/auto/alert_dialog.rs:440,611,704` — responses, default response and the
  response signal.
- `gtk4-0.11.5:src/auto/svg.rs:35` and `src/auto/image.rs:86` — the existing
  P2g `gtk::Svg` path used for Float and Close.
- `fm-core/src/workspace/mod.rs:884` — the new `focus_window`;
  `fm-core/src/session.rs:118,130,233` — `save`, `load`, `preserve_bad`;
  `fm-core/src/workspace/mod.rs:1880` — `open_request`.

No `unsafe` block was added. No dependency was added or changed. `[observed]`

## Not verified live (Claude)

- **The header look against the spike's.** Sizes come from the same token
  table and CSS, and the header logs its pills and fit, but the pixels
  (nav spacing, pill width, path fill, search crossfade, window icons) were
  not compared side by side. `[inferred]`
- **Pill drag.** The code claims past the surface threshold, raises with
  `insert_after`, never unparents and applies `drag_layout`; no pointer input
  was injected. `[inferred]`
- **The accordion.** The overflow decision is logged and its popover/list is
  built from the model, but the hover, its ✕ entries and the re-fill/replace
  moment need a pointer and eyes. `[inferred]`
- **The right-click menu and the two confirmations.** Split View, Close,
  Close Selected Tabs, Open in New Window, Rename Group, Close Group and Add
  to Group are wired; no menu or dialog was opened live. `[inferred]`
- **Float under niri.** `NIRI_SOCKET` selects `NiriToggle`; the adapter runs
  `niri msg action toggle-window-floating` only on a click. No action was
  run from here. `[inferred]`
- **The path bar.** Editing, Ctrl+L, Enter, the file-to-parent rule and the
  red invalid state compile and are wired; no text was typed live. `[inferred]`
- **A second launch.** Run 4 proves the GApplication handoff and the saved
  tabs in the log; how the raised/focused window looks at handoff needs the
  eye. `[inferred]`
- **The session across a real restart.** Run 2 restores the hand-written file
  and run 3 preserves a bad one; closing and relaunching by hand, including
  the last-window save, was not done. `[inferred]`
- **A live scale/opacity change.** The Settings path now loops over windows;
  no hotkey or slider was injected. `[inferred]`

## Anything outside my files

None. Only `crates/fm-core/src/workspace/{mod.rs,tests.rs}` and
`crates/fm-ui/src/` plus this notes file were changed. `spike/` was read only;
`Cargo.toml` and `Cargo.lock` were not edited. The temporary timer harness and
its `/tmp/gimbal-p2h-timer` scratch folder are not part of the delivery.
`[observed]`

## Review and live check (Claude, Nov 2, 2026): passes, with one fix by Claude

`scripts/check.sh` from `/tmp`: exit 0. The tests are 46 `fm-ui`, 174 `fm-core` (with the fix
below), the 6 trash parents, 7 CLI and 1 agent, and deny is clean. The process/window split is
right. The session writer is one thread working through saves in order, so an older save can't
overwrite a newer one.

**Live**, release build, niri-tiled on DP-3, with scratch `XDG_CONFIG_HOME` and `XDG_STATE_HOME`.
Kyle's real config and state were never created.
- **The header** matches the spike's:
  - left: menu, back, forward, up and search;
  - the pills, then the path bar with refresh;
  - right: Trash, Float and Close.

  The active pill is the lighter one: fill (41, 13, 85) against about (21, 8, 43) for the others.
- **A second launch with four folders** passed them to the running copy (exit 0). The log shows
  `pills ["kyle", "a", "b", "c", "d"] (active 4)`, and the path bar and content followed.
- **Pill click, two Ctrl-clicks, right-click:** the menu showed Split View, Close, Close Selected
  Tabs, Open in New Window and Add to Group; there were no group items on a single tab, as
  expected. Split View made one group pill, and the content listed both tiles' folders.
- **Ctrl+-** went through the window's hotkeys (scale 0.95).
- **The session file** saved the strip in order, with the group. Only the allowed `VK_SUBOPTIMAL`
  notice was logged.
- Letter hotkeys (Ctrl+T, Ctrl+N) couldn't be tested: Claude's virtual device delivers `-` and
  `=`, but its letter keys never reach the session. That's a limit of the test device, not the
  app; the agent's timer log covers `tab.new`.

**Fix (Claude): group numbers counted single tabs.**
- **The bug:** five pills split at pills 3–4 showed "Group 3 · 2 tabs" as the window's only
  group, because P2d gave every group, single tabs included, the lowest free number.
- **The fix:** only split groups take numbers.
  - `next_group_number` now counts split groups only (`split_number_excluding`).
  - A single tab that becomes a split through `group.add-tab` takes the next free split number at
    that moment.
  - `check_invariants` and the session restore's duplicate repair only require unique numbers
    among splits.
  - The field's doc comment says this.
- **Tests:** `group_split_uses_the_lowest_free_number` no longer asserts numbers on single tabs.
  The new `only_split_groups_take_group_numbers` reproduces the live case ("Group 1") and the
  add-tab path ("Group 2"). The random-sequence invariant test and the restore tests pass.

Not checked live (Kyle started using the display settings partway through, so Claude stopped
driving the pointer):
- the group ✕'s confirmation;
- Float under niri;
- pill drag and the accordion;
- typing in the path bar;
- a close-and-relaunch restore.
