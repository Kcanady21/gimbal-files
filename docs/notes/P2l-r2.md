# P2l-r2: finish drop-action negotiation before transferring files

Tags: **[read]** for source/crate facts, **[observed]** for a command or run,
**[inferred]** for anything that needs a real pointer, real keyboard or live feel.

## What I built

All code changes are in `crates/fm-ui/src/`: `dnd.rs` and `app.rs`. No `fm-core`,
`Cargo.toml`, runner or `spike/` change.

### 1. The final negotiated action is frozen before the worker runs

`dnd.rs` now has one live `Negotiation` per folder target. It is the same state
machine the GTK callbacks use; tests drive its `enter` / `observe` /
`filesystem_result` / `final_action` transitions rather than a separate
test-only model.

- **Enter** records the source capability and the live `GdkDrop::actions` view,
  then asks for provisional Copy. Copy is the only safe provisional action: it
  cannot become an unexpected move. The initially observed selected action is
  recorded.
- **Motion or negotiated-action event** re-reads `GdkDrop::actions` and
  `GdkDrag::selected_action`; it never reuses the enter-time mask. A selected
  action that changes and differs from our last preference is remembered as an
  override so later motions do not fight it. An echo of our own preference is
  not an override.
- **The asynchronous filesystem result** stores same/different, computes the
  default (same filesystem Move, different Copy), updates the target preference,
  and calls `GdkDrop::status(target.actions(), preferred)` from the async
  callback. This sends protocol status even when the pointer has not moved; it
  is not merely a GTK signal. For a local drag the preference is clamped to
  `GdkDrag::actions`; for an external offer it is a hint only, because the live
  `GdkDrop::actions` view may already be narrowed to the selected action.
- **Drop** re-observes the current view, then freezes the compositor's selected
  action if the live view still allows it. With no selected action, Copy is the
  safe supported fallback; a move-only offer with no selected action is refused
  with `Cannot drop: drag action is not settled`.
- `Process::drop_on_folder` no longer runs a second filesystem query. It uses
  `DropIntent::action()` directly, so a completed Copy drop cannot be
  reinterpreted as Move. The P2k worker, conflicts, progress, Cancel, undo and
  Trash/pin semantics are untouched.

### 2. External source capability is no longer cached as an immutable mask

- A local drag has a full source mask through `GdkDrag::actions`
  (`gdkdrag-wayland.c:414-415`). An external FileList has no `GdkDrag`; its only
  view is the live `GdkDrop::actions`. On Wayland that property is narrowed to
  the selected action once one exists (`gdkdrop-wayland.c:287-315`), and a
  preselected action is applied before enter (`gdkseat-wayland.c:401-402`).
- Therefore `Capability::External` is logged as **unknown capability evidence**
  with the current view. The current view is re-read every event, never saved as
  the source mask. Entering with a narrowed `COPY` does not cap the drag: a later
  Shift-selected Move arrives as a live `MOVE` view and is accepted. A genuine
  copy-only source keeps its selected Copy and the final job copies.
- A local mask still clamps both preference and final action; an action the
  source does not advertise is never executed.
- While a background viewport target is masked by a child target (a cell), it
  clears its negotiation and returns no preferred action. It does not record the
  child's response as its own key choice. A later unmasked motion starts a fresh
  negotiation.

### 3. Logs

With `GIMBAL_LOG=1` the hover log now prints:

```
hover folder target: source <local mask | external unknown (view ...)>,
selected <Copy|Move|unknown>, target prefers <...>, filesystem <same|different|unknown>,
modifiers <...>
```

The drop log prints source evidence, selected action, target preference,
filesystem result, modifiers and the final action before the worker job id.

## State transitions, in order

```
enter: capability + live view, provisional Copy, observe selected
motion/action: reread live view + local selected; update override/preference
async filesystem: default = Move(same) / Copy(different); send Drop::status
drop: re-observe; freeze selected if supported; else Copy if supported; else refuse
```

The important freeze property: `filesystem_result` only changes the **next
preference**. It does not change `selected`. A drop before the compositor answers
uses the last selected action. That is why `enter -> provisional Copy ->
compositor Copy -> same-filesystem result -> drop without motion` stays Copy,
while a later motion/answer that selects Move makes the same state move.

## API citations

- `gdk4-0.11.5:src/auto/drop.rs:32` `Drop::actions`; `:135`
  `Drop::status(actions, preferred)`.
- `gtk4-0.11.5:src/auto/drop_target.rs:46` `DropTarget::actions`; `:55`
  `DropTarget::current_drop`.
- `gdk4-0.11.5:src/auto/drag.rs:61` `Drag::actions`; `:102`
  `Drag::selected_action`.
- `gtk-4.22.4:gdk/gdkdrop.c:552-571` documents `gdk_drop_status` and says the
  destination should call it again when its exact actions become known. This is
  the verified mechanism used by the async filesystem callback.
- `gtk-4.22.4:gdk/wayland/gdkdrop-wayland.c:123-153`
  `gdk_wayland_drop_commit_status` -> `wl_data_offer_set_actions` + `accept`;
  `:287-315` `gdk_wayland_drop_update_actions` narrows `Drop::actions` to the
  selected action.
- `gtk-4.22.4:gdk/wayland/gdkseat-wayland.c:266-283` source actions update and
  synthetic motion; `:291-308` selected action update and synthetic motion;
  `:401-402` pending source action and action applied before enter.
- `gtk-4.22.4:gdk/wayland/gdkdrag-wayland.c:327-338` source `action` becomes
  `GdkDrag::selected_action`; `:414-415` source advertises its actions.
- `gtk-4.22.4:gtk/gtkdrop.c:69-90` one status per event and the end-of-event
  default; `:96-110` GTK's internal status wrapper.
- `gtk-4.22.4:gtk/gtkdroptarget.c:451-454`, `:533-536` GTK sends the returned
  unique preference with the target's accepted actions.
- `gtk-4.22.4:gdk/wayland/gdkseat-wayland.c:1453-1477` keyboard leave clears
  the seat modifiers; `gdk/wayland/gdkdevice-wayland.c:453-466` reads that state.
  This remains why an empty Wayland modifier state is logged as `unknown`.

## Regression tests added or reworked

All are in `dnd.rs` and drive the same `Negotiation` transitions used by the
callbacks:

- `drop_negotiation_freezes_provisional_copy_before_filesystem_result`
- `drop_negotiation_preserves_shift_move_across_drives_before_filesystem_result`
- `drop_negotiation_keeps_selected_action_when_preference_changes`
- `drop_negotiation_release_during_renegotiation`
- `drop_negotiation_external_preselected_copy_then_shift_moves`
- `drop_negotiation_external_preselected_move_then_ctrl_copies`
- `drop_negotiation_external_release_returns_to_the_selected_default`
- `drop_negotiation_external_narrowed_copy_can_prefer_the_default`
- `drop_negotiation_copy_only_external_stays_copy`
- `drop_negotiation_refuses_a_move_only_unsettled_offer`
- `drop_intent_schedules_the_negotiated_operation`
- `drop_intent_survives_async_filesystem_query` (reworked for the frozen intent)

The existing defaults, Ctrl/Shift, supported-action, external-without-local-drag,
background/padding, file-cell refusal, same-folder/self/descendant refusal,
badge and pin tests still pass.

## Verified

### `scripts/check.sh`

Run from `$PROJ` with cargo-deny on `PATH` and the pinned 1.95.0 toolchain; exit 0.

```text
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 175 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 6 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 157 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Totals: 346 tests passed, 6 ignored, no doc-tests. `fm-ui` has 157 tests, 11
more than P2l-r1's 146. `fm-core` 175, CLI 7, agent 1, trash binary 6 plus 6
ignored children. **[observed]**

`cargo deny` still printed the pre-existing duplicate `system-deps` warning and
ended `all checks passed`. **[observed]**

### App launch

`pgrep -a gimbal-files` was empty before launch. A release build was started
with scratch `XDG_CONFIG_HOME` and `XDG_STATE_HOME` under `/tmp/gimbal-p2l-r2`,
a scratch folder with two files and a subfolder, and `GIMBAL_LOG=1`. It listed
the scratch folder and saved its scratch session. It was stopped with SIGTERM
by its own PID and the scratch folders were removed. **[observed]**

Honest note on the run: a shell PID mistake made the first `kill` target the
wrapper rather than the app, so the app ran longer than the intended 5 seconds
(roughly under a minute) before it was noticed and stopped by PID 1015509 with
SIGTERM. It used only the scratch config/state/folders; no user file was touched
and no private session bus was used. **[observed]**

## Not verified live (Kyle checks these)

Unit tests do not prove real compositor negotiation or GTK picking.

- Quick Ctrl/Shift drops and stationary Ctrl/Shift drops; change and release
  keys during hover and during the filesystem refresh.
- Same-filesystem Move and cross-filesystem Copy defaults with no modifier.
- External Dolphin drags in both directions: preselected Copy -> Shift Move,
  preselected Move -> Ctrl Copy, and releasing the key.
- File-cell refusal, folder-cell destination, background padding, empty folder
  and every split tile, plus the drag icon/badge and the P2l-r1 padding fixes.
- Pinned-folder drops, pin-by-drop, Trash, Cancel and undo of a test transfer.
  Inspect source and destination contents, not only the cursor or status text.
- Hover/drop logs should show `source`, `selected`, `target prefers`,
  `filesystem`, `modifiers` and the final Copy/Move.
- A quick unmodified same-filesystem drop that beats the asynchronous query
  safely falls back to Copy. Check whether that is acceptable in real use; a
  later drop with the same-filesystem answer already applied should move.

## Anything outside my files

None. No `fm-core`, dependency, command, runner or `spike/` change. The P2l note's
runner workaround (autodrag has no `app` entry) is unchanged and remains outside
this brief.

## Review and live check (ChatGPT, Nov 2, 2026): passes in part

**[observed]** Final `scripts/check.sh`: exit 0. Fmt, clippy with -D warnings,
tests, release build and cargo-deny all pass. 347 tests passed, 6 ignored; fm-ui
has 158 tests. Existing duplicate system-deps warning only. Full output:
`/tmp/gimbal-p2l-r2-review-check.log`. Initial sandbox run could not fetch the
RustSec database; the approved network-enabled rerun and final gates passed.
`git diff --check` also passed before this notes update.

**[read]** Both P2l-r1 review blockers are addressed: the worker uses the frozen
negotiated operation without another filesystem query, the async hover result sends
GDK protocol status, and external capabilities are distinguished from the live
selected-action view. Masked background targets relinquish negotiation and invalidate
pending queries. Drag-end still ignores delete_data; no source deletion added.
No new dependency, core, spike or runner changes. Git diff/status confirm changes
in app.rs, dnd.rs and folder/view.rs plus the reviewer/agent notes. folder/view.rs
is the outstanding P2l-r1 change carried with this uncommitted stack.

**Fix (ChatGPT): releasing a modifier must restore the filesystem default.**
**[read]** `Negotiation::observe` remembered a temporary compositor override and
returned it as the target's new preferred action. Once Ctrl selected Copy on one
filesystem, the target preferred Copy too, so releasing Ctrl could leave Copy
selected. Shift across filesystems could similarly leave Move selected after release.
Removed the remembered override. The target keeps requesting its filesystem default;
the compositor applies the held key, and the final selected action still determines
the operation. This supersedes the override-retention description earlier in this note.

**[observed]** Added `drop_negotiation_modifier_release_keeps_filesystem_preference`.
It failed before the fix (Copy preference instead of Move) and passes afterward.
It covers Ctrl and Shift release for both local and external negotiations. Updated
existing assertions to distinguish the target's default preference from the selected
operation. All 27 drag tests pass; the final full gates above include the fix.

**Live:** not checked by ChatGPT. No app was launched and no input was injected.
The agent reports its earlier launch exceeded the 10-second limit due to a PID
mistake; that is a process-rule violation, not reviewer live verification.

**Still open (Kyle's live checks, disposable files):**
- One-file icon follows pointer; three-file selection shows badge 3.
- Ctrl on one drive copies; Shift across drives moves. While hovering, press then
  release each key: the cursor and final operation return to the default.
- No modifier: same drive moves, different drives copy. A quick drop before
  negotiation completes can safely copy; check this timing behavior in use.
- Padding, below the last row, empty folders, and each split tile accept one transfer
  to the intended folder. Folder cells keep their destination; file cells refuse.
- Dolphin both ways, pinned folder, pin-by-drop, cancel, and undo still work.
- Zen matrix remains separate and unverified. No commit or push yet.

**Rule update (Kyle approved, Nov 2):** CLAUDE.md now allows read-only git for the
primary ChatGPT reviewer and Claude; changing git state requires Kyle's explicit
approval, and push requires his instruction. Other coding agents may run no git.

## Follow-up live check (Kyle reports, Nov 2, 2026): fails modifier acceptance

**[read: Kyle's report]** The drag icon appears and a three-file drag shows 3,
but the badge is hard to read and needs bolder text and more contrast. Ctrl-drag
and Shift-drag still do nothing different from an ordinary drag, including the
press/release checks. Padding drops and the grouped interoperability/pinning/
cancel/undo checks pass by Kyle's report. These are reported observations, not a
ChatGPT live session. Do not treat the earlier code-review pass as a full pass.

**Still open:** real modifier handling and badge readability are assigned to
`P2l-r3-real-modifiers-and-badge.md`. Requested an opt-in GIMBAL_LOG capture of Kyle's
Ctrl/Shift drags in `/tmp/gimbal-dnd-kyle.log` to diagnose the actual event path.
No new application code or gate run for this follow-up. No commit or push.

**Captured-log follow-up (ChatGPT, Nov 2):** Read `/tmp/gimbal-dnd-kyle.log`.
Two completed drops (17.504 s and 45.716 s) both report local COPY|MOVE, selected
Move, preference Move, filesystem same, modifiers unknown, final Move. Preferences
transition from provisional Copy to the same-filesystem Move default. This supports
missing usable modifier evidence but does not prove the failing layer. Neither
sequence is labelled by key, and both report the same filesystem; the capture does
not verify Shift across drives. Added the evidence and target-identity diagnostic
requirement to P2l-r3. No application changes or gates for this log inspection.
