# P2m-r3: keyboard previews, readable visuals and file details

Tags: **[observed]** the working tree, test output and smoke log; **[read]** pinned crate/app source or installed CLI output; **[inferred]** behaviour that still needs Kyle's live display/interaction.

## What was built

**[observed] Keyboard selection uses the same model and preview as a pointer.**
`FolderView` keeps a separate `cursor` for the keyboard and a capture-phase
`gtk::EventControllerKey` on the grid. Arrow/Home/End/Page moves go through
`folder::keyboard_request`, which returns the same `ClickRequest`
(`Plain`/`Ctrl`/`Shift`) a pointer click uses, then calls the existing
`Process::file_click_tab`. That focuses the real tile/tab, updates
`Workspace` selection/status, refreshes the header and content, and finally
`PreviewPanel::set_selection_user` loads the selected path. `item.set_selectable(false)`
and the custom cell click remain, so GTK's old duplicate-click selection bug
does not return. Enter calls `GridView` activation through the existing
`open_position`. Empty listings return `None`; unrealised positions are loaded
from the model; keys are owned by whichever widget has focus, so the preview
`TextView`, path/name entries and modal dialogs never move file selection.

**[observed] Image and PDF previews render at device pixels.**
`fm-ui/src/image.rs` decodes with `gdk-pixbuf`'s `size-prepared` target, applies
EXIF orientation, and returns a bounded RGBA buffer. The preview computes a
logical display size from Fit/FitWidth/Actual/Manual zoom and the current
viewport, then asks for `display × monitor scale` pixels. It uses
`fm_core::preview::render_plan` to cap at `MAX_RENDER_EDGE` (8192) and
`MAX_RENDER_PIXELS` (16 MP, 64 MiB RGBA). A reduced request is reported in the
zoom label rather than stretched. Resize uses a 120 ms debounce; a new render
generation makes stale image results no-ops. The image page has local Fit,
100% and +/- buttons and an automatic scroller for panning. Images start at
Fit; PDFs start at fit-width.

**[observed] PDF page 1 uses installed Poppler.** `PreviewContent::Pdf` stores
the PDF and its valid cached-thumbnail candidates. `fm-ui/src/pdf.rs` creates an
exclusive `/tmp/gimbal-preview-pdf-<pid>-<sequence>` directory (mode 0700), runs
`pdftoppm` through `std::process::Command` with individual `OsString` arguments
(`-f 1 -l 1 -singlefile -png -scale-to-x N`), reads only `page.png`, checks the
IHDR dimensions and byte bound before decoding, and removes its own directory.
A 10 s child deadline, a cancellation flag and only that child's `kill`/`wait`
are used. Two renderer workers are global; the queue is bounded. The preview
labels success `Page 1`; a missing helper, corrupt/encrypted PDF or unreadable
cache falls back to a valid cached thumbnail with the explicit
`Cached thumbnail, not a fresh page render.` label. PDF input is bounded to
64 MiB. No password prompt, default document application, shell, download or
new Rust dependency was added.

**[observed] Main-grid thumbnails.** `fm-ui/src/thumbs.rs` has a 64 MiB
thread-local LRU texture cache and a two-worker, 64-job bounded queue. Bind only
requests realised cells; unbind cancels that binding. Keys contain path,
device/inode/size/mtime/ETag revision, requested device-pixel box and
`ThumbKind` (image/PDF/video). A result is applied only when its token is still
bound and the current `FileInfo` name and revision match the binding, so a
recycled cell cannot display A under B. Folders and unsupported files keep the
existing icon; videos use only a valid cached thumbnail. Image/PDF/video decode
and `pdftoppm` run off the main thread; only `gdk::MemoryTexture` creation and
widget changes happen there. Selected-image preview has its own worker, so grid
work does not delay it. Cache eviction, corrupt/missing candidate fallthrough
and revision-key tests are in the module.

**[observed] Grid names, hover details and selected status.** Cells now set
three default wrapped lines with End ellipsis only when still clipped, no
character-count cap, and a selected cell gets unlimited lines with no
ellipsis. The whole cell/label has plain-text `details_text` (name, type,
Folder/actual size, modified or Unavailable). CSS adds a theme-alpha hover fill
distinct from the accent selected fill and the focus-ring drop target. The
status line is now two labels: an ellipsized primary name/selection and a
non-expanding suffix that keeps the real byte size and file/folder counts
visible. One folder says `Folder`; multiple selection sums known regular-file
bytes and reports unknown sizes and unsized folders instead of pretending the
total is complete. `StatusCache` caches selection updates while an operation
summary is held and restores them when the summary ends.

**[observed] Pinned single-click.** `pin_release` now opens on the first
ordinary release; the second release of a double-click only selects, and a
per-slot 450 ms guard prevents a rebuilt row list from producing a second
navigation. A drag must pass GTK's threshold and claim the sequence; a
same-slot drag return is `Select` and never opens. Right-click/unpin, row
selection, drop-to-folder and empty-list pinning are unchanged.

**[observed] Preview header wrapping and compact mode.** The title label wraps
at word/character boundaries with no line limit by default, so the filename,
size and dirty suffix stay readable without widening the sidebar. A double-click
only on that label toggles a per-panel `compact_title` that sets one line and
End ellipsis; double-click again expands it. Single click does nothing. Size,
dirty suffix, tooltip and accessible label are updated in both forms.

**[observed] Save As owns the new preview.** `PreviewSession` now tracks a
`saved_override` and the last passive model selection. A successful new-path
save stores the new path/revision and shows its clean document. `frame.update_content`
uses `apply_model_selection`, so a passive listing/monitor/status reapply of
the old browser selection does not replace it. An explicit pointer/keyboard
selection, `selection.all/none`, navigation, tab focus or Trash uses
`select_user`/`release_saved_override`; even re-selecting the old
already-selected item force-loads it. A dirty or stale save completion cannot
create the override, and another window's session is independent.

**[observed] Dirty-pin explanation is persistent and prominent.** A dedicated
`gl-preview-notice` label uses the shared larger text token, warning-tone
background and a two-pulse opacity animation through `look::motion`; the
animation is disabled cleanly when GTK's `gtk-enable-animations` is off. It
names the edited file, names the current candidate separately, explains
Save/Discard and stays until save/discard or candidate match. It is separate
from the expiring status line and from external-change/save-error text.
Repeated attempts replace the single animation.

## API citations

- **[read]** `gtk4-0.11.5:src/event_controller_key.rs:11` —
  `connect_key_pressed` returns `glib::Propagation`; the capture phase is set
  through `gtk4-0.11.5:src/auto/event_controller.rs:134`.
- **[read]** `gtk4-0.11.5:src/auto/grid_view.rs:89` (`max_columns`) and
  `:129` (`scroll_to`). `scroll_to` is behind feature `v4_12`, which this
  workspace already enables.
- **[read]** `gtk4-0.11.5:src/auto/settings.rs:226` —
  `is_gtk_enable_animations()` used to choose the static notice.
- **[read]** `libadwaita-0.9.2:src/auto/timed_animation.rs:25` —
  `TimedAnimation::new(widget, from, to, duration, target)`.
- **[read]** `gdk-pixbuf-0.22.0:src/auto/pixbuf.rs:248-252` —
  `apply_embedded_orientation()`.
- **[read]** `gio-0.22.10:src/auto/file_info.rs:144` (`attribute_string`),
  `:173` (`attribute_uint64`), `:256` (`modification_date_time`) and `:272`
  (`size`). GIO's C type table in gio-sys says `unix::device` is uint32 and
  `unix::inode` is uint64; the grid uses the matching accessors.
- **[read]** `gio-sys-0.22.9:src/lib.rs:467,472` gives
  `thumbnail::is-valid-xxlarge` and `thumbnail::path-xxlarge`; the full
  normal/large/xlarge/xxlarge set is queried.
- **[read]** Installed `/usr/bin/pdftoppm -h` (version 26.01.0) documents
  `-f`, `-l`, `-singlefile`, `-png`, `-scale-to-x`, `-scale-to-y` and crop
  options. Only page-1/first/last/singlefile/PNG/scale-to-x are used.
- **[read]** GTK 4.22 `gtkimage.c:512-516`: a paintable is still subject to the
  image's icon size, so grid textures can be rendered larger than their 80 px
  logical icon slot for sharpness.
- **[read]** `gdk-pixbuf`/GIO thumbnail cache files are validated as regular
  files before use; the existing `FileRevision` (device/inode plus size, mtime,
  ctime, ETag) is the cache identity.

## Production adapters tested

**[observed]** Tests drive production code rather than a parallel model:

- `folder::keyboard_request` -> `Workspace::file_click`/`file_shift_click` ->
  `Selection::from_names` -> `PreviewSession::select`/`load_finished` in
  `the_keyboard_adapter_drives_the_model_and_preview_together`.
- `PreviewSession::apply_model_selection`, `select_user`,
  `release_saved_override` and `save_finished` in the new Save As ownership
  tests, including the passive old-selection reapply, explicit old selection,
  editing/saving the new file, Trash release and stale completion.
- `SelectionStatus`/`StatusCache`, label mode, binding revision/name validation,
  thumbnail cache eviction and corrupt/missing cache fallthrough.
- `pdf::render_for_test`, `wait_bounded_for`, `pdf_output_allowed`,
  `png_dimensions`, `PdfRenderRequest::arguments`, `PdfFailure` fallback.
- `image::decode_image_file` with generated high-detail PNG output and source
  byte preservation.
- `sidebar::pin_release` click/double-click/threshold drag/reorder/same-slot.

## Gates

**[observed]** Final `./scripts/check.sh` output (pinned 1.95.0 toolchain, from
the repository root; log `/tmp/gimbal-p2m-r3-check.log`):

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
test result: ok. 1 passed; 0 failed; 0 ignored
test result: ok. 7 passed; 0 failed; 0 ignored
test result: ok. 275 passed; 0 failed; 0 ignored
test result: ok. 6 passed; 0 failed; 6 ignored
test result: ok. 11 passed; 0 failed; 11 ignored
test result: ok. 231 passed; 0 failed; 0 ignored
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Total: **531 passed, 17 ignored** (baseline 506/17; 25 new tests). Counts by
target: agent 1, CLI 7, core 275, isolated Trash parents 6 + 11, UI 231.

## Smoke run

**[observed]** `pgrep -a gimbal-files` was empty. The release binary ran for
four seconds with `XDG_CONFIG_HOME=/tmp/gimbal-preview-r3-smoke2/config`,
`XDG_STATE_HOME=/tmp/gimbal-preview-r3-smoke2/state`, `GIMBAL_LOG=1` and an
owned folder containing a generated one-pixel PNG plus a generated one-page
PDF. The listing completed, both `thumb: request Image 80x80` and
`thumb: request Pdf 80x80` appeared, and the log had no panic, GLib critical or
preview error. It was stopped with SIGTERM on its own PID. The environment's
existing accessibility-bus warning appeared; no desktop input, clipboard,
`file.open`, disk-usage or real Trash operation was used. The earlier smoke
also exposed and led to fixing a GIO type misuse (`unix::device` is uint32,
not uint64).

## Save As refresh cause and ownership trace

**[observed]** `apply_saved` used to update `PreviewSession` to the new
document, but `frame.update_content` still calls `PreviewPanel` with the
browser's current `Selection::from_names` on every passive content refresh.
That reapplication was indistinguishable from a user selection by path alone.
The new `apply_model_selection` path compares against the last passive model
selection and keeps the saved override for an identical reapply; explicit
clicks/keyboard/`selection.all/none`/navigation/focus call the user path and
release it. The new-file save still leaves the browser's own selected folder
and old item untouched.

## Not verified live

**[inferred]** No GTK pointer/keyboard injection was performed. These need
Kyle's live run:

- arrow/Home/End/Page/Shift/Ctrl and Enter in single and split grids, while
  the text editor/path search/dialog has focus;
- high-detail PNG/JPEG/PDF readability at Fit, fit-width, 100% and repeated
  zoom, including panning and the rendering-limit label;
- PDF helper success, encrypted/corrupt fallback and a missing-helper build;
- visible image/PDF/video grid thumbnails, hover highlight/tooltip, long
  selected/Unicode names and recycled cells;
- pinned single-click vs double-click vs reorder/drop/cancel;
- selected filename/size status at narrow widths and held operation summaries;
- wrapped header and double-click compact mode across file changes;
- Save As new-name, actual listing refresh and explicit old-file reselect;
- dirty-notice pulses and reduced-motion/static display.

## Remaining limits / outside this brief

- **[read]** Poppler's internal memory use during a render is not hard-capped;
  only input bytes, output bytes/dimensions and the 10 s child deadline are.
- **[inferred]** The 450 ms pinned-click guard is a heuristic against a rebuilt
  row list; a very slow double-click beyond it still starts a second
  navigation.
- **[read]** PDF rendering uses only the installed `pdftoppm`; `pdftocairo` and
  Poppler GLib are not used. No page navigation, video decode, hover popup,
  recursive folder size, new command/dependency/manifest, session schema,
  runner or spike change was made.
- **[read]** Existing accepted text gutter, dirty navigation, Save/Replace,
  close/Quit and Trash/DnD tests remain untouched and pass.

## Review and live check (ChatGPT, Nov 3, 2026): passes in part

**[observed]** Reviewer ran the pinned `scripts/check.sh`. In the sandbox,
fmt/clippy/core tests passed but UI tests ended 228 passed/3 failed: image
fixture writing, thumbnail decode and PDF decode encountered gly-loader-error
`D-Bus error: I/O error: Operation not permitted`. The host-enabled rerun passed
all five gates: **531 passed, 17 ignored** (core 275, UI 231, CLI 7, agent 1,
isolated Trash parents 6 + 11), release and deny included. Final output:
`advisories ok, bans ok, licenses ok, sources ok` and `all checks passed`.
Logs: `/tmp/gimbal-p2m-r3-review-check.log` and
`/tmp/gimbal-p2m-r3-review-check-host.log`. `git diff --check` passed.

**[read/observed tests]** r3 implements keyboard selection routing, bounded
render plans, optional first-page Poppler, worker-backed grid thumbnails,
three-line/full-selected labels, details tooltips/hover style, selected status,
pinned single-click, wrapping header/compact toggle and persistent pulsing
notice. New tests exercise source preservation, adapter transitions and cache
limits. There are no new dependencies/manifests/commands/config/session formats.
The earlier specifically accepted behavior still needs regression checks live.

**Live: [observed]** No reviewer application launch, desktop interaction,
user-data/real-Trash/clipboard operation. Reviewer probes used exclusively owned
`/tmp/gimbal-preview-r3-review-qagafv2l`, existing built core library and installed
pdftoppm on a generated tiny PDF. `probe.rs`, `result.log`, generated input/output
and `pdf-result.log` remain there. No production source was changed.

**Still open:**

- **[observed core save/reconcile probe] P2, Save As ownership ends on the next
  normal Save.** Real scratch A -> Save As B -> passive A reapply shows B. Edit
  B and Save: B is correctly written, but save_finished clears saved_override
  for a non-new-path ticket, and reconcile immediately requests A. Keep ownership
  through ordinary saves of B until genuine user selection/navigation releases it.
- **[read] P2, repeated keyboard range moves reset to the first selected item.**
  key_pressed sets cursor before file_click_tab; its synchronous refresh reaches
  apply_selection, which assigns cursor to the first selected name's position.
  Shift+Right from A to B resets to A, so another Shift+Right targets B again.
  Ctrl movement also produces Ctrl-click toggles instead of focus-only movement.
  Tests drive one key/model step but omit this production cursor reapplication.
- **[observed generated-PDF CLI probe] P1, requested PDF height is ignored
  before rasterization.** PdfRenderRequest argv passes only -scale-to-x. A
  requested 400x400 render of an owned generated tall-page PDF produced a
  **400x14063** PNG, beyond MAX_RENDER_EDGE=8192. Current PNG guards reject only
  after allocation/render/write. Bound both axes/pixels with preserved aspect
  before rendering; keep the post-render checks. This concerns output raster
  bounds, separately from the acknowledged lack of a hard internal-memory cap.
- **[read] P2, thumbnail completion counting and queue pressure can strand
  visible thumbnails.** Unbinding/cancel decrements thumb_pending; a previously
  queued stale result decrements it again before checking its binding. It can
  consume a new B request's pending count and stop polling before B completes.
  Separately, a full thumbnail queue drops demand with no retry while the cell
  remains bound. Token-owned accounting and bounded deferred visible demand
  need scheduler regressions (not only cache tests).
- **[read] P2, obsolete render work remains unbounded and incompletely owned.**
  Preview Load/RenderImage/Save share one unbounded mpsc worker; generation
  checks occur after image decode. Obsolete work can accumulate ahead of a
  current text load/save. Grid and selected PDFs share a FIFO pool without
  selected priority/recovery. Thumbnailer has no Drop cancellation, and process
  shutdown does not wait for renderer workers to kill/reap their children.
  Bound/coalesce work, prioritize the selected page and finish own-child cleanup.

**[read]** Other keyboard/thumbnail/render observations require Kyle's actual
desktop run after these fixes; source findings above are not claimed as observed
GTK interactions. No broader visual acceptance is inferred from the agent smoke.

**[observed]** Wrote `P2m-r4-preview-state-and-render-work.md`; updated index
and handoff log. No reviewer production fix, commit or push. Existing unrelated
P2l-r3 notes and Zen captures remain untouched. P2m remains open for revision.
