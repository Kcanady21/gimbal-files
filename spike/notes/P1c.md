# P1c notes: drag and drop

## What I built (`spike/src/dnd.rs`, 196 lines; only `dnd.rs` and this notes file were written)

**Drag out (`attach_drag_source`, one `gtk::DragSource` per row, like Nautilus — P0e Q1):**

- Actions `COPY | MOVE` (`DragSource::set_actions`).
- `prepare`: if the pressed row (`ListItem::position()`) is selected, all selected items become the
  drag; otherwise just the pressed row. Each `FileInfo` becomes `dir.child(info.name())`, where
  `dir` is recovered from the model chain `MultiSelection → SortListModel → DirectoryList → file()`.
  Returns `ContentProvider::for_value(&FileList::from_array(&files).to_value())`. **No MIME added by
  hand** — GTK adds `text/uri-list`, `text/plain;charset=utf-8` and, when the document portal was
  reachable at `gtk_init`, both portal types (P0e Q4/Q5). The empty-list case returns `None` from
  `prepare` instead of calling `from_array(&[])`, which panics in glib 0.22.10 (`boxed.rs:523`).
- Icon: the pressed row's `FileInfo::icon()` through `IconTheme::lookup_by_gicon` →
  `IconPaintable` → `DragSource::set_icon` (set inside `prepare`, as Nautilus does). No count badge.
- `drag-end`: logs the action and `delete_data` and **ignores both**. GTK4 emits
  `delete_data = true` whenever a drop succeeds with MOVE and Firefox reports MOVE on every Wayland
  drop (P0e review 2), so acting on it would delete sources on a plain copy drag. Moves are done by
  the drop side. The code contains no delete/remove/trash call anywhere.

**Drop in (`attach_drop_target`, one `gtk::DropTarget` on the folder view):**

- GType `gdk::FileList::static_type()`, actions `COPY | MOVE`. GTK turns `text/uri-list` and the
  portal types into a `FileList` value (deserializer citations below).
- On drop: for each file, `query_file_type`; directories are skipped with the log line "folders not
  supported in the spike"; anything not regular is skipped; a file whose parent is already the shown
  folder (`dir.borrow()`) is skipped ("dropped onto its own folder"). Each regular file is copied
  with `gio::File::copy_async` (`FileCopyFlags::NONE`, priority DEFAULT). **Every drop is a copy,
  even when the negotiated action is MOVE — this is the spike.** A comment in the code says so.
- Never overwrites: the destination name comes from the pure `copy_name(name, exists)`; `exists`
  is `folder.child(candidate).query_exists()`. Extension = everything from the first non-leading
  dot, so `a.tar.gz` → `a (copy).tar.gz` (the brief's example), `a.txt` → `a (copy).txt` →
  `a (copy 2).txt` …, no-extension names get `README (copy)`, dotfiles count as extension-less
  (`.bashrc (copy)`). Documented on the function.
- Drop handler returns `true` only if at least one `copy_async` started. Logs the file count, the
  negotiated action (`Drop::drag().selected_action()`) and every copy result.

## Verified

All commands from `spike/` (rustup 1.95.0 for fmt/clippy, distro cargo 1.93.1 otherwise).

### `cargo fmt --check`

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
(exit 0)
```

### `cargo clippy --all-targets -- -D warnings`

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.11s
```

### `cargo test`

```text
running 4 tests
test dnd::tests::conflicts_get_copies ... ok
test dnd::tests::extension_edge_cases ... ok
test listing::tests::sorts_folders_first_then_names ... ok
test tests::parses_cli ... ok

test result: ok. 4 passed; 0 failed; ...
```

(`conflicts_get_copies` covers no-conflict, one conflict and several conflicts;
`extension_edge_cases` covers no extension, dotfile, two dots.)

### `cargo build --release`

```text
$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.58s
```

### Startup smoke test (no interaction, per the rules)

```text
$ GIMBAL_SPIKE_LOG=1 ./target/release/gimbal-spike --select a.txt /tmp/gimbal-spike-p1c …
[gimbal-spike 0.068] window: window created
[gimbal-spike 0.109] listing: folder load finished: /tmp/gimbal-spike-p1c (2 items, 40 ms)
[gimbal-spike 0.109] listing: selected 1/1 name(s)
```

Stopped with SIGTERM by PID; the folder was `/tmp/gimbal-spike-p1c`, created by me and removed
afterwards. The controllers attach without panicking. **[observed]**

### Done-means grep

```text
$ grep -nE 'remove|delete|trash|move_|rename' src/dnd.rs
50:    source.connect_drag_end(|_, drag, delete_data| {
51:        // GTK4 sets delete_data when a drop succeeds with MOVE, and Firefox
57:            "drag-end: action={:?} delete_data={} (ignored)",
59:            delete_data
```

The only hits are the `drag-end` callback's parameter, its log line and its comment — the brief
requires `delete_data` to be logged there. There is no call that changes a source file. **[observed]**

## API citations

gtk-rs sources in `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`:

- `gtk4-0.11.5:src/auto/drag_source.rs:25` (`DragSource::new`), `:65` (`set_actions`), `:83`
  (`set_icon(Option<&impl IsA<Paintable>>, hot_x, hot_y)`), `:158` (`connect_drag_end`, closure
  `Fn(&Self, &gdk::Drag, bool)`), `:191` (`connect_prepare`, returns `Option<ContentProvider>`).
- `gdk4-0.11.5:src/auto/content_provider.rs:40` (`for_value(&Value)`).
- `gdk4-0.11.5:src/auto/file_list.rs:24` (`from_array`, feature `v4_8`), `:37` (`files()`).
  `FileList::to_value()` works because `glib::wrapper!` boxed types get `ToValue`
  (`glib-0.22.10:src/boxed.rs:371`); the `from_array(&[])` panic is the
  `debug_assert!(!ptr.is_null())` at `glib-0.22.10:src/boxed.rs:523`. **[read]**
- `gtk4-0.11.5:src/auto/drop_target.rs:26` (`DropTarget::new(GType, actions)`),
  `gtk4-0.11.5:src/drop_target.rs:52` (`connect_drop`, closure
  `Fn(&DropTarget, &Value, f64, f64) -> bool`), `:16` (`set_types` — not needed, one GType).
- `gio-0.22.10:src/file.rs:372` (`copy_async`, 6 args incl. optional progress callback),
  `gio-0.22.10:src/auto/file.rs:1640` (`query_file_type`), `:674` (`FileExt::equal`).
- `gtk4-0.11.5:src/auto/icon_theme.rs:124` (`lookup_by_gicon` → `IconPaintable`), `:197`
  (`for_display`); `gtk4-0.11.5:src/auto/flags.rs:730` (`IconLookupFlags::NONE`, feature `v4_18`);
  `gtk4-0.11.5:src/auto/enums.rs:11926` (`TextDirection::None`).
- `gdk::FileList` value extraction on drop: `value.get::<FileList>()`, via the boxed `FromValue`
  generated at `glib-0.22.10:src/boxed.rs:348`.

GTK 4.22.4 clone (`/home/kyle/Documents/03.) Code-Projects/gimbal-refs/gtk-4.22.4`), drop-in
deserializer side:

- `gdk/gdkcontentdeserializer.c:973-977`: `text/uri-list` registered for `GDK_TYPE_FILE_LIST`
  (`file_uri_deserializer`, `:834`). **[read]**
- `gdk/filetransferportal.c:537-546` (`application/vnd.portal.filetransfer` → `GDK_TYPE_FILE_LIST`
  and `G_TYPE_FILE`) and `:565-574` (`application/vnd.portal.files`, same), both via
  `portal_file_deserializer` (`:498`, `RetrieveFiles` at `:274`). **[read]**
- `gdk/gdkcontentdeserializer.c:986`: the portal deserializers are registered at startup only when
  the portal is reachable (`file_transfer_portal_register`, `filetransferportal.c:598-623`). **[read]**

## Not verifiable without a live session (for Claude's checks)

- Drag out into Zen from outside `~/Downloads` (portal path): expected to work, GTK offers
  `.filetransfer` + `.files` when the portal was up at `gtk_init` — Kyle's desktop has it (P0e
  review 1). Source files must still exist afterwards (we ignore `delete_data`).
- Drag out into Dolphin and Thunar: they read `text/uri-list`, so expected to work from anywhere.
- Spike-to-spike: expected to work (`uri-list` on both sides). The receiving spike copies in.
- Drop in from Dolphin: expected to copy; check the copy logs and the "(copy)" naming under a
  conflict.
- `set_icon`: `IconLookupFlags::NONE` needs the `v4_18` feature, which the spike has; the icon
  lookup itself is untested.

## Expected to fail / needs checking

- A dropped **symlink** is skipped ("not a regular file"): `standard::type` reports
  `SYMBOLIC_LINK` and Phase 0 found no capture covering symlinks (gap). Dragging a symlink *out*
  is untested on the wire too (GTK may or may not export it through the portal).
- A dropped folder logs "folders not supported in the spike" and is skipped (per brief).
- If the portal is down at startup, drag-out still works via `text/uri-list` inside `~/Downloads`
  but not outside (P0f/P0g pattern) — GTK decides, not us.
- `copy_name`'s `exists` check and the copy are not atomic; two rapid drops could race. Spike-only.
- The drop handler runs `query_file_type`/`query_exists` synchronously on the UI thread. Fine for
  a spike; Phase 1 should use `enumerate_children` once per drop instead.

## Non-obvious findings

- `gdk::Drop` has no `selected_action()` (that's `DragExt`); the drop's negotiated action comes via
  `DropTarget::current_drop().and_then(Drop::drag).selected_action()`
  (`gdk4-0.11.5:src/auto/drop.rs:50`, drag `selected_action` at `src/auto/drag.rs:102`). **[read]**
- `connect_prepare`'s closure must own its captures, so `selection` and `list_item` are cloned into
  it; `ListItem` is a refcounted GObject, so this is cheap. **[read]**
- rustfmt keeps `let ... else { return false }` multi-line, and the 100-width limit fights
  one-lining chains (`chain_width` = 60) — the module stays at 196 lines with that in mind.

## Outside my files

- None written. Note for the runner: `listing.rs` and `main.rs` were being edited by other agents
  while I worked; my smoke test ran against their current state and passed. I did not touch them.
- Suggested (not done, other agents' files): P1e's large-folder work could also replace the
  per-drop `query_exists` loop with a single `enumerate_children` (see above).

## Review (Claude, Nov 1, 2026)

**Verdict: accepted on the code. Live checks still to run** (drag into Zen from outside
`~/Downloads`, Dolphin, spike to spike, drop in from Dolphin).

Checked: only `dnd.rs` and this file changed; hook signatures match `P1b-contract.rs`; the four
RULES commands pass from a clean `cargo build` (rerun by me); 14 citations spot-checked against
the pinned crates and the GTK clone, all correct; `FileList` provider with no hand-added MIME;
`drag-end` only logs; drop-in copies with `FileCopyFlags::NONE` and the copy-name tests pass.

1. For Phase 2, not now: the extension rule "from the first non-leading dot" gives
   `report.v2.txt` → `report (copy).v2.txt`. Use the last dot, with a short list of double
   extensions (`.tar.gz`, `.tar.xz`, …) treated as one.
2. Agreed with the notes: the synchronous `query_file_type`/`query_exists` per dropped file moves
   off the UI thread in Phase 2.

### Live check (Claude, Nov 1, 2026): accepted

- **Into Zen** (P1g's runner, 8 rows): from outside `~/Downloads` and from inside it, single
  file, folder, symlink and two files all opened in Zen through the document portal
  (`RetrieveFiles`). Every source file was intact afterwards: `drag-end` ignored
  `delete_data` although Zen reported MOVE. Details in `NOTES-autodrag.md`, "Live calibration of
  P1g".
- **Spike to spike** (`/tmp/gimbal-spike-src` → `/tmp/gimbal-spike-dropin`, twice):
  `sample.png`, then `sample (copy).png`. The source logged `delete_data=false (ignored)`.
- **Dolphin to spike:** `sample (copy 2).png`. All three copies match the source's sha256.
- Minor, for Phase 2: the drop handler logs `action=DragAction(0x0)` on every drop, so the
  `current_drop()`/`drag()` lookup doesn't give the negotiated action there. It's only a log
  line now, but the real move/copy choice will need the action from the `DropTarget` itself.
