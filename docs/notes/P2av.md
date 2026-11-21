# P2av (portal F5): the switch, the health check and the packages

Tags: **[read]** for source or a man page, **[observed]** for a command and
its output, **[inferred]** for anything only a live session can settle.

Starts from main at `dd4abe4` (P2au) or later. Status: code, tests, notes and
all five gates are complete. The package tests pass; the live matrix at the end
is for Claude and Kyle. No run read or wrote Kyle's real
`~/.config/xdg-desktop-portal/`, restarted the real portal or called
`RestartUnit` on the real bus.

## What changed

- `crates/fm-core/src/portals_conf.rs` (new): the config text model, the
  effective-file and chain rules, `enable`/`disable`, the restart trait, the
  status value and the real folder lists.
- `crates/fm-core/src/command/catalogue.rs`, `command/mod.rs`,
  `command/tests.rs`: `integration.set-file-chooser`, in
  `UI_ONLY_SETTINGS_COMMANDS`, with caller and argument tests.
- `crates/fm-core/src/config.rs`: only its `every_settings_command_has_a_change_for`
  test learned that this one Settings command writes `portals.conf`, not
  `config.toml`. There is no `change_for` arm.
- `crates/fm-ui/src/app.rs`: `SystemdRestarter`, the command handler and the
  health scan.
- `crates/fm-ui/src/header.rs`: the Integration row, status line, Test button,
  the confirmation dialog and the portal test call.
- `scripts/build-deb.sh`, `scripts/test-deb.sh`, `scripts/install.sh`,
  `scripts/test-install.sh`, `scripts/release.sh`: the two new files.
- `README.md`: "Use Gimbal for Open and Save dialogs".

## 1. How the portal picks a backend

The brief's rule 1 is **not** what 1.21.1 does. The frontend does not stop at
the first configuration file: it loads one file from every directory and asks
each file for the interface in precedence order. This matters, and the code
follows the binary rather than the brief.

### Which file wins

- The search order is `$XDG_CONFIG_HOME/xdg-desktop-portal/`,
  `$XDG_CONFIG_DIRS/xdg-desktop-portal/`, the build-time
  `/etc/xdg-desktop-portal`, `$XDG_DATA_HOME/xdg-desktop-portal/`,
  `$XDG_DATA_DIRS/xdg-desktop-portal/`, then `/usr/share/xdg-desktop-portal`.
  **[read]** `portals.conf(5)` lists the same locations; the binary builds
  them at `0x316a1` (user config), `0x316e1` (system config), `0x31756`
  (`/etc/xdg-desktop-portal`), `0x3178a` (user data), `0x317ca` (system data)
  and `0x31836` (`/usr/share/xdg-desktop-portal`). The upstream 1.21.1 source
  (`src/xdp-portal-config.c`, `load_portal_configurations`) matches.
- Within one directory, the first desktop name in `XDG_CURRENT_DESKTOP`
  (lower-cased, ASCII alphanumerics plus `-` and `_`) that has
  `<desktop>-portals.conf` wins; otherwise `portals.conf` is used.
  `load_config_directory` in the same source. The man page's Budgie:GNOME
  example agrees. **[read]**
- **Files are layered, not merged into one file.** `xdp_portal_config_find`
  loops over every loaded config. For each, the interface-specific line is
  tried, then `default`; the first installed backend wins. A file with no
  FileChooser line and no default does not hide the system file below it.
  **[read]** This contradicts the brief's "the first file found is used
  whole; files are not merged". The code therefore has `config_files` (all
  layers) for the status and `effective_file` (the first file that supplies a
  `[preferred]` group) for the copy the switch makes.
- `XDG_DESKTOP_PORTAL_DIR` replaces the configuration search with that one
  folder as well as the `.portal` folders. **[read]** (source;
  `0x31362`/`0x312d9` check the variable).

### The fallback

`FileChooser=gimbal;gtk;` uses `gtk` only when no `gimbal` `.portal` file is
installed. `xdp_portal_config_find_impl_config_by_iface` looks each name up in
the loaded `.portal` files, skips a missing backend or one that does not list
FileChooser, and returns the first that does. There is no crash check: an
installed backend that crashes is still selected. **[read]** (source and the
man page's "searched for an implementation ... in the same order").

### Where `.portal` files are read from

1.21.1 reads `$XDG_DATA_HOME/xdg-desktop-portal/portals/`,
`$XDG_DATA_DIRS/xdg-desktop-portal/portals/`, then
`/usr/share/xdg-desktop-portal/portals/`. **[read]** The man page names
`$XDG_DATA_HOME` for configuration; the binary proves the same for the
`.portal` folder: `load_installed_portals` in the source, and the disassembly
at `0x315b1` (`g_get_user_data_dir`), `0x315e4` (system data dirs) and
`0x31646` (`/usr/share/xdg-desktop-portal/portals`). The tarball's per-user
install therefore works. `XDG_DESKTOP_PORTAL_DIR` replaces the folder.
**[read]**

## 2. The configuration edit

`fm-core::portals_conf` is the whole text model. Nothing in its tests reads the
real environment; every test passes scratch `user`, `etc` and `usr` folders.
`Locations::from_environment` and `current_desktops` are the production
readers.

The file is split into lines that keep their original ending, so an untouched
line is written back byte-for-byte. The parser understands the INI subset the
man page documents: `[preferred]`, `key=value`, `#` comments, and `;` lists
(trailing `;` allowed). A target with a line outside a group, a line without
`=`, duplicate `[preferred]` groups, or duplicate `default`/FileChooser lines
is refused rather than guessed at. A read-only target is refused before any
write. All writes go through `fm_core::fsutil::save_atomic`
(`fsutil.rs:19-81`); a new file is chmod 0644.

### Before and after

System file, no user file yet:

```ini
[preferred]
default=gtk
org.freedesktop.impl.portal.ScreenCast=wlr
```

After turning the switch on (`~/.config/xdg-desktop-portal/mango-portals.conf`):

```ini
# Copied from /usr/share/xdg-desktop-portal/mango-portals.conf by Gimbal Files when "Use Gimbal for Open and Save dialogs" was turned on.
[preferred]
default=gtk
org.freedesktop.impl.portal.ScreenCast=wlr
# Gimbal Files: was none
org.freedesktop.impl.portal.FileChooser=gimbal;gtk;
```

A user file that already named a backend:

```ini
[preferred]
org.freedesktop.impl.portal.FileChooser=gnome;
```

becomes:

```ini
[preferred]
# Gimbal Files: was org.freedesktop.impl.portal.FileChooser=gnome;
org.freedesktop.impl.portal.FileChooser=gimbal;gnome;gtk;
```

The chain is Gimbal first, the old names with `gimbal` removed, then `gtk` if
it is not already there. `none` and `*` are dropped from the old chain: either
would stop the search before GTK. If there is no file at all, the switch
creates the `[preferred]` group with `default=gtk` and says so in a header.

Turning off restores the marked line exactly, or removes the line when the
marker says `none`. It removes the marker. It deletes the file only when the
copy header is present and the body still matches the file it was copied from
(a trailing newline is normalised); otherwise the body stays, minus Gimbal's
lines. A hand-written FileChooser line naming `gimbal` with no marker loses
just `gimbal`; an empty remainder removes the line.

**Why copying is still safe with layering:** if there is no user file, a
minimal user file would be enough. Copying the effective file keeps the
pre-brief behaviour and preserves every other key in the same file, including
if a future frontend stops layering. Because the copied body is unchanged,
turning off deletes it and the lower layers take over again, exactly as before.

## 3. Restarting the portal

`portals_conf::Restarter` has one method. `SystemdRestarter` in `app.rs` calls
`org.freedesktop.systemd1.Manager.RestartUnit("xdg-desktop-portal.service",
"replace")` through `gio::DBusConnection::call_sync`; it never spawns
`systemctl`. The frontend reads the configuration at startup, so the restart is
what applies it.

Tests use a counting fake; **no test and no run here restarted the real
service**. `enable` and `disable` call the restarter once when they changed
something, and not at all when the file already said what was asked.

If the restart fails, the file stays written, the command still returns
success, and a thread-local flag makes every Settings row say **Saved; sign out
and back in to apply**. The next restart that works clears it. The log line is
`portals: the config is saved but the portal did not restart: …`. The command
handler refreshes every open popover either way.

Before switching, `adw::AlertDialog` asks: **Turn on Gimbal for Open and Save
dialogs?** with the body "The desktop's portal service restarts, so an open
file dialog or screen share in another app will close." Buttons are **Turn On**
and **Cancel** (Turn Off / Cancel going the other way). Cancelling puts the
switch back and writes nothing.

## 4. The Settings switch, status and Test button

The row sits in Integration under the Show-in-folder row, following the same
`filemanager1_widgets` pattern. Its state is not in `config.toml`: opening the
popover recomputes `portals_conf::health` from the files, the `.portal` folders
and `ListActivatableNames`, so a hand edit shows correctly.

Status texts, as the pure `status_text` function:

| State | Text |
| --- | --- |
| no `gimbal.portal` or no activatable name | `Not installed: the portal backend files are missing` (switch and Test insensitive) |
| Gimbal first | `Open and Save dialogs: Gimbal` |
| another installed backend first | `Open and Save dialogs: GTK` (or the backend's name) |
| `none` | `Open and Save dialogs: none` |
| restart failed | `Saved; sign out and back in to apply` |

The **Test** button calls `org.freedesktop.portal.FileChooser.OpenFile` on
`org.freedesktop.portal.Desktop`. It exports the window as a Wayland parent
with `gdk4_wayland::WaylandToplevel::export_handle` (falling back to an empty
parent if the compositor has no `xdg-foreign`) and sends the title "Test
dialog: nothing will be opened". It subscribes to
`org.freedesktop.portal.Request.Response` **before** the call, on the request
path predicted from a `handle_token`, so a fast response cannot be missed. The
status line then shows `Test: <uri>`, `Test: Cancelled`, `Test: no file` or the
call's error. Nothing is opened. The dialog takes focus and may close the
popover, so the last answer is kept in a thread-local and shown again when
Settings is reopened; a switch change clears it.

`integration.set-file-chooser` (`enabled` bool, `Effect::Settings`) is in
`UI_ONLY_SETTINGS_COMMANDS` beside `agent.set-enabled`. The registry refuses
`Hotkey`, `Cli` and `Agent` before the handler; the catalogue and registry
tests cover all four callers. It is not bindable (required argument). The
handler is in `app.rs`'s `settings_change` before the `config::change_for`
lookup, because it writes the portals file rather than `config.toml`.

## 5. The packages

- **.deb**: `scripts/build-deb.sh` installs
  `packaging/portal/gimbal.portal` to
  `/usr/share/xdg-desktop-portal/portals/gimbal.portal` and
  `packaging/dbus/org.freedesktop.impl.portal.desktop.gimbal.service` to
  `/usr/share/dbus-1/services/`, both 0644. `scripts/test-deb.sh` asserts the
  exact contents list, the modes, the `Name`, `DBusName`,
  `Interfaces=…FileChooser;` and `Exec=/usr/bin/gimbal-files --portal`.
- **Tarball and source**: `scripts/release.sh` stages both files.
  `scripts/install.sh` installs the `.portal` into
  `$data_dir/xdg-desktop-portal/portals/` and rewrites the portal service's
  `Exec` to the quoted installed binary with `--portal`, the same way the
  FileManager1 service is rewritten. `--uninstall` removes both. Because rule
  3 confirmed `$XDG_DATA_HOME` is read, there is no "cannot register" branch.
- `scripts/test-install.sh` covers `--prefix`, `--data-dir`, the odd prefix
  with a single quote and a backslash, reinstall, uninstall and the kept
  neighbouring files, for both service files and the `.portal`.
- **Uninstall safety**: because the chain is always `gimbal;…;gtk;`, removing
  the package leaves GTK working (rule 2). No package script or installer
  touches a user's portals configuration; only the switch does. The README
  says both.

## 6. The README

A new "Use Gimbal for Open and Save dialogs" section covers the switch, the
file it writes, the marker, the confirmation and restart, the Test button, a
manual undo, the app table (Flatpak; Zen/Firefox's
`widget.use-xdg-desktop-portal.file-picker = 1`; GTK3 `GTK_USE_PORTAL=1`; GTK4
`GDK_DEBUG=portals`; Qt `QT_QPA_PLATFORMTHEME=xdgdesktopportal`; Electron and
Chromium usually; LibreOffice/ONLYOFFICE depends), and the GTK fallback after
removal.

## API citations

- `xdg-desktop-portal 1.21.1:src/xdp-portal-config.c` (read from the 1.21.1
  tag): `load_installed_portals`, `load_portal_configurations`,
  `load_config_directory`, `xdp_portal_config_find`,
  `xdp_portal_config_find_impl_config_by_iface`.
- `/usr/libexec/xdg-desktop-portal` (stripped 1.21.1): strings
  `XDG_DESKTOP_PORTAL_DIR`, `%s-portals.conf`, `portals`,
  `/etc/xdg-desktop-portal`, `/usr/share/xdg-desktop-portal`,
  `/usr/share/xdg-desktop-portal/portals`; code at `0x312d9`, `0x31362`,
  `0x315b1-0x31655`, `0x316a1-0x3185a`.
- `portals.conf(5)`, `/usr/share/man/man5/portals.conf.5.gz`: search order,
  case-folding, `[preferred]`, `;` lists, `none`, `*`.
- `glib-0.22.10:src/auto/functions.rs:446-476`:
  `system_config_dirs`, `system_data_dirs`, `user_config_dir`,
  `user_data_dir`.
- `gio-0.22.10:src/dbus_connection.rs:613-655` (`subscribe_to_signal`) and
  `:125-160` (`SignalSubscription`); `gio-0.22.10:src/auto/dbus_connection.rs:84`
  (`call`).
- `gdk4-wayland-0.11.5:src/wayland_toplevel.rs:9-57` (`export_handle`).
- `adw-0.9.2` `AlertDialog::new` / `add_response` / `connect_response` /
  `choose`, as the existing dialogs use them
  (`fileops/dialogs.rs:390-417`).
- `fm-core/src/fsutil.rs:19-81` (`save_atomic`, mode preservation, new-file
  umask plus the explicit 0644).

No new dependency and no `unsafe`. `fm-core` still has no GTK.

## Tests and gates

`scripts/check.sh` with the 1.95.0 toolchain (`PATH="$T:$PATH"`) passes all five
gates: fmt, clippy with `-D warnings`, tests, release build and `cargo deny
check` (`advisories ok, bans ok, licenses ok, sources ok`). **[observed]**

`cargo test --workspace`: **1,183 passed, 0 failed, 30 ignored** (summed
`test result:` lines; P2au was 1,158 / 30). **[observed]** The first run of
`check.sh` hit the existing real-save-dialog timing flake
(`"notes.md "` with a trailing space); the test passed alone and the full
workspace run after it passed. Nothing here touches the save-dialog code.

New tests:

- `portals_conf::tests::the_first_desktop_wins_across_every_root`,
  `a_desktop_file_beats_a_plain_user_file`,
  `a_plain_file_is_used_when_no_desktop_file_exists`,
  `config_files_keeps_the_lower_layers`, `a_file_without_preferred_is_skipped`.
- `chains_read_interface_then_default_and_keep_semicolon_lists`,
  `the_effective_chain_layers_and_skips_uninstalled_backends`.
- `enable_creates_a_copy_of_the_system_file_and_keeps_other_interfaces`,
  `enable_keeps_an_existing_filechooser_and_marks_the_exact_old_line`,
  `enable_when_already_on_changes_nothing_and_does_not_restart`,
  `enable_with_no_effective_file_creates_a_minimal_one`.
- `disable_restores_a_user_file_byte_for_byte_and_deletes_a_copy`,
  `disable_leaves_a_copy_that_was_edited_after_enabling`,
  `disable_removes_a_hand_written_gimbal_with_no_marker`,
  `disable_is_a_no_op_when_the_file_does_not_name_gimbal`.
- `an_unparsable_target_is_refused_and_left_alone`,
  `an_unwritable_target_is_refused_and_left_alone`,
  `a_failed_restart_keeps_the_file_and_reports_it`,
  `a_successful_restart_clears_the_pending_flag`, `round_trips_keep_every_other_byte`.
- `health_and_status_text_cover_every_state`,
  `desktop_names_are_split_validated_and_lowercased`,
  `portal_dirs_follow_the_environment_override`.
- `catalogue::tests::file_chooser_is_a_ui_only_settings_command_with_a_required_bool`
  and `command::tests::the_file_chooser_switch_is_ui_only_and_refused_for_every_other_caller`.
- `scripts/test-install.sh` and `scripts/test-deb.sh` pass, including the
  two new files, their modes, their `Exec` rewrite and uninstall. **[observed]**

## Live checks for Claude and Kyle

No pointer or keyboard input was used here, and Gimbal was not launched. The
run script must use scratch `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and
`XDG_DATA_HOME` for anything that is not the real switch.

1. Install the new `.deb`, open Settings: the status first says GTK (or the
   current backend). Turn the switch on, approve the restart: the file gets
   the copied header, the marker and `gimbal;…;gtk;`; the status says Gimbal.
   Test shows Gimbal's dialog and an answered URI.
2. **Zen** (with `widget.use-xdg-desktop-portal.file-picker = 1` if it is not
   a Flatpak): Open one, Open several, Save with a filter, Save over an
   existing file, Cancel.
3. A Flatpak app, an Electron app (VS Code or Claude desktop), a GTK4 app
   launched with `GDK_DEBUG=portals`, a Qt app launched with
   `QT_QPA_PLATFORMTHEME=xdgdesktopportal`, and ONLYOFFICE: Open and Save each.
4. Close the parent app while its dialog is open: the dialog goes away. Kill
   `gimbal-files --portal` mid-dialog: the app gets a failure or cancel, not a
   hang.
5. Turn the switch off: the config is exactly as before and Test shows the GTK
   dialog again.
6. Remove the package (or rename `gimbal.portal`) with the switch on: apps
   fall back to GTK, and Settings says Not installed.

The one thing this build cannot settle without a live restart is the
frontend's restart path on Kyle's Mango session (`RestartUnit` succeeds and the
next Test uses the new configuration). Everything before that is covered by
the pure tests and the scratch install tests.

## Anything to change outside these files

Nothing was needed. The existing `every_settings_command_has_a_change_for`
test in `config.rs` needed the one-command exemption described above; it is
the only change outside the brief's main list, and it is a test correction, not
a behaviour change.

## Claude's review (Nov 21)

Read the notes and the diff, and checked the safety rules on Kyle's machine:
- `~/.config/xdg-desktop-portal/` still holds only the Nov 8 `niri-portals.conf`;
- `xdg-desktop-portal.service` was last started at 07:17, before this work;
- no portal source tree was saved to disk. The agent read upstream 1.21.1 online, which the
  rules allow ("verify, don't recall"; only downloads are restricted).

**The agent was right to override the brief.** My rule 1 said the first config file is used
whole. 1.21.1 actually loads one file per folder and asks them in order, so lower files still
apply when a higher one is silent. The agent showed this from the man page, the binary and the
source, and kept the full copy on enable anyway. That copy is harmless with layering, and safe if
a future frontend stops layering.

**A dry run on Kyle's real layout** (a one-off test since deleted). It used scratch
`config_home` seeded with a copy of Kyle's `~/.config/xdg-desktop-portal/`, Kyle's real system
folders read-only, `XDG_CURRENT_DESKTOP=mango` and a fake restarter. Enable wrote
`mango-portals.conf` as a copy of `/usr/share/xdg-desktop-portal/mango-portals.conf`. It kept
`ScreenCast=wlr`, `Screenshot=wlr` and `Inhibit=none`, and added `# Gimbal Files: was none` and
`FileChooser=gimbal;gtk;`. Disable deleted the copy. The niri file was untouched.

**Verified:** `scripts/check.sh` exit 0, 1183 passed, 0 failed, 30 ignored. `scripts/test-install.sh`
PASS and `scripts/test-deb.sh` PASS (the `.deb`, a two-version signed repo and a scratch apt).

**One note, not fixed:** disable deletes the copy only if its body still equals the **current**
system file. If a package update changes `/usr/share/xdg-desktop-portal/mango-portals.conf`
while the switch is on, turning it off leaves the old copy in place, minus Gimbal's lines, and
that copy then shadows the new system defaults. It's rare and visible in the README's manual
undo. A fix would record a hash of the copied text in the header; that can be a follow-up if it
ever matters.

**The agent's flaky run:** the first `check.sh` run saw `"notes.md "` with a trailing space in
the real save-dialog test. That's the known effect of a test window on Kyle's desktop receiving
a real keystroke (P2at review), not a code fault.

**Live matrix:** needs the new package installed (sudo, Kyle) and the switch turned on in
Kyle's real session. Both are Kyle's to approve. Planned with Kyle after commit.
