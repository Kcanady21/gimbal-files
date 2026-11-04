# P2s: unmount and eject from the Devices sidebar

## What this change does

- **Two reviewed commands.** `drive.unmount device=<text>` and
  `drive.eject device=<text>` are `Effect::System`, have no undo, take the same
  opaque process-local `volume-N` id as `drive.mount`, and stay UI-only through
  the existing socket/CLI `System` refusal. The socket capability list and
  `docs/agent-socket.md` now name all three drive operations as unavailable.
- **One shared attempt model.** The controller has a separate
  `RemovalRequest` with a process-monotonic attempt token, the exact volume
  incarnation and mount object, visible ownership, its own cancellable and
  `MountOperation`, one-slot backend accounting, and stale delivery guards.
  It reuses P2p's four-operation cap. A callback from an older attempt can
  retire only its own backend slot; it cannot finish, fail, cancel or
  re-target a newer request.
- **Backend calls.** Unmount is
  `Mount::unmount_with_operation(MountUnmountFlags::NONE, Some(&GtkMountOperation), …)`.
  Eject is `Mount::eject_with_operation(MountUnmountFlags::NONE, …)`; GIO
  unmounts every mount on the drive and powers it down where the service can.
  `FORCE` is never passed. The operation is parented to the window that asked.
- **Capabilities.** `RawMount` carries the live mount's `can_unmount` and
  `can_eject`; `DeviceState::Mounted` and `DeviceEntry` expose them. Unmount and
  Eject appear only when the live backend says yes. No path list decides. The
  main drive and root are never `DeviceEntry` rows, so they offer neither action.
- **Mounted-row menu.** Open, Open in new tab, then Unmount when allowed and
  Eject when allowed. While an operation runs the same row shows
  `Unmounting…`/`Ejecting…`/`Cancelling…` with P2p's Cancel control, and the
  menu carries Cancel instead of another drive action. Every window reads the
  one shared request table.
- **Success and failure.** Success refreshes the inventory: an unmounted row
  returns to `Not mounted` and loses its usage bar; a successful eject removes
  the volume row where the volume is gone. A terminal failure keeps the drive
  mounted, rebuilds any listing the release step dropped and shows the short
  message in the initiating window. A successful eject shows
  `<label> can be removed safely` through P2k's existing summary line.
- **Gimbal releases its own handles first.** Before the backend sees the
  request the UI gate:
  1. refuses when a running, queued or undoable file operation has a source
     or destination under the root:
     `<label> is in use by a file operation. Wait for it to finish or cancel it.`;
  2. refuses when a preview editor has unsaved changes to a file under the
     root: `Save or discard the changes to “<name>” first`;
  3. cancels folder-size walks under the root;
  4. closes search sessions whose root/current walk is under the root;
  5. cancels thumbnail demand and preview loads/monitors for files under the
     root;
  6. drops folder monitors for every single view and split tile showing a
     folder under the root.

  `GIMBAL_LOG=1` writes one `volumes:` line per request, release step, backend
  result, refusal and tab relocation.
- **Busy and errors.** Shared error mapping distinguishes cancellation,
  permission denial, removal, unsupported service, busy/would-block, a dialog
  GTK already showed (`FailedHandled`) and generic failure. Busy without a GTK
  process dialog becomes `<label> is busy: another app is using files on it`.
  An authentication dialog the user cancels is `Cancelled`, not an error.
- **Tabs on a disappeared mount.** The workspace has one pure relocation rule.
  `Workspace::relocate_removed_mount(root, home, is_dir)` moves every tab and
  split tile whose folder starts with the vanished root to the nearest existing
  parent, else home. History is untouched (Back to the missing folder follows
  ordinary navigation failure), the Recent Directories list is untouched, and
  strip position, group membership and split layout are untouched. The UI
  compares the mounted-root set on the volume notifications it already receives,
  so an unmount from this feature, a pulled cable and another application all
  take the same path.

No new dependency, manifest/lockfile change, config or session schema change,
or socket protocol change. No `udisksctl`, zbus/udisks2, fstab or shell command
was added. No real volume was mounted, unmounted or ejected by this agent.

## Files written

- `crates/fm-core/src/command/catalogue.rs`
- `crates/fm-core/src/command/tests.rs` (the existing exact catalogue-order
  test must list the two new rows; this is the one file outside the brief's
  literal Output list — noted below)
- `crates/fm-core/src/volumes.rs`
- `crates/fm-core/src/workspace/mod.rs`, `crates/fm-core/src/workspace/tests.rs`
- `crates/fm-ui/src/volumes/mod.rs`, `volumes/gio_backend.rs`, `volumes/tests.rs`
- `crates/fm-ui/src/sidebar.rs`, `app.rs`, `frame.rs`
- Release plumbing, only as item 3 needs it:
  `crates/fm-ui/src/folder/size.rs`, `folder/view.rs`, `search/mod.rs`,
  `thumbs.rs`, `preview.rs`, `content.rs`, `split.rs`, `fileops/mod.rs`
- `docs/notes/P2s.md`, `docs/agent-socket.md`

`crates/fm-ui/src/commands.rs` needed no change: `UiHandler` already forwards
every registry command to `Process::handle_command`.

## API citations

Checked against the pinned sources:

- `gio-0.22.10:src/auto/mount.rs:32` `MountExt::can_eject`;
  `:37` `MountExt::can_unmount`;
  `:42` `eject_with_operation(flags, mount_operation, cancellable, callback)`;
  `:358` `unmount_with_operation(flags, mount_operation, cancellable, callback)`.
- `gio-0.22.10:src/auto/flags.rs:2438-2447` `MountUnmountFlags` has only `NONE`
  and `FORCE`; this code uses `NONE`.
- `gio-0.22.10:src/auto/enums.rs:2094` `IOErrorEnum`; `:2133` `Cancelled`,
  `:2123` `PermissionDenied`, `:2125` `NotSupported`, `:2127` `NotMounted`,
  `:2147` `Busy`, `:2149` `WouldBlock`, `:2155` `FailedHandled`.
- `gtk4-0.11.5:src/auto/mount_operation.rs:26` `MountOperation::new(parent)`.
- GTK C source `../gimbal-refs/gtk-4.22.4/gtk/gtkmountoperation.c:75` documents
  the operation as showing “processes blocking unmount”; `:1448`
  `create_show_processes_dialog` builds the list dialog from the backend's
  process array and choices; `:1670` `gtk_mount_operation_show_processes`
  presents it. The message (“these applications are using the drive”) comes
  from the volume service, while GTK owns the process list and choices.
  A `FailedHandled` result is therefore treated as already shown and gets no
  second summary.
- `gio-0.22.10:src/auto/volume.rs:32` `VolumeExt::can_eject` and
  `drive.rs:32` `DriveExt::can_eject` were checked; per this brief the mounted
  device row follows the `Mount`'s own answer because the call is
  `Mount::eject_with_operation`.

## Tests and verification

`scripts/check.sh` from the project root
(`/tmp/gimbal-p2s-check-final.log`) passed all five gates:

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
==> cargo build --workspace --release
==> cargo deny check
```

Test totals: **805 passed, 22 ignored** across the workspace. P2s adds 19
passing tests (3 `fm-core::volumes`, 3 workspace relocation, 1 catalogue row,
12 `fm-ui::volumes`); the remaining increase over the brief's 778 baseline is
the parallel, uncommitted P2r sort work present in this checkout. No new test
is ignored.

Focused runs used during development:

```
cargo test -p fm-core volumes            # 13 passed
cargo test -p fm-core workspace::tests::a_vanished   # 2 passed
cargo test -p fm-core workspace::tests::root_and_unrelated_folders_are_never_relocated
                                         # 1 passed
cargo test -p fm-core command            # 57 passed
cargo test -p fm-ui volumes::tests       # 38 passed
cargo test -p fm-ui real_sidebar_mount_menu_pending_cancel_and_live_refresh
                                         # 1 passed, 1 ignored inner
```

New `fm-core::volumes` tests:

- `removal_targets_obey_the_mount_capabilities_and_pending_state`
- `root_and_system_mounts_never_become_removal_targets`
- `same_root_replacement_gets_a_new_removal_mount_object`

New workspace tests:

- `a_vanished_mount_moves_tabs_and_split_tiles_to_its_nearest_existing_parent`
- `a_vanished_mount_falls_back_to_home_and_moves_tabs_in_every_window`
- `root_and_unrelated_folders_are_never_relocated`

New `fm-ui::volumes` tests (all fake backend, no real device):

- `unmount_success_returns_the_row_to_unmounted_and_drops_its_bar`
- `eject_success_removes_the_volume_row_and_reports_it_safely_removable`
- `capabilities_and_pending_state_refuse_before_the_backend`
- `busy_and_auth_cancelled_are_distinct_and_leave_the_drive_mounted`
- `cancel_cancels_only_that_requests_own_operation`
- `a_stale_removal_callback_cannot_finish_a_replacement_attempt`
- `release_runs_before_the_backend_call`
- `a_refused_gate_never_reaches_the_backend`
- `registry_unmount_and_eject_route_through_the_shared_controller`
- `a_running_file_operation_refuses_unmount_before_the_backend`
- `a_dirty_editor_refuses_unmount_before_the_backend`
- plus the existing mount/usage regressions unchanged.

The real-GTK subprocess sidebar regression now also checks a mounted row's
real Unmount/Eject menu, the shared `Unmounting…` pending row with Cancel, and
the return to `Not mounted` with no bar after success. The dirty-editor refusal
test runs through the headless process's preview seam and then the real
`Process::before_drive_removal` gate; the production dirty check itself reads
`PreviewPanel::dirty_file_name_under` (type-checked, and the same real panels
are built by the GTK widget regression).

### Evidence classes

- **Source reasoning [read]:** all API citations above; the opaque-id,
  attempt-token, slot-accounting and stale-callback rules are the P2p/P2p-r1
  code extended with a parallel removal request table.
- **Automated fake-backend tests [observed]:** identity, targets/refusals,
  success, busy, cancel, auth-cancelled, stale callback, release-before-backend
  order, file-operation refusal, dirty-editor refusal, relocation, command
  routing and the real widget menu/pending/success path.
- **Actual hardware live observation:** none by this agent. No app run was made
  and no real GIO mount was unmounted or ejected.

## Not verified live (Claude and Kyle hardware checks)

On a USB stick:

1. Mount it, open it in a tab, right-click → Unmount: the row returns to
   unmounted and the tab moves to the parent folder. `findmnt` no longer lists it.
2. Mount again and remount after unmount (the P2p-r1 remount case, never checked
   on hardware).
3. Open a file from the stick in another app, then Unmount: Gimbal shows
   “busy” and the stick stays mounted and usable.
4. Hover a large folder on the stick (folder-size walk running), then Unmount at
   once: it succeeds, because Gimbal released its own handles.
5. Eject: the row goes away and “can be removed safely” appears; pulling the
   stick afterwards produces no error.
6. Pull the stick while a tab shows it, without unmounting: the tab moves to the
   parent and nothing crashes.
7. The internal drives that P2p mounted (Nvme-Storage-1, WD-Blue-SSD) offer
   Unmount; the main drive and root offer neither action.

## Outside these files

- `crates/fm-core/src/command/tests.rs` had to change: its
  `the_v1_catalogue_has_every_reviewed_command_in_order` test hard-codes the
  complete ordered name list, so adding the two reviewed commands without
  editing it would fail the gate. No production behaviour outside the brief was
  changed. The other option was to leave the catalogue test red.
- The plan's historical “Mount a drive now (right-click)” row still names only
  mounting; this brief supersedes it with the two reviewed commands. I did not
  edit the plan file.
- The checkout also has parallel, uncommitted P2r sort work in
  `catalogue.rs`, `command/tests.rs`, `config.rs`, `app.rs` and `header.rs`.
  P2s preserved it; the gate counts above include it.

## Review (Claude, Nov 4, 2026)

**Code review passes. Hardware checks pending (Kyle; USB stick 6BF9-DAAC is mounted as sdb1).**

- Gates: `scripts/check.sh` exit 0 on the combined P2r + P2s tree, 805 passed / 23 ignored.
  [observed]
- `touches_root` checks only running and queued jobs (plus a running Undo). "Undoable" in the
  summary above means a queued Undo request; a finished copy to the stick does not block unmount.
  [read]
- Relocation is fed only by Devices rows (`mounted_roots`). The main drive and `/` are never rows, so
  a false "vanished" root can't move home tabs. A pending unmount keeps the entry `Mounted` (pending
  state lives in the request table), so tabs move only after the backend reports the mount gone,
  never at the start of an attempt that later fails as busy. [read]
- Every `Failed` kind, including `Cancelled` and `Handled`, runs `refresh_all_windows`, so views
  emptied by the release step come back. [read]
- Expected trade-off: a search under the stick is closed by the release step even if the unmount
  then fails.
