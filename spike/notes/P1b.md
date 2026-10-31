# P1b notes: the spike skeleton

## What I built

- `spike/Cargo.toml`: exactly the two pinned dependencies (`gtk4 =0.11.5` with `v4_22`,
  `libadwaita =0.9.2` with `v1_9`), edition 2024, Rust 1.92 minimum.
- `spike/src/main.rs`: `adw::Application` with ID `io.github.kcanady21.GimbalSpike` and
  `gio::ApplicationFlags::NON_UNIQUE`; hand-rolled CLI parsing; `run_with_args(&[])`; the `log!`
  macro.
- `spike/src/window.rs`: `adw::ApplicationWindow` + `adw::HeaderBar` + `adw::TabBar` +
  `adw::TabView` inside an `adw::ToolbarView`; new-tab button, go-up button, menu; `win.new-tab`,
  `win.close-tab`, `win.open-in-new-window`; `new_window` and `open_dir_in_tab` per the contract.
- `spike/src/listing.rs`: `DirectoryList` → `SortListModel` (folders first, then case-insensitive
  display name) → `MultiSelection` → `ListView`; icon + display name rows; folder activation
  navigates in place; `--select`; load timing logs.
- `spike/src/dnd.rs`, `tabs.rs`, `theme.rs`: the hook stubs with exactly the P1b signatures and
  `// Filled by P1x` comments.
- `spike/README.md`, `spike/.gitignore`.

The `--select` names are applied after the first tab's `DirectoryList` has `loading == false`.
Selection is by `standard::name`, not display name.

## Verified

All commands were run from `spike/`.

### `cargo fmt --check`

The distro `cargo` 1.93.1 has no `fmt` subcommand here. I used the installed rustup toolchain:

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
$ echo $?
0
```

### `cargo clippy --all-targets -- -D warnings`

Same PATH prefix, because the distro cargo has no `clippy` subcommand:

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.01s
```

### `cargo test`

```text
$ cargo test
running 2 tests
test listing::tests::sorts_folders_first_then_names ... ok
test tests::parses_cli ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The CLI test covers directories, one `--select` name, several names, and an unknown flag.

### `cargo build --release`

```text
$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.06s
```

This build used the distro rustc 1.93.1, so edition 2024 is confirmed on the required compiler
**[observed]**.

### Startup check

```text
$ mkdir -p /tmp/gimbal-spike-p1b/a-folder
$ touch /tmp/gimbal-spike-p1b/b.txt /tmp/gimbal-spike-p1b/C.png
$ GIMBAL_SPIKE_LOG=1 ./target/release/gimbal-spike --select b.txt /tmp/gimbal-spike-p1b \
    > /tmp/gimbal-spike-p1b-run.log 2>&1 &
$ sleep 5; niri msg --json windows > /tmp/gimbal-spike-p1b-windows.json
$ kill -TERM <pid>; wait <pid>

[gimbal-spike 0.072] window: window created
[gimbal-spike 0.073] listing: folder load started: /tmp/gimbal-spike-p1b
[gimbal-spike 0.073] window: tab opened: /tmp/gimbal-spike-p1b
[gimbal-spike 0.118] listing: folder load finished: /tmp/gimbal-spike-p1b (3 items, 45 ms)
[gimbal-spike 0.118] listing: selected 1/1 name(s)
```

`niri msg --json windows` contained:

```text
{'id': 163, 'title': 'Gimbal Spike', 'app_id': 'io.github.kcanady21.GimbalSpike',
 'pid': 1235517, 'workspace_id': 1, 'is_focused': True, ...}
```

The process was stopped with SIGTERM by PID (exit status 143 = terminated by SIGTERM) **[observed]**.

Two simultaneous launches also produced two windows with two PIDs (1235896 and 1235897), so
`NON_UNIQUE` behaves as intended **[observed]**.

### Safety checks

```text
$ grep -rnE 'remove|delete|trash' src/
$ echo $?
1
$ wc -l src/*.rs
   16 src/dnd.rs
  178 src/listing.rs
  127 src/main.rs
    5 src/tabs.rs
    5 src/theme.rs
  116 src/window.rs
  447 total
```

`cargo tree --depth 1` shows only `gtk4 v0.11.5` and `libadwaita v0.9.2` **[observed]**.

## API citations

Checked against the pinned crate sources in `~/.cargo/registry/src/`:

- `adw::ApplicationWindow` content: `libadwaita-0.9.2:src/auto/application_window.rs:465` declares
  `AdwApplicationWindowExt`, and `:552-557` is `set_content`.
- `adw::ToolbarView`: `libadwaita-0.9.2:src/auto/toolbar_view.rs:61` (`add_top_bar`) and `:173`
  (`set_content`).
- Tabs: `libadwaita-0.9.2:src/auto/tab_view.rs:55` (`add_page`), `:102` (`close_page`), `:161`
  (`n_pages`), `:174` (`page`), `:370` (`set_selected_page`); `libadwaita-0.9.2:src/auto/tab_page.rs:277`
  (`set_title`), `:285` (`set_tooltip`).
- `DirectoryList::new` / `set_file` / loading signal: `gtk4-0.11.5:src/auto/directory_list.rs:24`,
  `:82`, `:200`.
- `CustomSorter::new`: `gtk4-0.11.5:src/custom_sorter.rs:11`.
- `SortListModel::new`: `gtk4-0.11.5:src/auto/sort_list_model.rs:39`.
- `MultiSelection::new`: `gtk4-0.11.5:src/auto/multi_selection.rs:39`; `select_item`:
  `gtk4-0.11.5:src/auto/selection_model.rs:70`.
- `ListView::new` / activate: `gtk4-0.11.5:src/auto/list_view.rs:47`, `:225`.
- `gio::ActionEntry::builder`: `gio-0.22.10:src/action_entry.rs:37`; `add_action_entries`:
  `gio-0.22.10:src/action_map.rs:8`.
- `run_with_args`: `gio-0.22.10:src/application.rs:27`.
- `glib::home_dir`: `glib-0.22.10:src/auto/functions.rs:368`.
- Widget-attached state: `glib-0.22.10:src/object.rs:2555` (`set_data`) and `:2559` (`data`).

## Non-obvious findings

- `AdwApplicationWindow` rejects both `gtk_window_set_titlebar()` and `gtk_window_set_child()` at
  runtime (`Adwaita-ERROR`), even though those methods are inherited **[observed]**. The window must
  use `adw::ToolbarView` with `add_top_bar`, then `AdwApplicationWindowExt::set_content`.
- After `set_content`, `gtk_window_get_child()` returns libadwaita's internal `AdwDialogHost`, not
  the toolbar **[observed]**. I therefore do not walk the widget tree to find the tab view; the
  listing holds a weak reference to its `TabPage` via `listing::set_on_navigate`.
- The listing's shared `Rc<RefCell<gio::File>>` is updated by `navigate()` so P1c's drop target keeps
  following the shown folder.

## Not verified without a live session

- Visual layout: header bar, tab bar, go-up button, new-tab button, menu, row icons.
- `Ctrl+T`, `Ctrl+W`, `Ctrl+N`, double-click/Enter on a folder, file activation doing nothing.
- The go-up button and tab title/tooltip after navigating.
- That `tab-new-symbolic` and `open-menu-symbolic` exist in the icon theme.

## Outside my files

- `Cargo.lock` was generated by cargo; the brief's Output list does not name it, but it is required
  for reproducible builds. `target/` is ignored as asked.
- The distro cargo lacks `rustfmt` and `clippy`; the rustup 1.95.0 toolchain was used only for those
  two checks. Nothing in the project was changed for that.
- I created `/tmp/gimbal-spike-p1b/{a-folder,b.txt,C.png}` and left it in place for the reviewer.
- I suggest adding `GIMBAL_SPIKE_THEME_CSS` to P1f's README text when P1f lands; P1b only reserves it.

## Review (Claude, Oct 31, 2026)

**Verdict: accepted.** Rerun from `cargo clean`: `fmt --check`, `clippy -D warnings` (rustup
1.95.0, as noted above), `cargo test` (2 passed) and `cargo build --release` all pass. `cargo tree
--depth 1` shows only `gtk4 0.11.5` and `libadwaita 0.9.2`. The delete/remove/trash/rename grep is
empty. Every source file has the SPDX header. Spot-checked citations, all correct: toolbar_view.rs:61,
tab_view.rs:55, tab_page.rs:285, directory_list.rs:200, custom_sorter.rs:11, application.rs:27,
object.rs:2555 and application_window.rs:552. The hook signatures match the brief exactly, and they
are saved in `notes/P1b-contract.rs` for later reviews.

Live (Claude): two launches gave two PIDs and two windows; `--select b.txt,C.png` selected 2/2; an
unknown flag exits 2. The screenshot shows the header (go-up, tab bar, menu, new-tab), folders first
with icons, and Noctalia's colors already applied at startup (useful for P1f).

Changed by Claude during review:

1. With several `DIR`s the **last** tab ended up selected, while `--select` applies to the first.
   `main.rs` now selects the first tab after opening them all (checked with a screenshot: the first
   tab shows, with `b.txt` selected). The total is now 456 lines; the 6 over budget are this fix.

Notes for later briefs (no change needed now):

2. **Memory leak per closed tab (P1e).** `State` holds the `DirectoryList`, the list's
   `loading`-notify closure holds `state`, `selection` and `pending`, and the `ListView`'s `activate`
   closure holds `state`. Those are Rc↔GObject cycles, so a closed tab's listing is never freed. For
   a 100k-file folder that's a lot of memory per closed tab. P1e should break the cycles (weak refs in
   the closures).
3. **`win.open-in-new-window` unwraps the selected page (P1d).** Closing the last tab with its ×
   leaves an empty window (only `win.close-tab` closes it), and `Ctrl+N` there would panic. P1d's
   "empty windows close" item removes that case.
4. The sorter lowercases both names on every comparison (an allocation each time). P1e's measurements
   will show whether that matters at 100k.
5. `spike/.gitignore` has no SPDX line. It's a one-line ignore file, so I left it.

Not checked live (no keyboard injection): `Ctrl+T`/`Ctrl+W`/`Ctrl+N`, double-click into a folder,
go-up, and that activating a file opens nothing. These moved to the P1h checklist as row 0.
