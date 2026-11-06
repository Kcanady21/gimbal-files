# P2z: administrator folder tabs

## What this change does

- Adds `Tab.admin` to the workspace model. Every older path creates `false`;
  `tab.new`, `tab.open-in-new-window`, `group.split` and `group.open` keep the
  scope of the tab they start from. `tab.open-as-admin` creates an
  administrator tab next to the current one, and `tab.leave-admin` turns the
  current tab back into a normal tab in place, keeping folder, history,
  selection and scroll anchor.
- Session saves skip administrator tabs. A group left empty is dropped; a
  window left empty is written as one ordinary home tab, matching the restore's
  missing-folder fallback. No session schema changed.
- Adds `fm_core::admin::Scope` and `admin::gfile(path, scope)`. `Scope::User`
  is `gio::File::for_path`; `Scope::Admin` is `gio::File::for_uri` of the
  `admin://` URI built by P2y's `admin_uri`. `Scope` is carried by every native
  `fileops::Request` and its `Plan`; copy and move carry a separate destination
  scope, and undo records keep both scopes.
- Every fm-core file operation now builds its GIO objects with `gfile`.
  Planning and execution use scope-aware existence/folder queries, so an
  administrator path need not be readable by this process. Cross-scope copy
  and move build each side with its own scope.
- Adds `tab.open-as-admin path=<folder>` and `tab.leave-admin`, both
  `Effect::System` and UI-only. The first refuses a file, a missing path and a
  folder the user owns; the second is a no-op on a normal tab.
- Adds `Request::DeletePermanent` and `Action::PermanentDelete`. An
  administrator tab's Delete is permanent because GVfs's `admin://` backend
  does not offer trash; the UI confirmation names the count, the first names
  and the word "permanently".
- Adds the confirmation rule as a pure function,
  `admin::asks_first(scope, risk)`: only an administrator tab asks, and only
  for a permanent delete, a conflict overwrite or a move outside the tab's
  folder tree. The conflict dialog asks again before Replace in an
  administrator plan; the move path asks before submitting.
- Mounts GVfs's administrator location before the first administrator tab:
  `gio::File::mount_enclosing_volume` with a `gtk::MountOperation` on the
  window. A cancelled or failed mount opens no tab and shows
  `Administrator access was not granted.` or the error text. A successful
  mount is remembered for the process lifetime; Gimbal never unmounts it.
- Adds the red administrator tab pill with the lock icon in normal, accordion
  and group views, and an `adw::Banner` above each administrator folder view
  (and therefore above each administrator split tile). The banner reads
  `Administrator: changes here affect the whole system.` and its `Leave`
  button runs `tab.leave-admin`. Split tiles get the error-coloured border.
- Administrator tabs disable dragging out, dropping in, preview editing and
  saving, recursive search and thumbnails. The grid tooltip says why. Opening a
  file the user does not own uses `file.open-as-admin`'s app-family rules; a
  file the user owns opens normally.
- The item and background menus offer `Open as Administrator`, with the lock,
  for a folder the user does not own. It is never offered inside an
  administrator tab, and P2aa's `Open With` is hidden there.
- The agent socket's `state` lists `admin = true` on an administrator tab.
  Every `run` that resolves to one, explicitly or through the focused tab, is
  refused with the typed error `administrator tabs are UI-only`.
- Logs one `admin:` line for an opened, left or closed tab, a mount result, a
  refused agent request and each administrator file operation with its kind
  and count. File contents are never logged.

No root process is started. There is no new dependency, config change or
session schema change.

## Files written

The brief's Output list, plus the smallest adjacent changes needed to compile
the same scope plumbing (listed under "Outside the Output list").

- `crates/fm-core/src/workspace/mod.rs`
- `crates/fm-core/src/session.rs`
- `crates/fm-core/src/admin.rs`
- `crates/fm-core/src/fileops/mod.rs` and `src/fileops/tests.rs`
- `crates/fm-core/src/command/catalogue.rs` and `command/tests.rs`
- `crates/fm-core/tests/trash.rs` (existing request constructors)
- `crates/fm-ui/src/app.rs`, `header.rs`, `folder/view.rs`
- `crates/fm-ui/src/fileops/{menu,dialogs,conflict,progress,worker}.rs`
- `crates/fm-ui/src/look/css.rs`, `look/icons.rs`
- `crates/fm-ui/src/events.rs` (the new event's touch map)
- `crates/fm-ui/src/agent/mod.rs` and `agent/tests.rs`
- `crates/fm-ui/src/agent/work.rs`, `volumes/tests.rs` (existing request constructors)
- `crates/fm-agent/src/protocol.rs`, `protect.rs`
- `docs/agent-socket.md`
- `docs/notes/P2z.md`

## Scope and the pure decisions

`Scope` is a two-value enum in `fm_core::admin`; its `Default` is `User`.
`gfile` reuses `admin_uri(AppFamily::Gio, path)`, which calls
`glib::filename_to_uri` and swaps the scheme. Spaces, `#`, `%`, parentheses and
non-UTF-8 bytes therefore have exactly one spelling, tested in `admin.rs`.
**[read]** **[observed]**

`asks_first` is deliberately narrow: `Copy`, `Rename` and `NewFolder` never
ask, and no user-scope operation asks through this rule. **[read]**

`check_admin_folder` refuses a missing path, a file and a folder the current
user owns, in that order. **[read]**

## File operations and undo

`Request` now carries `scope` (source side) and, for copy/move,
`target_scope`. `Plan` exposes both and `is_admin`. Planning uses
`path_facts`/`path_exists`, which ask GIO for the administrator scope and
`std::fs` for the user scope. Executing copy, move, rename, new folder, trash
and permanent delete uses `gfile` on both sides. Folder walks enumerate through
GIO, so an admin directory tree is walked through GVfs rather than the local
path. **[read]**

`UndoRecord` stores both scopes; `Request::Undo` plans with the record's
scopes, so an administrator rename or copy is undone through the
administrator backend. **[read]**

The tests in `fileops/tests.rs` construct every native request with both
scopes, check that the plan keeps them, and construct an undo record directly
to prove the stack remembers them. `gfile` itself is tested for both schemes
with spaces, `#`, `%`, parentheses and non-UTF-8 bytes. There is no real GVfs
admin mount in these tests; the URI assertions are the fake.

## Look: verified names

- `adw::Banner` exists in the pinned libadwaita binding:
  `libadwaita-0.9.2:src/auto/banner.rs:22` and its methods
  `set_button_label` at `:94`, `set_revealed` at `:112`, `set_title` at
  `:120`, `connect_button_clicked` at `:137`. The C widget is
  `libadwaita-1.9.1/src/adw-banner.c:38`. **[read]**
- The red uses `@error_bg_color`, `@error_fg_color` and `@error_color`, defined
  in `libadwaita-1.9.1/src/stylesheet/_colors.scss:22-23` and exposed as
  `--error-bg-color`/`--error-fg-color` at `:196-197`. No fixed colour is
  added. **[read]**
- The pill and banner classes are new rules in `look/css.rs`; the lock name is
  now `look::icons::ADMIN_LOCK_ICON` (`changes-prevent-symbolic`) and the menu
  re-exports it, so P2y's menu and P2z's tabs share one icon. **[read]**

## Mounting and "once per session"

`/usr/share/gvfs/mounts/admin.mount` on this machine says
`AutoMount=false`, `DBusName=org.gtk.vfs.mountpoint_admin` and
`MountPerClient=true`; its `Exec` is the `pkexec /usr/libexec/gvfsd-admin`
line quoted by P2y. GVfs keeps a mount alive for the session; Gimbal does not
unmount it. The process remembers one successful mount and skips the prompt
afterwards. **[observed]** for the file, **[inferred]** for the lifetime.

`gtk::MountOperation::new(Some(parent))` is
`gtk4-0.11.5:src/auto/mount_operation.rs:26`; it extends
`gio::MountOperation`. `mount_enclosing_volume` is
`gio-0.22.10:src/auto/file.rs:1175`. **[read]**

## Tests and gates

`scripts/check.sh` passes on the final tree:

```text
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Test totals: **931 passed, 23 ignored** (23 + 16 + 12 + 450 + 6 + 11 + 413
passing; 0 + 0 + 0 + 1 + 6 + 11 + 5 ignored). P2y's reported baseline was 892
passing / 23 ignored; this adds 39 passing tests. **[observed]**

The new tests cover:

- `Tab.admin` through `tab.new`, `group.open`, `group.split`,
  `tab.open-in-new-window` and in-place `tab.leave-admin`;
- session skipping for a mixed group, an empty group and an empty window;
- `gfile` for both scopes with spaces, `#`, `%`, parentheses and a non-UTF-8
  byte;
- each native request's scope and the undo record's scopes;
- the two catalogue commands, their arguments and their `Effect::System`;
- the confirmation pure function and the administrator-folder refusals;
- administrator menu visibility (folder entry, no entry inside an admin tab,
  no `Open With` there);
- the agent's `admin = true` state and the `administrator tabs are UI-only`
  refusal for explicit and implicit targets, end to end through the socket;
- the fm-agent protocol's `admin = true` serialization.

## What Claude checks live

The scratch tree needs root ownership, so Kyle runs:

```bash
sudo install -d -o root -m 0755 /tmp/gimbal-p2z/root-dir
sudo install -o root -m 0644 /dev/null /tmp/gimbal-p2z/root-dir/a.txt
```

Claude's own scratch folder is `/tmp/gimbal-p2z/user-dir` (owned by kyle).

1. Right-click `root-dir` → `Open as Administrator`; polkit prompts. A red tab
   with the lock and the banner opens, listing `a.txt`. The background menu of
   a folder needs-admin tab also shows the entry.
2. Cancel the prompt: no tab opens; the status line shows
   `Administrator access was not granted.` The next attempt should prompt
   again.
3. In the administrator tab: New Folder, then Undo; rename `a.txt`, then Undo;
   Delete `a.txt`. This build treats Delete as a confirmed permanent delete
   (the dialog names the count, first names and "permanently"). Record whether
   GVfs offered any trash path; `stat` should show the remaining results owned
   by root. This is the one place the live check can correct the code's
   assumption.
4. `Leave` on the banner keeps the same folder and history; the tab loses red
   and the banner disappears in place. Do it in a split view too, and check
   that only the administrator tile had a banner and the error border.
5. Restart: no administrator tab returns. A window that held only an
   administrator tab comes back as one normal home tab.
6. `gimbal-files-cli run --window <id> nav.up` against an administrator tab is
   refused with `administrator tabs are UI-only`; the socket's `state` shows
   `admin = true`.
7. Check light and dark, tiled and floating: the red pill keeps its lock in the
   accordion and group views; the banner is always present and has no dismiss
   control.
8. Try dragging a file out of, and dropping a file into, an administrator tab:
   both are off and the grid tooltip explains why. Preview editing, recursive
   search and thumbnails are off as well.
9. Copy a user-owned file into the administrator tab and back with copy/paste,
   and rename or new-folder through the tab: `stat` confirms the owner.

## Outside the Output list

The Output list was not sufficient to compile the same change; these are the
smallest adjacent edits, all reported here rather than silently spread:

- `crates/fm-core/tests/trash.rs` and `crates/fm-ui/src/{agent/work.rs,
  volumes/tests.rs,fileops/worker.rs,fileops/progress.rs}`: existing `Request`
  constructors and exhaustive matches needed the new scope and permanent-delete
  fields/actions. The worker's production paths are unchanged otherwise.
- `crates/fm-ui/src/fileops/conflict.rs`: the administrator overwrite
  confirmation needs to know whether a plan is an administrator plan.
- `crates/fm-ui/src/fileops/dialogs.rs`: the shared destructive confirmation
  helper, following the Trash page's `adw::AlertDialog` pattern.
- `crates/fm-ui/src/fileops/progress.rs`: wording and progress for the new
  permanent delete.
- `crates/fm-ui/src/events.rs`: the new `TabScopeChanged` event's touch map.
- `crates/fm-agent/src/protocol.rs` and `protect.rs`: `admin = true` in state,
  the `DeletePermanent` protection match and the admin flag on `TabInfo`.
- `crates/fm-ui/src/agent/tests.rs`: the end-to-end refusal test.
- `docs/agent-socket.md`: the two new UI-only commands and the refusal.

## Known limits and follow-ups

- The live check decides whether GVfs's admin backend supports trash. The
  implementation chooses the safe branch described by the brief: permanent
  delete behind confirmation. If the live check shows `g_file_trash` works,
  Delete should submit `Request::Trash` with the admin scope instead.
- A cut paste whose destination is outside the administrator tab's folder tree
  asks the same move-out confirmation as Move To. The clipboard payload still
  carries only URIs; the source scope is inferred from each file's owner, so a
  root-owned source is read through `admin://` even when pasted into a normal
  tab.
- No live GUI launch was run by this agent: the rules reserve pointer and
  keyboard interaction for Claude, and a live password prompt needs Kyle.
  Everything above is static, unit-tested behaviour.
- The brief says "every `gio::File::for_path` (16 in fm-core)". `fileops` and
  the trash helpers now use `gfile`; `fm-core/src/preview.rs` and
  `folder_size/walk.rs` still build user paths directly. They are outside this
  brief's file list and administrator tabs switch them off, so no administrator
  path reaches them. If they are ever used on an admin path, they should take
  the same scope.

## Claude's review (Nov 6, 2026)

The return matches the brief, and its out-of-list edits are the minimum the
scope plumbing needed. Kyle's live runs found real bugs; all are fixed below,
each with the evidence and, where the logic is pure, a test.

### Fixes

1. **The admin-tab commands never ran (`crates/fm-ui/src/app.rs`).**
   `tab.open-as-admin` and `tab.leave-admin` are `Effect::System`, and the
   dispatcher's effect gate let only Settings and View commands reach the tab
   handlers. Every click logged `… is not implemented by this version`, so no
   polkit prompt appeared. The gate now lets exactly these two through. Test
   `the_admin_tab_commands_reach_their_handlers` dispatches both through the
   real `UiHandler`; it fails with the "not implemented" outcome when the gate
   change is removed. **[observed]**
2. **Paste read readable system files through admin
   (`crates/fm-core/src/admin.rs`, `app.rs`).** The source scope came from
   ownership, so copying a readable root-owned file (a wallpaper under
   `/usr/share`) between two normal tabs needed the admin mount. Now
   `paste_source_scope` uses GIO's `access::can-read` (and
   `access::can-delete` for a cut). GIO answers `/etc/hostname` readable and
   `/etc/shadow` not. Test `a_readable_root_owned_source_pastes_as_the_user`.
   **[observed]**
3. **Phantom rows after a rename, and a stuck highlight
   (`crates/fm-ui/src/folder/view.rs` and the admin re-read path in
   `content.rs`, `split.rs`, `frame.rs`, `app.rs`).** Disk operations were
   correct, but the admin grid showed the old name beside the new one, and
   after Undo two `a.txt` rows for one file. Kyle deleted the "duplicate",
   which was the real file. Two causes:
   - GtkDirectoryList ignores `G_FILE_MONITOR_EVENT_MOVED`
     (`gtk-4.22.4:gtk/gtkdirectorylist.c:790`), and GVfs's admin monitor
     reports the rename that way.
   - `FolderView::reload` called `set_file` with the same folder, which GTK
     treats as a no-op (`gtk_directory_list_set_file` returns early on
     `g_file_equal`). Meanwhile Gimbal reset its own counts and selection,
     hence the stuck highlight.

   Now every administrator job re-reads every administrator view (single view
   and split tiles), and `reload` clears the file first so GTK really
   enumerates again. That also makes plain Refresh a real refresh.
   **[observed]** Re-checked by Kyle: only `renamed.txt` after the rename, only
   `mine.txt` after Undo, and the highlight follows clicks.
4. **Undo of New Folder failed: "Operation not supported"
   (`crates/fm-core/src/fileops/mod.rs`).** Undo trashed the folder, and the
   admin backend has no trash. An administrator New Folder undo now plans
   `PermanentDelete`, and runs it only while the folder is still empty (checked
   again at run time). Undo of a copy into an administrator folder is refused
   with `Conflict::NoAdminTrash` rather than deleting copies that may have
   been edited. Test `administrator_undo_never_plans_a_trash`. **[observed]**
5. **A drop got into an admin tab through the spring-loaded pill (`app.rs`
   `drop_on_folder`).** The drop ran as the user and failed with Permission
   denied. A drop whose target belongs to an administrator tab in the window,
   focused or a split tile, is now refused: `Cannot drop: dropping is off in
   an administrator tab` (re-checked by Kyle). **[observed]**
6. **Copies into an admin folder kept the user as owner (`fileops/mod.rs`
   `copy_tree`).** `ALL_METADATA` copies the owner, and root may set it, so a
   paste left a `kyle:kyle` file in a root folder. Copies into an
   administrator folder no longer use `ALL_METADATA` and belong to root.
   `stat` after the fix: `root:root`. **[observed]**
7. **Rename dropped the extension (`crates/fm-ui/src/fileops/dialogs.rs`, from
   P2k).** The entry held only the stem, so the typed text replaced the whole
   name. `rename_prefill` now fills the full name and pre-selects the stem.
   Test covers `a.txt`, `archive.tar.gz`, `.bashrc`, a folder and
   `résumé.pdf`. **[observed]**

### Look changes Kyle asked for during the review

- **The administrator banner is red.** libadwaita paints a banner's
  background on `banner > revealer > widget` (a 30% grey mix,
  `libadwaita-1.9.1/src/stylesheet/widgets/_toolbars.scss:243-261`) and
  styles the button `suggested-action` (`adw-banner.c:769`). The rules now
  target that node, in the theme's error colours, including the backdrop
  state.
- **Status bar.**
  - Messages pulse three times in the accent colour (`gl-status-pulse`,
    `motion::ms(600)`), alternating two classes so back-to-back messages
    restart the animation.
  - The bar is centered: the name, a dash (`gl-status-sep`, the `{gs}` gap
    token) and the size/counts sit together as one centered group. The name
    no longer fills the width with the size pushed far right.

### Gates and live checks

`scripts/check.sh`: 935 passed, 23 ignored (P2z's 931 plus four tests from this
review). A short launch logged no CSS parser warnings. **[observed]**

Live (Kyle did the clicks and the polkit prompt; Claude ran the CLI checks and
read the logs):

- The admin tab is red with the lock and banner; `Leave` works in place.
- Split view puts the banner and error border on the admin tile only.
- No admin tab survives a restart.
- The socket's `state` shows `admin = true`, and `run` against the window or
  the tab is refused with `administrator tabs are UI-only`.
- Rename, undo and new-folder undo pass after fixes 3 and 4, and Delete is
  permanent behind its confirmation.
- Drag out is blocked; drag in is blocked after fix 5.
- Copy in is owned by root after fix 6.
- Cancelling the polkit prompt opened no tab and denied access (Kyle).
- The status bar's centered layout was checked by Kyle.
