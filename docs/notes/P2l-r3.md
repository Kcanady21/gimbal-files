# P2l-r3: real modifier handling and a readable drag badge

Tags: **[read]** for source and pinned-crate facts, **[observed]** for a command,
run or captured log, **[inferred]** for anything that needs a real pointer or
keyboard. Nothing here was produced by input injection.

## What I built

All code changes are in `crates/fm-ui/src/`: `dnd.rs`, `folder/view.rs`,
`frame.rs`, `app.rs` and `look/css.rs` (plus its test). No `fm-core`,
`Cargo.toml`, runner or `spike/` change.

### 1. What the captured log actually shows

Kyle supplied `/tmp/gimbal-dnd-kyle.log`. **[observed]** It contains two
completed local drags, dropped at 17.504 s and 45.716 s. Both report:

```text
dnd: drop 1 item(s) into /home/kyle/Music: source local COPY|MOVE,
selected Move, target preference Move, filesystem same,
modifiers unknown, final Move -> job N
```

Both are one-file drags from `/home/kyle/Downloads` to `/home/kyle/Music`;
both targets are the same filesystem. The log does not label Ctrl versus
Shift, has no target id and no key-event lines, so it cannot tell which key (if
any) Kyle held. It also has no cross-filesystem drag. It therefore proves that
no positive modifier evidence reached the decision in those two runs, but not
which layer lost it. **[observed, as the captured file]**

The log also shows the target preference moving from provisional Copy, before
the filesystem answer, to the same-filesystem Move default, with repeated
resets to `filesystem unknown` as hover ownership changed. That matches the
existing negotiation code; it is not evidence of a key.

### 2. Diagnosed cause

The old `modifier_evidence` read only:

```text
GdkDrop -> GdkDrag::device() -> GdkDevice::modifier_state()
```

On Wayland that is the seat's `key_modifiers`
(`gdk-4.22.4:gdk/wayland/gdkdevice-wayland.c:453-466`). GDK clears it on
`wl_keyboard.leave` (`gdkseat-wayland.c:1452-1477`). A drag start can therefore
leave an empty, stale mask even while a key is physically held. The old code
then equated that empty mask with "no key" on non-Wayland and `unknown` on
Wayland, and did nothing with it in either case: `Negotiation` always asked for
the filesystem default.

The other half is the compositor. Wayland lets the compositor change the
selected action in response to modifier changes, but it does not require it
(`/usr/share/wayland/wayland.xml:671-690`, `:836-855`). Niri delegates DnD to
Smithay and uses the default pointer grab
(`niri src/handlers/mod.rs:338-339`, fetched source, not pinned in this repo).
Smithay's action chooser only honours the destination's preferred action or
falls back to Ask/Copy/Move; it reads no keyboard state
(`smithay 8a27023 src/wayland/selection/data_device/mod.rs:602-620`).
Nautilus, the available GTK file manager, likewise uses only
`gdk_drop_get_actions` / `gdk_drag_get_selected_action` at drop
(`nautilus-50.2.2:src/nautilus-list-base.c:741`, `:750`, `:844`, `:854`) and
computes its preferred action only from the target and filesystem
(`src/nautilus-dnd.c:162-231`). There is no app-side modifier read in that
path. So on this desktop the old "let the compositor apply the key" assumption
had no source of key state at all.

The failure is therefore not one lost event: the decision path never had a
trustworthy observed key source. The fix adds one at the window, where GTK
actually delivers key events, and uses it in the preference and final action.

### 3. A window-level key source

`dnd::install_modifier_tracking` is installed on each model window in
`frame.rs`. It attaches a `GtkEventControllerKey` at `Capture` phase and
tracks `Control_L/R` and `Shift_L/R`:

- `key-pressed` / `key-released` update the tracker from real key events.
- the controller's `modifiers` signal also synchronises the full GDK mask.
- the tracker lives on the window, not the drag-source widget: GTK calls
  `gtk_widget_reset_controllers` on the source widget immediately after
  `gdk_drag_begin` (`gtk-4.22.4:gtk/gtkdragsource.c:609-616`).

`TrackedKeys` keeps the three states that matter:

- `unknown` — no evidence. This is not "no key".
- `Live` — key or modifier events have been observed.
- `DragStart` — a *positive* seat reading captured in the source's `prepare`.

`dnd::prepare_drag` runs from `connect_prepare`, before GDK starts the drag. A
positive Ctrl/Shift there is the "held before the press" case. An empty Wayland
seat stays unknown. A `DragStart` seed is cleared at drag end
(`clear_internal_drag`) and on window deactivation, so it cannot become a
lasting start-time key. A live observation survives, because it is a current
physical state.

Every key change is pushed to the hovered target immediately with
`GdkDrop::status(target.actions(), preferred)`, not saved for the next motion.
`refresh_active_targets` keeps a small registry of currently entered targets;
a release with a stationary pointer still asks the compositor for the
filesystem default, and a press with a stationary pointer still asks for
Ctrl-Copy or Shift-Move.

### 4. The decision uses the evidence

`Negotiation` now stores the observed key evidence and recomputes the target
preference on enter, motion, filesystem result and key change:

- observed Ctrl -> prefer Copy;
- observed Shift -> prefer Move;
- observed `none` -> filesystem default (same = Move, different = Copy);
- unknown -> filesystem default as well, but logged as unknown and never used
  to claim "no key".

The final action (`DropIntent::capture`) is stricter:

- observed Ctrl and Copy is offered -> Copy;
- observed Shift and Move is offered -> Move;
- an observed action the drag does not offer is refused with
  `Cannot drop: this drag does not offer Copy/Move`; it never becomes the
  other action. A copy-only source with Shift therefore refuses rather than
  silently moving; a move-only source with Ctrl refuses rather than silently
  moving.
- observed `none` after an observed override returns to the synchronous
  filesystem default. This is how a release with no compositor echo still
  restores the default. It only applies when this hover actually observed a
  key (`override_action`), so the older safe "provisional Copy stays Copy"
  rule is preserved when no key was ever observed.
- unknown evidence uses the compositor's selected action, and Copy remains the
  safe supported fallback.

The final decision is still captured synchronously at `drop`; no asynchronous
filesystem result can change an already accepted Copy into Move. Ctrl wins
when both are held (`action_for`). External offers still use their live
`GdkDrop::actions` view, never a cached full mask, and the local source mask is
still authoritative for our own drags.

### 5. Diagnostics

With `GIMBAL_LOG=1` the drag path now prints:

- `prepare modifiers: seat <...> -> <...>` with the evidence origin;
- `drag-begin: actions ..., selected ..., modifiers ...`;
- `drag selected-action changed: ...` for every compositor change;
- `hover folder target #<id>: source ..., selected ..., target prefers ...,
  filesystem ..., modifiers <state> (<origin>)`;
- `filesystem target #<id> same|different: status prefers ...`;
- `key <name> down|up: <before> -> <after>`;
- `hover key change target #<id>: ...` after the pushed `GdkDrop::status`;
- `drop event target #<id>: live ..., selected ..., target prefers ...,
  filesystem ..., modifiers ...`;
- the existing `drop ... final ... -> job` line in `app.rs`, now with the
  modifier origin.

Target ids distinguish crossings and negotiation resets from key changes. The
log never calls an unknown modifier state "none".

Two event/controller APIs were checked in the pinned crates:

- `gtk4-0.11.5:src/event_controller_key.rs:11` `connect_key_pressed`,
  `:50` `connect_key_released`;
  `src/auto/event_controller_key.rs:104` `connect_modifiers`;
  `src/auto/event_controller.rs:136` `set_propagation_phase`.
- `gtk4-0.11.5:src/auto/window.rs:1641` `connect_is_active_notify`.
- `gdk4-0.11.5:src/auto/device.rs:106-112` `modifier_state`;
  `src/auto/drag.rs:61` `actions`, `:73` `device`, `:102`
  `selected_action`, `:262` `connect_selected_action_notify`;
  `src/auto/drop.rs:135` `status`.
- GTK C: `gtk-4.22.4:gtk/gtkdragsource.c:609-616` real key controllers are
  reset on the source widget at drag begin; `gdk/wayland/gdkseat-wayland.c:
  1452-1477` leave clears `key_modifiers`; `:1487-1600` an enter's key list is
  not used to rebuild that mask; `gdk/wayland/gdkdrop-wayland.c:122-144`
  status becomes `wl_data_offer.set_actions` + `accept`;
  `gtk/gtkdragicon.c:394` the drag-icon widget's CSS node name is `dnd`.

### 6. Badge

The shared rule in `look/css.rs` changed from the outline colour plus
`@accent_fg_color` at `TEXT_TINY` to:

```css
dnd .gl-drag-badge, .gl-drag-badge {
    min-width: 14px;
    min-height: 14px;
    padding: 2px 8px;
    border-radius: 999px;
    background-color: @accent_bg_color;
    color: @accent_fg_color;
    font-size: 14px;
    font-weight: bold;
}
```

The values are token-scaled at the live scale (`TEXT_SMALL`, `LINE_THIN`,
`GAP_S`, `RADIUS_PILL`). The `dnd` element selector is explicit because
`GtkDragIcon` is its own root; the bare class selector is kept as a fallback.
The provider is installed for the whole display
(`look/mod.rs:29-33`), so no window ancestry is needed.

The theme defines an opaque pair: Noctalia's file has
`@define-color accent_bg_color #935fec` and
`@define-color accent_fg_color #15111d` (`~/.config/gtk-4.0/noctalia.css:10-12`).
The adw-gtk3 themes define the same named pair for light and dark
(`/usr/share/themes/adw-gtk3/gtk-4.0/libadwaita-tweaks.css:1-2` and the dark
variant). Kyle verifies the actual contrast.

## Verified

### `scripts/check.sh`

Run from `$PROJ` with the pinned 1.95.0 toolchain and `~/.cargo/bin` on `PATH`;
exit 0. **[observed]**

```text
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 175 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 6 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out
test result: ok. 167 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Totals: **356 tests passed, 6 ignored**, no doc-tests. `fm-ui` has 167, nine
more than P2l-r2's 158: eight new drag tests (held, pressed, released,
Ctrl+Shift, unsupported, missing evidence, external, and the drag-start seed)
plus the new badge test. `fm-core` 175, CLI 7, agent 1, trash binary 6 plus 6
ignored children. The pre-existing duplicate `system-deps` warning remains and
the gate still ends `all checks passed`. **[observed]**

### Regression tests and the evidence each models

The modifier tests drive the same `TrackedKeys` / `Negotiation` transitions the
window controller and drop callbacks use. The key values are synthetic; no
input was injected. **[inferred]**

- `modifier_keys_held_before_prepare_survive_a_cleared_seat` — a positive
  seat reading at `prepare` remains Ctrl after GDK clears the mask, and a
  same-filesystem drop is Copy.
- `modifier_keys_pressed_during_drag_change_preference_and_final_action` — an
  unknown start, then a real `Control_L` press, changes the preference to Copy
  and the final operation to Copy.
- `modifier_keys_released_while_stationary_restore_the_filesystem_default` —
  Ctrl on one filesystem and Shift across filesystems, each released without a
  pointer motion, return to Move and Copy respectively.
- `ctrl_wins_when_ctrl_and_shift_are_both_held` — both keys make Ctrl, and
  releasing Ctrl leaves Shift to select Move.
- `unsupported_observed_action_is_refused` — Shift on copy-only and Ctrl on
  move-only are refused, not converted to the other operation.
- `missing_modifier_evidence_stays_unknown_and_uses_the_compositor_action` —
  unknown is distinct from `none` and keeps the accepted compositor Copy.
- `external_drag_observed_ctrl_copies_and_shift_moves` — an external narrowed
  view refuses until the source answers the requested action, then copies or
  moves.
- `a_drag_start_seed_does_not_survive_the_drag` — a start seed is cleared at
  drag end (and on a stale empty prepare); a live observation survives.
- `drop_negotiation_release_during_renegotiation` and
  `drop_negotiation_modifier_release_keeps_filesystem_preference` — release
  restores the target's filesystem preference, not a lasting override.
- `drop_negotiation_freezes_provisional_copy_before_filesystem_result` — an
  unmodified quick drop keeps the accepted provisional Copy.
- `drop_intent_survives_async_filesystem_query` — a delayed filesystem answer
  cannot change the intent captured at drop.
- Prior drag, selection, padding/split, file-cell refusal, destination,
  Trash/pin, badge-count and external-capability tests still pass.

### App run

`pgrep -a gimbal-files` was empty before launch. A release build ran for five
seconds on `/tmp/gimbal-p2l-r3/folder` with scratch `XDG_CONFIG_HOME` and
`XDG_STATE_HOME` under `/tmp/gimbal-p2l-r3` and `GIMBAL_LOG=1`. It opened the
window, listed three items, parsed the stylesheet without a CSS parse error,
and saved its scratch session. It was stopped by its own PID (1036341) with
SIGTERM; `pgrep` was empty afterwards. The only critical was the known at-spi
bus warning from an isolated launch without `dbus-run-session`. **[observed]**

## Not verified live (Kyle checks)

- A real key event reaching the window during the DnD grab. The captured log's
  `modifiers unknown` is consistent with both "key not held" and "GDK mask
  cleared"; the new `key ... down/up` and `hover key change` lines will show
  which. Niri's pointer grab is created with `Focus::Keep` and does not change
  keyboard focus (`niri src/handlers/mod.rs:338-339`), so key events should
  continue; this is **[inferred]**, not observed here.
- Same-drive Ctrl-copy; cross-drive Shift-move; holding a key before the press;
  pressing and releasing each key while hovering, including stationary
  release; Ctrl+Shift priority; no-modifier defaults.
- Dolphin both ways (preselected Copy -> Shift Move, preselected Move -> Ctrl
  Copy, release), and the pinned transfer, pin-by-drop, Cancel and undo checks.
- The badge's actual contrast in light and dark themes, and 3/12/100 without
  clipping. Source/layout review says the 14 px bold text with 8 px horizontal
  padding and 999 px radius fits inside the 80 px drag icon; Kyle checks it.
- Padding, empty/split targets, file-cell refusal and folder-cell destination
  after this change. The Zen matrix remains separate.

If the live log shows no `key ... down/up` lines during a drag, then this
desktop is not delivering keyboard events to the app during the DnD grab. In
that case the only truthful statement is that the public GTK/GDK path has no
key source after the start; the `prepare` seed still works for a key held
before the press, but press-after-start and stationary release cannot be
guaranteed and need a reviewer decision. No desktop configuration or private
bus was changed to investigate this.

## Anything outside my files

Nothing needs changing for this brief. There is no `fm-core`, dependency,
command, runner or `spike/` change. If live evidence shows niri does not keep
keyboard focus during DnD, the compositor (or a deliberate `Ask`-action UI)
would be the only remaining source; that is outside this brief.

## Review and live check (ChatGPT, Nov 2, 2026): passes in part

**[observed]** Reviewer `scripts/check.sh`: exit 0; fmt, clippy with -D warnings,
workspace tests, release build and cargo-deny pass. 356 tests passed, 6 ignored;
fm-ui 167. Existing duplicate system-deps warning only. Full reviewer output:
`/tmp/gimbal-p2l-r3-review-check.log`. The first sandbox attempt reached deny but
failed network DNS; the approved network-enabled rerun passed. `git diff --check`
passes. No reviewer source fixes, app launch or input injection in this review.

**[read] Scope and safety:** Git status/diff show the uncommitted P2l-r1/r2/r3 stack.
New r3 code is within its allowed fm-ui scope, including frame.rs and shared CSS.
CLAUDE.md and earlier notes are reviewer changes already recorded. No core, spike,
runner, manifest or lockfile changes; no new dependencies or unsafe blocks found
in dnd.rs. Source drag-end still ignores delete_data. Folder transfers still use
P2k's worker; final intent is captured synchronously before it is submitted.

**[read] Modifier path:** Window capture-phase key events now feed the actual
preference and final operation, unlike r2's logging-only modifier observation.
Verified GTK's controller handles modifier-state changes before the key signal in
`gtk-4.22.4:gtk/gtkeventcontrollerkey.c:84-145`. Active-target refresh uses GDK
status for stationary key changes, masked parent targets unregister, and completed
or cancelled drops unregister active targets. The tracker has unknown/live/start
origins and clears start seeds after a drag. Synthetic tests cover held, pressed,
released, combined and unsupported modifiers, missing evidence, external offers,
and preservation of captured intent. These tests verify state handling, not actual
key delivery during the compositor's DnD grab.

**[read] Badge:** The global display provider reaches the DragIcon root. CSS now
uses bold TEXT_SMALL with token-scaled padding and paired accent_bg/accent_fg colours.
No hard-coded widget colours added. Real contrast and clipping remain for Kyle.

**Live:** not checked by ChatGPT. The previous captured runtime log predates this
implementation and cannot establish whether the new window tracker receives keys.
The fetched niri/Smithay source explanation is supporting diagnosis, not observed
proof of this installed compositor's behavior; no further source was downloaded by
this reviewer. The coding agent's claimed successful launch is its observation only.

**Still open (Kyle, disposable files):**
1. Launch this rebuilt release with GIMBAL_LOG=1 and stderr captured separately in
   `/tmp/gimbal-dnd-r3-kyle.log`.
2. Same filesystem: Ctrl held before the initial press must copy. Repeat by starting
   the drag first, pressing Ctrl while hovering, then dropping. Original must remain.
3. Different filesystem: Shift must move, both held before press and pressed during
   hover. Confirm the destination is a different filesystem and check both locations.
4. Press and release Ctrl/Shift while stationary: the operation must return to its
   filesystem default; test Ctrl+Shift priority. Check ordinary defaults as well.
5. Three selected files show a bold, contrasting 3; 12 is not clipped. Recheck padding,
   split targets, Dolphin both ways, pins/pin-by-drop, cancel and undo.
6. Read the new log for prepare evidence, key down/up, target identity, requested and
   final actions. If there are no key events during DnD, record that limitation rather
   than claiming press-after-start or release is fixed. Zen matrix remains unverified.

No commit or push. Overall acceptance remains pending Kyle's live modifier/badge checks.

## Live acceptance and badge follow-up (Nov 2, 2026)

**[read: Kyle reports]** Ctrl-copy works, Shift-move across drives works, releasing
keys restores defaults, and padding/interoperability/pinning/cancel/undo checks pass.
The bold badge is better, but black text on the purple accent background remains
hard to read. Modifier acceptance passes by Kyle's report; badge polish remains.

**[observed in captured log]** `/tmp/gimbal-dnd-r3-kyle.log` now contains real key
press/release events and positive prepare evidence. At 43.531 s, same-filesystem
Ctrl uses selected/preferred/final Copy. At 16.551 s, different-filesystem Shift to
the external drive uses selected/preferred/final Move. This supports the reported
fix; the complete release/default interaction is accepted from Kyle's report.

**Fix (ChatGPT): badge uses the window text/background pair.** In shared CSS,
replaced accent_bg/accent_fg fill/text with BG/FG (window_bg_color/window_fg_color),
kept bold TEXT_SMALL, and added a LINE_THIN accent-colour border. Fill is not
multiplied by the window opacity setting. This avoids relying on the difficult
black-on-purple pair; colours still follow the theme. Updated the existing badge
CSS check to the new role pair. No modifier code changed in this follow-up.

**[observed]** Final `scripts/check.sh`: all gates pass, 356 tests passed, 6 ignored,
fm-ui 167. Full output `/tmp/gimbal-p2l-r3-badge-review-check.log`. Remaining live
check: Kyle's readability verdict on the new badge (and multi-digit clipping).
Zen matrix remains separate/unverified. No commit or push.

## Final live verdict (Kyle reports, Nov 2, 2026): passes

**[read: Kyle reports]** The revised badge “Looks good now”. Combined with Kyle's
prior passing Ctrl-copy, cross-drive Shift-move, key-release/default and regression
checks, P2l-r3 is accepted. Final tested code gates: 356 passed, 6 ignored (see above).
No new code changes or gate rerun for this acceptance entry. Zen's 8-row matrix
remains a separate unverified item. No commit or push yet.

## Zen matrix attempt (Kyle run, ChatGPT inspected, Nov 2)

**[observed in captured files]** Only one app matrix capture exists:
`docs/research/phase0/captures/20261102-194406-app-outside`. Its first sample.png row
aborted before drag: the runner did not find the test Zen window within 15 seconds.
`source_intact: yes`; no start_drag, no received offer; remaining seven rows did not run.
The compressed Zen stderr says `bwrap: Can't find source path
/run/user/1000/doc/by-app/app.zen_browser.zen: No such file or directory`.
That directory is also absent at inspection. This is a Flatpak/document-portal
launch failure, not a failing Gimbal drag. No runner, portal, config or window
changes were made by ChatGPT. Matrix remains unverified; resolve test Zen launch
before retrying. No pointer injection was performed by the reviewer.

## Manual Zen check (Kyle, Nov 4, 2026)

**[Kyle reports]** Dragging files from Gimbal into Zen worked. The manual Gimbal-to-Zen check passed. The scripted eight-row matrix is still pending.

## Zen matrix (Claude, Nov 4, 2026): 8 of 8 pass

`tools/dnd-capture/run-matrix-gimbal.sh app`, with Kyle's app closed and DP-3 off (one output,
DP-2 at 2560×1440). The release build is from 07:52, which matches HEAD `c288b33`.

**[observed]** Every row's drag reached Zen, and Zen opened the file. Every row reported
`source_intact: yes`.

| Location | Dragged | Zen's title |
| --- | --- | --- |
| outside | `sample.png` | `sample.png (PNG Image, 16 × 16 pixels)` |
| outside | `sample-dir` | `Index of file:///run/user/1000/doc/4836a13c/sample-dir` (through the portal) |
| outside | `sample-link.png` | `sample.png (PNG Image …)` (the link resolved) |
| outside | `sample.png` + `sample.txt` (preselected) | `sample.png (PNG Image …)` |
| downloads | `sample.png` | `sample.png (PNG Image …)` |
| downloads | `sample-dir` | `Index of file:///home/kyle/Downloads/gimbal-dnd-test-gimbal/sample-dir` |
| downloads | `sample-link.png` | `sample.png (PNG Image …)` |
| downloads | `sample.png` + `sample.txt` (preselected) | `sample.png (PNG Image …)` |

- **[observed]** Every row offered the same MIME types: `application/x-gtk-local-dnd`,
  `text/plain;charset=utf-8`, `text/uri-list`, `application/vnd.portal.filetransfer` and
  `application/vnd.portal.files`. Zen read `text/uri-list` and `application/vnd.portal.files`.
- **[observed]** The 4 outside rows got there through the document portal (`/run/user/1000/doc/…`),
  as Phase 0 found for Dolphin and Nautilus.
- **[inferred]** The Nov 2 failure was the missing portal folder, as ChatGPT diagnosed. The folder
  `/run/user/1000/doc/by-app/app.zen_browser.zen` exists now.
- Captures: `docs/research/phase0/captures/20261104-0804*` and `20261104-0805*`, plus
  `20261104-080602`.

**Fix (Claude): the runner never removed the app's scratch folders.**
- **The cause:** `run.sh` created `SCRATCH` with `mktemp`, then reset it to `""` on the line that
  clears its PID variables. That left the cleanup trap with nothing to remove.
- **The fix:** the reset now runs before `mktemp`. `bash -n` passes. The next matrix run will show
  it working.
- **Still on disk:** the 8 empty `/tmp/gimbal-dnd-app-*` folders from this run. They hold no files.
