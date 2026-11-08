# P2af: a Properties window, and New Tab / New Window / Settings in the empty-space menu

## What I built

- `fm_core::properties`, the pure parts:
  - permission words (`Read and write`, `Read only`, `None`), the
    file-only `Executable` fact and the four-digit octal mode for 0644, 0600,
    0755, 0700, 0444 and 0000;
  - the multi-item count (`N items: X files, Y folders`), the type tally
    (`3 PNG images, 1 PDF document`) and the common parent;
  - the local date/time with `Unknown` for a missing attribute;
  - the file size text, the folder-size note and the
    `Some items couldn't be read` note.
- `fm_core::command`:
  - `file.properties paths=<selection>`, `Effect::System`, read-only and
    UI/hotkey only;
  - `app.settings`, `Effect::System`, no arguments, UI/hotkey only;
  - the default `<Alt>Return` hotkey in `default_hotkeys`.
- `crates/fm-ui/src/properties.rs`, a transient `gtk::Window` per invocation
  (several can be open at once), with the shared `gimbal-look` class:
  - one item: icon and name, Type, Size, Location link, Link target, Created,
    Modified, Accessed, Owner, Group, permissions, Mode, Opens with/Change…,
    Disk/Disk Usage;
  - several items: count, combined size, common parent and type tally;
  - folder totals use the existing `fm_core::folder_size::scan_with_root`
    walker, with `Calculating…` while it runs. An administrator tab uses the
    same walker with a GIO `admin://` `ScanSource`, so no second recursive
    walker exists;
  - GIO reads every fact through `fm_core::admin::gfile(path, scope)`, so an
    administrator tab sees root-only entries;
  - every value label names its row in its accessible label (`Size: 4.9 KB`),
    and the close, change and disk-usage buttons have accessible names.
- The item menu has `Disk Usage` for exactly one folder and `Properties` as
  its last entry. Both Disk Usage entries stay visible when Baobab is missing,
  disabled with `Install baobab to see what uses the space`.
- The empty-space menu is now: Open as Administrator (when it applies), New
  Tab, New Window, separator, Undo (when it applies), New Folder…, Paste,
  separator, Pin This Folder, Refresh, Disk Usage, Properties, separator,
  Settings…. `MenuItem` gained a separator kind, the renderer draws it with
  the look module's `gl-rule inset`, and disabled entries carry their tooltip.
- `tab.new` in an administrator tab immediately turns the new tab back into a
  normal tab in place; `window.new` already creates a normal window.
- `app.settings` opens the main menu's Settings submenu, anchored on the main
  menu button, from an idle after the background popover closes.
- `org.freedesktop.FileManager1`'s `ShowItemProperties` opens the Properties
  window for the accepted paths. The log line is now
  `opening the Properties window for the item`. A bus-started forwarding
  process sends the reserved `gimbal:properties` open hint, which the primary
  turns into the same window.
- `dnd`'s window active/inactive and prepare-modifiers lines are
  `log_verbose!`; drops and drag-ends remain everyday lines.
- `docs/agent-socket.md` lists both new commands as unavailable through the
  socket.

No new dependency and no session schema change.

There is no `phase2/RULES.md`; I read `phase1/RULES.md`, the repository's
`CLAUDE.md` (which `phase2/README.md` names as its replacement) and the whole
P2af brief. **[read]**

## Files written

- `crates/fm-core/src/properties.rs` (new)
- `crates/fm-core/src/lib.rs`
- `crates/fm-core/src/command/catalogue.rs`
- `crates/fm-core/src/command/tests.rs`
- `crates/fm-core/src/config.rs` (the default hotkey and its existing test)
- `crates/fm-ui/src/properties.rs` (new)
- `crates/fm-ui/src/fileops/menu.rs`
- `crates/fm-ui/src/fileops/dialogs.rs` (separator and tooltip rendering)
- `crates/fm-ui/src/app.rs`
- `crates/fm-ui/src/header.rs` (`Header::open_settings` for the command)
- `crates/fm-ui/src/filemanager1.rs` (`ShowItemProperties` only)
- `crates/fm-ui/src/dnd.rs` (only item 6)
- `crates/fm-ui/src/look/css.rs` (only the new Properties classes)
- `docs/agent-socket.md`
- `docs/notes/P2af.md` (this file)

No file outside the brief's Output list changed. `crates/fm-ui/src/main.rs` was
not touched: the new UI module is declared with
`#[path = "properties.rs"] pub mod properties;` inside `app.rs`, next to the
existing `update` and `openwith` modules. **[read]**

## The Properties window

`crates/fm-ui/src/properties.rs` is built like P2aa's chooser: a plain
`gtk::Window` over the Gimbal window, `gimbal-look`, token sizes and Escape to
close. It is not modal, so one Properties window per invocation can coexist.
`window.connect_destroy` stops the folder-size poll when the window goes away.
**[read]**

`Facts::read` queries GIO twice: once following the link for the target's type,
size and dates, and once with `NOFOLLOW_SYMLINKS` for the link itself. A broken
link has no followed result, so the link's own facts stand in and the Link
target row appends `(broken link)`. **[read]**

The content type uses `standard::content-type`, then
`fm_core::admin::lookup_type` with `gio::content_type_guess` by name. That is
P2aa's empty-file rule, so an empty `a.txt` reports `text/plain` rather than
`application/x-zerosize`. **[read]**

Size text is `fm_core::preview::format_size` plus the exact byte count for one
file. A folder starts at `Calculating…`, then shows the walker's contents total
and `FolderTotals::contains_text()`, and appends
`Some items couldn't be read` for a `PermissionDenied` or `Unreadable` partial
reason. Several items add the file sizes immediately and each folder's
`contents_bytes` as its walk finishes. **[read]**

Every row is a `gtk::Grid` with a visible name label and a value label. The
value gets `gtk::accessible::Property::Label("<row>: <value>")`, so a screen
reader reads `Size: 4.8 KiB (4904 bytes)`, not just the number. `Opens with`,
`Disk`, `Location` and the buttons use the same rule. **[read]**

### Administrator tabs

`file.properties` picks `Scope::Admin` when the focused tab is an
administrator tab, and every GIO query then goes through
`fm_core::admin::gfile`. For a folder's recursive total, `AdminSource`
implements the existing walker's `ScanSource` with GVfs `admin://` enumeration
and metadata; `scan_with_root` does all the walking. This is the same walker,
not a second implementation. **[read]** **[inferred]** for a live GVfs mount.

### Disk and applications

Disk rows ask `filesystem::free` and `filesystem::size` through
`query_filesystem_info`. The Disk Usage button is only on a folder and
dispatches `app.disk-usage` with that path; when `find_on_path("baobab", PATH)`
fails it is insensitive with `Install baobab to see what uses the space`.
`Change…` calls P2aa's `openwith::show_chooser` for the one file. **[read]**

## The menus

`MenuItem` now has a `separator` flag and a `tooltip`. A separator carries no
command; the renderer appends a `gtk::Box` with `gl-rule inset`, so the rule is
still styled only by the look module. Disabled entries set their tooltip.
**[read]**

The item menu's order ends with `Disk Usage` (only for exactly one folder) and
then `Properties`, so Properties is always last. The empty-space menu follows
the brief's three sections exactly. `New Tab` and `New Window` are ordinary
`tab.new` / `window.new` calls in both normal and administrator tabs; the menu
never names an administrator command for them. Inside an administrator tab the
`tab.new` handler opens the tab and then calls `tab_leave_admin` in place, so it
is a normal tab before the UI applies the events. `window.new` already built a
normal `Tab`. **[read]** **[inferred]** for the absence of a visible flash.

`app.settings` is dispatched from `Settings…`; the handler schedules
`ui.header().open_settings()` from an idle, so the background popover is gone
before the Settings popover is parented to the main-menu button. **[read]**

## Commands and caller rules

Both new commands are `Effect::System`, the same rule P2x and P2aa used. The
agent socket refuses every `System` effect before dispatch
(`crates/fm-ui/src/agent/mod.rs:909` and `:1152`), and the CLI talks through
that socket. The UI and a configured hotkey reach `file.properties`; no
default hotkey is bound to `app.settings`. **[read]**

`file.properties` keeps `paths` as `Presence::Context(Selection)` so the item
menu and the hotkey can leave it for the handler, which falls back to the
window's selection. `app.settings` has no arguments. **[read]**

## FileManager1

`ShowItemProperties` still decodes the same accepted paths, but its handler now
calls `Process::open_properties` instead of `open_arguments_with_startup`.
The log line is `opening the Properties window for the item`. A bus-started
forwarding process uses the reserved `gimbal:properties` open hint; the primary
intercepts that hint in `open_arguments_with_startup` and shows the same
window. The call still answers before any GTK work. **[read]**

## API citations

- `gio-0.22.10:src/auto/file_info.rs:48` — `access_date_time`.
- `gio-0.22.10:src/auto/file_info.rs:144` — `attribute_string`.
- `gio-0.22.10:src/auto/file_info.rs:166` / `:174` — `attribute_uint32` /
  `attribute_uint64`.
- `gio-0.22.10:src/auto/file_info.rs:190` / `:256` — `creation_date_time` /
  `modification_date_time`.
- `gio-0.22.10:src/auto/file_info.rs:224` — `file_type`.
- `gio-0.22.10:src/auto/file_info.rs:230` — `icon`.
- `gio-0.22.10:src/auto/file_info.rs:248` / `:290` — `is_symlink` /
  `symlink_target`.
- `gio-0.22.10:src/auto/file.rs:649` — `File::enumerate_children`.
- `gio-0.22.10:src/auto/file.rs:1655` — `query_filesystem_info`.
- `gio-0.22.10:src/auto/file.rs:1756` — `query_info`.
- `gio-0.22.10:src/auto/functions.rs:144` —
  `gio::content_type_get_description`.
- `gio-0.22.10:src/content_type.rs:10` — `gio::content_type_guess`.
- `gio-0.22.10:src/file_enumerator.rs:9` — `FileEnumerator` yields
  `Result<FileInfo, glib::Error>`.
- `glib-0.22.10:src/auto/date_time.rs:73` / `:302` — `from_unix_local` /
  `format`.
- `gtk4-0.11.5:src/accessible.rs:31` — `update_property`.
- `gtk4-0.11.5:src/accessible.rs:100` — `accessible::Property`.
- `gtk4-0.11.5:src/auto/image.rs:62` — `Image::from_gicon`.
- `gtk4-0.11.5:src/auto/link_button.rs:52` / `:101` —
  `LinkButton::with_label` / `connect_activate_link`.
- `gtk4-0.11.5:src/auto/widget.rs:1574` — `connect_destroy`.
- `fm-core/src/folder_size/walk.rs:198` — `scan_with_root`.
- `fm-core/src/folder_size/walk.rs:231` — `scan`.
- `fm-core/src/preview.rs:2887` — `format_size`.

## Verification

`scripts/check.sh` passed on the final tree
(`/tmp/gimbal-p2af-check-final.log`):

```text
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

The five steps are `cargo fmt --all --check`, clippy with `-D warnings`, the
workspace tests, the release build and `cargo deny check`. **[observed]**

Test totals: **974 passed, 23 ignored, 0 failed** (23 + 16 + 13 + 480 + 6 + 11
+ 425 passing; 1 + 6 + 11 + 5 ignored). P2ae's baseline was 960 passed /
23 ignored, so P2af adds 14 passing tests: 8 in `fm-core/src/properties.rs`,
one catalogue test, one command/caller test, and four menu tests in `fm-ui`.
**[observed]**

The new tests cover:

- permission words for 0644, 0600, 0755, 0700, 0444 and 0000, for a file and a
  folder;
- the multi-item count for files only, folders only and mixed, and the type
  tally with plurals;
- the common parent, the missing-time `Unknown` fallback, the exact size text
  and the unreadable folder note;
- the empty-space menu's order and three separators, New Tab/New Window inside
  an administrator tab, and a missing Baobab's disabled Disk Usage with its
  tooltip;
- Properties last in every item selection, and Disk Usage for exactly one
  folder;
- both commands' catalogue rows, arguments, `Effect::System` rule and caller
  rules;
- the existing FileManager1 plan test now names the Properties window.

A debug build ran on scratch `XDG_CONFIG_HOME`, `XDG_STATE_HOME`,
`XDG_DATA_HOME` and `HOME` under `/tmp/gimbal-p2af`, with
`[integration] show_in_folder = true`. `gdbus` `ShowItemProperties` on
`a.txt` and on `folder` each opened an additional Properties window
(`properties: opened for 1 item(s) in the user scope`). The startup log shows
`hotkey: bound "<Alt>Return" to file.properties`. No CSS parser warning
appeared, and the process was stopped by PID with SIGTERM. **[observed]**

A second three-second run without `GIMBAL_LOG=1` wrote no
`dnd: window active` or `dnd: window inactive` line to stderr or to
`gimbal.log` (`grep -c` was `0` in both). **[observed]**

## What Claude and Kyle check live

Debug build and scratch XDG dirs:

```bash
SCRATCH=/tmp/gimbal-p2af-live
mkdir -p "$SCRATCH/config/gimbal-files" "$SCRATCH/state" "$SCRATCH/data" "$SCRATCH/home"
printf 'hello\n' > "$SCRATCH/home/a.txt"
mkdir -p "$SCRATCH/home/folder"
ln -s "$SCRATCH/home/a.txt" "$SCRATCH/home/link.txt"
ln -s "$SCRATCH/home/missing.txt" "$SCRATCH/home/broken.txt"
XDG_CONFIG_HOME="$SCRATCH/config" XDG_STATE_HOME="$SCRATCH/state" \
XDG_DATA_HOME="$SCRATCH/data" HOME="$SCRATCH/home" \
  ./target/debug/gimbal-files "$SCRATCH/home"
```

1. Properties on `a.txt`: type, human and exact size, Location link, dates,
   owner, `0644` with words, Opens with plus **Change…** (P2aa's chooser opens).
2. Properties on `folder`: `Calculating…`, then the total size and item count.
   **Disk Usage** opens Baobab on that folder.
3. Properties on three mixed items: the count, combined size and type tally.
4. Properties on `link.txt`, and on `broken.txt`: the target, and `(broken
   link)` for the broken one.
5. `<Alt>Return` opens Properties for the selection.
6. In an administrator tab: Properties on a root-only file shows its details.
7. The empty-space menu: New Tab and New Window open at this folder, Settings…
   opens Settings, and the separators look right in light and dark.
8. `gdbus` `ShowItemProperties` on a file opens its Properties window.
9. The log no longer has `dnd: window active/inactive` lines without
   `GIMBAL_LOG=1`.

## Outside the Output list

- No file outside the brief's Output list changed.
- `crates/fm-ui/src/main.rs` was not touched; `properties` is declared inside
  `app.rs` with `#[path = "properties.rs"]`, as P2aa did for `openwith`.
- `crates/fm-ui/src/filemanager1.rs` also gained the reserved
  `gimbal:properties` forwarding hint. That is still the `ShowItemProperties`
  path, not a new FileManager1 method or protocol.
- `crates/fm-core/src/config.rs`'s existing default-hotkey test now expects 19
  hotkeys and includes `<Alt>Return`.
- The administrator folder total reuses `scan_with_root` through a GIO
  `admin://` `ScanSource`. If GVfs's admin backend does not answer an
  enumeration, the window shows `Unavailable` rather than falling back to a
  user-path walk.

## Known limits

- The Properties window is read-only, as the brief asks; there is no rename or
  permission change control.
- The Disk row shows only free and total space, not per-mount used/percent.
- The agent socket refuses both commands as `System`, so scripts cannot open a
  Properties window or Settings.
- A folder in an administrator tab walks through GVfs and can be slow; the
  window shows `Calculating…` until the existing 5-second walker budget
  finishes.
- No pointer or keyboard interaction was possible for this agent. The menu
  clicks, the chooser, and the visual look are Claude and Kyle's live checks
  above.

## ChatGPT's review (Nov 8, 2026)

### New Folder and Rename

- **[observed]** The installed 0.1.3 log recorded the popup watchdog at
  15:37:36 after Kyle chose **New Folder…**. Kyle reported that clicks stayed
  locked after the watchdog fired.
- **[read]** New Folder and Rename used to open a second popover inside the
  context menu's click handler. The menu now closes before an idle opens the
  name form or the Copy To / Move To chooser. **[inferred]** Wayland could
  reject the second grab while the menu popup was still being torn down.
- **[read]** A Wayland seat grab starts during popup presentation. If
  presentation fails before the widget maps, GTK's popdown skips the widget
  unmap handler. The watchdog now hides the native surface explicitly, which
  runs Wayland GDK's seat ungrab, then pops the widget down so its close
  handlers run. See GTK 4.22.4 `gtk/gtkpopover.c:1152` and `:1226`,
  `gdk/wayland/gdksurface-wayland.c:1146`, and gdk4 0.11.5
  `src/auto/surface.rs:203`.
- **[observed]** The first scratch run stayed responsive and logged a
  successful Rename, but Kyle saw no name form after choosing New Folder.
  The scratch log had no `file.new-folder` command.
- **[read]** New Folder now reuses the context menu's click rectangle. P2k
  had already fixed the same placement issue in the empty-space menu. See
  gtk4 0.11.5 `src/auto/popover.rs:514` for `set_pointing_to`.
- **[observed]** The isolated GTK regression activated New Folder through the
  menu, checked the form's pointing rectangle, and verified that Create made
  the directory on disk. Kyle then confirmed the rebuilt debug version
  worked in the scratch click-through. **[read]** Verification of the
  compositor-refusal cleanup was limited to reading the GTK and GDK source.

### Properties and administrator tabs

- **[read]** The folder grid's capture controller consumed Return with any
  modifier before the configurable window shortcut could see it. The grid
  now handles unmodified Return and keypad Enter and passes modified Enter
  onward. The unit test checks Alt, Ctrl, Shift, and Caps Lock. See
  GTK 4.22.4 `gtk/gtkshortcutmanager.c:45` for capture and bubble routing.
  **[observed]** Kyle confirmed Alt+Enter opens Properties.
- **[read]** The footer Close button is gone. Escape and the title-bar close
  remain. Disk Usage uses the shared `gl-icon-button` style and retains its
  accessible name and the missing-Baobab tooltip. Folder Properties uses
  `look::icons::image(MockupIcon::Folder)` with the grid's symbolic
  palette. **[observed]** Kyle confirmed the button opened Baobab on the
  selected folder.
- **[read]** Claude's broken-link fix was already in the tree. GIO can return
  the symlink's own metadata when the target is missing. The check therefore
  looks for a followed result that is still typed `SymbolicLink`, or a
  missing followed result. Properties then shows `(broken link)` and
  `Permissions: Not applicable to a broken link`.
- **[observed]** Kyle approved the existing `tab.new` behavior. New Tab and
  Ctrl+T from an administrator tab open a normal tab.

### Verification

- **[observed]** All three gates exited 0. `scripts/check.sh` ended with
  `all checks passed` (976 passed, 24 ignored, 0 failed). The ignored
  New Folder child test runs through its passing subprocess wrapper.
  `scripts/test-install.sh` passed install, reinstall and uninstall.
  `scripts/test-deb.sh` passed the byte-identical rebuild, signed scratch
  repository and tampered-index rejection.
- **[observed]** On the first review build, a scratch Wayland run opened four
  Properties windows through D-Bus `ShowItemProperties`. AT-SPI read the
  file, folder, symlink and broken-link headings, the folder's calculated
  size, the Disk Usage button and the broken-link details. The footer Close
  button was absent and the default log had no drag-and-drop window focus
  chatter. SIGTERM stopped the scratch process by PID.
- **[read]** The lock-up and hotkey fixes extended the original Output list
  to `crates/fm-ui/src/popup.rs` and `crates/fm-ui/src/folder/view.rs`.
  The regression for the menu-to-worker path lives in `app.rs`.
