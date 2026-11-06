# P2y: open a file you don't own as administrator

## What this change does

- Adds `fm_core::admin`, the pure administrator decisions:
  - `needs_admin(owner_uid, current_uid)`;
  - `current_uid()`, moved from `fm-ui/src/agent/mod.rs` so both callers use
    one function;
  - `AppFamily::{Kio, Gio, PathOnly, Sandboxed}` from a desktop entry's
    `Categories`, `Exec`, `X-Flatpak` and `X-SnapInstanceName` fields;
  - `admin_uri`, using `glib::filename_to_uri` and then swapping `file:` for
    GVfs's `admin://` or KIO's `admin:`;
  - the selection checks for the command's three refusals, plus missing paths
    and broken links.
- Adds `file.open-as-admin paths=<selection>` with `Effect::System`.
- Adds `Open as Administrator` under `Open` in a file's right-click menu, only
  for one regular file whose target owner differs from the current user.
- Launches the file's default application through the window's GDK launch
  context. GIO applications get `admin:///path`; KDE applications get
  `admin:/path`. Path-only and sandboxed applications, and KIO without
  `kio-admin`, launch nothing and show one summary line.
- Logs one `admin:` line per attempt with the application id, family and URI
  scheme. It never logs file contents.
- Lists `file.open-as-admin` as unavailable through the agent socket.

No root process is started and there is no new dependency, config change or
session schema change.

## Files written

- `crates/fm-core/src/admin.rs` (new)
- `crates/fm-core/src/lib.rs`
- `crates/fm-core/src/command/catalogue.rs`
- `crates/fm-core/src/command/tests.rs`
- `crates/fm-ui/src/app.rs`
- `crates/fm-ui/src/fileops/menu.rs`
- `crates/fm-ui/src/fileops/dialogs.rs` (the menu renderer; see "Outside the
  Output list")
- `crates/fm-ui/src/agent/mod.rs` (only the moved `current_uid`)
- `docs/agent-socket.md` (the UI-only list)
- `docs/notes/P2y.md` (this file)

There is no `phase2/RULES.md`; I read `phase1/RULES.md` and the repository's
`CLAUDE.md`, which `phase2/README.md` names as its replacement. **[read]**

## The desktop-entry problem in this gio version

The brief asks for `gio::DesktopAppInfo`'s `string`, `categories` and
`commandline`. That type is not in the pinned `gio` 0.22.10 crate: it has no
`src/auto/desktop_app_info.rs`, no `DesktopAppInfo` re-export, and its
`Gir.toml` has no `Gio.DesktopAppInfo` entry. The docs.rs page for
`gio/0.22.10/struct.DesktopAppInfo.html` is also 404. **[observed]**

The installed GLib 2.88 moved the C type into the `GioUnix` namespace:
`/usr/share/gir-1.0/Gio-2.0.gir` points at
`/usr/share/gir-1.0/GioUnix-2.0.gir:87`, where `GDesktopAppInfo` now lives.
`gio-unix` is not a workspace dependency and the brief forbids a new one.
**[read]**

The UI therefore uses the pinned safe APIs that do exist:

- `gio::AppInfo::default_for_type` for the content type;
- `gio::AppInfo::id`, `name`, `commandline` for the application;
- `glib::KeyFile` to read the entry named by `AppInfo::id` from
  `$XDG_DATA_HOME/applications` and `$XDG_DATA_DIRS/applications`;
- `gio::AppInfo::launch_uris` with `gdk::Display::app_launch_context`.

For an entry that cannot be found, `Exec` falls back to
`AppInfo::commandline`, with no `Categories`, Flatpak or Snap evidence. The
search also checks the Flatpak and Snap export directories when they are not
on `XDG_DATA_DIRS`. That still distinguishes `%u`/`%U` from `%f`/`%F`; an
installed Flatpak or Snap app is normally found in its exported applications
directory. **[inferred]**

## The pure decisions

`needs_admin` is exactly `owner_uid != current_uid`. `current_uid` reads
`/proc/self` metadata, which is owned by the process's real uid, and is now in
`fm_core::admin`; `agent/mod.rs` imports it instead of defining a second copy.
**[read]**

For a selection, `check_admin_file` refuses, in order:

1. anything other than one path;
2. a folder;
3. a missing path, broken link or non-regular file;
4. a file the current user owns.

`file_facts` uses `std::fs::metadata`, which follows a symlink, so a link is
judged by its target and a broken link has no owner and is not a regular file.
The menu therefore does not offer the entry for a broken link. **[read]**

`app_family` applies the brief's precedence: Flatpak or Snap is `Sandboxed`;
otherwise `KDE` in `Categories` is `Kio`; otherwise an unescaped `%u`/`%U` in
`Exec` is `Gio`; everything else is `PathOnly`. `%%u` is a literal percent and
does not count. Kate on this machine has `Exec=kate -b %U` and
`Categories=Qt;KDE;Utility;TextEditor;Development;`, so it is `Kio`.
**[observed]**

`admin_uri` calls `glib::filename_to_uri(path, None)`, strips the `file://`
prefix and adds `admin://` or `admin:`. `filename_to_uri` percent-encodes the
raw path bytes, so spaces, `#`, `%`, parentheses and non-UTF-8 names all keep
one representation. **[observed]**

## The command and menu

`file.open-as-admin` takes the same `paths` selection argument as `file.open`.
It is `Effect::System`, so the local socket refuses it before a handler and the
CLI, which speaks to Gimbal through the socket, cannot run it. The handler also
refuses any caller other than the UI. **[read]**

The menu entry is inserted immediately after `Open`. It requires one item,
`is_file`, no `is_dir`, a known owner, and `needs_admin(owner, current_uid)`.
`current_uid()` is read once in the UI when the menu is built; if it cannot be
read, no administrator entry is offered. **[read]**

The icon is the standard symbolic name `changes-prevent-symbolic`, held in
`menu::ADMIN_LOCK_ICON` so P2z can use the same icon. `dialogs::show_menu`
draws an icon before the label when an item has one. P2z can move this constant
into the look module when it adds the red tab and banner. **[read]**

## Launching

The handler queries `standard::content-type` through GIO and falls back to
`gio::content_type_guess`, then asks `gio::AppInfo::default_for_type` for the
default application with `must_support_uris = false`. Keeping that flag false
is what lets a `%f`-only application be classified and reported, instead of
silently launching something else. **[read]**

`kio-admin` is considered installed only when
`/usr/share/polkit-1/actions/org.kde.kio.admin.policy` exists. On this machine
the policy, the KIO plugin and `/usr/share/gvfs/mounts/admin.mount` all exist;
Kate is the `text/plain` default. **[observed]**

The launch uses the window's `gdk::AppLaunchContext` from
`gdk::Display::app_launch_context`, so GDK supplies the display and activation
context to the editor. `file.open` keeps its existing
`launch_default_for_uri` path. **[read]**

When there is no administrator URL, the window shows exactly one of:

- `<App> can't open administrator files. Open it with a GNOME or KDE editor.`
- `<App> runs in a sandbox and can't open administrator files.`
- `<App> needs kio-admin to open administrator files (sudo apt install kio-admin).`

The refusal is logged as `admin: no launch for <id> (<family>): <reason>`.
A successful launch logs `admin: launched <id> (<family>) via admin`. Neither
line contains file contents. **[read]**

## API citations

- `gio-0.22.10:src/auto/app_info.rs:61` —
  `AppInfo::default_for_type(content_type, must_support_uris)`.
- `gio-0.22.10:src/auto/app_info.rs:404` — `AppInfo::commandline`.
- `gio-0.22.10:src/auto/app_info.rs:450` — `AppInfo::id`.
- `gio-0.22.10:src/auto/app_info.rs:456` — `AppInfo::name`.
- `gio-0.22.10:src/auto/app_info.rs:494` — `AppInfo::launch_uris`.
- `gio-0.22.10:src/auto/file.rs:1756` — `File::query_info`.
- `gio-0.22.10:src/auto/file_info.rs:182` — `FileInfo::content_type`.
- `gio-0.22.10:src/content_type.rs:10` — `content_type_guess`.
- `glib-0.22.10:src/auto/functions.rs:288` — `filename_to_uri`.
- `glib-0.22.10:src/key_file.rs:51` — `KeyFile::load_from_dirs`.
- `glib-0.22.10:src/auto/key_file.rs:141` — `KeyFile::has_key`.
- `glib-0.22.10:src/auto/key_file.rs:197` — `KeyFile::value`.
- `gdk4-0.11.5:src/auto/display.rs:107` —
  `Display::app_launch_context`.
- `gtk4-0.11.5:src/auto/window.rs:951` — `Window::set_startup_id`, used by
  P2x's D-Bus path, not by this click path.

GLib's own admin mount is `exec /usr/share/gvfs/mounts/admin.mount`:
`Exec=/bin/sh -c 'pkexec /usr/libexec/gvfsd-admin …'`, so the password prompt
belongs to the desktop's polkit agent, not to Gimbal. **[observed]**

## Verification

`scripts/check.sh` passed on the final tree
(`/tmp/gimbal-p2y-check-final.log`):

```text
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

The five steps are `cargo fmt --all --check`, clippy with `-D warnings`, the
workspace tests, the release build and `cargo deny check`. **[observed]**

Test totals: **890 passed, 23 ignored** (23 + 16 + 12 + 419 + 6 + 11 + 403
passing; 0 + 0 + 0 + 1 + 6 + 11 + 5 ignored). P2x's baseline was 871 passed /
23 ignored, so P2y adds 19 passing tests: 12 in `fm-core/src/admin.rs`, one
catalogue test and one command/caller test in `fm-core`, and five menu tests.
**[observed]**

The new tests cover:

- equal and different owners;
- Kate's real fields, a `%U` GNOME entry, `%f`/`%F`/no-field-code entries,
  `%%u`, Flatpak, Snap, and KDE + Flatpak;
- both URI families with spaces, `#`, `%`, parentheses and a non-UTF-8 byte;
- one/many paths, a folder, an owned file and a missing file;
- a symlink judged by its target, and a broken link with no owner;
- the catalogue entry and its `Effect::System` rule;
- the menu entry's position, lock icon and action, and its absence for an
  owned file, a folder, several files, a broken/unknown item and an unreadable
  current uid.

No GUI launch was run. The brief says Claude runs the live checks, and this
agent may not inject pointer or keyboard input. Kyle's `gimbal-files` was not
running, so a launch would not have been blocked by his window, but the live
password prompt still needs a human. **[observed]**

## What Claude checks live

Kyle creates the root-owned scratch file first:

```bash
sudo install -o root -m 0644 /dev/null /tmp/gimbal-p2y/root-owned.txt
```

Claude's own scratch file is `/tmp/gimbal-p2y/user-owned.txt` (already created,
owned by kyle).

1. **Menu visibility.** Right-click `root-owned.txt`: `Open as Administrator`
   is directly under `Open` and carries the lock. Right-click
   `user-owned.txt`: the entry is absent. A folder and a multi-file selection
   also have no entry.
2. **Kate.** With Kate still the `text/plain` default, choose
   `Open as Administrator` on `root-owned.txt`. Kate opens
   `admin:/tmp/gimbal-p2y/root-owned.txt`. Edit and Save; polkit asks for the
   password once. Then `stat` shows the file still owned by root and containing
   the new text.
3. **A GIO editor.** `org.gnome.TextEditor.desktop` is not installed on this
   machine, so this check is **not run** unless Kyle installs it. If it is
   installed, set a scratch `XDG_CONFIG_HOME` whose `mimeapps.list` makes
   `org.gnome.TextEditor.desktop` the `text/plain` default, launch Gimbal with
   that config and the same scratch config/state homes, and confirm the editor
   opens `admin:///tmp/gimbal-p2y/root-owned.txt`.
4. **A `%f`-only default.** Point the scratch MIME default at a `%f`/`%F`
   application. Choosing the entry shows
   `<App> can't open administrator files. Open it with a GNOME or KDE editor.`
   and launches nothing.
5. **Automation.** `gimbal-files-cli run file.open-as-admin
   paths=/tmp/gimbal-p2y/root-owned.txt` (with the socket on or off) is refused
   as UI-only; the error says system operations are not available through the
   local socket.

## Outside the Output list

- `crates/fm-ui/src/fileops/dialogs.rs` was changed because it is the menu
  renderer; without it a `MenuItem` icon cannot appear. The item menu itself is
  `menu.rs`.
- `crates/fm-ui/src/look/icons.rs` and a new lock SVG were **not** changed
  because the brief's Output list is exclusive. The lock is therefore the
  existing symbolic icon `changes-prevent-symbolic`, named by
  `menu::ADMIN_LOCK_ICON`. P2z's Output does include `look/icons.rs`; it should
  move this constant into the look module there so the tab lock and menu lock
  stay one icon.
- `gio::DesktopAppInfo` cannot be used from `gio` 0.22.10 (see above).
  Adding `gio-unix` would break the brief's no-new-dependency rule. The
  `glib::KeyFile` read is the safe replacement; P2z's `Scope::Admin` can reuse
  `admin::admin_uri` and its encoding directly.
- No config or session schema changed, and `packaging/debian/copyright` needs
  no crate update.

## Claude's review (Nov 6, 2026)

P2y was handed off before P2x was committed and ran in the same working tree.
Kyle chose to review and commit the two together.

**Code.** The return matches the brief. Reading desktop entries with
`glib::KeyFile` instead of `gio::DesktopAppInfo` is justified: the pinned gio
0.22.10 doesn't expose the GioUnix type, and the brief allows no new
dependency. `changes-prevent-symbolic` resolves in Kyle's Adwaita theme.
**[read]** **[observed]**

**Fix: empty files found no application (`crates/fm-ui/src/app.rs`,
`crates/fm-core/src/admin.rs`).** GIO types every empty file
`application/x-zerosize` regardless of its name, and no application handles
that type. So `Open as Administrator` on the empty scratch file failed with
`No default application opens root-owned.txt.` (live run A). An empty
root-owned config file is a normal thing to open as administrator, so the
handler now falls back to the name's type (`fm_core::admin::lookup_type`, with
a test): `root-owned.txt` → `text/plain` → Kate. **[observed]**

Plain `file.open` has the same gap: GIO's own `query_default_handler` also
fails on an empty `.txt`. That predates P2y and is left for a follow-up.
**[observed]**

**Gates on the combined P2x + P2y tree:** `scripts/check.sh` 892 passed, 23
ignored (P2y's 890 plus the two tests Claude added in this review).
`test-install.sh` and `test-deb.sh` pass. **[observed]**

### Live checks (Claude and Kyle, Nov 6)

Debug build on scratch XDG dirs (`/tmp/gimbal-p2y-x`, a scratch
`mimeapps.list` per run); `root-owned.txt` made by Kyle with `sudo install -o
root`. The item menu opens from a right-click gesture that AT-SPI can't
trigger, and niri reports no tile position for the window, so Kyle did the
clicks.

1. **Menu: pass (Kyle).** `Open as Administrator`, with the lock, sits under
   `Open` on `root-owned.txt`, and is absent on `user-owned.txt`.
2. **Kate: pass** after the fix above. The log shows
   `admin: launched org.kde.kate.desktop (kio) via admin`. Kyle typed in Kate
   and saved through kio-admin's polkit prompt; afterwards `root-owned.txt`
   held his text (2 bytes) and was still `root:root` 0644.
3. **A GIO editor: not run.** No GTK text editor is installed.
4. **A path-only default: pass (Kyle).** A scratch `%f` app as the
   `text/plain` default shows the "can't open administrator files" line; the
   log shows `admin: no launch for p2y-pathonly.desktop (path-only)`, and
   nothing launched.
5. **Automation: pass (Claude).** `gimbal-files-cli run --window <id>
   file.open-as-admin paths=…` against a live socket returns `code =
   "unsupported"`, `system operations are not available through the local
   socket`, and nothing launches.

Kyle's real `~/.config/mimeapps.list` is unchanged by these runs. **[observed]**
