# P2c: the TOML config

## What I built

- `crates/fm-core/src/config.rs`: the typed settings, the default pins and
  hotkeys, loading with line/column problems, one-key atomic saves, and the
  `Settings` command → `Change` mapping.
- `crates/fm-core/src/lib.rs`: `pub mod config;` only.
- `Cargo.toml`: `glib = "=0.22.10"` and `toml_edit = "=0.25.17"` in
  `[workspace.dependencies]`.
- `crates/fm-core/Cargo.toml`: the two workspace dependencies.
- `Cargo.lock`: updated by Cargo; `fm-core` now lists `glib` and `toml_edit`.
- `docs/notes/P2c.md`: this file.

The public shape is:

- `Config` holds `look`, `sidebars`, `sections`, `pins`, `sort` and `hotkeys`,
  plus two runtime-only fields: `home` (for `~` expansion) and
  `current_folder` (for commands whose P2b argument is optional from the
  current folder). Neither runtime field is read from or written to the file.
  `current_folder` defaults to home so `change_for` always has a folder.
  `[inferred]`
- `Config::sort_for(folder) -> SortRule` is a pure method over the loaded
  config and the folder. `[observed]`
- `load(path) -> (Config, Vec<ConfigProblem>)` and
  `ConfigFile::load(path) -> (ConfigFile, Vec<ConfigProblem>)`.
  `ConfigFile::apply(change) -> Result<(), ApplyError>`. `[observed]`
- `change_for(&CheckedCommand, &Config) -> Option<Change>` maps every
  `Settings` command from P2b. `[observed]`

Choices where the brief leaves room:

- Numeric settings accept TOML floats and integers. Scale is compared in
  integer hundredths, so `0.95`, `0.9` and `1` are exact; `0.57` and `0.555`
  are reported as off the 0.05 step. Base and opacity have no step. `[inferred]`
- A folder sort rule may omit `by` or `order`; the missing value inherits the
  global rule. This is more forgiving than rejecting the whole rule and keeps
  the “bad value falls back” rule. `[inferred]`
- `view.set-sort` with scope `folder` uses `Config::current_folder`; the P2b
  signature has no folder argument. With no runtime value it uses home, which
  is the same starting folder the plan gives an empty window. `[inferred]`
- `pins.add` with no `path` also uses `Config::current_folder`. `[inferred]`
- When a file does not parse, `apply` still updates the in-memory `Config` so
  the current session keeps the change, but returns `ApplyError::Unparsable`
  and never writes. `[inferred]`
- Unknown keys and tables are warnings; every other problem is an error. The
  problem list is sorted by line and column before it is returned. `[observed]`

## Dependencies

`toml_edit = "=0.25.17"` was already in `Cargo.lock` through the GTK tree, and
`glib = "=0.22.10"` was already there; adding them to `fm-core` adds no new
crate versions. `cargo deny check` still passes. `[observed]`

No serde: serde would deserialize into a shadow model, hide the spans needed
for line/column problems, and still need a second document to preserve
comments, key order, unknown keys and formatting on save. Using
`toml_edit::DocumentMut` directly is less code for these requirements, not
more. `[inferred]`

## Problems

`ConfigProblem` carries `file`, `line`, `column`, `key`, `kind` and a
`message`; `Display` renders one line:

```
<file>:<line>:<column>: <key>: <error|warning>: <message>
```

The kinds are parse, read, wrong type, out of range, off the step, bad choice,
unknown key, unknown table, missing value, unknown hotkey command, invalid
hotkey arguments, empty accelerator, and a relative pin path after `~`
expansion. Spans come from `toml_edit` keys and items; the byte offset is
converted to one-based line and column over the original text. `[observed]`

Hotkeys are validated as P2b’s `Hotkey` caller through
`Registry::validate`. An inline table’s `command` is the name; every other key
becomes a `key=value` token, and an array becomes repeated keys. A context
argument may be omitted because the Hotkey caller may use context. The core
does not parse the accelerator; it only rejects an empty accelerator key.
`[observed]`

Sample problem list from a deliberately broken file (the test
`config::tests::prints_a_sample_problem_list` prints this):

```
/tmp/gimbal-p2c-sample-problems-487951-0/config.toml:2:9: look.scale: error: value `0.57` is off the 0.05 step
/tmp/gimbal-p2c-sample-problems-487951-0/config.toml:3:8: look.base: error: value `3` is outside 0.50..2.00
/tmp/gimbal-p2c-sample-problems-487951-0/config.toml:7:37: sidebars.left.width: error: width `250` is below the minimum 300
/tmp/gimbal-p2c-sample-problems-487951-0/config.toml:10:16: hotkeys."<Control>t": error: unknown command `no.such-command`
/tmp/gimbal-p2c-sample-problems-487951-0/config.toml:12:2: surprise: warning: unknown table; kept in the file
```

`[observed]`

## Saving

`ConfigFile` keeps the parsed `DocumentMut`. `apply` changes the in-memory
config and only the changed key in the document, then:

1. creates the parent folder if it is missing;
2. writes a uniquely named temp file in that same folder with
   `create_new(true)`;
3. `write_all` and `sync_all` (fsync);
4. copies the original file’s permissions to the temp file;
5. renames the temp file over the original;
6. fsyncs the parent directory where the platform allows opening it.

A save on a commented file was tested byte-for-byte for `opacity`, `font`,
a sidebar width inside an inline table, a section value, the global sort and
pins. An unparsable file is never written; `is_writable()` is false and
`apply` returns `ApplyError::Unparsable`. `[observed]`

## Verified

Toolchain from `CLAUDE.md`:

```
PROJ="/home/kyle/Documents/03.) Code-Projects/gimbal-files"
T="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin"
```

### `scripts/check.sh`

Run from `$PROJ`; exit status 0. The duplicate `system-deps` warning and its
dependency tree are elided at the same warning P2a and P2b had.

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
    Checking fm-core v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-core)
    Checking fm-agent v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-agent)
    Checking fm-ui v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-ui)
    Checking fm-cli v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-cli)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.28s
==> cargo test --workspace
   Compiling fm-core v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-core)
   Compiling fm-agent v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-agent)
   Compiling fm-cli v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-cli)
   Compiling fm-ui v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-ui)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.24s
     Running unittests src/lib.rs (target/debug/deps/fm_agent-cc4ee4bf374e51f2)

running 1 test
test tests::socket_lives_under_the_runtime_directory ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/main.rs (target/debug/deps/gimbal_files_cli-4295b4be5da538bb)

running 7 tests
test tests::anything_else_is_a_usage_error ... ok
test tests::check_without_a_command_is_a_usage_error ... ok
test tests::check_keeps_the_command_and_its_arguments ... ok
test tests::version_shows_the_version ... ok
test tests::help_shows_usage ... ok
test tests::commands_lists_the_registry ... ok
test tests::no_arguments_shows_usage ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (target/debug/deps/fm_core-408d0e2ff853beac)

running 69 tests
test command::tests::a_checked_command_prints_in_catalogue_order ... ok
test command::tests::a_forbidden_call_never_reaches_the_handler ... ok
test command::tests::a_files_command_defaults_dry_run_to_false ... ok
test command::tests::a_known_command_is_not_an_unknown_command ... ok
test command::tests::a_single_valued_argument_may_not_repeat ... ok
test command::tests::a_valid_call_reaches_the_handler_once ... ok
test command::tests::a_token_without_equals_is_an_error ... ok
test command::tests::a_value_may_contain_equals_and_spaces ... ok
test command::tests::an_empty_argument_name_is_an_error ... ok
test command::tests::an_invalid_call_never_reaches_the_handler ... ok
test command::tests::argument_kinds_are_sane ... ok
test command::tests::bool_accepts_true_and_false_only ... ok
test command::tests::argument_names_are_unique_within_a_command ... ok
test command::tests::a_distant_typo_gets_no_suggestion ... ok
test command::tests::choice_must_be_one_of_the_fixed_list ... ok
test command::tests::cli_and_agent_may_not_fill_files_paths_from_context ... ok
test command::tests::context_rules_are_pure_functions_over_every_caller ... ok
test command::tests::cli_and_agent_must_give_every_files_context_path ... ok
test command::tests::every_files_command_gets_exactly_one_dry_run_added_by_the_registry ... ok
test command::tests::every_caller_may_give_explicit_files_paths ... ok
test command::tests::missing_required_argument_is_named ... ok
test command::tests::int_accepts_the_range_and_rejects_bad_and_out_of_range_values ... ok
test command::tests::no_command_deletes_permanently ... ok
test command::tests::names_are_unique ... ok
test command::tests::names_are_area_dot_verb_in_lowercase_kebab_case ... ok
test command::tests::no_tokens_is_a_missing_command ... ok
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
test command::tests::unknown_command_suggests_the_closest_name ... ok
test config::tests::config_path_is_a_pure_function_of_xdg_config_home_and_home ... ok
test config::tests::every_default_matches_the_brief ... ok
test config::tests::an_unparsable_file_is_never_written ... ok
test config::tests::an_unparsable_file_reports_one_problem_with_a_position ... ok
test config::tests::every_settings_command_has_a_change_for ... ok
test config::tests::a_hotkeys_table_replaces_listed_keys_and_an_empty_value_unbinds ... ok
test config::tests::default_pins_include_home_then_only_existing_special_folders ... ok
test config::tests::a_bad_value_falls_back_and_the_good_values_still_load ... ok
test config::tests::an_unknown_key_is_a_warning_with_its_position ... ok
test config::tests::a_read_error_reports_the_file_and_position ... ok
test config::tests::set_scale_and_sort_map_to_the_expected_changes ... ok
test config::tests::a_save_rewrites_only_the_changed_key_on_a_commented_file ... ok
test config::tests::saving_creates_the_folder_and_the_changed_key ... ok
test tests::app_id_matches_the_niri_rule_and_is_a_valid_gapplication_id ... ok
test config::tests::zoom_stops_at_the_scale_limits_and_toggles_flip ... ok
test config::tests::a_sidebar_and_section_save_keep_their_inline_formatting ... ok
test config::tests::saving_is_atomic_and_keeps_permissions ... ok
test config::tests::pin_changes_write_the_pins_list ... ok
test config::tests::a_hotkey_with_an_unknown_command_or_bad_arguments_keeps_the_default ... ok
test config::tests::unknown_keys_and_tables_are_warnings_and_stay_in_the_file ... ok
test config::tests::sort_for_expands_home_and_normalizes_dot_dot_and_trailing_slashes ... ok
test config::tests::sort_changes_write_one_global_setting_or_one_folder_rule ... ok
test config::tests::prints_a_sample_problem_list ... ok
test config::tests::every_problem_kind_reports_its_line_and_column ... ok

test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

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
    Finished `release` profile [optimized] target(s) in 0.79s
==> cargo deny check
warning[duplicate]: found 2 duplicate entries for crate 'system-deps'
   ... (dependency tree elided: system-deps 7.0.8 and 9.0.1;
        same duplicate warning as P2a and P2b, multiple-versions = "warn")
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Workspace test count: 77 (23 config, 45 registry, 1 `fm-core` app-ID, 7 CLI,
1 agent). `[observed]`

## API citations

- `toml_edit::DocumentMut`, `Document::parse` and `into_mut`:
  `toml_edit-0.25.17+spec-1.1.0:src/document.rs:133` and `:139`.
- `Key::span()`, `Item::span()` and `Value::span()` for problem positions:
  `toml_edit-0.25.17+spec-1.1.0:src/key.rs:132`,
  `src/item.rs:327`, `src/value.rs:213`.
- `InlineTable::insert_formatted` for preserving the key’s formatting:
  `toml_edit-0.25.17+spec-1.1.0:src/inline_table.rs:414`.
- `Array::remove`, `insert_formatted`, `replace`:
  `toml_edit-0.25.17+spec-1.1.0:src/array.rs:305`, `:238`, `:252`.
- `glib::user_special_dir` and `glib::UserDirectory`:
  `glib-0.22.10:src/auto/functions.rs:494` and `src/enums.rs:11`. This is the
  only use of `glib`; the home folder comes from `std::env::var_os("HOME")`.
- `std::fs::rename`, `OpenOptions::create_new`, `File::sync_all` and
  `Permissions`: the standard library documentation for the pinned toolchain.
  `[read]`

All of these were read in the pinned crate source under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. `[observed]`

## What needs a live session

Nothing. The config module is pure data plus file I/O, and every file test ran
in a `/tmp/gimbal-p2c-*` folder that the test created and removed. No test read
or wrote `~/.config`, and no UI is wired to the config yet. `[inferred]`

The only behaviour Claude should check in the app later is the one this brief
does not build: applying a `Change` from the real UI and seeing the live scale,
opacity, sidebar and section follow it.

## Anything outside my files

Nothing needs changing. Two notes for later briefs:

- P2d should keep `Config::current_folder` up to date as the focused tab moves,
  or pass the folder explicitly when it builds a `Change`. The config module
  only uses it for `view.set-sort scope=folder` and `pins.add` without a path.
- When the UI writes a config change, it should hold one `ConfigFile` for the
  process so the `DocumentMut` and the typed `Config` stay in step. Reloading
  separately would lose the parsed document’s formatting.

No file outside the brief’s Output list was created or changed. `spike/` was
read only. `[observed]`

## Review (Claude, Nov 2, 2026): passes, with one fix by Claude

`scripts/check.sh` from `/tmp`: exit 0, with 120 `fm-core` tests plus 7 CLI and 1 agent, and deny
clean. The tests left nothing in `/tmp` and never created `~/.config/gimbal-files`. The choice of
`toml_edit` alone, with no serde, is right for spans and comment-keeping saves.

Claude probed the config from a scratch program outside the repo:
- **A hand-written file** with comments, an aligned `scale   = 1.0   # big screen`, a multi-line
  pins array with a comment and an unknown `[mystery]` table, then four changes:
  - every comment and the alignment survived;
  - `[mystery]` was kept, with a warning on load;
  - the file stayed mode 600, and no temp file was left.
- **An unparsable file** (`[look` unclosed) gave one problem at 1:6. The file stayed unwritable,
  and `apply` refused and left it byte-identical.

**Bug found and fixed (Claude):** `Change::SetSort` with a folder also overwrote the global
`[sort] by` / `order` in the file. The in-memory config was right, so the bug only showed after a
reload: every folder would then sort the way the last folder rule did. The fix moves the two
global writes into the `folder: None` branch. `sort_changes_write_one_global_setting_or_one_folder_rule`
now reloads the file and asserts that the global rule (size, descending) and the folder rule
(modified, descending) both survive. The test fails on the old code and passes on the fix.

For later briefs (not blocking):
- `Config::current_folder` is UI state living in the config struct. P2e or the first handler
  brief should pass the current folder to `change_for` as an argument instead, so the config
  holds only settings.
- `AddPin` on a multi-line pins array appends to the last line (`"~/Code", "/tmp",`). It's valid
  and keeps comments, but a new line per pin would match the file's style.
