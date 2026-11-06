# P2aa: "Open With" in the right-click menu

## What this change does

- Adds `fm_core::openwith`, the pure decisions for the feature:
  - which applications the submenu shows, in order, without duplicates or
    hidden entries, capped at eight after the default;
  - which applications handle every selected content type;
  - the chooser's `Suggested` and `All Applications` sections, sorted by
    display name;
  - the search match over display name, desktop id and `Keywords`, ignoring
    case and accents;
  - the unknown, hidden and folder refusals.
- Adds `file.open-with app=<desktop id> paths=<selection>` and
  `file.set-default-app app=<desktop id> type=<content type>`, both
  `Effect::System`, so the agent socket and CLI refuse them before a handler.
- Adds `Open With` directly under `Open` for one or more selected files. It is
  absent for a folder and for a mix of files and folders.
- Adds the submenu: the default application first with `(default)`, then up to
  eight recommended applications, then `Other Application…`.
- Adds Gimbal's own chooser window: search, two sections, the opt-in
  `Always use for <description>` checkbox, and `Cancel` / `Open`.
- Launches with `gio::AppInfo::launch_uris`, the selected `file://` URIs and the
  window's GDK launch context. After a successful launch it records
  `set_as_last_used_for_type` for each content type, so GIO learns the choice
  without changing the default.
- Logs one `openwith:` line per launch with the desktop id, file count and
  content types.

No new dependency, config key or session schema field.

There is no `phase2/RULES.md`; I read `phase1/RULES.md`, the repository's
`CLAUDE.md` (which `phase2/README.md` names as its replacement) and the whole
P2aa brief. **[read]**

## Files written

- `crates/fm-core/src/openwith.rs` (new)
- `crates/fm-core/src/lib.rs`
- `crates/fm-core/src/command/catalogue.rs`
- `crates/fm-core/src/command/tests.rs`
- `crates/fm-ui/src/openwith.rs` (new)
- `crates/fm-ui/src/app.rs`
- `crates/fm-ui/src/fileops/menu.rs`
- `crates/fm-ui/src/look/css.rs`
- `docs/agent-socket.md`
- `docs/notes/P2aa.md` (this file)

No file outside the brief's Output list changed. In particular,
`crates/fm-ui/src/fileops/dialogs.rs` (the menu renderer) was not touched:
`Open With` is a normal registry-command row, and the handler opens the
submenu after that menu has closed, the same way the Settings submenu opens
from `header.rs`. `crates/fm-ui/src/main.rs` was not touched either: the new UI
module is declared with `#[path = "openwith.rs"] pub mod openwith;` inside
`app.rs`, next to the existing `update` module.

## Which applications

`crates/fm-core/src/openwith.rs` works on plain `AppEntry` values, so its tests
need no installed desktop entries.

The UI builds the entries from GIO:

- `gio::AppInfo::all()` supplies every installed application.
- `gio::AppInfo::all_for_type(type)` supplies the types each application
  handles.
- `gio::AppInfo::default_for_type(type, false)` supplies the default.
- `gio::AppInfo::recommended_for_type(type)` supplies GIO's recommended order.
- `gio::AppInfo::should_show()` hides entries.

`submenu_apps` puts a visible default first, then walks the recommended list in
GIO's order. It drops duplicates by desktop id, drops `should_show == false`,
drops applications that do not handle every selected type, and takes at most
eight after the default. An application that only handles one of two selected
types is therefore absent from the submenu. If nothing survives, the caller
still appends `Other Application…`, so the only row is that one.

The chooser uses `chooser_sections`: visible applications only, deduplicated by
id, sorted by folded display name with the id as tie-break. Applications that
handle every selected type go under `Suggested`; the rest go under
`All Applications`. **[read]**

The live query for one file is `standard::content-type` through GIO, with the
name's type as the empty-file fallback. The brief says "from the folder view's
file info"; the folder view already queries that attribute, but it does not
expose a selected item's `gio::FileInfo`, and `folder/view.rs` is outside this
brief's Output list. Querying the same attribute through `gio::File` is the
closest read-only equivalent. **[read]**

## Search

`search_matches` folds case and common Latin diacritics, then asks P2o's
matcher (`crate::search::SearchQuery`, cited below) about:

- the display name;
- the desktop id, and the id without `.desktop`;
- each `Keywords` value separately.

P2o's matcher ranks exact, prefix and substring matches before ordered-character
subsequences and bounded typos. It only lowercases, so the accent folding is
done first, here. **[read]**

`Keywords` is read from the desktop entry named by `AppInfo::id` through
`glib::KeyFile`, because `gio` 0.22.10 does not expose `GDesktopAppInfo` (P2y
found the same limit). **[read]**

## Commands

`file.open-with` has a `Text` argument named `app` and the selection's `paths`.
The `app` argument is optional: the item menu dispatches the command without an
application and the handler shows the submenu; the submenu and chooser dispatch
it with the chosen desktop id and launch. Keeping one command means the menu
row is still a named registry action, without a third UI-only command. The
documented `app=<desktop id>` form is the launch path and refuses an unknown id,
a hidden application and a folder. **[read]**

`file.set-default-app` requires both `app` and `type`. Only the chooser's
checkbox dispatches it. **[read]**

Both are `Effect::System`. The socket refuses every `System` effect in
`crates/fm-ui/src/agent/mod.rs:909` and `:1151` before dispatch, before any
handler, and the CLI talks to Gimbal through that socket. The handlers also
check `Caller::Ui` themselves. `file.open` keeps its `Effect::Launch` policy.
**[read]**

## Launching

`launch_with`:

1. Finds the desktop id in `AppInfo::all()`; an unknown id or a hidden
   application is refused with one summary line.
2. Refuses a folder in the selection: folders open in Gimbal.
3. Builds one `file://` URI per path with `gio::File::for_path(path).uri()`.
4. Calls `launch_uris` with `Display::app_launch_context()`, so niri receives
   the activation context.
5. After success, calls `set_as_last_used_for_type` for every content type.
6. Logs `openwith: launched <id> with <n> file(s), types <types>`.

Errors show exactly one line: `Could not open <name> with <App>: <error>`.
**[read]**

GIO converts the URIs to paths for applications whose `Exec` takes `%f`/`%F`.
This is GLib's own expansion, not something Gimbal does. I verified it with a
scratch desktop entry and the installed `gio launch`:

```text
%f  → /tmp/gimbal-p2aa/home/a.txt
%F  → /tmp/gimbal-p2aa/home/a.txt
      /tmp/gimbal-p2aa/home/b.txt
```

The probe's `Exec` was `/tmp/gimbal-p2aa/launch-test/record.sh %f` and `%F`.
**[observed]** The binding passes the URIs to GIO at
`gio-0.22.10:src/auto/app_info.rs:494`; GLib's GIR documentation for
`g_app_info_launch_uris` says it "passes the @uris to the launched application
as arguments" (`/usr/share/gir-1.0/Gio-2.0.gir:5185`). **[read]**

## The submenu

`right_click_item` remembers the grid widget whose item menu it opened.
`Open With` is a normal `MenuItem` with command `file.open-with` and only
`paths`; choosing it dispatches through the registry, the menu closes, and the
handler builds a second popover on the same anchor. This is the pattern the
Settings submenu already uses (`header.rs`'s `open_main_menu` schedules
`open_settings_menu` from an idle). **[read]**

Each row shows `AppInfo::icon()` through `gtk::Image::from_gicon`, which draws
through the icon theme, and the display name. The default row appends
`(default)`. `Other Application…` opens the chooser from an idle, after the
submenu's popup surface is gone. **[read]**

## The chooser

`crates/fm-ui/src/openwith.rs` builds a plain transient `gtk::Window` over the
Gimbal window (`modal`, `transient_for`, title `Open With`) with the shared
`gimbal-look` class, so it follows the live theme and the config's opacity.
Sizes come from the look tokens: the width is `SEARCH_W` scaled, the list's
maximum height is six `PIN_ROW_H` rows, and the new classes in `css.rs` use the
same spacing, text and radius tokens. No CSS is built outside the look module.
**[read]**

Parts, in order:

1. a search entry in a `gl-field`, focused after `present()`;
2. the `Suggested` / `All Applications` list, rebuilt on every search change,
   with the first matching application selected;
3. `Always use for <description>` from
   `gio::content_type_get_description`, off every time and hidden when the
   selection has more than one distinct type;
4. `Cancel` and `Open`.

Enter in the entry opens the selected row; double-clicking a row activates it;
Escape closes the window. Ticking the checkbox and pressing `Open` dispatches
`file.set-default-app` first, then `file.open-with`. If the default change
fails, the open still runs; a successful open then shows
`The default wasn't changed: <error>`. **[read]**

## API citations

- `gio-0.22.10:src/auto/app_info.rs:45` — `AppInfo::all`.
- `gio-0.22.10:src/auto/app_info.rs:51` — `AppInfo::all_for_type`.
- `gio-0.22.10:src/auto/app_info.rs:61` — `AppInfo::default_for_type`.
- `gio-0.22.10:src/auto/app_info.rs:238` — `AppInfo::recommended_for_type`.
- `gio-0.22.10:src/auto/app_info.rs:444` — `AppInfo::icon`.
- `gio-0.22.10:src/auto/app_info.rs:450` — `AppInfo::id`.
- `gio-0.22.10:src/auto/app_info.rs:456` — `AppInfo::name`.
- `gio-0.22.10:src/auto/app_info.rs:494` — `AppInfo::launch_uris`.
- `gio-0.22.10:src/auto/app_info.rs:556` — `AppInfo::set_as_default_for_type`.
- `gio-0.22.10:src/auto/app_info.rs:574` —
  `AppInfo::set_as_last_used_for_type`.
- `gio-0.22.10:src/auto/app_info.rs:592` — `AppInfo::should_show`.
- `gio-0.22.10:src/auto/functions.rs:144` —
  `gio::content_type_get_description`.
- `gio-0.22.10:src/content_type.rs:10` — `gio::content_type_guess`.
- `gtk4-0.11.5:src/auto/image.rs:62` — `Image::from_gicon`.
- `gtk4-0.11.5:src/auto/image.rs:186` — `Image::set_from_gicon`.
- `gtk4-0.11.5:src/auto/window.rs:817` — `Window::set_default_size`.
- `gtk4-0.11.5:src/auto/window.rs:935` — `Window::set_modal`.
- `gtk4-0.11.5:src/auto/window.rs:981` — `Window::set_transient_for`.
- `gtk4-0.11.5:src/auto/list_box.rs:140` — `ListBox::row_at_index`.
- `gtk4-0.11.5:src/auto/list_box.rs:157` — `ListBox::selected_row`.
- `gtk4-0.11.5:src/auto/list_box.rs:294` —
  `ListBox::set_activate_on_single_click`.
- `gtk4-0.11.5:src/auto/list_box.rs:521` —
  `ListBox::connect_row_activated`.
- `gtk4-0.11.5:src/event_controller_key.rs:11` —
  `EventControllerKey::connect_key_pressed`.
- `glib-0.22.10:src/key_file.rs:51` — `KeyFile::load_from_dirs`.
- `glib-0.22.10:src/auto/key_file.rs:197` — `KeyFile::value`.
- `fm-core/src/search/matcher.rs:88` — `SearchQuery::matches_name`.
- `fm-core/src/search/matcher.rs:127` — the exact/prefix/substring ranks.
- `fm-core/src/search/matcher.rs:185` — ordered-character subsequences.
- `fm-core/src/search/matcher.rs:261` — the tightest span.
- `fm-core/src/search/matcher.rs` (the bounded edit distance) — the typo path.

## Verification

`scripts/check.sh` passed on the final tree
(`/tmp/gimbal-p2aa-check-final.log`):

```text
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

The five steps are `cargo fmt --all --check`, clippy with `-D warnings`, the
workspace tests, the release build and `cargo deny check`. **[observed]**

Test totals: **908 passed, 23 ignored** (23 + 16 + 12 + 435 + 6 + 11 + 405
passing; 0 + 0 + 0 + 1 + 6 + 11 + 5 ignored). P2y's baseline was 892 passed /
23 ignored, so P2aa adds 16 passing tests: 13 in `fm-core/src/openwith.rs`, one
catalogue test and one command/caller test in `fm-core`, and one menu test in
`fm-ui`. **[observed]**

The new tests cover:

- default first, recommended order, duplicate ids, hidden applications and the
  cap of eight (with and without a default);
- two text types sharing an editor, an image and a text file with no common
  application, and one type;
- chooser sectioning and display-name order;
- search by display name, desktop id, keywords, case, accents and an empty
  query;
- the unknown, hidden and folder refusals;
- both catalogue entries, their argument shapes, their `Effect::System` rule,
  the UI caller rule and the missing-argument refusals;
- the menu entry's position under `Open`, its paths-only action, and its
  absence for a folder and a mix.

A debug build started on scratch `XDG_CONFIG_HOME`, `XDG_STATE_HOME`,
`XDG_DATA_HOME` and `HOME` under `/tmp/gimbal-p2aa`, opened one window at the
scratch home, and was stopped by PID with SIGTERM after four seconds. No CSS
parse warning appeared. **[observed]**

No pointer or keyboard interaction was possible for this agent, so the menu,
submenu and chooser are Claude's live checks below.

## What Claude checks live

Scratch dirs and a scratch MIME default keep the default-change check away from
Kyle's real `~/.config/mimeapps.list`:

```bash
SCRATCH=/tmp/gimbal-p2aa-live
mkdir -p "$SCRATCH/config" "$SCRATCH/state" "$SCRATCH/data" "$SCRATCH/home"
printf '[Default Applications]\ntext/plain=org.kde.kate.desktop\n' \
  > "$SCRATCH/config/mimeapps.list"
sha256sum ~/.config/mimeapps.list > "$SCRATCH/real-before.sha256"
XDG_CONFIG_HOME="$SCRATCH/config" \
XDG_STATE_HOME="$SCRATCH/state" \
XDG_DATA_HOME="$SCRATCH/data" \
HOME="$SCRATCH/home" \
  ./target/debug/gimbal-files "$SCRATCH/home"
```

Use a `.txt` and a `.png` in `$SCRATCH/home`.

1. **Menu.** Right-click the `.txt`: `Open With` is directly under `Open`.
   It lists `Kate (default)` first, then the other text applications, then
   `Other Application…`. Choose one: it opens with the file.
2. **Chooser.** `Other Application…` opens `Open With`. Typing `kate` leaves
   Kate in the list; Enter opens the file in Kate. Escape closes; Cancel
   closes.
3. **Default.** Tick `Always use for Plain text document`, choose another
   editor and press `Open`. The app opens; `$SCRATCH/config/mimeapps.list` now
   names it for `text/plain`; `sha256sum -c` on
   `$SCRATCH/real-before.sha256` still passes. Then make the default write fail
   (for example, `chmod 0500` the scratch `mimeapps.list`): the app still opens
   and the status line says the default wasn't changed.
4. **Mixed types.** Select the `.txt` and the `.png`: the submenu shows only
   `Other Application…`, unless an installed application handles both.
5. **Folders.** `Open With` is absent for a folder and for a mix of a file and
   a folder.
6. **Automation.** With the socket on,
   `gimbal-files-cli run --window <id> file.open-with
   app=org.kde.kate.desktop paths=$SCRATCH/home/a.txt` returns `code =
   "unsupported"` and `system operations are not available through the local
   socket`. The same for `file.set-default-app`.
7. **Look.** Check the chooser in light and dark, tiled and floating. The
   list, headings, icons and checkbox follow the theme; the window background
   follows the configured opacity; Escape and Enter still work.

## Outside the Output list

- No file outside the brief's Output list changed.
- The brief asks for the one-file content type "from the folder view's file
  info". The folder view keeps that `gio::FileInfo` private and is not in this
  brief's Output list, so the handler asks GIO for `standard::content-type`
  directly. If the file info is later exposed, this is the one call to switch.
- The item menu's `Open With` row dispatches `file.open-with` without `app` so
  the submenu needs no third command or a menu-renderer change. The catalogue's
  `app` is therefore optional; the launch form still always carries it.
- The chooser is a plain `gtk::Window`, not a `gtk::Dialog`, so it can use the
  shared `gimbal-look` window class without GTK's deprecated
  `AppChooserDialog`/`AppChooserWidget` (deprecated since GTK 4.10,
  `gtk4-0.11.5:src/auto/app_chooser_dialog.rs:43`).

## Claude's review (Nov 6, 2026)

**Code.** The return matches the brief. Querying `standard::content-type`
through `gio::File` instead of the folder view's private `FileInfo`, and the
optional `app` argument that lets the menu row open the submenu, are both
reasonable and documented above. **[read]**

`set_as_last_used_for_type` makes GIO write a last-used order into the user's
`mimeapps.list` (`[Added Associations]`) after every Open With. That reorders
suggestions but never changes the default. The brief asked for it. **[observed]**

**Fix: plain Open on an empty file (`crates/fm-ui/src/app.rs` `launch_file`,
`crates/fm-core/src/admin.rs`).** `launch_default_for_uri` finds no handler for
`application/x-zerosize`, the type GIO gives every empty file, so Open and
double-click did nothing on an empty file (found in the P2y review). Kyle's
call: an empty file should still open. `launch_file` now picks the default app
for the name's type when the queried type is `fm_core::admin::EMPTY_FILE_TYPE`,
and launches it with `launch_uris`. `lookup_type` uses the same constant. Live
check: a scratch `%f` "recorder" app as the `text/plain` default received
`/tmp/gimbal-empty/home/empty.txt` from `file.open` sent over the agent socket.
**[observed]**

**Gates.** `scripts/check.sh`: 908 passed, 23 ignored. **[observed]**

### Live checks (Claude and Kyle, Nov 6)

Debug build on scratch XDG config/state/cache (`/tmp/gimbal-p2aa-live`), with a
scratch `mimeapps.list` that makes Kate the `text/plain` default. Kyle did the
clicks: the item menu opens from a right-click gesture AT-SPI can't trigger.

1. **Menu: pass (Kyle).** `Open With` sits between `Open` and `Open as
   Administrator`; Kate is first with `(default)`; choosing an app opens the
   file (`openwith: launched org.kde.kate.desktop`, then OnlyOffice).
2. **Chooser: pass (Kyle).** Search, Enter, Escape and Cancel.
3. **Default: pass.** `Always use for Plain text document` set OnlyOffice, then
   Kate again (`openwith: default for text/plain is now …`), in the scratch
   `mimeapps.list` only. `sha256sum -c` on Kyle's real `~/.config/mimeapps.list`
   still passes. **[observed]**
4. **Mixed types: pass (Kyle).** `.txt` + `.png` shows only `Other
   Application…`.
5. **Folders: pass (Kyle).** No `Open With` for a folder or a file + folder mix.
6. **Automation: pass (Claude).** `file.open-with` and `file.set-default-app`
   over the socket both return `code = "unsupported"`, `system operations are
   not available through the local socket`, and nothing launches.
7. **Look: pass (Kyle).**
8. **Empty file: pass (Kyle).** Double-clicking `empty.txt` opened it in Kate
   (`open: launched …/empty.txt`).
