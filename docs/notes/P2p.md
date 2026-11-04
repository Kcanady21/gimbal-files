# P2p: mount local volumes from the Devices sidebar

## What this change does

- **One inventory for the process.** `fm-core::volumes::Inventory` folds a
  backend `RawSnapshot` into `DeviceEntry` rows: unmounted local volumes,
  mounted volumes, and mounts with no `Volume` (navigation-only mounts). A
  mounted volume and its mount are one row by object/root. `/` is never a
  device row; it stays the separate Main Drive row.
- **Opaque, non-reusable identities.** An entry's `VolumeId` (`volume-N`) and
  incarnation are assigned by the inventory. A surviving backend object keeps
  its id/incarnation across rebuilds and monitor reordering. Removing an object
  retires that id permanently; reinserting the same label, UUID and
  `/dev` unix-device identifier produces a new object serial and a new id. A
  stale command therefore fails with `UnknownId` and can never be retargeted at
  the replacement.
- **Only native local mount candidates.** `Inventory::mount_target` accepts
  only an unmounted entry with `can_mount` and GIO's `unix-device` identifier.
  Labels, arbitrary paths and remote/activation-only URIs are never mount
  specifications. Local volumes without a unix device or without `can_mount`
  remain visible with a short explanation and no Mount item.
- **Explicit right-click mount.** An unmounted row shows its name and
  `Not mounted`, hides the drive bar and has no folder/drop target. A primary
  click explains `Right-click to mount` (or the ineligibility reason). The
  context menu carries that row's opaque id into the existing
  `drive.mount device=<id>` command through `Registry` as `Caller::Ui`.
- **Shared request state.** `fm-ui::volumes::Volumes` owns the one inventory,
  request table and usage queries. `Mounting…` appears immediately in every
  window, duplicate requests for the same id are refused, and at most four
  requests are active process-wide. A fifth request is reported busy; no
  authentication prompt is queued.
- **Truthful results.** A callback result is never treated as a mounted
  volume. Success refreshes the inventory and waits for the actual mount/root;
  a short bounded reconcile poll handles callback-before-monitor. Monitor
  arrival before the callback completes the request first, and the late
  callback is idempotent. Cancellation is shown as `Mount cancelled`,
  permission denial as `Permission denied`, removal as `Device removed`, and a
  missing/unsupported volume service distinctly. If a cancelled callback races
  a real mount, the inventory wins and the row is mounted.
- **Explicit cancellation.** The pending row has a `Cancel` button whose
  accessible label is `Cancel mounting <name>`. It cancels that request's own
  `gio::Cancellable`; it is request lifecycle, not a new command, and it never
  unmounts a completed mount. Closing the initiating window cancels only that
  window's request. Quit cancels every pending request and usage query without
  joining a worker.
- **Nonblocking usage.** The old synchronous `query_filesystem_info` call in
  `sidebar.rs` is gone. Mounted entries and Main Drive use cancellable async
  `GFile::query_filesystem_info_async` queries shared by root and exact
  volume/mount incarnation. Four queries run at once, 32 may wait, excess rows
  say `Usage busy`, and a failed query says `Usage unavailable` rather than
  drawing a zero-full bar. Stale roots and incarnations are ignored. The
  existing bar ramp, scale tokens and full-drive contrast are unchanged.
- **No new dependency, protocol or automation path.** No manifest/lockfile,
  registry command, permission or config/session schema changed. No zbus or
  udisks2 crate was added; GIO's installed desktop volume monitor is used.
  `fm-agent` still refuses every `System` operation, including `drive.mount`;
  no socket state/query/navigation/startup path mounts anything, and no CLI
  bypass exists.

## Files written

- `crates/fm-core/src/volumes.rs`, `crates/fm-core/src/lib.rs`
- `crates/fm-ui/src/volumes/mod.rs`, `volumes/gio_backend.rs`,
  `volumes/tests.rs`, `crates/fm-ui/src/main.rs`
- `crates/fm-ui/src/sidebar.rs`, `app.rs`, `frame.rs`
- `crates/fm-ui/src/look/css.rs` (row status/cancel and menu-note styles)
- `docs/notes/P2p.md`, `docs/agent-socket.md`

## Backend and identity contract

`GioBackend` enumerates `gio::VolumeMonitor` volumes and mounts. It does not
speak D-Bus directly and does not call `/dev`, mount, udisksctl, sudo or a shell.
It records the monitor's actual GType name and current counts in
`Volumes::backend_note()` and logs that once at startup, for example
`GIO volume monitor GProxyVolumeMonitorUDisks2; N volume(s), M mount(s)`.
The concrete desktop service is therefore recorded rather than assumed; a
different desktop may expose a different monitor.

Object serials are tied to the live GObject, not to labels or UUIDs. The GIO
adapter keeps a `glib::WeakRef` per object pointer; a new object at a reused
address gets a new serial. This is what makes remove/reinsert with identical
metadata produce a new opaque id.

The mount call is `VolumeExt::mount(MountMountFlags::NONE, Some(&gtk::MountOperation::new(Some(parent))), Some(&cancellable), callback)`.
The operation is parented to the initiating `gtk::Window`. The desktop volume
service performs any authentication through that operation; this code never
reads, stores, remembers or logs credentials, and never requests privilege.

## API citations

All checked against the pinned crate sources:

- `gio-0.22.10:src/auto/volume.rs:37` `VolumeExt::can_mount`;
  `:153` `identifier`; `:191` `uuid`; `:196` `VolumeExt::mount(flags,
  mount_operation, cancellable, callback)`.
- `gio-0.22.10:src/auto/volume_monitor.rs:32` `VolumeMonitorExt`;
  `:55` `mounts`; `:76` `volumes`; `:241` `mount-added`; `:272`
  `mount-changed`; `:334` `mount-removed`; `:365` `volume-added`; `:396`
  `volume-changed`; `:427` `volume-removed`.
- `gio-0.22.10:src/auto/mount.rs:111` `name`; `:139` `uuid`; `:146` `root`;
  `:174` `volume`.
- `gio-0.22.10:src/auto/file.rs:1677`
  `query_filesystem_info_async(attributes, priority, cancellable, callback)`.
- `gio-0.22.10:src/auto/flags.rs:2359` `MountMountFlags` (`NONE`).
- `gio-0.22.10:src/auto/cancellable.rs:43` `cancel`; `:56` `is_cancelled`.
- `gtk4-0.11.5:src/auto/mount_operation.rs:26` `MountOperation::new(parent)`.
- `gtk4-0.11.5:src/accessible.rs:31` `update_property`; `:105`
  `Property::Label` (the Cancel control's accessible name).
- `glib-0.22.10:src/object.rs:3510` `WeakRef`; `:3550` `upgrade`.
- `glib-0.22.10:src/source.rs:353` `idle_add_local_once` (coalesced monitor
  rebuild); `:532` `timeout_add_local_once` (bounded callback/monitor
  reconcile).

## Production-path tests

All mount/usage tests inject `FakeBackend`; no test opens, mounts, unmounts or
ejects a real device, creates a loop device, changes a mount namespace or runs
sudo/mount/udisksctl.

### `fm-core::volumes` (8)

- `mounted_unmounted_and_navigation_mounts_dedupe_and_root_is_never_a_device`
- `duplicate_labels_are_not_identity`
- `reinsert_with_the_same_device_uuid_and_label_gets_a_new_id`
- `stable_objects_keep_their_id_and_order_across_unrelated_rebuilds`
- `unsupported_and_non_native_volumes_explain_the_refusal`
- `remote_volume_without_a_drive_or_native_id_is_not_a_device_row`
- `mount_targets_bind_the_live_incarnation_and_reconcile_only_that_one`
- `empty_or_missing_labels_fall_back_without_becoming_identity`

### `fm-ui::volumes` (20)

- `duplicate_requests_across_windows_call_the_backend_once`
- `refusal_and_stale_ids_never_reach_the_backend`
- `the_process_wide_cap_is_four_and_releases_only_at_a_terminal_result`
- `cancellation_denial_and_failure_are_distinct_and_retry_is_explicit`
- `removal_revokes_pending_ownership_and_cancels_the_operation`
- `closing_one_window_cancels_only_its_own_request`
- `late_success_cannot_revive_an_old_id_or_target_a_replacement`
- `callback_before_monitor_stays_pending_then_reconciles`
- `monitor_before_callback_completes_once_and_late_callback_is_idempotent`
- `cancelled_callback_racing_a_live_mount_reports_the_actual_mounted_state`
- `an_externally_mounted_volume_needs_no_request`
- `mounts_without_a_volume_stay_visible_and_do_not_duplicate_a_volume`
- `usage_queries_are_shared_bounded_and_not_restarted_by_event_floods`
- `main_usage_is_one_shared_query`
- `usage_pending_requests_stop_at_thirty_two_and_rows_say_busy`
- `stale_usage_results_and_replaced_roots_never_acquire_bars`
- `failed_usage_is_unavailable_not_zero`
- `shutdown_cancels_requests_and_usage_without_waiting`
- `registry_mount_refuses_unknown_ids_before_the_backend_and_never_touches_config`
- `registry_mount_accepts_pending_and_duplicate_across_windows_calls_once`

The two `registry_...` tests run the real `Registry` → `commands::UiHandler` →
`Process::handle_command` → shared controller → injected backend path on a
headless process. They assert no backend call on refusal, one call for a
duplicate across two windows, pending-not-mounted acceptance, and unchanged
config/workspace.

### Real GTK widget/main-loop regression

`app::tests::real_sidebar_mount_menu_pending_cancel_and_live_refresh` runs in
its own process (the existing owned-GTK subprocess pattern) and builds the real
`WindowUi`, header, content and `Sidebar` over an injected inventory:

- unmounted row text `Not mounted`, no bar, no Cancel;
- an ineligible local volume has no Mount item and keeps its explanation;
- the real context-menu `Mount` button carries the row id and reaches the real
  controller (one backend call);
- pending row text `Mounting…`, Cancel visible;
- no second Mount item while pending;
- real Cancel button cancels the request's `gio::Cancellable`;
- monitor-first completion refreshes the live row to a usage bar;
- a callback-before-monitor request stays pending until the live root appears,
  then its real row gains the usage bar;
- late callback is idempotent;
- Main Drive uses the async usage path;
- blocked usage + 20 monitor floods keep the four-active/32-pending caps and
  the widget/main loop still responds.

## Verification

`scripts/check.sh` results:

```
==> cargo fmt --all --check     PASS
==> cargo clippy --workspace --all-targets -- -D warnings     PASS
==> cargo test --workspace     PASS (see counts below)
==> cargo build --workspace --release     PASS
==> cargo deny check     PASS
```

Final test counts: **762 passed, 20 ignored** across the workspace. The
baseline was 733 passed, 19 ignored; this change adds 29 passing tests and 1
ignored subprocess inner (the sidebar widget wrapper adds one passing outer and
one ignored inner).

Additional focused runs during development:

```
cargo test -p fm-ui volumes::            # 20 passed
cargo test -p fm-core volumes            # 8 passed
cargo test -p fm-ui real_sidebar_mount_menu_pending_cancel_and_live_refresh
                                           # 1 passed, 1 ignored inner
```

### Evidence classes

- **Source reasoning [read]:** API signatures and signal names cited above;
  the caller policy is the unchanged registry code and the existing explicit
  `Effect::System` refusal in `fm-ui/src/agent/mod.rs` and `docs/agent-socket.md`.
- **Automated fake-backend tests [observed]:** every identity, refusal, cap,
  callback/monitor order, cancellation/denial/error, removal, window-close,
  late-callback, retry and usage test listed above.
- **Automated real-widget observation [observed]:** the subprocess GTK test
  above, including the actual menu button, pending Cancel control and live row
  refresh.
- **Actual hardware live observation:** none by this agent. No app run was made
  and the real GIO mount adapter was never invoked, so no real device was
  mounted, unmounted or ejected.

## Not verified live (Kyle/Claude checks)

1. An unmounted local volume actually appears in Devices with `Not mounted`,
   no bar and the right-click Mount item on Kyle's desktop.
2. An explicit Mount starts, immediately shows `Mounting…`, and a normal
   desktop authentication prompt appears when the volume service requires one.
3. Successful mount, user Cancel, permission denial and a real failure each
   show the correct distinct row text; the failed row offers a deliberate retry.
4. A mount completion refreshes every open window's sidebar without moving
   tabs, searches, selections or previews.
5. Clicking the newly mounted row opens its actual native root, and its usage
   bar/capacity match the mounted filesystem.
6. Removing the device (or its volume service) while pending shows
   `Device removed`/`Volume service unavailable`, cancels the request and does
   not revive the old row after reinsertion.
7. Ordinary browsing, folder navigation, drag/drop, search and preview behavior
   remain unchanged when no mount is pending.

## Outside these files

Nothing needs changing outside the brief's output. The plan's historical
"UDisks2 `Filesystem.Mount` over D-Bus" wording is superseded by this brief's
GIO-only rule; the plan file was not edited. No `Cargo.toml`/lockfile, registry
catalogue, config/session schema, socket protocol, runner or Zen file changed.

## Review (ChatGPT, Nov 4, 2026) — revision needed

**[observed]** All five host gates passed with 762 passed and 20 ignored.
Log is /tmp/gimbal-p2p-review-check.log. Staged work was not changed.

**[read]** The existing command reaches the shared controller, the production
GIO adapter uses asynchronous mounting, and the socket's System refusal stays
in place. Widget tests cover the menu and Cancel through a fake backend.

- **[P2] Mount callbacks identify the volume instead of the attempt.** A
  monitor-first completion clears the pending record. After an unmount and
  retry on the same volume incarnation, the old callback or reconciliation timer
  can act on the new request. Match a request token as well as volume identity.
- **[P2] Backend concurrency can exceed four.** Monitor completion and removal
  release a slot in complete_request before the backend call reports completion.
  Keep a separate count of outstanding calls until their callbacks retire.
- **[P2] Usage can cross a same-root remount.** Mounted DeviceState drops the
  RawMount object identity and UsageKey relies on volume incarnation plus root.
  A coalesced replacement can reuse stale capacity or accept stale evidence.
  Carry the actual mount identity into the usage key and invalidation path.

These are source findings. Hardware mounting awaits Kyle's live check after
P2p-r1. The reviewer ran the isolated automated gates and made no source patch.

## P2p-r1 correction (Nov 4, 2026)

`docs/notes/P2p-r1.md` resolves the three review findings above. The earlier
request and usage descriptions remain the historical P2p record, with these
corrections:

- A mount request is now identified by a per-attempt token, not only by volume
  id/incarnation. Monitor completion, removal and window close retire visible
  ownership; an old callback can retire only its own backend accounting.
- The four-request cap counts outstanding backend calls until their callbacks
  (or a synchronous launch failure), not visible pending rows. Monitor
  completion and removal no longer free a slot early.
- A mounted `DeviceState` carries the live mount object, and `UsageKey` includes
  it for device rows and Main Drive. A same-volume/same-root remount retires
  the old result and queries the new mount. The four-active/32-pending query
  bounds and refresh deduplication are unchanged.
- The r1 test totals supersede the P2p totals: **770 passed, 20 ignored**.
  `fm-core::volumes` has 10 tests and `fm-ui::volumes` has 26. All five gates
  pass. The new fake-backend regressions are listed in `P2p-r1.md`.
- The hardware checks in P2p remain open, with the four additional same-root
  retry/cap/remount checks in `P2p-r1.md`.
