# P2ab: edit a file you don't own in the preview, as administrator

## What this change does

- Adds `WriteAccess::admin_editable(owner_uid, current_uid)` and
  `PreviewDocument::admin_editable(owner_uid, current_uid)` in `fm-core`.
  A document is administrator-editable only when its sole block is
  `NotWritable` and/or `ParentNotWritable`, the text is not mixed-ending, and
  `admin::needs_admin(owner, current)` holds.
- Adds `PreviewSession::begin_admin_edit`, `begin_edit_for` and
  `begin_save_original_for`, plus `PinnedDocument.admin` and
  `SaveTicket.admin`. Only `Caller::Ui` may enter administrator edit mode or
  save one; a hotkey is refused with `administrator edits are UI-only`.
- Adds `SaveTarget::Admin`. It writes the same bytes P2m would write through
  `gio::File::replace_contents` on the `admin://` file, passes the load-time
  ETag, uses `make_backup = false`, and restores the original owner, group and
  mode through the same administrator file before reporting success.
- The preview shows `Alt-click to edit as administrator.` for these documents.
  Alt-click enters administrator mode without asking for a password: the
  preview becomes editable, the P2z red/error banner with the lock appears
  above the text, and the panel gets the error border. The banner reads
  `Administrator: saving changes a file owned by <owner>.`
- `preview.save` in administrator mode always asks first. The dialog names the
  full path and owner, says it changes a system file, and has `Cancel` and a
  destructive `Save as Administrator`. Cancel changes nothing.
- After confirming, GVfs's administrator mount is requested through P2z's
  helper. Cancelling polkit changes nothing and reports
  `Administrator access was not granted.` The write then happens on the
  preview worker. A successful save says `Saved as administrator.`
- `preview.save-as` is refused in administrator mode with
  `Save As isn't available while editing as administrator.` `preview.discard`
  works as before and leaves administrator mode.
- P2z's administrator tabs now allow these preview commands only for an
  administrator-editable document (or an active administrator editor). Every
  other P2z restriction remains.
- Logs one `admin:` line for entering administrator edit mode, each warning
  shown and its answer, and each save result with its byte count. File contents
  are never logged.

No dependency, config or session schema changed.

## Files written

- `crates/fm-core/src/preview.rs`
- `crates/fm-core/src/preview/tests.rs`
- `crates/fm-ui/src/preview.rs`
- `crates/fm-ui/src/look/css.rs`
- `crates/fm-ui/src/app.rs` — outside the brief's Output list; see
  "Outside the Output list".
- `docs/notes/P2ab.md` (this file)

No catalogue or agent-module change was needed: P2m already refuses
`preview.edit`, `preview.save`, `preview.save-as` and `preview.discard` for the
CLI and agent before a handler. The caller-aware administrator refusal is
inside `PreviewSession`, where it can be unit-tested. **[observed]**

## Which files and the owner

`WriteAccess::admin_editable` is true only for a permission block:

- `NotWritable` alone, `ParentNotWritable` alone, or both flags false with no
  recorded reason;
- `reason == NotWritable` or `ParentNotWritable`;
- never for `Symlink`, `MultipleHardLinks` or `MixedLineEndings`;
- never when the owner is the current uid;
- never when nothing is blocked at all.

The brief's "regular file" condition is guaranteed at load: `preview::load`
returns `UnsupportedReason::SpecialFile` for every non-regular file before it
builds a `PreviewDocument` (`crates/fm-core/src/preview.rs:2064`). A symlink is
the non-regular case the unit test exercises; its `Symlink` reason is refused
by the same function. **[read]** **[observed]**

`PreviewDocument::admin_editable` also rejects mixed line endings, because
`WriteAccess` is only about permissions. **[read]**

The owner is recorded with the loaded document in the preview panel
(`PreviewPanel::record_owner`) using `std::fs::metadata(path).uid()`. It is used
for the status line, banner, warning and `admin_editable` decision. The owner is
read when the document is displayed, which is the first point the panel knows
which document is current. **[observed]**

`preview::owner_name(uid)` returns `root` for 0, otherwise the `/etc/passwd`
name, falling back to the uid. **[observed]**

## Entering the mode

Alt-click continues to dispatch `preview.edit` as `Caller::Ui`; the command
handler now passes the caller through. `PreviewSession::begin_edit_for` checks
the document first:

- administrator-editable + `Caller::Ui` → `begin_admin_edit`;
- administrator-editable + any other caller → `administrator edits are UI-only`;
- ordinary writable text → P2m's `begin_edit`, which still accepts UI or a
  configured hotkey.

No mount or polkit prompt happens here. The banner and border are refreshed from
`PreviewSession::is_admin_editing()` in `update_actions`, so Discard, a
successful save, Reload and close all remove them through the existing state
transitions. **[observed]**

The banner is a `gtk::Box` with class `gl-preview-admin-banner` and the
`changes-prevent-symbolic` lock. It uses the theme's `@error_bg_color` and
`@error_fg_color`; the panel's `gl-preview-admin` class adds the error border.
No fixed colour was added. This extends P2z's `.gl-admin-banner` colours in
`look/css.rs`; it does not duplicate a palette. **[read]** **[observed]**

## Saving

`preview.save` in administrator mode never starts a write directly:

1. `PreviewPanel::show_admin_save_warning` shows the every-time dialog. Its
   answer is logged.
2. On `Save as Administrator`, `Process::ensure_admin_mount` (the shared
   helper extracted from P2z's `open_admin_tab`) mounts the `admin://` location
   if this process has not mounted it yet. This is where polkit asks.
3. On success, `PreviewSession::begin_save_original_for(Caller::Ui)` freezes
   the ticket and `start_save_ticket` builds `SaveTarget::Admin`.
4. The preview worker runs `preview::save`, which encodes the text with P2m's
   exact `TextEncoding` and calls `replace_contents` on
   `admin::gfile(path, Scope::Admin)`.
5. The load-time ETag is the `etag` argument. `make_backup` is `false`.
6. After the replacement, the original uid, gid and mode are set through the
   same admin file in that order. Only then is success reported.

The dialog and warning are in `crates/fm-ui/src/preview.rs`.

### ETag and conflict

GIO documents that `g_file_replace_contents` compares a supplied ETag and
returns `G_IO_ERROR_WRONG_ETAG` when it differs:
`/usr/share/gir-1.0/Gio-2.0.gir:53252-53269`. The pinned binding maps it to
`gio::IOErrorEnum::WrongEtag` (`gio-0.22.10:src/auto/enums.rs:2142`), and the
administrator path maps that to P2m's `SaveError::ChangedOnDisk`, whose message
is `File changed on disk.` **[read]** **[observed]**

The load-time ETag comes from the ordinary `gio::File::for_path` query, not an
`admin://` query, because querying the admin URI before the mount would trigger
the polkit prompt just by looking at a file. GVfs's admin backend forwards
`query_info` to `g_file_query_info` on the real file with the caller's
attribute mask (`gvfsbackendadmin.c:169-201`), so `etag::value` has the same
representation on both sides; the value is then the precondition on the admin
`replace`. This preserves the brief's "no password on entering the mode". The
live conflict check is still the one that matters. **[read]** **[inferred]**

### Owner, group and mode

GVfs's admin backend `do_replace` calls `g_file_replace` from a process running
under `pkexec` with `fsuid(0)` and `CAP_CHOWN` (`gvfsbackendadmin.c:356-380`,
`1018-1043`). The replacement is therefore created with the administrator's
ownership/umask rather than necessarily the original metadata. Its
`do_set_attribute` forwards `g_file_set_attribute` on the real file
(`gvfsbackendadmin.c:745-763`), so Gimbal restores `unix::uid`, `unix::gid` and
`unix::mode` through that same file **after** the replace and **before** it
reports success. **[read]** **[observed]**

The fake in `preview/tests.rs` records the exact order:
`replace`, `unix::uid`, `unix::gid`, `unix::mode`. It also proves the file URI
is the `admin://` one and that the call carries the load ETag with
`make_backup=false`.

## Refusals

- `preview.save-as`: `PreviewSession::begin_save_as` refuses an administrator
  editor with `Save As isn't available while editing as administrator.` The
  button is disabled with the same tooltip, and `PreviewPanel::save_plan`
  refuses a Save As dry run in administrator mode.
- `preview.edit` from a non-UI caller on an administrator-editable document:
  `administrator edits are UI-only`.
- `preview.save` from a non-UI caller while in administrator mode:
  `administrator edits are UI-only`.
- CLI and agent callers never reach a preview handler (P2m's existing caller
  rule), so they cannot enter the mode or save.

The core tests call `begin_edit_for` with `Caller::Hotkey`, then `Ui`, and
`begin_save_original_for` with both. The agent socket's existing end-to-end
refusal for `preview.edit` still passes.

## API citations

- `gio-0.22.10:src/auto/file.rs:2096` — `replace_contents(contents, etag,
  make_backup, flags, cancellable)`.
- `gio-0.22.10:src/auto/file.rs:2460` — `set_attribute_uint32`.
- `gio-0.22.10:src/auto/constants.rs:334,343,352` — `unix::gid`, `unix::mode`,
  `unix::uid` attribute names.
- `gio-0.22.10:src/auto/enums.rs:2142` — `IOErrorEnum::WrongEtag`.
- `/usr/share/gir-1.0/Gio-2.0.gir:53252-53269` — `g_file_replace_contents`
  ETag precondition and `G_IO_ERROR_WRONG_ETAG`.
- `gtk4-0.11.5:src/auto/image.rs:74` — `Image::from_icon_name` for the lock.
- `libadwaita-0.9.2:src/auto/alert_dialog.rs:40` — `AlertDialog::new(heading,
  body)`; `:441` `add_response`; `:613` `set_default_response`; `:669`
  `set_response_appearance`; `:704` `connect_response`.
- `libadwaita-1.9.1/src/stylesheet/_colors.scss:22-23` and
  `:196-197` — the `@error_*` colours P2z already uses. **[read]**

## Verification

`scripts/check.sh` passed on the final tree:

```text
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Test totals: **941 passed, 23 ignored** (23 + 16 + 12 + 458 + 6 + 11 + 415
passing; 0 + 0 + 0 + 1 + 6 + 11 + 5 ignored). P2z's recorded baseline was 935
passed / 23 ignored, so this adds six passing tests. Log:
`/tmp/gimbal-p2ab-check-final2.log`. **[observed]**

The new tests cover:

- `admin_editable` for `NotWritable`, `ParentNotWritable`, both permission flags,
  each recorded permission reason, every other `ReadOnlyReason`, an owned file,
  nothing blocked, and the symlink/non-regular case;
- mixed line endings rejected even with a permission block;
- `owner_name(0) == "root"`;
- the non-UI `preview.edit` and `preview.save` refusals, and Save As refused in
  administrator mode;
- the administrator save call against a fake: `Scope::Admin` URI, load ETag,
  `make_backup=false`, byte count, owner/group/mode order, and the
  `WrongEtag → ChangedOnDisk` mapping with no metadata write after a conflict.

## What Claude checks live

Kyle creates the root-owned scratch file first (Claude creates
`/tmp/gimbal-p2ab` beforehand):

```bash
sudo sh -c 'printf "line one\n" > /tmp/gimbal-p2ab/root.conf' && sudo chmod 0644 /tmp/gimbal-p2ab/root.conf
```

1. Select `root.conf`: its text appears and the status says
   `Alt-click to edit as administrator.`
2. Alt-click: the preview turns red, with the lock and
   `Administrator: saving changes a file owned by root.` No password is asked.
3. Type a line and press Save:
   - the warning names `/tmp/gimbal-p2ab/root.conf` and `root`;
   - `Cancel` leaves the file unchanged (check with `cat`);
   - Save again, then `Save as Administrator`: polkit asks once, and the status
     says `Saved as administrator.`
4. `stat` shows `root:root 0644` and `cat` shows the new line.
5. A second save in the same run warns again, but polkit does not ask again
   because the mount is still up.
6. Change the file from a terminal with `sudo` while an edit is open: Save is
   refused as a conflict (`File changed on disk.`).
7. Save As is refused with `Save As isn't available while editing as
   administrator.` Discard returns to the read-only view.
8. Check the look in light and dark, tiled and floating.

Also worth checking inside a P2z administrator tab: select a root-owned 0644 text
file, Alt-click, and confirm the same banner/warning/save path; a normal writable
file in that tab stays read-only. **[inferred]**

The app was not launched by this agent: the rules reserve pointer/keyboard
interaction and the polkit prompt for Claude and Kyle. **[observed]**

## Outside the Output list

`crates/fm-ui/src/app.rs` is not in the brief's Output list, but items 5 and 6
cannot be implemented without it:

- the P2z administrator-tab gate must ask the preview whether a document is
  administrator-editable or already in administrator mode before refusing the
  four preview commands;
- `preview_command` must pass the dispatched `Caller` through to
  `begin_edit`/`save`/`save_as`, which is what makes the `administrator edits
  are UI-only` refusal caller-aware.

The same file also receives the shared `ensure_admin_mount` helper extracted
from P2z's `open_admin_tab`, so the preview save and P2z's administrator tab use
one mount path and one `admin_mounted` flag. Kyle approved this minimal
`app.rs` change before the work started. No other file outside the Output list
was touched.

`WriteAccess` and `PreviewDocument` keep their existing fields, so the three
test-only struct literals in `crates/fm-ui/src/app.rs` and
`crates/fm-ui/src/folder/mod.rs` did not need changes. The owner is therefore
recorded by the preview panel at load display rather than in the core struct.
If the reviewer prefers the owner in `WriteAccess`, adding one field would mean
touching those test literals too.

`crates/fm-ui/src/folder/view.rs:321` still says preview editing is off in an
administrator tab. That tooltip is now too broad; the brief's Output list does
not include the file, so it is reported here rather than changed. **[observed]**

## Known limits

- The administrator save is synchronous on the preview worker, like P2m's
  ordinary save. A very large file can block that panel's worker until GVfs
  returns; it cannot block the GTK main thread.
- The ETag precondition is the conflict check. If GVfs supplies no ETag, the
  admin replace runs without one and the conflict window is wider; the live
  check uses a normal local file where the ETag exists.
- Restoring the mode after the replace is a separate call. If the replace
  succeeds and a later `set_attribute` fails, the contents are already written
  but the save is reported as failed; the edits stay in the buffer. This is the
  honest order the brief asks for.
- A root-owned file that is also world-writable (or whose parent is writable)
  has no permission block, so it stays on P2m's ordinary edit path rather than
  administrator mode.
- No live app or polkit run was performed by this agent.

## Claude's review (Nov 6, 2026)

**Code.** The return matches the brief. Its `app.rs` edits stay within what
Kyle approved:
- the admin-tab preview gate, which now lets a preview command through only
  when the preview says the document is admin-editable or already in
  administrator mode;
- the caller passed into `preview_command`;
- the shared `ensure_admin_mount` helper, extracted from P2z.

**[read]**

Reading the ETag through `file://` keeps "no password just to look", at the
cost of relying on GVfs's admin `query_info` returning the same value. P2m's
file monitor catches an outside change before a save is even attempted, so the
ETag check is a second guard. **[read]** **[observed]**

**Changes by Claude:**
- `crates/fm-ui/src/folder/view.rs`: the admin-tab grid tooltip said preview
  editing is off, which P2ab makes untrue. It now says Alt-click edits a file
  you don't own as administrator (the agent reported this but couldn't touch
  the file).
- `crates/fm-ui/src/preview.rs` (Kyle's request after the live run): in
  administrator mode **Save As is hidden**, not just greyed out. The action
  row is Discard, a stretching spacer, Save and Save As, so Save moves into
  Save As's place at the right edge. Save's tooltip reads `Save as
  administrator (asks first)` in that mode.
- Then, also at Kyle's request, the change is animated. Entering the mode,
  Save As fades out (`.gl-fade`/`.gl-faded`, an opacity transition), then its
  `gtk::Revealer` slot collapses (`SlideLeft`) so Save slides over. Leaving
  the mode, the slot opens and Save As fades back in. Durations are
  `motion::ms(120)` for the fade and `motion::ms(160)` for the slide, and a
  generation counter drops a stale second step when the mode flips quickly.

**Gates.** `scripts/check.sh`: 941 passed, 23 ignored. **[observed]**

### Live checks (Kyle did the clicks, typing and sudo; Claude read the log and the file)

Debug build on scratch XDG dirs, `/tmp/gimbal-p2ab/root.conf` created by Kyle
as root, 0644.

1. Alt-click entered administrator mode with no password
   (`admin: entered administrator edit mode … (owner root)`).
2. The red banner showed with the owner.
3. Every save warned, and Cancel was logged as `answered cancel`, with no
   write.
4. Confirmed saves logged `saved … (15 bytes)` and then `(22 bytes) as
   administrator`. The mount was granted once; the second save didn't prompt.
5. After both saves the file is `root:root 0644` with Kyle's lines. Owner and
   mode were kept.
6. Kyle appended to the file with `sudo tee` while editing. The preview
   monitor logged `Changed`, and the outside change was caught before any
   save (Kyle saw the conflict).
7. Save As was refused, and Discard returned to the read-only view (Kyle).

Kyle checked the hidden Save As and its fade/slide animation on screen (Nov 6). **[observed]**
