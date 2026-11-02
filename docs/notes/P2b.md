# P2b: the command registry

## What I built

- `crates/fm-core/src/command/` (four files):
  - `mod.rs`: the public types (`Caller`, `Effect`, `ContextSource`, `Presence`,
    `ArgKind`, `ArgSpec`, `CommandSpec`, `Number`, `Value`, `CheckedArgument`,
    `CheckedCommand`, `Outcome`, `Handler`), the caller-rule functions
    (`may_use_context`, `must_supply`, `permanently_deletes`), the name check,
    and the one `Registry` with `validate` and `dispatch`.
  - `catalogue.rs`: the V1 table, 46 commands in the brief's order. It is the
    only place command names appear.
  - `check.rs`: `Invocation` (`key=value` tokens), the typed checked command,
    `CommandError`/`CommandErrorKind`, every argument parser, and the
    Levenshtein suggestion (edit distance 2 or less, no dependency).
  - `tests.rs`: 45 unit tests.
- `crates/fm-core/src/lib.rs`: `pub mod command;` only.
- `crates/fm-cli/src/main.rs`: `commands` and `check <command> [key=value …]`,
  reading the registry. `--help` and `--version` still work.
- `docs/notes/P2b.md`: this file.

The catalogue has 46 commands; 8 are `Files`, 10 are `Settings`, 25 are `View`,
2 are `Launch` (`app.disk-usage` and `file.open`), and 1 is `System`
(`drive.mount`). Effects per command are exactly the brief's table. `[read]`

Choices I made where the brief leaves room:

- `group.split` has no declared argument. The brief's argument kinds have no
  tab identifier, and "(the selected tabs)" is tab-strip UI state, so the
  handler reads it from the UI like other context. `[inferred]`
- `dry-run` is added by `CommandSpec::arguments()` to every `Files` command,
  last in the list, as an optional bool with default `false`. A checked `Files`
  command therefore always carries `dry-run`, and `CheckedCommand::dry_run()`
  reads it. `[observed]`
- `view.set-sort`'s `scope` default `folder` is materialised in the checked
  command, so a handler never has to repeat the default. `[observed]`
- The Cli/Agent context ban is applied to *every* context-filled path on a
  `Files` command, not only an argument literally named `paths`. Thus
  `file.new-folder name=X` is refused for Cli (the `in` folder must be given),
  while `file.new-folder name=X in=/tmp` is valid. `[observed]`
- A repeated key is a list only for the `Paths` kind; every other argument
  repeated is `DuplicateArgument`. `[observed]`
- No `Scope`/`Tab` argument kinds were invented; the six kinds are exactly the
  brief's. `[read]`

## Verified

Toolchain from `CLAUDE.md`:

```
PROJ="/home/kyle/Documents/03.) Code-Projects/gimbal-files"
T="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin"
```

### `scripts/check.sh`

Run from `$PROJ`; exit status 0. Compile progress is included; the
`cargo deny` dependency tree is elided at the same duplicate warning P2a had.

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
    Checking fm-core v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-core)
    Checking fm-agent v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-agent)
    Checking fm-ui v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-ui)
    Checking fm-cli v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-cli)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s
==> cargo test --workspace
   Compiling fm-core v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-core)
   Compiling fm-agent v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-agent)
   Compiling fm-cli v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-cli)
   Compiling fm-ui v0.1.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/crates/fm-ui)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.21s
     Running unittests src/lib.rs (target/debug/deps/fm_agent-ebba753514746c50)

running 1 test
test tests::socket_lives_under_the_runtime_directory ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/main.rs (target/debug/deps/gimbal_files_cli-d9fbb46d8a0a06d0)

running 7 tests
test tests::check_keeps_the_command_and_its_arguments ... ok
test tests::check_without_a_command_is_a_usage_error ... ok
test tests::anything_else_is_a_usage_error ... ok
test tests::help_shows_usage ... ok
test tests::commands_lists_the_registry ... ok
test tests::version_shows_the_version ... ok
test tests::no_arguments_shows_usage ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/lib.rs (target/debug/deps/fm_core-0ccb5d2c35a24b1e)

running 46 tests
test command::tests::a_checked_command_prints_in_catalogue_order ... ok
test command::tests::a_files_command_defaults_dry_run_to_false ... ok
test command::tests::a_known_command_is_not_an_unknown_command ... ok
test command::tests::a_single_valued_argument_may_not_repeat ... ok
test command::tests::a_forbidden_call_never_reaches_the_handler ... ok
test command::tests::a_valid_call_reaches_the_handler_once ... ok
test command::tests::a_value_may_contain_equals_and_spaces ... ok
test command::tests::a_token_without_equals_is_an_error ... ok
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
test command::tests::missing_required_argument_is_named ... ok
test command::tests::int_accepts_the_range_and_rejects_bad_and_out_of_range_values ... ok
test command::tests::names_are_area_dot_verb_in_lowercase_kebab_case ... ok
test command::tests::no_command_deletes_permanently ... ok
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
test command::tests::undo_is_only_offered_on_files_commands ... ok
test command::tests::ui_and_hotkey_may_fill_files_paths_from_context ... ok
test command::tests::unknown_argument_is_named ... ok
test tests::app_id_matches_the_niri_rule_and_is_a_valid_gapplication_id ... ok
test command::tests::usage_lists_arguments_in_catalogue_order ... ok
test command::tests::unknown_command_suggests_the_closest_name ... ok

test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/main.rs (target/debug/deps/gimbal_files-e0eae169b3cac5a8)

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
    Finished `release` profile [optimized] target(s) in 0.39s
==> cargo deny check
warning[duplicate]: found 2 duplicate entries for crate 'system-deps'
   ... (dependency tree elided: system-deps 7.0.8 and 9.0.1;
        same duplicate warning as P2a, multiple-versions = "warn")
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Workspace test count: 54 (45 registry, 1 `fm-core` app-ID, 7 CLI, 1 agent).
`[observed]`

The required `check` commands, using `target/release/gimbal-files-cli`:

```
$ gimbal-files-cli check nav.go path=/tmp
nav.go path=/tmp
exit=0

$ gimbal-files-cli check file.trash
command `file.trash`: argument `paths`: missing; the Cli caller may not fill a Files path from context
exit=2

$ gimbal-files-cli check file.trash paths=/tmp/x dry-run=true
file.trash paths=/tmp/x dry-run=true
exit=0

$ gimbal-files-cli check view.set-scale scale=0.57
command `view.set-scale`: argument `scale`: value `0.57` is off the 0.05 step
exit=2

$ gimbal-files-cli check tab.clsoe
unknown command `tab.clsoe`; did you mean `tab.close`?
exit=2
```

`[observed]`

## `gimbal-files-cli commands`

Exit status 0. One line per command, catalogue order, columns
`name | effect | arguments | summary`:

```
app.quit | view | (none) | Quit Gimbal Files.
app.disk-usage | launch | path? (path, from current folder) | Show a folder's disk usage in the system analyser.
window.new | view | path? (path) | Open a new window, optionally at a folder.
window.close | view | (none) | Close the current window.
tab.new | view | path? (path) | Open a new tab, optionally at a folder.
tab.close | view | (none) | Close the current tab.
tab.close-selected | view | (none) | Close the selected tabs.
tab.focus | view | index (int, 0 or more) | Focus the tab at an index.
tab.move | view | from (int, 0 or more), to (int, 0 or more) | Move a tab to another index.
tab.open-in-new-window | view | (none) | Open the current tab in a new window.
group.split | view | (none) | Split the selected tabs into a tiled group.
group.close | view | (none) | Close the current group.
group.rename | view | name (text) | Rename the current group.
group.add-tab | view | group (int, 0 or more) | Add the current tab to a group.
nav.go | view | path (path) | Go to a folder.
nav.back | view | (none) | Go back in this tab's history.
nav.forward | view | (none) | Go forward in this tab's history.
nav.up | view | (none) | Go to the parent folder.
nav.refresh | view | (none) | Reload the current folder.
nav.focus-path | view | (none) | Focus the path bar.
search.open | view | query? (text) | Open search, optionally with a query.
selection.all | view | (none) | Select every item.
selection.none | view | (none) | Clear the selection.
file.open | launch | paths? (paths, from selection) | Open files with their default application.
file.trash | files | paths? (paths, from selection), dry-run? (bool; default false) | Move files to the trash.
file.copy | files | paths? (paths, from selection), to (path), dry-run? (bool; default false) | Copy files into a folder.
file.move | files | paths? (paths, from selection), to (path), dry-run? (bool; default false) | Move files into a folder.
file.rename | files | path? (path, from selection), name (text), dry-run? (bool; default false) | Rename one file in place.
file.new-folder | files | name (text), in? (path, from current folder), dry-run? (bool; default false) | Create a new folder.
file.undo | files | dry-run? (bool; default false) | Undo the last file operation.
trash.open | view | (none) | Open the trash.
preview.edit | view | (none) | Edit the file shown in the preview.
preview.discard | view | (none) | Discard edits to the previewed file.
preview.save | files | dry-run? (bool; default false) | Save edits to the previewed file.
preview.save-as | files | path (path), dry-run? (bool; default false) | Save the previewed file to a new path.
view.set-scale | settings | scale (number, 0.50..1.00, step 0.05) | Set the interface scale.
view.zoom-in | settings | (none) | Increase the interface scale by one step.
view.zoom-out | settings | (none) | Decrease the interface scale by one step.
view.set-opacity | settings | opacity (number, 0.00..1.00) | Set the window background opacity.
view.toggle-sidebar | settings | side (left|right) | Show or hide a sidebar.
view.toggle-section | settings | section (recent|devices|remote) | Expand or collapse a sidebar section.
view.set-sort | settings | by (name|modified|size|type), order (ascending|descending), scope? (folder|global; default folder) | Set the sort order.
pins.add | settings | path? (path, from current folder) | Pin a folder.
pins.remove | settings | path (path) | Unpin a folder.
pins.move | settings | from (int, 0 or more), to (int, 0 or more) | Move a pinned folder to another position.
drive.mount | system | device (text) | Mount a drive.
```

`[observed]`

## Catalogue suggestions for Kyle

I did not add or drop commands. These are suggestions only:

1. `trash.empty` is deliberately absent, since it permanently deletes and the
   catalogue test would reject it. If it is ever wanted, it needs an explicit
   exception or a different effect.
2. `group.split` could take an explicit tab identifier list if a future
   `tabs` argument kind exists; today the selected tabs are tab-strip state.
3. `file.rename` has no collision policy argument. A later brief may want
   `overwrite` (bool) or a refuse/rename choice; the file operation brief can
   add it after review.
4. `preview.save` is `Files` with `undo: false` because it overwrites the file
   in place; the table says no undo. `preview.save-as` likewise.
5. `tab.focus`, `tab.move`, `group.add-tab` and `pins.move` accept any
   non-negative index; the handler must still reject an index beyond the
   current list. The registry cannot know list lengths.
6. Consider `tab.close-others` and `view.reset-scale` later; neither is in V1.

## API citations

No new dependencies. Everything is `std`:

- `Path::is_absolute`: <https://doc.rust-lang.org/std/path/struct.Path.html#method.is_absolute>
  `[read]`
- `str::split_once`: <https://doc.rust-lang.org/std/primitive.str.html#method.split_once>
  `[read]`
- `i32::rem_euclid`, used for the step check so negatives would also work:
  <https://doc.rust-lang.org/std/primitive.i32.html#method.rem_euclid> `[read]`
- `u32::unsigned_abs` for formatting hundredths:
  <https://doc.rust-lang.org/std/primitive.i32.html#method.unsigned_abs> `[read]`
- The `if let … && …` let-chain in `check.rs` is stable in the pinned
  rustc 1.95.0 and needs edition 2024, which the workspace already uses.
  `[observed]`

No GTK, GIO or other crate names are used by `fm-core`'s command module.
`[observed]`

## What needs a live session

Nothing. The registry and CLI are pure functions plus process output; the CLI
was run in this session and its exits checked. The UI menu, hotkey and agent
wiring do not exist yet, so there is nothing to click. `[inferred]`

## Anything outside my files

Nothing needs changing. Two notes for P2c/P2d:

- Hotkeys can resolve names with `Registry::command(name)` and compare
  `ArgSpec`s directly; the registry already rejects an unknown hotkey command
  through `validate`.
- `ContextSource::FocusedTab` exists for completeness but no V1 command uses
  it. When the workspace model lands, a command can adopt it without changing
  the argument kinds.

No file outside the brief's Output list was created or changed. `spike/` was
read only. `[observed]`

## Review (Claude, Nov 2, 2026): passes

The catalogue lists all 46 commands (the brief's table rows expand to 46) with the reviewed
effects and undo flags. It's the only place names appear, and a test pins it to the reviewed
list and order. Validation keeps numbers as integer hundredths, and the caller rules are small
pure functions. `dispatch` validates before the handler, so a refused call never reaches it.

`scripts/check.sh` from `/tmp`: exit 0, with 46 + 7 + 1 + 1 tests and deny clean. Claude also ran
`gimbal-files-cli check` on cases beyond the brief's five, and each gave the right result:

| Case | Result |
|---|---|
| `scale=1` | accepted as `1.00` |
| `scale=0.45` | out of range |
| repeated `paths` | a list |
| `to=relative` | refused: not absolute |
| repeated `path` on `nav.go` | refused |
| `file.new-folder name=x` | refused for `Cli`: `in` must be given |
| `query=a=b` | kept whole |
| `view.set-sort` without `scope` | `scope=folder` |
| `index=-1` | out of range |
| `nav.go` with no path | missing |

For later briefs (not blocking):
- `Handler::handle` gets the checked command but not the caller. When handlers fill context or
  ask for confirmation, they'll likely want the caller too (for example, an agent's file
  operation could show in the UI). Add it with the first real handler.
- An int's range prints as `0..9223372036854775807` when it has no upper bound; "0 or more", as
  `commands` already says, would read better in errors.
- Kyle should look at the agent's catalogue suggestions above, especially `file.rename`'s
  collision policy and the absent `trash.empty`.
