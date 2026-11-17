# P2ar — files without a thumbnail show their default app's icon

## What I built

A new `crates/fm-ui/src/file_icon.rs` decides the icon for a file:

1. a folder gives `Folder` (the mockup folder SVG);
2. otherwise the default app for the file's content type gives `App(gio::Icon)`;
3. otherwise `Placeholder`, drawn as `text-x-generic`.

The lookup is `gio::AppInfo::default_for_type(content_type, false).and_then(|app| app.icon())`,
the same call Open uses (`app.rs`), so a tile and a double-click agree. `info.icon()` (GIO's
MIME-theme icon) is no longer used for any file type. Folders, thumbnails, sizes and Open are
unchanged.

The one decision function is pure and testable with an injected lookup:

```rust
pub fn decide<F>(is_folder: bool, content_type: Option<&str>, lookup: F) -> FileIcon
```

`for_file_info` uses the listing's existing `standard::content-type`; `for_content_type` takes a
type directly (Properties); `for_name` guesses from a name alone (Trash).

## Icon sites, before and after

| Site | Before | After |
| --- | --- | --- |
| Grid cell, `folder/view.rs:3173` (`show_type_icon`) | folder SVG; else `info.icon()`; else `text-x-generic` | folder SVG; else default app's `gio::Icon`; else `text-x-generic` |
| Drag icon, `folder/view.rs:1816-1862` (`setup_cell_drag_source`) | `info.icon()` of the first dragged file | `file_icon::for_file_info` of the first dragged file; a folder still uses `info.icon()` because `dnd::drag_icon` takes a `gio::Icon` and folders already drew GIO's folder icon |
| Trash row, `trash.rs:973-985` | trash item's `icon_name` (GIO's MIME icon); else folder SVG; else `text-x-generic` | folder SVG; else `file_icon::for_name(display_name)`; else `text-x-generic` |
| Properties header, `properties.rs:170-183` | folder SVG; else `fact.icon` (`info.icon()`); else `text-x-generic-symbolic` | folder SVG; else `file_icon::for_content_type(fact.content_type)`; else `text-x-generic-symbolic` |
| Search results, `search/view.rs:543-550` | small symbolic kind icon by design | **unchanged, on purpose** |

The file chooser uses the same `FolderView`, so it follows the grid automatically. The Trash view
has no content type, so it guesses from the name with `gio::content_type_guess(Some(name), None)`
— name only, no read and no sniff. The returned `uncertain` flag is deliberately ignored: a
confident-looking or uncertain guess goes through the same default-app lookup.

Because the Trash view no longer reads `TrashItem::icon_name()`, `fm-core`'s accessor is now unused
by the UI. I left `fm-core` alone (outside this brief's output list); the public accessor is still
valid and its removal can be a later cleanup.

## Cache and invalidation

- One `thread_local!` cache in `file_icon.rs` maps `content type -> Option<gio::Icon>`, so `None`
  answers are cached too. `gio::Icon` is not `Send`, which is why this is on the GTK main thread
  like `look/icons.rs`'s `CACHE`. A folder of 5,000 files of one type costs one lookup.
- The hit check borrows the cache only for the clone; the injected lookup runs with no borrow
  held, so a test lookup may call `clear_cache()` without a `RefCell` panic.
- `gio::AppInfoMonitor::get()` is obtained once and kept in a second `thread_local!` slot for the
  process lifetime. Its `changed` signal clears the cache, logs one line, then calls every
  registered view listener. The default-app changes from Open With's "Set as default" and from
  edits to `mimeapps.list` reach this path through GIO.
- GIO emits `changed` only once until a `g_app_info_*()` call re-arms it
  (`/usr/share/gir-1.0/Gio-2.0.gir:6295-6320`). The refresh below calls
  `AppInfo::default_for_type` for each redrawn type, which is one of the listed re-arming calls.
  If a change arrives while no file icon is on screen, no lookup happens and no stale icon is
  shown either; the next bind does a fresh lookup because the cache was already cleared.
- `FolderView` registers a listener in `new` (`folder/view.rs:381-390`) and disconnects it in its
  existing `Drop` (`folder/view.rs:3070`). On a change,
  `FolderView::refresh_app_icons` (`folder/view.rs:2713`) redraws the type icon of every bound cell
  that is currently drawn from an app icon or the placeholder. It skips cells whose image holds a
  `GdkPaintable`, so a loaded thumbnail and the mockup folder SVG stay. No listing reload and no
  rebind happen.
- If a platform change bypasses `GAppInfoMonitor`, the next rebind, scroll or F5 still picks up the
  new answer; the cache-clear path itself never reloads the listing.

Logging (only with `GIMBAL_LOG=1`, via `log_verbose!`): the first lookup of each content type
prints one line, e.g.

```
[gimbal 1.234] icons: text/x-python -> org.gnome.TextEditor
[gimbal 1.234] icons: application/x-no-app -> placeholder
```

The desktop-file suffix `.desktop` is trimmed for readability. A cache clear prints
`icons: default-app icon cache cleared`. Nothing is logged per cell.

## Thumbnails

An image, PDF or video still shows its thumbnail on top when it loads. Before it arrives the cell
now shows the default app's icon (for example the image viewer's) instead of the MIME icon, and
keeps it if the thumbnail fails. The recycled-cell reset at the top of `connect_bind`
(`show_type_icon` before the thumbnail request) is unchanged and still runs first.

## Tests

`crates/fm-ui/src/file_icon.rs` has six unit tests:

- a folder is `Folder` even when the lookup would panic;
- a returned icon is `App`;
- no icon is `Placeholder`;
- no content type, or an empty one, is `Placeholder` without calling the lookup;
- the cache runs the lookup once for two calls of one type, caches a `None`, and runs the lookup
  again after a clear;
- the same for an icon answer.

No test asserts which real application this machine has as a default, and no test changes a
default app, `mimeapps.list` or anything under `~/.config`. The existing grid, trash, thumbnail
and Properties tests still pass.

## API citations

Verified against the pinned crate sources [read]:

- `gio-0.22.10:src/auto/app_info.rs:61` — `gio::AppInfo::default_for_type(content_type: &str, must_support_uris: bool) -> Option<AppInfo>` (alias `g_app_info_get_default_for_type`).
- `gio-0.22.10:src/auto/app_info.rs:444` — `AppInfoExt::icon() -> Option<gio::Icon>` (alias `g_app_info_get_icon`).
- `gio-0.22.10:src/auto/app_info.rs:450` — `AppInfoExt::id() -> Option<glib::GString>`.
- `gio-0.22.10:src/auto/app_info.rs:456` — `AppInfoExt::name() -> glib::GString`.
- `gio-0.22.10:src/auto/app_info_monitor.rs:25` — `AppInfoMonitor::get() -> AppInfoMonitor` (alias `g_app_info_monitor_get`).
- `gio-0.22.10:src/auto/app_info_monitor.rs:29` — `connect_changed<F: Fn(&Self) + 'static>(&self, f: F) -> SignalHandlerId`, the `changed` signal.
- `gio-0.22.10:src/content_type.rs:10` — `content_type_guess(filename: Option<impl AsRef<Path>>, data: Option<&[u8]>) -> (glib::GString, bool)` (alias `g_content_type_guess`); the second value is `uncertain`.
- `/usr/share/gir-1.0/Gio-2.0.gir:6290-6320` — `GAppInfoMonitor` watches the app info database; `changed` is emitted once until a `g_app_info_*()` call (`get_all`, `get_default_for_type`, …) re-arms it.
- `gtk4-0.11.5:src/auto/image.rs:157,188,206` — `GtkImage::paintable()`, `set_from_gicon()`, `set_paintable()`.
- `gtk-4.22.4:gtk/gtkimage.c:798-812` — `gtk_image_set_from_gicon` calls `gtk_image_clear` first, so a recycled cell cannot keep the previous storage. The icon-name and paintable setters work the same way, which is why `show_type_icon` starts by clearing the paintable.

## Gates

From `$PROJ`, `./scripts/check.sh` passed on the final run:

```
cargo fmt --all --check                                  pass
cargo clippy --workspace --all-targets -- -D warnings    pass
cargo test --workspace                                   pass
cargo build --workspace --release                        pass
cargo deny check                                         pass
```

Test counts from that run:

- `fm-agent` lib: 23 passed
- `fm-agent` `tests/socket.rs`: 16 passed
- `fm-cli`: 13 passed
- `fm-core` lib: 515 passed, 1 ignored
- `fm-core` `tests/trash.rs`: 6 passed, 6 ignored
- `fm-core` `tests/trash_browser.rs`: 11 passed, 11 ignored
- `fm-ui`: 506 passed, 9 ignored (includes the 6 new `file_icon` tests)

Total: 1,090 passed, 0 failed, 27 ignored.

Note [observed]: an earlier `check.sh` run failed
`look::css::tests::header_fits_stretches_then_collapses` at `look/css.rs:837` while unrelated
files in the tree (`header.rs`, `look/css.rs`, `look/icons.rs`, one new SVG) were being saved by
another task. That test passed standalone immediately afterwards and on the next full run. None of
my files touch that code.

## Live checks (Claude)

Kyle's Gimbal is running (`pgrep -a gimbal-files` showed pid 1482720), so I did not launch it.
[observed]

1. Scratch folder under `/tmp/gimbal-*` with `a.txt`, `a.py`, `a.zip`, `a.odt`, `a.svg`, `a.blend`
   or `a.kra`, and a file with no extension. Each tile shows its default app's icon; a type with
   no default app shows the placeholder (`text-x-generic`). Expected [inferred].
2. An image and a PDF: the thumbnail still replaces the icon when it loads. Before it loads the
   default app's icon is visible; a failed thumbnail keeps that icon.
3. Open With → "Set as default" for one type: after GIO emits `changed`, the visible tiles of that
   type redraw in place (the listener path). If the signal does not arrive, F5 or a scroll-rebind
   shows the new icon. Set the old default back afterwards.
4. Drag a file: the drag icon uses the same app icon as its tile. A folder drag keeps GIO's folder
   icon, as before.
5. Trash a scratch file under `~/.cache/gimbal-tests/<test>-*` with `XDG_DATA_HOME` inside that
   folder (CLAUDE.md's trash rule), then open Trash: the row shows the name-guessed app icon.
   Restore or remove the scratch file afterwards.
6. The file chooser (`FolderView` in the portal picker) shows the same icons.
7. A folder of a few thousand files of one type scrolls as before, and with `GIMBAL_LOG=1` the log
   has exactly one `icons: <type> -> ...` line for that type, not one per cell.

## Files changed

- `crates/fm-ui/src/file_icon.rs` (new)
- `crates/fm-ui/src/main.rs` (one `mod` line)
- `crates/fm-ui/src/folder/view.rs` (grid, drag, listener, Drop, `refresh_app_icons`)
- `crates/fm-ui/src/trash.rs` (Trash row icon)
- `crates/fm-ui/src/properties.rs` (header icon; removed the now-unused `Facts::icon` field and
  `standard::icon` from its private query)
- `docs/notes/P2ar.md` (this file)

Nothing outside these files needs a change for this brief. No new dependency. No `unsafe`.

## Claude's review (Nov 17)

Read the notes and the diff, including all of `file_icon.rs`. The decision is pure and tested,
and the cache holds `None` answers too. The monitor lives for the whole process. The refresh
skips thumbnails and the folder SVG, because those cells hold a paintable. `log_verbose!` lines
print with `GIMBAL_LOG=1` (`log.rs:86`, `:116-125`), as the brief asked. I checked one
possible trap: an empty file with a known extension keeps its real type, because GIO guesses from
the name first and uses `application/x-zerosize` only when the name is uncertain. So a new empty
`.txt` file still gets the editor icon. No fixes.

Gates on the combined tree (P2ao–P2ar), run twice in a row after all three agents had finished: `scripts/check.sh` exit 0 both times, 1093 passed, 0 failed, 27 ignored. fmt and clippy are clean, and `cargo deny` shows only the old duplicate warnings. Both agents saw one test failure that didn't reproduce. Each happened while another agent was saving files in the same tree, so the two runs collided; nothing is flaky.

Live run: `/tmp/gimbal-p2ar/run.sh`, checks 1–7. It copies Kyle's `~/.config/mimeapps.list`
into the scratch config and adds `~/.local/share` to `XDG_DATA_DIRS`, so the defaults match
his desktop while Open With writes only to the scratch copy. The Trash check uses
`~/.cache/gimbal-tests/p2ar-run` with `XDG_DATA_HOME` inside it, and the script removes that
folder at the end. The file chooser still needs a check through the portal.

**Kyle's live run (Nov 17):** all checks passed except check 6, where the trashed `script.py`
didn't appear in Trash. **It's the test setup, not P2ar.**
- The log shows the trash succeeded (`fileops: trashed …/script.py`). GIO trashes local files
  in-process, so the file went to the scratch `XDG_DATA_HOME` Trash.
- The Trash page lists `trash:///` through the session's `gvfsd-trash`, which has no
  `XDG_DATA_HOME` (checked in `/proc/<pid>/environ`). It reads Kyle's real Trash and the
  per-drive trash folders. The page listed 6 items; the scratch Trash could only hold 1.
- So the page showed Kyle's real items (with their new icons), and the scratch file was deleted
  with the scratch folder at the end of the run. Nothing reached Kyle's real Trash.

Lesson for run scripts: a scratch `XDG_DATA_HOME` can't be seen through the Trash page. Check
Trash icons on existing real items, and never trash a scratch file into the real Trash to test
it.
