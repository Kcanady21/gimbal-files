# P2p-r1: mount attempts and usage identity

## What this change does

- **Each accepted mount attempt has its own token.** A monitor completion,
  removal, window close or retry retires the old attempt's visible ownership;
  a callback carrying an old token can release only its own backend slot. It
  cannot complete, fail, cancel or schedule a retry for a newer attempt.
- **Visible rows no longer own the four-call cap.** `Volumes` keeps the current
  visible request per volume separately from `outstanding_mounts`, the count of
  backend calls whose callbacks have not arrived. A monitor completion retires
  the row but keeps its slot. A slot is released exactly once, by the callback
  or by a synchronous launch failure. Removal and window close retire UI
  ownership and cancel the operation, but keep the call counted until its
  callback.
- **A stale reconciliation timer checks both the volume and its attempt
  token.** The timer body is shared with a test seam; a timer delivered after
  a monitor completion, unmount or retry does nothing.
- **A mounted row now carries the live mount object.** `DeviceState::Mounted`
  has `root` and `mount_object`; `MountStatus::Mounted` carries both too.
  `UsageKey` includes the mount object, so a same-volume/same-root remount
  retires the old capacity result, cancels/ignores its queued or delayed
  callback, and queries the new mount. Main Drive uses the root mount object
  reported by the backend when one is available.
- **Sidebar row identity includes the mount object.** A same-root replacement
  rebuilds the affected row state instead of reusing the old bar, while
  unrelated rows keep their focus and menus.

Files written:

- `crates/fm-core/src/volumes.rs`
- `crates/fm-ui/src/volumes/mod.rs`, `volumes/tests.rs`
- `crates/fm-ui/src/sidebar.rs` (mount identity in affected row state)
- `docs/notes/P2p-r1.md`, `docs/notes/P2p.md`

No command, registry caller rule, socket policy, manifest, lockfile, config or
session schema changed. No real device was mounted, unmounted or ejected.

## Attempt and slot contract

**[read]** The controller now has one `MountRequest` per accepted attempt with
a process-monotonic `token`. The visible `requests` map contains only the
current attempt for a volume. Retired attempts are kept alive only by their
backend callback closure, which captures its own `Rc<MountRequest>`.

**[read]** `finish_mount` always calls `release_mount_slot`; that method uses the
request's `slot_released` cell, so the count drops exactly once. If the attempt
is no longer current, the function returns without refreshing inventory,
changing status or scheduling a timer. The four-call cap is checked against
`outstanding_mounts`, not against visible rows.

**[read]** Monitor completion calls `complete_attempt` and leaves the slot
alone. Removal and `cancel_window` call `retire_attempt` with a truthful
terminal status after cancelling the operation. Explicit row Cancel keeps the
row visible as `Cancelling…` until its own callback; monitor success still wins
because inventory is the truth. `shutdown` cancels and discards ownership
without waiting for callbacks.

**[read]** The retry timer calls `deliver_mount_retry(id, token)`. It only
resets `retry_scheduled` and refreshes when that exact token is still the
visible request. A callback from attempt A that reports success without a root
can therefore not start attempt B's reconcile loop.

## Usage identity contract

**[read]** `RawMount.object` is now stored in `DeviceState::Mounted` and returned
by `Inventory::reconcile_mount` together with the root. `Inventory` also records
the root mount's object for Main Drive. `UsageKey` includes `mount_object` for
both device entries and the main row, and `usage_key_live` checks it as well as
the volume id/incarnation/root.

**[read]** `reconcile_usage` builds the live key set from the current entries
and root mount. A replaced mount key is removed, its cancellable is cancelled
and any queued job for it is dropped. The new key is queued immediately or
waits behind the existing four-active/32-pending bounds. Repeated monitor
refreshes reuse the single shared per-key record; they do not add duplicate
queries.

## Red/green evidence

**[observed]** Regressions were added before the attempt lifecycle change. The
focused old-code run is `/tmp/gimbal-p2p-r1-red.log`:

```
the_process_wide_cap_is_four_and_releases_only_at_a_terminal_result
  monitor completion cannot release a backend slot before the callback
  left: 3, right: 4

monitor_completion_then_unmount_and_same_volume_retry_have_independent_outcomes
  attempt A still owns its backend slot
  left: 0, right: 1

removal_and_reinsertion_churn_keeps_four_outstanding_calls
  the four callbacks still own all four slots
  left: 0, right: 4
```

**[observed]** To check the usage regression against the old identity shape,
`UsageKey::entry` and the main key were temporarily made to drop the mount
object, then restored. Both new usage tests failed because a key without the
live mount object could not validate or retain the result
(`/tmp/gimbal-p2p-r1-red-usage.log`):

```
same_root_mount_replacement_retires_cached_usage_and_queries_only_the_new_mount
  left: Unknown, right: Ready { total: 1000, free: 800 }

main_usage_follows_the_root_mount_object_across_refreshes
  left: Unknown, right: Ready { total: 10000, free: 4000 }
```

**[observed]** With the token and usage identity implemented, the focused
suites pass:

```
cargo test -p fm-ui volumes::tests::
  26 passed; 0 failed; 0 ignored
cargo test -p fm-core volumes
  10 passed; 0 failed; 0 ignored
cargo test -p fm-ui real_sidebar_mount_menu_pending_cancel_and_live_refresh
  1 passed; 0 failed; 1 ignored
```

The timer regression is delivered through `deliver_mount_retry_for_tests`,
which calls the same body as the production timer. This is deliberate:
**[read]** `glib-0.22.10:src/source.rs:455` shows `timeout_add_local` attaches to
`MainContext::default()` rather than the test thread's default context, so a
real timeout in a parallel test thread is not a reliable seam. The test still
asserts the token decision, including no backend snapshot after stale delivery.

## New and changed tests

`fm-core::volumes` now has 10 tests. New:

- `same_volume_same_root_mount_replacement_carries_the_new_mount_object`
- `root_mount_identity_is_reported_for_main_drive`

`fm-ui::volumes` now has 26 tests. New:

- `monitor_completion_then_unmount_and_same_volume_retry_have_independent_outcomes`
- `stale_mount_reconciliation_timer_cannot_refresh_a_retry`
- `removal_and_reinsertion_churn_keeps_four_outstanding_calls`
- `synchronous_launch_failure_releases_its_slot_once`
- `same_root_mount_replacement_retires_cached_usage_and_queries_only_the_new_mount`
- `main_usage_follows_the_root_mount_object_across_refreshes`

Changed:

- `the_process_wide_cap_is_four_and_releases_only_at_a_terminal_result` now
  proves that a monitor completion keeps all four outstanding calls counted
  and that a fifth call is refused until the first callback arrives.
- `closing_one_window_cancels_only_its_own_request` now proves that close
  retires visible ownership, keeps both backend calls counted and leaves the
  other window's request pending.

All use the real `Volumes` controller and injected `FakeBackend` deferred
callbacks. The fake also counts backend snapshots, can fail the next launch
synchronously and can complete same-root usage queries in order. No real mount
backend is constructed in tests.

## API citations

- `glib-0.22.10:src/source.rs:455` `timeout_add_local` uses
  `MainContext::default()`; `:532` `timeout_add_local_once` wraps the same
  path. This is why the timer test uses a token-delivery seam.
- `gio-0.22.10:src/auto/mount.rs:146` `Mount::root` and the existing P2p
  citations for `VolumeExt::mount`/`VolumeMonitor` are unchanged.
- `gtk4-0.11.5:src/auto/mount_operation.rs:26` `MountOperation::new(parent)` is
  unchanged.
- No new GTK/GIO name was needed for this revision.

## Final gates

`scripts/check.sh` from the project root (`/tmp/gimbal-p2p-r1-check.log`):

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
==> cargo build --workspace --release
==> cargo deny check
```

All five passed. Final test totals: **770 passed, 20 ignored** across the
workspace. Breakdown: 23 + 16 + 12 + 343 + 6 + 11 + 359 passed; ignored
1 + 6 + 11 + 2. The P2p baseline was 762 passed, 20 ignored; r1 adds eight
passing tests (two core, six UI).

One intermediate full-check run hit the existing `fm-agent` socket suite's
worker-budget timing check (four failures, including `BrokenPipe`); an isolated
rerun and the final full `scripts/check.sh` run both passed all 16 socket
tests. No agent file was changed by this revision.

## Not verified live (Kyle performs hardware checks)

No app run and no real GIO mount call were made. The P2p seven live checks still
apply. Specific to r1:

1. On a real device, complete a mount, unmount it, and retry the same volume.
   The retry must show its own `Mounting…`/result; a late callback from the
   first attempt must not finish or fail it.
2. With four real mount operations awaiting callbacks, a fifth attempt is
   refused as busy even if one row already shows mounted from a monitor event;
   the fifth becomes available after a callback.
3. Remove a device or close the initiating window while its call is
   outstanding. The row/ownership retires, but a fifth mount remains refused
   until the backend callback actually arrives.
4. Remount the same volume at the same root and confirm every window's bar
   changes only after the new mount's own usage query; the old capacity must
   not reappear. The fake-backend suite covers Main Drive mount replacement.
5. Ordinary browsing, search, preview, drag/drop, Cancel, permission denial
   and explicit retry remain unchanged.

## Outside these files

Nothing needs changing outside the listed files. The GIO backend and the fake
backend's existing interfaces were sufficient; only test helpers were added.

## Review (ChatGPT, Nov 4, 2026)

**[read]** Attempt tokens gate callbacks and reconciliation timers. Backend
slots survive monitor-first completion and removal until their own callback
or synchronous failure. Usage keys include the mount object for device rows
and Main Drive, so the same root can receive fresh capacity after replacement.

**[read] Reviewer fix.** A GIO mount-changed event previously requested a
generic inventory refresh, which retained cached Ready usage for the same
mount object. It now marks completed usage records for the affected root.
The existing idle refresh coalesces notifications, invalidates those records
and reuses pending queries. The focused fake-backend regression
mount_change_refreshes_only_its_usage_and_coalesces_pending_queries verifies
that the affected bar gets fresh values and Main Drive keeps its existing
query when another root changes.

**[observed]** All five gates passed both before the reviewer fix (770 passed,
20 ignored) and after it (771 passed, 20 ignored). Final log is
/tmp/gimbal-p2p-r1-review-final.log. The isolated GTK widget subprocess passed.
Hardware mounting and desktop authentication await Kyle's live check.

## Live acceptance (Kyle, Nov 4, 2026)

**[Kyle reports]** Sidebar mounting worked, including the requested double-click
gesture after the follow-up change. The gesture checks live eligibility and uses
the existing drive.mount command. All five gates passed after the gesture change
with 771 passed and 20 ignored. P2p through r1 is accepted.
