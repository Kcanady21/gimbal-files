# P2j: the split view

`[observed]` unless a line says otherwise. The tags are `[read]` for source or
crate facts, `[observed]` for a command or a run, and `[inferred]` for anything
that needs a real pointer, a screenshot or a live feel.

## What I built

- **A tile per tab** -- `crates/fm-ui/src/split.rs`.
  - The split page is a `GtkScrolledWindow` (EXTERNAL horizontally, AUTOMATIC
    vertically) around one `GtkFixed`. Each tile is a framed pane in its own
    clipping scroller, and its pill is a separate `GtkFixed` child in a
    clipping scroller directly after the tile, so the pill paints over its own
    frame and can stay at the view top while the page scrolls.
  - `SplitView::show` reconciles the model's `TabId`s with the widgets already
    built. A tab that appears gets one tile and one `FolderView`; a tab that
    closes has its widget removed; every other tile keeps its widget and its
    folder view. `P2i`'s listing, sort, live updates, hidden-file filter,
    selection and scroll anchor are the per-tile grid, one instance per tab.
  - Tiles are placed by the spike's pixel rules: `frame` (gap-2xl sides and
    bottom, gap-l gutter), `layout`, `column_widths`, `default_heights`,
    `stored_heights`, `fill_last`, `heights_after_push`, `column_offsets`,
    `stuck_geometry`, `pill_layout`, `frame_rect`, `grid_size`, `drop_at`,
    `drop_line`, `border_at` and `drag_layout`. Model rules (`columns_without`,
    `apply_drop`, `heights_after_change`) are P2d's and are not duplicated.
  - The model's `SplitLayout` is converted by `LayoutState::from_layout`;
    heights are design px and are scaled in `layout`. The width share is
    permille and becomes the column share.
  - The content stack crossfades over `motion::ms(250)`: entering a split and
    leaving one when a group dissolves both fade. A normal pill keeps P2i's
    single grid.
- **Focus and selection.**
  - A capture-phase click on the tile, the pill's own click, and the grid all
    call `Process::focus_tab`. A grid click goes through
    `Process::file_click_tab`, which focuses the tile and then applies the
    plain / Ctrl / Shift click; `file_shift_click` still gets the visible order
    from that tile.
  - The focused tile's frame carries `gl-tile.focused`; its pill is `active`.
  - The one status line is gated: only the focused tile's `FolderView` writes
    it. `Content::listed_names`, `reload`, `open_path`, the header's path and
    history buttons, `nav.*`, `selection.*` and `file.open` all read the
    model's focused tab, which is the focused tile.
  - Scroll anchors are recorded per tab. `fm-core` records only the focused
    tab, so `Process::record_scroll_anchor_for` focuses the tile for that one
    update and restores the old focus without emitting UI events. See the note
    at the end.
- **Layout through the model.**
  - `SplitView` reads the group's columns, heights and share on every show.
    A model change that moves a rect animates it through `slide_to`; a
    viewport, scale or border change follows directly.
  - `slide_to` calls `place_parts` from the same interpolated rect every frame,
    so the tile, its pill wrap, the frame's top margin and the grid's content
    offset move together.
  - The outer scroller's value pins a fitting column at `scroll`; a taller
    column keeps offset 0. `stuck_geometry` keeps a stuck pill at view y 0 and
    the frame's top border `gap-s` below it while the contents slide under the
    frame's own top border.
- **Drags and borders, through the model.**
  - One `GtkGestureDrag` on the Fixed decides at the press between a tile drag
    (the pill hit test) and a border drag (the whole strip hit test). It
    ignores non-left buttons and claims nothing before the threshold.
  - A tile drag previews the post-drop slot layout with `drag_layout`, shows
    the accent `gl-drop-line` only for a gap insert, and commits through
    `Process::apply_tile_drop`; a refused drop clears the preview and
    `restore_tiles` slides every tile and pill back.
  - A cancelled tile drag (right button, Esc, focus loss, release outside)
    restores every tile. Esc and focus loss are wired in `WindowUi`; the
    right-button case is the gesture's own cancel; a release with no drop also
    restores. The right press cannot open the pill menu in that event.
  - The column border previews `width_permille` and writes
    `set_width_share` once at the end. A stacked border previews
    `Heights::Hand` from `heights_after_push` and writes
    `set_column_heights` at the end; its floor is `min_slot_height`, not the
    half floor. A double-click resets to `Heights::Default` or share 500.
    The drag's preview is kept in the split view's `preview`; the end reads
    its final values, so the model gets what the pointer reached, not what it
    started from.
  - A cancelled border drag commits the preview it had reached, matching
    P1m-r8's "its sizes stay" rule.
- **Closing and the menu.**
  - A tile's ✕ focuses the tab and dispatches `tab.close`. The model
    rebalances; the survivors animate; a group of one returns to the single
    grid through the stack crossfade.
  - A right-click on a tile's pill offers `Close` (`tab.close`),
    `Open in New Window` (`tab.open-in-new-window`) and `Close Group`
    (`group.close` through P2h's `Header::confirm_group`).
  - The header's group pill and its menu are untouched.
- **Commands.** Every command that says "the focused tab" still reaches the
  model, and the UI now follows the focused tile. `view.set-sort scope=folder`
  re-sorts every tile showing that folder, including hidden split tiles, and
  `scope=global` re-sorts all of them. `pins.add` without a path uses
  `change_for`'s current folder, which is the focused tile's folder.

No `fm-core` file was changed.

## Verified

### `scripts/check.sh`

Run from `$PROJ`; exit 0. `fm-ui` has 99 tests, 38 of them in `split.rs`.
`fm-core` has 175. The whole file is in the block below.

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
test tests::check_keeps_the_command_and_its_arguments ... ok
test tests::help_shows_usage ... ok
test tests::check_without_a_command_is_a_usage_error ... ok
test tests::version_shows_the_version ... ok
test tests::commands_lists_the_registry ... ok
test tests::no_arguments_shows_usage ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (target/debug/deps/fm_core-71b31b2576b0b33f)

running 175 tests
test command::tests::a_checked_command_prints_in_catalogue_order ... ok
test command::tests::a_files_command_defaults_dry_run_to_false ... ok
test command::tests::a_known_command_is_not_an_unknown_command ... ok
test command::tests::a_token_without_equals_is_an_error ... ok
test command::tests::an_empty_argument_name_is_an_error ... ok
test command::tests::a_single_valued_argument_may_not_repeat ... ok
test command::tests::a_valid_call_reaches_the_handler_once ... ok
test command::tests::an_invalid_call_never_reaches_the_handler ... ok
test command::tests::a_forbidden_call_never_reaches_the_handler ... ok
test command::tests::a_value_may_contain_equals_and_spaces ... ok
test command::tests::a_distant_typo_gets_no_suggestion ... ok
test command::tests::argument_kinds_are_sane ... ok
test command::tests::argument_names_are_unique_within_a_command ... ok
test command::tests::bool_accepts_true_and_false_only ... ok
test command::tests::choice_must_be_one_of_the_fixed_list ... ok
test command::tests::cli_and_agent_may_not_fill_files_paths_from_context ... ok
test command::tests::cli_and_agent_must_give_every_files_context_path ... ok
test command::tests::context_rules_are_pure_functions_over_every_caller ... ok
test command::tests::every_files_command_gets_exactly_one_dry_run_added_by_the_registry ... ok
test command::tests::every_caller_may_give_explicit_files_paths ... ok
test command::tests::missing_required_argument_is_named ... ok
test command::tests::int_accepts_the_range_and_rejects_bad_and_out_of_range_values ... ok
test command::tests::no_command_deletes_permanently ... ok
test command::tests::names_are_unique ... ok
test command::tests::no_tokens_is_a_missing_command ... ok
test command::tests::number_values_are_hundredths_and_print_with_two_places ... ok
test command::tests::names_are_area_dot_verb_in_lowercase_kebab_case ... ok
test command::tests::opacity_is_a_number_without_a_step ... ok
test command::tests::repeated_keys_are_kept_in_order ... ok
test command::tests::paths_print_once_per_key ... ok
test command::tests::repeated_paths_keys_make_a_list_in_input_order ... ok
test command::tests::path_must_be_absolute_and_stays_as_given ... ok
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
test config::tests::every_default_matches_the_brief ... ok
test config::tests::an_unparsable_file_is_never_written ... ok
test config::tests::an_unknown_key_is_a_warning_with_its_position ... ok
test config::tests::an_unparsable_file_reports_one_problem_with_a_position ... ok
test config::tests::a_bad_value_falls_back_and_the_good_values_still_load ... ok
test config::tests::default_pins_include_home_then_only_existing_special_folders ... ok
test config::tests::every_settings_command_has_a_change_for ... ok
test config::tests::a_hotkeys_table_replaces_listed_keys_and_an_empty_value_unbinds ... ok
test config::tests::set_scale_and_sort_map_to_the_expected_changes ... ok
test config::tests::a_save_rewrites_only_the_changed_key_on_a_commented_file ... ok
test config::tests::zoom_stops_at_the_scale_limits_and_toggles_flip ... ok
test config::tests::saving_creates_the_folder_and_the_changed_key ... ok
test config::tests::a_sidebar_and_section_save_keep_their_inline_formatting ... ok
test fileops::tests::a_command_outside_the_file_operations_is_not_a_request ... ok
test config::tests::a_hotkey_with_an_unknown_command_or_bad_arguments_keeps_the_default ... ok
test config::tests::sort_for_expands_home_and_normalizes_dot_dot_and_trailing_slashes ... ok
test fileops::tests::a_cancel_decision_runs_nothing ... ok
test fileops::tests::a_cancel_partway_still_runs_nothing ... ok
test config::tests::pin_changes_write_the_pins_list ... ok
test config::tests::unknown_keys_and_tables_are_warnings_and_stay_in_the_file ... ok
test config::tests::sort_changes_write_one_global_setting_or_one_folder_rule ... ok
test fileops::tests::apply_to_all_does_not_overwrite_an_earlier_answer ... ok
test fileops::tests::a_dry_run_changes_nothing_on_disk ... ok
test fileops::tests::a_new_folder_with_a_taken_or_invalid_name_is_a_conflict ... ok
test config::tests::pin_changes_without_a_pin_list_keep_the_defaults ... ok
test fileops::tests::a_folder_into_itself_is_refused ... ok
test fileops::tests::a_copy_with_a_conflict_lists_the_target ... ok
test fileops::tests::copy_names_add_copy_then_numbers ... ok
test fileops::tests::decisions_cancel_with_apply_to_all_marks_the_run_cancelled ... ok
test config::tests::saving_is_atomic_and_keeps_permissions ... ok
test fileops::tests::cancel_ends_the_questions_at_once ... ok
test config::tests::prints_a_sample_problem_list ... ok
test fileops::tests::every_files_command_maps_to_its_request ... ok
test fileops::tests::names_are_checked_against_the_rules ... ok
test fileops::tests::moving_into_the_folder_an_item_is_already_in_is_refused ... ok
test fileops::tests::missing_sources_are_refused_for_their_item ... ok
test config::tests::every_problem_kind_reports_its_line_and_column ... ok
test fileops::tests::the_first_answer_can_apply_to_all ... ok
test fileops::tests::request_mapping_refuses_unfilled_context_paths ... ok
test fileops::tests::renaming_to_an_invalid_or_taken_name_is_a_conflict ... ok
test fileops::tests::three_conflicts_are_answered_one_by_one ... ok
test fileops::tests::trashinfo_keeps_the_first_occurrence_of_a_key ... ok
test fileops::tests::trashinfo_percent_decodes_non_utf8_paths ... ok
test fileops::tests::trashinfo_percent_decodes_the_path ... ok
test fileops::tests::trashinfo_reads_both_date_forms ... ok
test fileops::tests::trashinfo_reports_a_missing_key_header_or_bad_value ... ok
test fileops::tests::unanswered_conflicts_refuse_the_whole_operation_and_name_the_items ... ok
test fileops::tests::undo_with_nothing_on_the_stack_is_an_error ... ok
test session::tests::a_missing_file_gives_one_window_at_home_without_a_problem ... ok
test session::tests::an_empty_workspace_is_never_saved ... ok
test session::tests::a_version_2_file_gives_one_window_at_home_and_a_problem ... ok
test workspace::split::tests::a_close_rebalances_two_or_more_apart ... ok
test tests::app_id_matches_the_niri_rule_and_is_a_valid_gapplication_id ... ok
test workspace::split::tests::a_swap_keeps_heights_while_an_insert_resets_the_changed_column ... ok
test session::tests::preserve_bad_replaces_an_older_copy ... ok
test workspace::split::tests::a_hand_made_three_plus_one_survives_a_close_from_the_long_column ... ok
test workspace::split::tests::dropping_on_a_tile_swaps_across_and_reorders_within ... ok
test workspace::split::tests::a_drop_never_empties_a_column ... ok
test workspace::split::tests::plain_click_clears_and_moves_the_anchor ... ok
test workspace::split::tests::a_slot_past_the_end_changes_nothing ... ok
test workspace::split::tests::closing_the_lone_tile_rebalances ... ok
test session::tests::session_path_is_a_pure_function_of_xdg_state_home_and_home ... ok
test workspace::split::tests::ctrl_click_toggles_without_moving_the_anchor ... ok
test session::tests::an_unparsable_file_gives_one_window_at_home_and_a_problem ... ok
test workspace::split::tests::initial_columns_fill_the_shorter_column ... ok
test workspace::split::tests::dropping_in_a_gap_inserts_and_the_column_gets_longer ... ok
test workspace::split::tests::shift_click_selects_the_range_from_the_anchor ... ok
test session::tests::a_missing_folder_falls_back_to_its_parent_then_to_home ... ok
test workspace::tests::a_failed_operation_changes_nothing ... ok
test workspace::tests::a_fresh_workspace_has_one_focused_tab_and_holds_the_invariants ... ok
test workspace::tests::a_drop_that_would_empty_a_column_is_refused_and_changes_nothing ... ok
test session::tests::a_non_utf8_folder_and_file_name_round_trip ... ok
test workspace::tests::a_hand_made_three_plus_one_survives_a_close_from_the_long_column ... ok
test workspace::tests::a_tile_drop_swaps_reorders_and_inserts ... ok
test workspace::tests::closing_a_window_focuses_another_and_the_last_says_so ... ok
test workspace::tests::closing_the_last_pill_closes_the_window ... ok
test workspace::tests::closing_a_tab_inside_a_group_focuses_the_next_tile_else_the_previous ... ok
test workspace::tests::file_clicks_follow_the_strip_rules ... ok
test workspace::tests::closing_two_tiles_from_a_three_plus_three_group_rebalances_to_two_plus_two ... ok
test workspace::tests::focus_after_a_close_goes_right_else_left ... ok
test workspace::tests::focus_window_moves_the_focus_and_refuses_an_unknown_id ... ok
test session::tests::a_rich_workspace_round_trips ... ok
test workspace::tests::group_add_tab_joins_a_single_pill_into_a_new_split ... ok
test workspace::tests::group_add_tab_uses_the_shorter_column_and_resets_that_columns_heights ... ok
test workspace::tests::group_rename_trims_and_rejects_an_empty_name ... ok
test workspace::tests::group_close_closes_every_tab_and_focuses_the_neighbour ... ok
test workspace::tests::group_split_inserts_at_the_first_selected_pill ... ok
test workspace::tests::group_split_uses_the_lowest_free_number ... ok
test workspace::tests::ids_are_stable_and_never_reused ... ok
test workspace::tests::group_split_merges_selected_groups_and_needs_two_tabs ... ok
test workspace::tests::nav_back_and_forward_move_between_folders_and_do_nothing_on_empty_stacks ... ok
test workspace::tests::hand_heights_need_one_entry_per_tile_and_the_width_share_is_permille ... ok
test workspace::tests::nav_go_pushes_history_and_clears_selection_and_anchor ... ok
test workspace::tests::nav_go_to_a_file_goes_to_its_folder_and_selects_the_name ... ok
test workspace::tests::nav_up_stops_at_the_root ... ok
test workspace::tests::history_stacks_are_capped_at_one_hundred_dropping_the_oldest ... ok
test workspace::tests::pill_clicks_follow_the_strip_rules ... ok
test workspace::tests::only_split_groups_take_group_numbers ... ok
test workspace::tests::restore_drops_an_empty_group_and_an_empty_window ... ok
test workspace::tests::restore_repairs_a_focused_tab_out_of_range ... ok
test workspace::tests::restore_repairs_a_width_share_over_1000 ... ok
test workspace::tests::restore_replaces_a_column_list_that_does_not_match_its_tabs ... ok
test workspace::tests::restore_repairs_hand_heights_of_the_wrong_length ... ok
test workspace::tests::selection_all_uses_the_handlers_names_and_none_clears_them ... ok
test workspace::tests::tab_close_selected_closes_the_selected_pills_and_focuses_right_else_left ... ok
test workspace::tests::restore_trims_history_from_the_oldest_end ... ok
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
test fileops::tests::cancelling_stops_after_the_current_item ... ok
test fileops::tests::a_failed_item_does_not_stop_the_others ... ok
test fileops::tests::skip_and_keep_both_are_applied_per_item ... ok
test fileops::tests::a_folder_tree_with_a_symlink_is_copied_faithfully ... ok
test fileops::tests::copying_into_its_own_folder_uses_copy_and_copy_2 ... ok
test fileops::tests::the_undo_stack_keeps_at_most_fifty_records ... ok
test fileops::tests::new_folder_and_rename_run_and_the_plan_lists_them ... ok
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
test replace_trashes_the_old_target_and_undo_restores_it ... ok
test trash_and_its_undo_use_the_scratch_trash ... ok
test every_decision_runs_together_in_one_mix ... ok

test result: ok. 6 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/main.rs (target/debug/deps/gimbal_files-d8e87882e1328c49)

running 99 tests
test events::tests::only_group_formation_and_dissolution_fade ... ok
test events::tests::a_window_event_opens_or_closes_the_one_window ... ok
test commands::tests::hotkey_mapping_keeps_the_command_and_its_arguments ... ok
test folder::sort::tests::names_are_case_insensitive_and_natural ... ok
test events::tests::the_other_events_touch_only_what_they_can_change ... ok
test events::tests::tab_group_and_focus_events_touch_the_header_and_content ... ok
test folder::sort::tests::folders_come_first_in_both_orders_and_a_name_breaks_ties ... ok
test folder::sort::tests::every_key_works_in_both_orders ... ok
test folder::sort::tests::only_the_sort_attribute_is_added ... ok
test folder::tests::dot_names_are_hidden ... ok
test folder::tests::shift_gets_the_visible_order_exactly ... ok
test folder::tests::status_counts_and_selection_read_naturally ... ok
test folder::view::tests::the_column_cap_follows_the_viewport_and_never_is_g_maxuint ... ok
test layout::tests::a_retarget_uses_the_new_end_width ... ok
test header::tests::a_single_tab_shows_its_folder_name ... ok
test header::tests::a_root_folder_cannot_go_up ... ok
test layout::tests::collapsed_sidebars_are_strips ... ok
test layout::tests::a_reverse_restarts_from_the_current_width ... ok
test header::tests::a_split_group_shows_its_title_and_count_then_its_name ... ok
test layout::tests::defaults_match_the_sketch ... ok
test header::tests::the_view_marks_the_focused_pill_active_and_keeps_selection ... ok
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
test look::css::tests::nov_1_outlines_and_contrast ... ok
test look::css::tests::scaling ... ok
test look::css::tests::opacity_follows_the_config_range ... ok
test look::css::tests::header_fits_stretches_then_collapses ... ok
test look::icons::tests::palettes_stay_on_theme_colours ... ok
test look::icons::tests::all_six_mockup_icons_are_embedded ... ok
test look::motion::tests::durations_scale_by_the_slowdown ... ok
test look::tokens::tests::scaled_tokens_keep_a_visible_pixel ... ok
test look::css::tests::pinned_list_has_no_fill_and_collapsed_pills_sit_six_apart ... ok
test look::tokens::tests::tokens_lost_nothing_in_the_merge ... ok
test open::tests::a_folder_navigates_and_a_file_launches ... ok
test look::css::tests::icon_palettes_map_the_spec_table ... ok
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
test split::tests::a_dragged_tile_keeps_its_pill_at_its_own_top ... ok
test split::tests::a_drag_respects_the_grab_offset ... ok
test split::tests::a_drop_never_empties_a_column ... ok
test split::tests::a_hand_drag_reaches_the_minimum_below_the_half_floor ... ok
test look::css::tests::spec_rules ... ok
test split::tests::a_hand_made_layout_survives_a_close_that_leaves_one_gap ... ok
test sidebar::tests::every_pin_reorder_moves_exactly_one_row ... ok
test split::tests::a_pills_wrap_sits_with_its_tile_when_not_stuck ... ok
test look::css::tests::spec_rules_and_css_shape ... ok
test split::tests::a_pinned_column_never_moves_its_pills ... ok
test split::tests::a_reset_returns_to_the_equal_default ... ok
test split::tests::a_slot_top_above_the_view_sticks_the_pill_and_the_frame_top ... ok
test split::tests::a_short_column_is_pinned_and_a_tall_one_scrolls ... ok
test split::tests::a_stuck_pills_wrap_holds_the_view_top_with_its_frame ... ok
test split::tests::below_the_minimum_the_whole_tile_scrolls_off ... ok
test split::tests::a_visible_slot_leaves_its_pill_at_the_slot_top ... ok
test split::tests::closing_from_either_column_leaves_the_other_taller ... ok
test split::tests::closing_the_lone_tile_rebalances ... ok
test split::tests::dragging_down_grows_the_tile_above_and_moves_the_rest_down ... ok
test split::tests::a_stuck_tiles_strip_follows_its_frame_top ... ok
test split::tests::drag_follows_the_pointer_and_previews_the_drop ... ok
test split::tests::dragging_up_shrinks_to_the_minimum_and_moves_the_rest_up ... ok
test split::tests::dropping_in_a_gap_inserts_and_the_column_gets_longer ... ok
test split::tests::dropping_on_a_tile_swaps_across_and_reorders_within ... ok
test split::tests::hand_heights_keep_their_design_px_through_resizes_and_scale_changes ... ok
test split::tests::border_hit_test_covers_the_whole_strip_except_the_pill ... ok
test split::tests::hit_test ... ok
test split::tests::pill_hit_test ... ok
test split::tests::the_column_border_still_trades_width ... ok
test split::tests::the_drop_line_spans_the_gap ... ok
test split::tests::the_drop_zone_is_the_gap_plus_a_fifth_on_each_side ... ok
test split::tests::the_frame_shrinks_to_one_grid_row_at_the_minimum ... ok
test split::tests::the_last_tile_fills_a_column_shorter_than_the_view ... ok
test split::tests::the_model_layout_converts_to_tile_rects_at_scale ... ok
test split::tests::the_state_keeps_the_model_permille ... ok
test split::tests::tiles_fill_the_height_and_split_the_width ... ok
test split::tests::uneven_columns_make_their_tiles_taller ... ok
test split::tests::which_tile_a_click_focuses ... ok
test split::tests::rects_never_overlap ... ok

test result: ok. 99 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests fm_agent

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests fm_core

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

==> cargo build --workspace --release
   Compiling fm-ui v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-ui)
    Finished `release` profile [optimized] target(s) in 1.47s
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

### 10-second runs

Both runs used scratch `XDG_CONFIG_HOME` and `XDG_STATE_HOME` under
`/tmp/gimbal-p2j-final*`, `GIMBAL_LOG=1`, and a written session file with one
2 + 2 group over four scratch folders, left hand heights `[500, 300]` and
`width_permille = 620`. They were repeated after the last UI change. The app
was stopped by PID with SIGTERM; `pgrep -a gimbal-files` was empty before each
run and after it. No GTK criticals or warnings appeared in either run.

At scale 1.0 the final rects are 500 and 716 tall left (the hand 300 is the
last tile's fill), and 608 + 608 right, 97 px wide each in this tiled window's
298 px content column. The pills are the top 50 px of each slot. At scale 0.75
the same design heights become 375 and 857 (fill), and the right tiles 616
each.

#### Scale 1.0

```text
[gimbal 0.083] config: reading /tmp/gimbal-p2j-final1/config/gimbal-files/config.toml
[gimbal 0.083] config: scale 1.00 (base 1.00), opacity 0.85, font "", left collapsed false width 348, right collapsed false width 648
[gimbal 0.083] theme: reloaded /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.083] theme: watching /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.083] theme: watching /home/kyle/.config/gtk-4.0/noctalia-extensions.css
[gimbal 0.084] session: restored 1 window(s)
[gimbal 0.091] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal 0.091] blur: capabilities Capability(Blur) (blur true)
[gimbal 0.091] hotkey: bound "<Alt>Left" to nav.back
[gimbal 0.091] hotkey: bound "<Alt>Right" to nav.forward
[gimbal 0.091] hotkey: bound "<Alt>Up" to nav.up
[gimbal 0.091] hotkey: bound "<Control>a" to selection.all
[gimbal 0.091] hotkey: bound "<Control>equal" to view.zoom-in
[gimbal 0.091] hotkey: bound "<Control>f" to search.open
[gimbal 0.091] hotkey: bound "<Control>l" to nav.focus-path
[gimbal 0.091] hotkey: bound "<Control>minus" to view.zoom-out
[gimbal 0.091] hotkey: bound "<Control>n" to window.new
[gimbal 0.091] hotkey: bound "<Control>q" to app.quit
[gimbal 0.091] hotkey: bound "<Control>t" to tab.new
[gimbal 0.091] hotkey: bound "<Control>w" to tab.close
[gimbal 0.091] hotkey: bound "<Control>z" to file.undo
[gimbal 0.091] hotkey: bound "Delete" to file.trash
[gimbal 0.091] hotkey: bound "F5" to nav.refresh
[gimbal 0.091] header: window 1 pills ["Group 1 · 4 tabs"] (active 0, overflow false)
[gimbal 0.091] split: layout columns [[], []] heights [Default, Default] share 500 at scale 1.00; rects []; pills []; actual []; pill wraps []
[gimbal 0.091] layout: scale 1.00: left 348, content 0, preview 648 (inset 8)
[gimbal 0.093] folder: listing /tmp/gimbal-p2j-final1/a (sort name ascending, folders first true)
[gimbal 0.093] folder: listing /tmp/gimbal-p2j-final1/c (sort name ascending, folders first true)
[gimbal 0.093] folder: listing /tmp/gimbal-p2j-final1/b (sort name ascending, folders first true)
[gimbal 0.093] folder: listing /tmp/gimbal-p2j-final1/d (sort name ascending, folders first true)
[gimbal 0.093] split: layout columns [[TabId(2), TabId(4)], [TabId(3), TabId(5)]] heights [Hand([500, 300]), Default] share 620 at scale 1.00; rects [Rect { x: 40, y: 0, w: 680, h: 500 }, Rect { x: 40, y: 524, w: 680, h: 716 }, Rect { x: 744, y: 0, w: 416, h: 608 }, Rect { x: 744, y: 632, w: 416, h: 608 }]; pills [(TabId(2), Rect { x: 245, y: 0, w: 270, h: 50 }), (TabId(4), Rect { x: 245, y: 524, w: 270, h: 50 }), (TabId(3), Rect { x: 817, y: 0, w: 270, h: 50 }), (TabId(5), Rect { x: 817, y: 632, w: 270, h: 50 })]; actual [(TabId(2), (40.0, 0.0), (680, 500)), (TabId(4), (40.0, 524.0), (680, 716)), (TabId(3), (744.0, 0.0), (416, 608)), (TabId(5), (744.0, 632.0), (416, 608))]; pill wraps [(TabId(2), (0.0, 0.0), (0, 0)), (TabId(4), (0.0, 0.0), (0, 0)), (TabId(3), (0.0, 0.0), (0, 0)), (TabId(5), (0.0, 0.0), (0, 0))]
[gimbal 0.093] window: built model window 1 (0×0)
[gimbal 0.864] window: opened model window 1
[gimbal 0.864] fm_ui: activated; the session is restored
[gimbal 0.865] folder: listing complete: 0 items (0 visible) in 772 ms
[gimbal 0.865] folder: listing complete: 0 items (0 visible) in 772 ms
[gimbal 0.865] folder: listing complete: 0 items (0 visible) in 772 ms
[gimbal 0.865] folder: listing complete: 0 items (0 visible) in 771 ms
[gimbal 0.875] header: window 1 pills ["Group 1 · 4 tabs"] (active 0, overflow true)
[gimbal 0.876] split: layout columns [[TabId(2), TabId(4)], [TabId(3), TabId(5)]] heights [Hand([500, 300]), Default] share 620 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 500 }, Rect { x: 40, y: 524, w: 97, h: 716 }, Rect { x: 161, y: 0, w: 97, h: 608 }, Rect { x: 161, y: 632, w: 97, h: 608 }]; pills [(TabId(2), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 524, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(5), Rect { x: 161, y: 632, w: 97, h: 50 })]; actual [(TabId(2), (40.0, 0.0), (97, 500)), (TabId(4), (40.0, 524.0), (97, 716)), (TabId(3), (161.0, 0.0), (97, 608)), (TabId(5), (161.0, 632.0), (97, 608))]; pill wraps [(TabId(2), (40.0, 0.0), (680, 58)), (TabId(4), (40.0, 524.0), (680, 58)), (TabId(3), (744.0, 0.0), (416, 58)), (TabId(5), (744.0, 632.0), (416, 58))]
[gimbal 0.876] split: layout columns [[TabId(2), TabId(4)], [TabId(3), TabId(5)]] heights [Hand([500, 300]), Default] share 620 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 500 }, Rect { x: 40, y: 524, w: 97, h: 737 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 619 }]; pills [(TabId(2), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 524, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(5), Rect { x: 161, y: 642, w: 97, h: 50 })]; actual [(TabId(2), (40.0, 0.0), (97, 500)), (TabId(4), (40.0, 524.0), (97, 737)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(5), (161.0, 642.0), (97, 619))]; pill wraps [(TabId(2), (40.0, 0.0), (680, 58)), (TabId(4), (40.0, 524.0), (680, 58)), (TabId(3), (744.0, 0.0), (416, 58)), (TabId(5), (744.0, 632.0), (416, 58))]
[gimbal 0.876] blur: surface layout changed to 1170×1512
[gimbal 0.876] blur: opacity 0.85: visible 1120×1462 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1120, h: 1438 }, Rect { x: 37, y: 25, w: 1096, h: 3 }, Rect { x: 37, y: 1484, w: 1096, h: 3 }, Rect { x: 30, y: 28, w: 1110, h: 3 }, Rect { x: 30, y: 1481, w: 1110, h: 3 }, Rect { x: 27, y: 31, w: 1116, h: 3 }, Rect { x: 27, y: 1478, w: 1116, h: 3 }, Rect { x: 26, y: 34, w: 1118, h: 3 }, Rect { x: 26, y: 1475, w: 1118, h: 3 }]
[gimbal 0.880] folder: viewport 0 px at scale 1.00 -> cap 2
[gimbal 0.880] folder: viewport 0 px at scale 1.00 -> cap 2
[gimbal 0.880] folder: viewport 0 px at scale 1.00 -> cap 2
[gimbal 0.880] folder: viewport 0 px at scale 1.00 -> cap 2
[gimbal 0.880] folder: viewport 0 px at scale 1.00 -> cap 2
[gimbal 1.864] session: saved /tmp/gimbal-p2j-final1/state/gimbal-files/session.toml

(gimbal-files:719100): Gdk-WARNING **: 11:20:49.916: vkAcquireNextImageKHR(): A swapchain no longer matches the surface properties exactly, but can still be used to present to the surface successfully. (VK_SUBOPTIMAL_KHR) (1000001003)
```

#### Scale 0.75

```text
[gimbal 0.080] config: reading /tmp/gimbal-p2j-final2/config/gimbal-files/config.toml
[gimbal 0.080] config: scale 0.75 (base 1.00), opacity 0.85, font "", left collapsed false width 348, right collapsed false width 648
[gimbal 0.080] theme: reloaded /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia-extensions.css
[gimbal 0.081] session: restored 1 window(s)
[gimbal 0.087] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal 0.087] blur: capabilities Capability(Blur) (blur true)
[gimbal 0.087] hotkey: bound "<Alt>Left" to nav.back
[gimbal 0.087] hotkey: bound "<Alt>Right" to nav.forward
[gimbal 0.087] hotkey: bound "<Alt>Up" to nav.up
[gimbal 0.087] hotkey: bound "<Control>a" to selection.all
[gimbal 0.087] hotkey: bound "<Control>equal" to view.zoom-in
[gimbal 0.087] hotkey: bound "<Control>f" to search.open
[gimbal 0.087] hotkey: bound "<Control>l" to nav.focus-path
[gimbal 0.087] hotkey: bound "<Control>minus" to view.zoom-out
[gimbal 0.087] hotkey: bound "<Control>n" to window.new
[gimbal 0.087] hotkey: bound "<Control>q" to app.quit
[gimbal 0.087] hotkey: bound "<Control>t" to tab.new
[gimbal 0.087] hotkey: bound "<Control>w" to tab.close
[gimbal 0.087] hotkey: bound "<Control>z" to file.undo
[gimbal 0.087] hotkey: bound "Delete" to file.trash
[gimbal 0.087] hotkey: bound "F5" to nav.refresh
[gimbal 0.087] header: window 1 pills ["Group 1 · 4 tabs"] (active 0, overflow false)
[gimbal 0.087] split: layout columns [[], []] heights [Default, Default] share 500 at scale 0.75; rects []; pills []; actual []; pill wraps []
[gimbal 0.087] layout: scale 0.75: left 261, content 0, preview 486 (inset 6)
[gimbal 0.088] folder: listing /tmp/gimbal-p2j-final2/a (sort name ascending, folders first true)
[gimbal 0.088] folder: listing /tmp/gimbal-p2j-final2/c (sort name ascending, folders first true)
[gimbal 0.089] folder: listing /tmp/gimbal-p2j-final2/b (sort name ascending, folders first true)
[gimbal 0.089] folder: listing /tmp/gimbal-p2j-final2/d (sort name ascending, folders first true)
[gimbal 0.089] split: layout columns [[TabId(2), TabId(4)], [TabId(3), TabId(5)]] heights [Hand([500, 300]), Default] share 620 at scale 0.75; rects [Rect { x: 30, y: 0, w: 696, h: 375 }, Rect { x: 30, y: 393, w: 696, h: 857 }, Rect { x: 744, y: 0, w: 426, h: 616 }, Rect { x: 744, y: 634, w: 426, h: 616 }]; pills [(TabId(2), Rect { x: 276, y: 0, w: 203, h: 38 }), (TabId(4), Rect { x: 276, y: 393, w: 203, h: 38 }), (TabId(3), Rect { x: 855, y: 0, w: 203, h: 38 }), (TabId(5), Rect { x: 855, y: 634, w: 203, h: 38 })]; actual [(TabId(2), (30.0, 0.0), (696, 375)), (TabId(4), (30.0, 393.0), (696, 857)), (TabId(3), (744.0, 0.0), (426, 616)), (TabId(5), (744.0, 634.0), (426, 616))]; pill wraps [(TabId(2), (0.0, 0.0), (0, 0)), (TabId(4), (0.0, 0.0), (0, 0)), (TabId(3), (0.0, 0.0), (0, 0)), (TabId(5), (0.0, 0.0), (0, 0))]
[gimbal 0.089] window: built model window 1 (0×0)
[gimbal 0.135] window: opened model window 1
[gimbal 0.135] fm_ui: activated; the session is restored
[gimbal 0.135] folder: listing complete: 0 items (0 visible) in 46 ms
[gimbal 0.135] folder: listing complete: 0 items (0 visible) in 46 ms
[gimbal 0.135] folder: listing complete: 0 items (0 visible) in 46 ms
[gimbal 0.135] folder: listing complete: 0 items (0 visible) in 46 ms
[gimbal 0.145] split: layout columns [[TabId(2), TabId(4)], [TabId(3), TabId(5)]] heights [Hand([500, 300]), Default] share 620 at scale 0.75; rects [Rect { x: 30, y: 0, w: 174, h: 375 }, Rect { x: 30, y: 393, w: 174, h: 857 }, Rect { x: 222, y: 0, w: 107, h: 616 }, Rect { x: 222, y: 634, w: 107, h: 616 }]; pills [(TabId(2), Rect { x: 56, y: 0, w: 121, h: 38 }), (TabId(4), Rect { x: 56, y: 393, w: 121, h: 38 }), (TabId(3), Rect { x: 222, y: 0, w: 107, h: 38 }), (TabId(5), Rect { x: 222, y: 634, w: 107, h: 38 })]; actual [(TabId(2), (30.0, 0.0), (174, 375)), (TabId(4), (30.0, 393.0), (174, 857)), (TabId(3), (222.0, 0.0), (107, 616)), (TabId(5), (222.0, 634.0), (107, 616))]; pill wraps [(TabId(2), (30.0, 0.0), (696, 44)), (TabId(4), (30.0, 393.0), (696, 44)), (TabId(3), (744.0, 0.0), (426, 44)), (TabId(5), (744.0, 634.0), (426, 44))]
[gimbal 0.145] split: layout columns [[TabId(2), TabId(4)], [TabId(3), TabId(5)]] heights [Hand([500, 300]), Default] share 620 at scale 0.75; rects [Rect { x: 30, y: 0, w: 174, h: 375 }, Rect { x: 30, y: 393, w: 174, h: 918 }, Rect { x: 222, y: 0, w: 107, h: 646 }, Rect { x: 222, y: 664, w: 107, h: 647 }]; pills [(TabId(2), Rect { x: 56, y: 0, w: 121, h: 38 }), (TabId(4), Rect { x: 56, y: 393, w: 121, h: 38 }), (TabId(3), Rect { x: 222, y: 0, w: 107, h: 38 }), (TabId(5), Rect { x: 222, y: 664, w: 107, h: 38 })]; actual [(TabId(2), (30.0, 0.0), (174, 375)), (TabId(4), (30.0, 393.0), (174, 918)), (TabId(3), (222.0, 0.0), (107, 646)), (TabId(5), (222.0, 664.0), (107, 647))]; pill wraps [(TabId(2), (30.0, 0.0), (696, 44)), (TabId(4), (30.0, 393.0), (696, 44)), (TabId(3), (744.0, 0.0), (426, 44)), (TabId(5), (744.0, 634.0), (426, 44))]
[gimbal 0.145] blur: surface layout changed to 1170×1512
[gimbal 0.145] blur: opacity 0.85: visible 1120×1462 at (25, 25), radius 9, 9 rects: [Rect { x: 25, y: 34, w: 1120, h: 1444 }, Rect { x: 34, y: 25, w: 1102, h: 2 }, Rect { x: 34, y: 1485, w: 1102, h: 2 }, Rect { x: 29, y: 27, w: 1112, h: 2 }, Rect { x: 29, y: 1483, w: 1112, h: 2 }, Rect { x: 27, y: 29, w: 1116, h: 2 }, Rect { x: 27, y: 1481, w: 1116, h: 2 }, Rect { x: 26, y: 31, w: 1118, h: 3 }, Rect { x: 26, y: 1478, w: 1118, h: 3 }]
[gimbal 0.149] folder: viewport 0 px at scale 0.75 -> cap 2
[gimbal 0.149] folder: viewport 62 px at scale 0.75 -> cap 2
[gimbal 0.149] folder: viewport 62 px at scale 0.75 -> cap 2
[gimbal 0.149] folder: viewport 0 px at scale 0.75 -> cap 2
[gimbal 0.149] folder: viewport 0 px at scale 0.75 -> cap 2
[gimbal 1.135] session: saved /tmp/gimbal-p2j-final2/state/gimbal-files/session.toml

(gimbal-files:719222): Gdk-WARNING **: 11:21:02.148: vkAcquireNextImageKHR(): A swapchain no longer matches the surface properties exactly, but can still be used to present to the surface successfully. (VK_SUBOPTIMAL_KHR) (1000001003)
```

### Temporary timer log (harness removed before delivery)

A temporary `GIMBAL_P2J_TIMER=1` harness drove a 3 + 3 group through
`focus_tab` on the bottom-right tile, `apply_tile_drop` of the top-left tile
onto the bottom-right, two `tab_close` calls from the right column (the
rebalance), `set_column_heights(0, Hand([600, 300]))`, and
`set_width_share(700)`. It read the split page's model and drawn geometry at
the start, 150 ms in, and 400 ms after each operation. The harness, its
`p2j` methods and its `log_split_state` plumbing were then removed; a grep for
`P2J_TIMER`, `p2j_` and `temp: step` in `crates/fm-ui/src` finds nothing
`[observed]`.

The lines below are the `temp: step ...` marker and the one `split: layout`
line that follows it (the other event noise is omitted). `rects` and `pills`
are the model's; `actual` and `pill wraps` are the drawn Fixed positions and
sizes. The mid frames show the tile x/y/size between its old and new rect, and
each pill wrap exactly one rounded pixel from its own tile. In this narrow
window the width share 700 is clamped to the 97 px column minimum, so the
final rects stay 97 wide while the model keeps `share 700`.

```text
[gimbal 2.067] temp: step 0 start
[gimbal 2.067] split: layout columns [[TabId(2), TabId(4), TabId(6)], [TabId(3), TabId(5), TabId(7)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 618 }, Rect { x: 40, y: 1284, w: 97, h: 618 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 618 }, Rect { x: 161, y: 1284, w: 97, h: 618 }]; pills [(TabId(2), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(6), Rect { x: 40, y: 1284, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(5), Rect { x: 161, y: 642, w: 97, h: 50 }), (TabId(7), Rect { x: 161, y: 1284, w: 97, h: 50 })]; actual [(TabId(2), (40.0, 0.0), (97, 618)), (TabId(4), (40.0, 642.0), (97, 618)), (TabId(6), (40.0, 1284.0), (97, 618)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(5), (161.0, 642.0), (97, 618)), (TabId(7), (161.0, 1284.0), (97, 618))]; pill wraps [(TabId(2), (40.0, 0.0), (97, 58)), (TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (40.0, 1284.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(5), (161.0, 642.0), (97, 58)), (TabId(7), (161.0, 1284.0), (97, 58))]
[gimbal 2.218] temp: step 0 mid
[gimbal 2.218] split: layout columns [[TabId(2), TabId(4), TabId(6)], [TabId(3), TabId(5), TabId(7)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 618 }, Rect { x: 40, y: 1284, w: 97, h: 618 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 618 }, Rect { x: 161, y: 1284, w: 97, h: 618 }]; pills [(TabId(2), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(6), Rect { x: 40, y: 1284, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(5), Rect { x: 161, y: 642, w: 97, h: 50 }), (TabId(7), Rect { x: 161, y: 1284, w: 97, h: 50 })]; actual [(TabId(2), (40.0, 0.0), (97, 618)), (TabId(4), (40.0, 642.0), (97, 618)), (TabId(6), (40.0, 1284.0), (97, 618)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(5), (161.0, 642.0), (97, 618)), (TabId(7), (161.0, 1284.0), (97, 618))]; pill wraps [(TabId(2), (40.0, 0.0), (97, 58)), (TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (40.0, 1284.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(5), (161.0, 642.0), (97, 58)), (TabId(7), (161.0, 1284.0), (97, 58))]
[gimbal 2.468] temp: step 0 end
[gimbal 2.468] split: layout columns [[TabId(2), TabId(4), TabId(6)], [TabId(3), TabId(5), TabId(7)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 618 }, Rect { x: 40, y: 1284, w: 97, h: 618 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 618 }, Rect { x: 161, y: 1284, w: 97, h: 618 }]; pills [(TabId(2), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(6), Rect { x: 40, y: 1284, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(5), Rect { x: 161, y: 642, w: 97, h: 50 }), (TabId(7), Rect { x: 161, y: 1284, w: 97, h: 50 })]; actual [(TabId(2), (40.0, 0.0), (97, 618)), (TabId(4), (40.0, 642.0), (97, 618)), (TabId(6), (40.0, 1284.0), (97, 618)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(5), (161.0, 642.0), (97, 618)), (TabId(7), (161.0, 1284.0), (97, 618))]; pill wraps [(TabId(2), (40.0, 0.0), (97, 58)), (TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (40.0, 1284.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(5), (161.0, 642.0), (97, 58)), (TabId(7), (161.0, 1284.0), (97, 58))]
[gimbal 2.668] temp: step 1 start
[gimbal 2.668] split: layout columns [[TabId(2), TabId(4), TabId(6)], [TabId(3), TabId(5), TabId(7)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 618 }, Rect { x: 40, y: 1284, w: 97, h: 618 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 618 }, Rect { x: 161, y: 1284, w: 97, h: 618 }]; pills [(TabId(2), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(6), Rect { x: 40, y: 1284, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(5), Rect { x: 161, y: 642, w: 97, h: 50 }), (TabId(7), Rect { x: 161, y: 1284, w: 97, h: 50 })]; actual [(TabId(2), (40.0, 0.0), (97, 618)), (TabId(4), (40.0, 642.0), (97, 618)), (TabId(6), (40.0, 1284.0), (97, 618)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(5), (161.0, 642.0), (97, 618)), (TabId(7), (161.0, 1284.0), (97, 618))]; pill wraps [(TabId(2), (40.0, 0.0), (97, 58)), (TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (40.0, 1284.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(5), (161.0, 642.0), (97, 58)), (TabId(7), (161.0, 1284.0), (97, 58))]
[gimbal 2.819] temp: step 1 mid
[gimbal 2.819] split: layout columns [[TabId(7), TabId(4), TabId(6)], [TabId(3), TabId(5), TabId(2)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 618 }, Rect { x: 40, y: 1284, w: 97, h: 618 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 618 }, Rect { x: 161, y: 1284, w: 97, h: 618 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(6), Rect { x: 40, y: 1284, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(5), Rect { x: 161, y: 642, w: 97, h: 50 }), (TabId(2), Rect { x: 161, y: 1284, w: 97, h: 50 })]; actual [(TabId(7), (54.824512185185185, 157.31135244444454), (97, 618)), (TabId(4), (40.0, 642.0), (97, 618)), (TabId(6), (40.0, 1284.0), (97, 618)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(5), (161.0, 642.0), (97, 618)), (TabId(2), (146.1754878148148, 1126.6886475555555), (97, 618))]; pill wraps [(TabId(2), (146.0, 1127.0), (97, 58)), (TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (40.0, 1284.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(5), (161.0, 642.0), (97, 58)), (TabId(7), (55.0, 157.0), (97, 58))]
[gimbal 3.069] temp: step 1 end
[gimbal 3.069] split: layout columns [[TabId(7), TabId(4), TabId(6)], [TabId(3), TabId(5), TabId(2)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 618 }, Rect { x: 40, y: 1284, w: 97, h: 618 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 618 }, Rect { x: 161, y: 1284, w: 97, h: 618 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(6), Rect { x: 40, y: 1284, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(5), Rect { x: 161, y: 642, w: 97, h: 50 }), (TabId(2), Rect { x: 161, y: 1284, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 618)), (TabId(4), (40.0, 642.0), (97, 618)), (TabId(6), (40.0, 1284.0), (97, 618)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(5), (161.0, 642.0), (97, 618)), (TabId(2), (161.0, 1284.0), (97, 618))]; pill wraps [(TabId(2), (161.0, 1284.0), (97, 58)), (TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (40.0, 1284.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(5), (161.0, 642.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
[gimbal 3.269] temp: step 2 start
[gimbal 3.269] split: layout columns [[TabId(7), TabId(4), TabId(6)], [TabId(3), TabId(5), TabId(2)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 618 }, Rect { x: 40, y: 1284, w: 97, h: 618 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 618 }, Rect { x: 161, y: 1284, w: 97, h: 618 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(6), Rect { x: 40, y: 1284, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(5), Rect { x: 161, y: 642, w: 97, h: 50 }), (TabId(2), Rect { x: 161, y: 1284, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 618)), (TabId(4), (40.0, 642.0), (97, 618)), (TabId(6), (40.0, 1284.0), (97, 618)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(5), (161.0, 642.0), (97, 618)), (TabId(2), (161.0, 1284.0), (97, 618))]; pill wraps [(TabId(2), (161.0, 1284.0), (97, 58)), (TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (40.0, 1284.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(5), (161.0, 642.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
[gimbal 3.421] temp: step 2 mid
[gimbal 3.421] split: layout columns [[TabId(7), TabId(4), TabId(6)], [TabId(3), TabId(5)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 618 }, Rect { x: 40, y: 1284, w: 97, h: 618 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 619 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(6), Rect { x: 40, y: 1284, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(5), Rect { x: 161, y: 642, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 618)), (TabId(4), (40.0, 642.0), (97, 618)), (TabId(6), (40.0, 1284.0), (97, 618)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(5), (161.0, 642.0), (97, 619))]; pill wraps [(TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (40.0, 1284.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(5), (161.0, 642.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
[gimbal 3.671] temp: step 2 end
[gimbal 3.671] split: layout columns [[TabId(7), TabId(4), TabId(6)], [TabId(3), TabId(5)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 618 }, Rect { x: 40, y: 1284, w: 97, h: 618 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 619 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(6), Rect { x: 40, y: 1284, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(5), Rect { x: 161, y: 642, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 618)), (TabId(4), (40.0, 642.0), (97, 618)), (TabId(6), (40.0, 1284.0), (97, 618)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(5), (161.0, 642.0), (97, 619))]; pill wraps [(TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (40.0, 1284.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(5), (161.0, 642.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
[gimbal 3.871] temp: step 3 start
[gimbal 3.871] split: layout columns [[TabId(7), TabId(4), TabId(6)], [TabId(3), TabId(5)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 618 }, Rect { x: 40, y: 1284, w: 97, h: 618 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 619 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(6), Rect { x: 40, y: 1284, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(5), Rect { x: 161, y: 642, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 618)), (TabId(4), (40.0, 642.0), (97, 618)), (TabId(6), (40.0, 1284.0), (97, 618)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(5), (161.0, 642.0), (97, 619))]; pill wraps [(TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (40.0, 1284.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(5), (161.0, 642.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
[gimbal 4.022] temp: step 3 mid
[gimbal 4.022] split: layout columns [[TabId(7), TabId(4)], [TabId(3), TabId(6)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 619 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 619 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(6), Rect { x: 161, y: 642, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 618)), (TabId(4), (40.0, 642.0), (97, 619)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(6), (150.69273600000002, 696.688128), (97, 619))]; pill wraps [(TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (151.0, 697.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
[gimbal 4.272] temp: step 3 end
[gimbal 4.272] split: layout columns [[TabId(7), TabId(4)], [TabId(3), TabId(6)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 619 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 619 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(6), Rect { x: 161, y: 642, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 618)), (TabId(4), (40.0, 642.0), (97, 619)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(6), (161.0, 642.0), (97, 619))]; pill wraps [(TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (161.0, 642.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
[gimbal 4.472] temp: step 4 start
[gimbal 4.472] split: layout columns [[TabId(7), TabId(4)], [TabId(3), TabId(6)]] heights [Default, Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 618 }, Rect { x: 40, y: 642, w: 97, h: 619 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 619 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 642, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(6), Rect { x: 161, y: 642, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 618)), (TabId(4), (40.0, 642.0), (97, 619)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(6), (161.0, 642.0), (97, 619))]; pill wraps [(TabId(4), (40.0, 642.0), (97, 58)), (TabId(6), (161.0, 642.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
[gimbal 4.624] temp: step 4 mid
[gimbal 4.624] split: layout columns [[TabId(7), TabId(4)], [TabId(3), TabId(6)]] heights [Hand([600, 300]), Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 600 }, Rect { x: 40, y: 624, w: 97, h: 637 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 619 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 624, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(6), Rect { x: 161, y: 642, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 602)), (TabId(4), (40.0, 625.5684246666667), (97, 635)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(6), (161.0, 642.0), (97, 619))]; pill wraps [(TabId(4), (40.0, 626.0), (97, 58)), (TabId(6), (161.0, 642.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
[gimbal 4.874] temp: step 4 end
[gimbal 4.874] split: layout columns [[TabId(7), TabId(4)], [TabId(3), TabId(6)]] heights [Hand([600, 300]), Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 600 }, Rect { x: 40, y: 624, w: 97, h: 637 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 619 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 624, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(6), Rect { x: 161, y: 642, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 600)), (TabId(4), (40.0, 624.0), (97, 637)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(6), (161.0, 642.0), (97, 619))]; pill wraps [(TabId(4), (40.0, 624.0), (97, 58)), (TabId(6), (161.0, 642.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
[gimbal 5.074] temp: step 5 start
[gimbal 5.074] split: layout columns [[TabId(7), TabId(4)], [TabId(3), TabId(6)]] heights [Hand([600, 300]), Default] share 500 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 600 }, Rect { x: 40, y: 624, w: 97, h: 637 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 619 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 624, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(6), Rect { x: 161, y: 642, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 600)), (TabId(4), (40.0, 624.0), (97, 637)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(6), (161.0, 642.0), (97, 619))]; pill wraps [(TabId(4), (40.0, 624.0), (97, 58)), (TabId(6), (161.0, 642.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
[gimbal 5.225] temp: step 5 mid
[gimbal 5.225] split: layout columns [[TabId(7), TabId(4)], [TabId(3), TabId(6)]] heights [Hand([600, 300]), Default] share 700 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 600 }, Rect { x: 40, y: 624, w: 97, h: 637 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 619 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 624, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(6), Rect { x: 161, y: 642, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 600)), (TabId(4), (40.0, 624.0), (97, 637)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(6), (161.0, 642.0), (97, 619))]; pill wraps [(TabId(4), (40.0, 624.0), (97, 58)), (TabId(6), (161.0, 642.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
[gimbal 5.475] temp: step 5 end
[gimbal 5.475] split: layout columns [[TabId(7), TabId(4)], [TabId(3), TabId(6)]] heights [Hand([600, 300]), Default] share 700 at scale 1.00; rects [Rect { x: 40, y: 0, w: 97, h: 600 }, Rect { x: 40, y: 624, w: 97, h: 637 }, Rect { x: 161, y: 0, w: 97, h: 618 }, Rect { x: 161, y: 642, w: 97, h: 619 }]; pills [(TabId(7), Rect { x: 40, y: 0, w: 97, h: 50 }), (TabId(4), Rect { x: 40, y: 624, w: 97, h: 50 }), (TabId(3), Rect { x: 161, y: 0, w: 97, h: 50 }), (TabId(6), Rect { x: 161, y: 642, w: 97, h: 50 })]; actual [(TabId(7), (40.0, 0.0), (97, 600)), (TabId(4), (40.0, 624.0), (97, 637)), (TabId(3), (161.0, 0.0), (97, 618)), (TabId(6), (161.0, 642.0), (97, 619))]; pill wraps [(TabId(4), (40.0, 624.0), (97, 58)), (TabId(6), (161.0, 642.0), (97, 58)), (TabId(3), (161.0, 0.0), (97, 58)), (TabId(7), (40.0, 0.0), (97, 58))]
```

The only warning in that timer run was the allowed intermittent
`Gdk-WARNING ... VK_SUBOPTIMAL_KHR ... (1000001003)` notice. `[observed]`

## API citations

- `GtkFixed::child_position`, used by the layout log:
  `gtk4-0.11.5:src/auto/fixed.rs:276-282`.
- `WidgetExt::insert_after` (the pill wrap is raised after its tile, never
  unparented): `gtk4-0.11.5:src/auto/widget.rs:922`.
- `WidgetExt::set_cursor_from_name` (border hover):
  `gtk4-0.11.5:src/auto/widget.rs:1169`.
- `WidgetExt::set_overflow` / `gtk::Overflow::Hidden` (the frame clips its
  contents under its own border): `gtk4-0.11.5:src/auto/widget.rs:1346`;
  `gtk4-0.11.5:src/auto/enums.rs:7486-7490`.
- `NativeExt::surface_transform`, subtracted from a surface pointer position
  before the Fixed's bounds: `gtk4-0.11.5:src/auto/native.rs:62`.
- `GestureSingleExt::current_button` (only a left press starts a drag):
  `gtk4-0.11.5:src/auto/gesture_single.rs:35`.
- `GestureDrag::{connect_drag_begin, connect_drag_update, connect_drag_end}`:
  `gtk4-0.11.5:src/auto/gesture_drag.rs:158`, `:224`, `:191`.
- `adw::TimedAnimation::new` and `adw::CallbackAnimationTarget::new` (the
  200 ms tile slide and per-frame placement):
  `libadwaita-0.9.2:src/auto/timed_animation.rs:25-31`;
  `libadwaita-0.9.2:src/auto/callback_animation_target.rs:21`.
- A `GtkFixed` allocates a child its preferred size, which is why the tile
  and its pill wrap are both Fixed children with explicit requests:
  `gtk-4.22.4:gtk/gtkfixedlayout.c:297-322`.
- An EXTERNAL `GtkScrolledWindow` contributes no minimum width, which is why
  a narrow tile clips its pill instead of pushing its neighbour:
  `gtk-4.22.4:gtk/gtkscrolledwindow.c:1847-1851`.
- The pill is a sibling of the tile and paints over it; it is never
  unparented during a drag (CLAUDE.md's GTK lesson).

## Not verified live (Claude's check)

- The look against the spike: tile fills, the 2 px `gl-tile` border, the
  focus ring, the pill band and the clipped pill at a narrow column.
- Every drag: tile swap, gap insert with the accent line, refused drop and
  slide-back; right-button cancel, Esc, focus loss and release outside;
  border drags (column and stacked), the whole-strip grab, the hand floor and
  the double-click reset.
- Stuck tiles while scrolling, including the sticky pill, the frame top and
  the contents disappearing under the frame's border.
- The pinned short column while the tall one scrolls.
- The split enter/leave crossfade.
- Focus with real clicks in a tile, its pill and its grid; the path bar,
  history buttons and status line following the focused tile.
- The per-tile scroll anchor after a tab switch or a Back, and the
  `view.set-sort scope=folder` effect on a hidden tile of the same folder.

## Anything outside my files

- None required. The one model gap: `Workspace::record_scroll_anchor` records
  the focused tab only. The UI wrapper above focuses the tile for the one
  update and puts the old focus back without emitting events, so an unfocused
  tile's anchor is still stored. A first-class
  `record_scroll_anchor_for(tab, name)` in `fm-core` would remove that
  workaround; it is not in this brief's Output.

## Review and live check (Claude, Nov 2, 2026): passes, with one fix by Claude

`scripts/check.sh` from `/tmp`: exit 0. The tests are 100 `fm-ui`, 175 `fm-core`, the 6 trash
parents, 7 CLI and 1 agent, and deny is clean. The agent cleaned up its `/tmp/gimbal-p2j-*`
folders, and Kyle's real config and session are untouched since his own run at 10:23.

**Live** (release build, niri-tiled on DP-2 at 2256×1504, scratch config and session). The test
session was one group of four tabs (a, b, c | d), each folder holding 40 files and a subfolder.
- **Restore and layout:** the group restored as one header pill, "Group 1 · 4 tabs". The tiles were
  laid out from the model with `gap-l` gutters, each with its own pill and its own folder grid.
  The status line and path bar followed the focused tile.
- **Focus:** a click in tile d's grid ran `focus_tab` (tab 5). The path bar showed `…/d`, and d got
  the focus ring (after the fix below).
- **Tile drag:** dragging c's pill onto d logged `dropped at Swap(TabId(5))`, and the two swapped
  columns.
- **Border push:** dragging the border between a and b down 200 px gave
  `Hand([604, 404, 405])`. a grew, and the rest moved down without shrinking.
- **Scrolling:** with the left column taller than the view, a stuck. Its pill and frame top held
  while its files scrolled under the border, and the right column (c) stayed put.
- **Closing:** b's ✕ closed it (`TabClosed`). The left column's heights reset to `Default`, as the
  model rule says, and the header read "Group 1 · 3 tabs".
- **The tile pill's right-click menu:** Close, Open in New Window and Close Group. It still stayed
  open after a full second, after a scroll, and after a border drag.
- **Cancel:** a right-click during a tile drag logged `tile drag cancelled; sliding everything back`,
  and the layout was unchanged.
- **Dissolving:** closing down to one tab turned the header pill back into "c", and the content
  faded back to P2i's single grid.
- No Gtk-CRITICAL in any run.

Earlier in the session, several clicks were lost: one ✕ and three right-clicks on a pill. Repeated
attempts, a fresh run and temporary logging (since removed) all showed the press arriving and the
menu opening. The lost clicks look like the virtual device's known habit of dropping a click; Kyle
should confirm by hand.

**Fix (Claude): the focus ring was invisible.**
- **The cause:** `.gl-tile.focused` drew `box-shadow: 0 0 0 2px` outside the frame, but each tile
  sits in a clipping scroller, so the ring was cut off. The focused tile looked exactly like the
  others (sampled: the pixels just outside both frames were background).
- **The fix:** the ring is now inset, so the focused tile shows a 4 px accent edge (border plus
  ring) against 2 px on the others. Verified live on tiles d and c.
- **Test:** `the_focused_tile_ring_is_drawn_inside_the_frame`.

For later:
- `fm-core` should get `record_scroll_anchor_for(tab, name)`. Today the UI briefly refocuses a tile
  to record its anchor (the agent's note).
- Kyle should check the look of the 4 px focus edge, since the spike's ring was meant to sit
  outside the frame.
