# P2m: preview sidebar and text editing

## What was built

Tags: **[observed]** the code and tests in this working tree; **[read]** pinned API sources; **[inferred]** behavior that needs a live display.

- **[observed]** **Core preview contract** in `crates/fm-core/src/preview.rs`:
  `Selection`, `PreviewSession`, `ViewState`, `PreviewDocument`,
  `TextDocument`, `FileRevision`, strict UTF-8/BOM/line-ending decoding,
  size/dimension limits, read/version inspection, the typed save request and
  the atomic save implementation. Unit tests are in
  `crates/fm-core/src/preview/tests.rs`. `crates/fm-core/src/lib.rs` exports
  only the new `preview` module.
- **[observed]** **One preview panel per model window** in `crates/fm-ui/src/preview.rs`,
  appended to the existing P2g right sidebar shell. It reuses the existing
  title row and collapse button, not a nested panel. Selection comes from the
  window's focused tab/tile, never a process-global selected file.
- **[observed]** **Text preview** with a virtualized, independently scrolling `GtkTextView`
  and a synced logical-line gutter. The title shows the file name and actual
  size; the action row stays at the bottom.
- **[observed]** **Image and cached-media previews** in a shrinkable `GtkPicture`, decoded on
  the panel worker and turned into a `GdkMemoryTexture` only on the main
  thread. PDF/video use only an existing GIO thumbnail cache entry.
- **[observed]** **Alt+click editing**, Discard/Save/Save As, conflict and changed-on-disk
  dialogs, dirty-close/quit guards, external file/parent monitors and text-key
  ownership.
- **[observed]** **Shared look support** for the desktop monospace family, gutter/status
  styles and a `PREVIEW_GUTTER` token in `look/tokens.rs`, `look/css.rs` and
  `look/mod.rs`. `Look` now holds its settings behind `Cell`/`RefCell` and is
  shared as `Rc<Look>` so the monospace GSettings watcher can regenerate the
  one stylesheet without a CSS copy in the widget.
- **[observed]** **Caller policy** in `crates/fm-core/src/command/mod.rs`: existing
  `preview.edit`, `preview.discard`, `preview.save` and `preview.save-as`
  refuse `Cli` and `Agent` before a handler; `Ui` and `Hotkey` are allowed.
  No command names, arguments or new commands were added.
- **[observed]** **Text-key ownership** in `crates/fm-ui/src/fileops/keys.rs`: while the
  preview `GtkTextView` owns focus, Ctrl+A/C/X/V/Z/Y, Delete/Backspace, arrows,
  Home/End/Page keys, Return and Tab are left to the editor, so configured
  file Delete/Undo shortcuts cannot act on the folder selection behind it.

## Design

### Selection and stale loads

**[inferred]** `Selection` is `None`, `Multiple { count }`, `Trash` or one `File` path.
`WindowUi::update_content` builds it from the focused tab's
`FileSelection.names`; on the Trash page it passes `Trash`, so the hidden
folder selection is not previewed. `PreviewSession::select` increments a
generation for every real candidate change and returns a `LoadTicket`. A
worker result is accepted only when its ticket generation still equals the
session generation. The worker is per panel, so a window never shares a buffer
or a file chooser with another window. A stale completion is dropped even if
the old read finishes after the new selection.

A dirty pinned document is separate from the clean `ViewState`. Selection
changes only update `candidate`; they never replace the pinned buffer. Trash,
tab/tile focus, navigation and sidebar collapse therefore leave the dirty
buffer in that window. No selection shows `Select a file to preview`, several
show `N items selected`, a folder shows its name and that it has no editor, and
Trash shows a neutral `Trash` state unless a dirty editor is pinned.

### Text, encodings and limits

**[observed]** `preview::load` refuses non-regular files before reading (symlinks are followed
only after checking the target is regular), bounds text at 2 MiB and uses a
bounded `std::fs` read for at most `MAX_TEXT_BYTES + 1` bytes (so a file
growing during the read is refused rather than shown truncated; GIO's
`load_contents` was not used because it allocates the whole file first). A UTF-8 BOM is stripped for display and recorded;
`decode_text` converts CRLF/lone CR to buffer newlines for display only. Uniform
LF/CRLF is saved back with the original BOM. Mixed or lone-CR endings are
previewed with a reason and `TextDocument::edit_block_reason` returns
`MixedLineEndings`, so they are read-only. NUL bytes and invalid UTF-8 are
unsupported and never decoded lossily. An empty file is valid. Logical lines
are `buffer.line_count()`: a trailing newline produces a final empty numbered
line; soft wraps do not add a buffer line. The gutter is drawn only for the
visible line range (`GtkTextView::iter_at_location` / `iter_location`) so a
2 MiB file with many lines does not create one widget per line.

### Images and cached media

**[observed]** Images are bounded at 64 MiB before reading. `PixbufLoader::connect_size_prepared`
reports dimensions before pixel allocation; `preview::image_target_size` rejects
more than 64 megapixels and scales allowed images to a 2048 longest edge with
`PixbufLoader::set_size`. Oversized images get a one-pixel loader size and are
discarded before a texture is created, so the refusal does not claim a
post-allocation check prevents allocation. The decoded RGBA bytes cross to the
main thread as `Vec<u8>`; `GdkMemoryTexture::new` creates the paintable there.
For PDF/video, `thumbnail::path`, `thumbnail::path-large` and
`thumbnail::path-normal` are queried through GIO; the cached file is accepted
only if it is a regular non-symlink file of at most 64 MiB. No thumbnailer is
spawned. Image/media previews have no editor and all three action buttons are
disabled.

### Save, revision and conflicts

**[observed]** `SaveRequest` carries the exact path, text, `TextEncoding`, and either an
`Original` expectation (load-time `FileRevision`) or a `NewPath` optional
`ReplaceConfirmation`. A save captures the pinned document, buffer revision and
text before starting; the text view is temporarily non-editable, and a result
is applied only if the ending buffer revision matches the captured one.

Original saves re-query device/inode, size, mtime, ctime and ETag, and refuse
with `ChangedOnDisk` when any differ. The write uses
`gio::File::replace_contents(bytes, expected_etag, make_backup=false,
FileCreateFlags::NONE)`. GIO's own documentation says this uses
`g_file_replace()` and atomic renames for local files, and returns
`G_IO_ERROR_WRONG_ETAG` when the precondition fails. The original mode is
restored after the atomic replacement; no parent folders are created, no backup
is requested and no in-place truncation is used.

Save As to a path that does not exist creates the destination with
`OpenOptions::create_new(true)` (the no-overwrite check), then commits through
the same conditional GIO replacement. If a file appeared after the chooser,
`create_new` fails with `DestinationAppeared`. If the write fails, only a
zero-length placeholder with the same device/inode is removed. Save As to an
existing path first asks the UI to inspect the destination on the worker; the
Replace confirmation stores that path and `FileRevision`, and the core refuses
`DestinationChanged` if it changed before the write. Save As to the original
path is routed through original Save's load-time checks instead. A successful
Save As changes the preview document path/revision to the chosen file and
leaves the original untouched.

`preview.save` and `preview.save-as` are the only save paths; `preview.save-as`
requires its existing explicit `path` argument. `dry-run=true` returns
`preview::save_plan`, which is pure and does not touch the filesystem.

### External changes and close guards

**[observed]** Each loaded document installs a `GFileMonitor` on the file and its parent
(`monitor_file` / `monitor_directory`, `WATCH_MOVES`). Events are debounced and
checked on the worker by re-reading the revision only. A clean preview reloads
when the revision differs; a dirty preview keeps its buffer, sets
`File changed on disk`, and offers Reload (discard edits), Save As and Cancel
(Cancel is the close/default/Escape response). A wrong ETag from a save takes
the same changed-on-disk path. Own-save notifications are harmless: the save
sets `saving` while the write runs, and after success the document revision is
the freshly committed one, so an arriving check compares equal and ignores the
event. Monitors are replaced/dropped with each document and when the panel is
dropped.

**[inferred]** Window close and `app.quit` ask Save/Discard/Cancel for each dirty preview.
Cancel keeps the window and buffer. A failed or cancelled save keeps the window
open. Discard clears the pin, reloads the still-selected candidate and returns
to read-only; Save clears the pin only after the committed result arrives.

## API citations (pinned sources)

**[read]** The following names and line numbers were checked in the pinned crate sources and installed GIR/schema files.

- GIO 0.22.10 conditional/atomic replacement:
  `gio-0.22.10:src/auto/file.rs:2096` (`replace_contents`). The installed GIR
  documentation (`/usr/share/gir-1.0/Gio-2.0.gir:53252-53269`) says the ETag
  precondition returns `G_IO_ERROR_WRONG_ETAG`, that `g_file_replace()` is used,
  and that atomic renames are used for local files. `make_backup=false` avoids
  backup litter.
- GIO 0.22.10 file attributes:
  `gio-0.22.10:src/auto/constants.rs:33` (`access::can-write`), `:52`
  (`etag::value`), `:248` (`thumbnail::path`); `:src/auto/file_info.rs:65`
  (`FileInfo::boolean`), `:144` (`attribute_string`), `:182` (`content_type`),
  `:272` (`size`).
- GIO 0.22.10 inspection/monitoring: `src/auto/file.rs:1153`
  (`monitor_file`), `:1756` (`query_info`); `src/file.rs:498`
  (`load_contents`, consulted but not used: it allocates the whole file before
  a size check, so the preview uses a bounded `std::fs` read instead);
  `src/auto/file_monitor.rs:74`
  (`connect_changed`). `src/auto/settings.rs:30` (`Settings::new`), `:509`
  (`connect_changed`) read the desktop monospace name.
- GTK 0.11.5 text/gutter: `src/auto/text_view.rs:711` (`iter_location`),
  `src/auto/text_buffer.rs:386` (`iter_at_line`), `:458` (`line_count`).
  `src/auto/file_dialog.rs:410` (`FileDialog::save`) is the Save As chooser.
- GTK 0.11.5 picture: `src/auto/picture.rs:188` (`set_can_shrink`), `:198`
  (`set_content_fit`).
- GDK 0.11.5 `GdkMemoryTexture`: `src/auto/memory_texture.rs:19` (main-thread
  construction from worker-produced RGBA bytes).
- gdk-pixbuf 0.22.0: `src/auto/pixbuf_loader.rs:27` (`new`), `:113`
  (`set_size`), `:120` (`write`), `:69` (`close`), `:246`
  (`connect_size_prepared`); `src/auto/pixbuf.rs:229` (`add_alpha`), `:477`
  (`rowstride`), `:502` (`read_pixel_bytes`).
- GTK 0.11.5 legacy controller / key facts: `src/auto/event_controller_legacy.rs:41`
  (`connect_event`), `gdk4-0.11.5:src/auto/event.rs:126` (`modifier_state`),
  `gdk4-0.11.5:src/auto/button_event.rs:27` (`button`).
- Desktop monospace schema:
  `/usr/share/glib-2.0/schemas/org.gnome.desktop.interface.gschema.xml:165`
  (`monospace-font-name`). The generic `monospace` family is only the fallback
  when the schema is absent.

## Limits, policy and honest limitations

**[read]** The limits and caller rules come from the code above; **[inferred]** the residual race descriptions are limitations of the chosen APIs.

- Text: 2 MiB (`MAX_TEXT_BYTES`), strict UTF-8, no NUL, no mixed-ending editing.
- Images: 64 MiB compressed and 64 megapixels decoded; longest decoded edge is
  2048. This is a preview texture, not an editor.
- Media: cached GIO thumbnails only; no decoder, player or thumbnailer process.
- `Cli` and `Agent` are refused for all four existing preview commands before a
  handler. `Ui` and `Hotkey` are allowed. There is still no revision-aware
  editing API for agents.
- Symlinks and multiply hard-linked files are shown read-only with a reason.
  The save path never replaces a symlink or silently breaks a hard link.
- ETag is a write precondition, not an object identity. Device/inode plus
  size/mtime/ctime are checked too. If a backend supplies no ETag, the
  identity/version check and replacement still happen, but there is a
  check-to-replace interval that the ETag would otherwise narrow; this is
  recorded as a limitation rather than called atomic identity validation.
- The Save As new-path placeholder is created with `create_new`, then GIO
  replaces it conditionally. A failure removes only a zero-length placeholder
  with the same device/inode; a partially committed foreign file is never
  deleted.
- `preview.save-as` command arguments are UTF-8 strings. A Save As chooser result
  with a non-UTF-8 path is refused with a message; original Save works for
  non-UTF-8 names because it uses the pinned `PathBuf`.
- The preview worker serialises its own jobs. A very large image read already in
  progress is not interrupted by a newer selection; its result is ignored by
  generation. The read is bounded, so the delay is bounded too.
- Alt+click will only reach the app if the desktop/compositor does not reserve
  Alt+primary-click for window movement. This is listed as unverified live.
- No GtkSourceView/syntax highlighting, PDF/video decoding or playback,
  binary/non-UTF-8 editing, mixed-ending editing, autosave, search, crash
  persistence or agent editing API was added.

## Gates

**[observed]**
`./scripts/check.sh` from the repo root passed on Nov 3, 2026:

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
test result: ok. 1 passed; 0 failed; 0 ignored
test result: ok. 7 passed; 0 failed; 0 ignored
test result: ok. 250 passed; 0 failed; 0 ignored
test result: ok. 6 passed; 0 failed; 6 ignored
test result: ok. 11 passed; 0 failed; 11 ignored
test result: ok. 189 passed; 0 failed; 0 ignored
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Total: **464 passed, 17 ignored**. The new preview tests cover
selection/no/multiple/folder; stale generations; dirty pin across
selection/navigation/Trash; Discard; save/reconcile; UTF-8/BOM/LF/CRLF round
trips; mixed/binary/invalid-UTF-8 refusals; text and image limits; logical
lines; non-UTF-8 names; original save and mode preservation; changed/missing/
wrong-ETag originals; symlink/hardlink policy; Save As race and Replace
confirmation; dry-run; and close choices. The UI tests cover text-key
ownership. Scratch files use only `/tmp/gimbal-preview-*`.

## App smoke runs (no interaction injected)

**[observed]**
- Checked `pgrep -a gimbal-files` first and ran only with
  `XDG_CONFIG_HOME=/tmp/gimbal-preview-smoke/config`,
  `XDG_STATE_HOME=/tmp/gimbal-preview-smoke/state`, `GIMBAL_LOG=1`, for 3–5 s.
- Launched `./target/debug/gimbal-files /tmp/gimbal-preview-smoke/note.txt`.
  The log showed `selection File`, `load generation 1`, worker read, and an
  accepted load. A second run selected `bad.png`; the worker classified and
  attempted a decode without a panic. Each run was stopped with SIGTERM on its
  own app PID, and no instance was left running.
- No `file.open`, `app.disk-usage`, clipboard write, real Trash operation or
  desktop config change was triggered.

## Not verified live

**[inferred]**
Kyle's checklist: text and line numbers; empty/large/binary/Unicode/CRLF files;
images, cached media and fallbacks; rapid selection and split focus; Alt+click
editing; text keys versus file Delete/Undo; Save, Save As and Discard;
navigation with dirty text; external-change warning; read-only file; dirty
close/quit Cancel; sidebar resize/collapse; light/dark/floating look; and the
accepted Trash/DnD regressions.

Specifically not exercised by the agent: visual gutter alignment on wrapped
lines, actual Alt+click delivery, Save/Save As dialogs, Replace/changed-on-disk
dialogs, dirty close/quit, monitor events, image texture display, media
thumbnail display, live scale/theme/font changes and floating layout.

## Changes outside this brief

**[observed]**
No file outside the brief's Output was edited. The look module's `Look` type
changed from `Rc<RefCell<Look>>` to `Rc<Look>` inside `fm-ui` so the monospace
GSettings watcher can regenerate the shared stylesheet without a second CSS
source; this is shared look support required by the preview.

## Review and live check (ChatGPT, Nov 3, 2026): passes in part

**[observed]** Reviewer ran `scripts/check.sh` with the pinned toolchain.
The sandbox run passed fmt, clippy, tests and release, then failed to resolve
github.com for RustSec. The network-enabled rerun passed all five gates:
**464 passed, 17 ignored** (core 250, UI 189, CLI 7, agent 1, isolated Trash
parents 6 + 11). Final output: `advisories ok, bans ok, licenses ok, sources ok`
and `all checks passed`. Logs: `/tmp/gimbal-p2m-review-check.log` and
`/tmp/gimbal-p2m-review-check-network.log`.

**[read]** Caller restrictions, encoding/size bounds, off-thread I/O and dirty
selection pinning are present. No new manifest/lockfile/dependency changes.
The existing unrelated P2l-r3 notes and Zen capture directories remain separate.

**Live: [observed]** No reviewer app launch or desktop interaction. No real
files/Trash/clipboard were changed. Kyle's P2m acceptance checklist is pending
the fixes below; agent smoke observations above do not establish these paths.

**Still open:**

- **[read; observed API probe] P1, expired monitor source.** In UI preview.rs
  schedule_monitor_check's one-shot callback never clears monitor_debounce.
  The next scheduling/clear calls SourceId::remove on an expired source. The
  normal clean-change path does this through apply_checked -> begin_load_display
  -> clear_monitors. glib-0.22.10/src/source.rs:29–35 unwraps the failed removal.
  A scratch Rust probe using the pinned library confirmed the panic after expiry.
- **[observed core probe] P2, stale clean edit pin after external reload.**
  begin_edit without changes -> note_external_change -> force_load/load_finished
  leaves pinned pointing at the old document. The probe loaded `new contents`
  while current_document still returned `old contents`. UI displays the new
  load but later editing/saving retains the old revision. Failed reloads can
  likewise retain an obsolete editor. Drop/reconcile the clean pin on reload.
- **[read] P1, Save As is not bound to its initiating editor.** destination_id
  is invalidated only by another save_as, not Discard/Reload/new editor. A late
  Destination result calls start_save_to against whichever buffer is now pinned.
  Chooser/Replace responses lack a document/revision token too. Bind and validate
  the whole request; stale replies must perform no write. Chooser lifetime also
  needs the existing dialog guard and duplicate/in-flight request protection.
- **[read] P1, close continuation can survive abandonment or bypass saving.**
  Close -> Save -> conflict -> Reload leaves after_save armed; a later unrelated
  save can close the window/resume quit. guard_close's Discard branch also always
  proceeds even when discard refused because a save is running. Bind the close
  request to a document/revision/save job, invalidate abandoned requests and wait
  for active save results. Recheck before an idle continuation closes the window.
- **[read] P1, Quit snapshots only initially dirty windows.** request_quit and
  advance_quit do not rescan before quit_now. Another model window can be dirtied
  or created while a window-modal prompt is open, or an already handled window
  edited again, and those edits are lost without a prompt. Validate all current
  windows/revisions before final quit and scope one active quit request.
- **[read] P2, gutter uses first-page coordinates after scrolling.**
  draw_gutter queries iter_at_location at y=0 and height-1 in buffer coordinates,
  then subtracts scroll while drawing that fixed range. GTK 4.22.4 source
  gtktextview.c:2531–2543 explicitly requires buffer coordinates. Use the visible
  buffer rectangle and correct conversion for scrolled/partially visible lines.

**[observed]** Independent probes were compiled from owned
`/tmp/gimbal-p2m-review-ekqimat0/probe.rs` against existing built core/glib
libraries. Their output is in that directory's `result.log`; no project test
or production source was modified. Timer removal panic was caught in the probe,
not triggered inside the application. Gutter and asynchronous UI findings are
source-verified, not claimed as observed desktop behavior.

**[observed]** Wrote `P2m-r1-preview-lifecycle.md` in the prompts repository,
updated the Phase 2 index and handoff log. No reviewer production fix, commit or
push. Review remains open until the revision and Kyle's live acceptance run.
