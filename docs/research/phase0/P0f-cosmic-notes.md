# P0f: COSMIC Files drag-out notes
Pinned: cosmic-files 09bdd825…, libcosmic 430f3d48…, smithay-clipboard 859b02c8…, winit 9567503d…, sctk 0.20.0, wayland-backend 0.3.17

The call path also crosses `window_clipboard` f68595e (same commit as its `clipboard_wayland` 0.2.2 crate), which is not in the brief's pin list. Licences: cosmic-files GPL-3.0-only; libcosmic (including its vendored iced crates) MPL-2.0 per its `LICENSE`; smithay-clipboard and window_clipboard MIT.

## Answers (one line each, Q1–Q7, each with its tag and path:line)

**Q1 [read]** The file view is wrapped once in `DndSource<Message, ClipboardCopy>` (`cosmic-files:src/tab.rs:4153`) and fed selected paths by the `drag_content` closure (`cosmic-files:src/tab.rs:4160-4162`); the actual start is libcosmic's 8-px threshold handler (`libcosmic:src/widget/dnd_source.rs:50,251-271`).

**Q2 [read]** Five types, in this order: `text/plain`, `text/plain;charset=utf-8`, `UTF8_STRING`, `text/uri-list`, `x-special/gnome-copied-files` (`cosmic-files:src/clipboard.rs:27-33`); the drop side accepts only the last two (`cosmic-files:src/clipboard.rs:113-120`).

**Q3 [read]** `Copy | Move` is libcosmic's default (`libcosmic:src/widget/dnd_source.rs:46,61`), cosmic-files never overrides it (`cosmic-files:src/tab.rs:4153-4162`), and sctk sends `set_actions` when the manager is v3 (`sctk:src/data_device_manager/mod.rs:86-88`).

**Q4 [read]** `cosmic-files` → `libcosmic::widget::DndSource` → `iced_core::clipboard::start_dnd` → `iced_winit` (cosmic-files enables libcosmic's `winit` feature, not a sctk shell, `cosmic-files:Cargo.toml:85`) → `window_clipboard` → `clipboard_wayland` → `smithay-clipboard` → sctk `wl_data_source.offer`/`set_actions`/`start_drag`; the payload is served on `send`.

**Q5 [read]** cosmic-files has no portal code at all (`cosmic-files:src/clipboard.rs:27-33`); libcosmic has a `file_transfer_send` helper (`libcosmic:src/command.rs:51-62`) and `FILE_TRANSFER_MIME` (`libcosmic:src/widget/dnd_destination.rs:33`), but only on the drop-receive side and behind the `xdg-portal` feature cosmic-files does not enable; smithay-clipboard has none.

**Q6 [read]** #830 was closed as a duplicate of cosmic-epoch#1175; #3926 is open with no comments; the portal fix is tracked as cosmic-files#1422 with open WIP PR #1586.

**Q7 [inferred]** **Small**: one crate, one file (`cosmic-files:src/clipboard.rs`), roughly 40–60 new lines for the minimal version, no toolkit or Wayland-protocol change; the upstream WIP patch is +137/−77 in that one file.

## Drag-out call path (numbered: crate:path:line and what happens at each step)

1. **cosmic-files:src/tab.rs:4153** — `view_responsive` wraps the whole file view (grid or list) in `DndSource::<Message, ClipboardCopy>::with_id(..., "tab-view")`. This is the only drag source for file views.
2. **cosmic-files:src/tab.rs:4157-4162** — only when a selected item was clicked, `.drag_content(...)` installs a closure that builds `ClipboardCopy::new(ClipboardKind::Copy, &files)` from the selected paths. `Copy` is hard-coded for drag-out.
3. **cosmic-files:src/tab.rs:5097-5117** — the `MouseArea::on_drag` nearby is a different feature: it publishes `Message::Drag(Some(rect))` for rubber-band selection, not DnD. Same for `Message::DragEnd`, which only clears `clicked` (`cosmic-files:src/tab.rs:1275-1278`).
4. **libcosmic:src/widget/dnd_source.rs:251-271** — on `CursorMoved` past `drag_threshold` (8.0 px, line 50) it calls `self.start_dnd(...)` with the cached bounds and cursor offset.
5. **libcosmic:src/widget/dnd_source.rs:99-125** — `start_dnd` evaluates the `drag_content` closure, chooses `DndSource::Widget(id)` (lines 107-111), builds the drag-icon surface, and forwards `Box<dyn AsMimeTypes>` plus `self.action` to `iced_core::clipboard::start_dnd` (line 104).
6. **libcosmic:iced/core/src/clipboard.rs:138-153** — thin forwarding shim: `clipboard.start_dnd(internal, source_surface, icon, content, actions)`.
7. **libcosmic:iced/winit/src/clipboard.rs:214-241** — the winit backend queues a `StartDnd` record and wakes the event loop through `Control::StartDnd` (line 233). It does not touch Wayland yet.
8. **libcosmic:iced/winit/src/lib.rs:608-611,1678-1695** — the event loop turns `Control::StartDnd` into `Event::StartDnd` and drains `clipboard.get_queued()`. `DndSource::Widget` is resolved to the containing window (`libcosmic:iced/winit/src/lib.rs:1695-1730`).
9. **libcosmic:iced/winit/src/clipboard.rs:309-327** — `start_dnd_winit` passes the window's raw surface and the content to `window_clipboard::Clipboard::start_dnd`.
10. **window_clipboard:src/dnd/mod.rs:16-29** — provider trait; **window_clipboard:src/platform/linux.rs:143-165,213-221** picks the Wayland backend at runtime from the raw display handle.
11. **window_clipboard:wayland/src/lib.rs:211-230** — converts the surface/icon/actions to smithay types and calls `smithay_clipboard::Clipboard::start_dnd`; `window_clipboard:dnd/src/platform/linux.rs:32-45` maps every MIME string into `MimeType` without filtering or reordering (the three text aliases round-trip through an enum but `as_ref()` restores the exact original string).
12. **smithay-clipboard:src/dnd/state.rs:307-380** — locks the latest seat, takes its serial, reads `content.available()` (line 363), calls `create_drag_and_drop_source(queue, mime_types, actions)` (lines 368-374), then `source.start_drag(data_device, surface, icon, serial)` (line 375). It stores the content and source actions at lines 378-379.
13. **sctk:src/data_device_manager/mod.rs:81-88** — `create_data_source` calls `wl_data_source.offer(mime)` for each MIME **in iterator order**, then `set_actions(dnd_actions)` only if `wl_data_device_manager.version() >= 3`. **sctk:src/data_device_manager/data_source.rs:142-150** calls `wl_data_device.start_drag`.
14. **sctk:src/data_device_manager/data_source.rs:88-90** — a target's `wl_data_source.send(mime_type, fd)` becomes `DataSourceHandler::send_request(..., mime, WritePipe)`.
15. **smithay-clipboard:src/state.rs:466-483** — if the source is the active DnD source, it calls `send_dnd_request(write_pipe, mime)` (line 481) instead of the clipboard path.
16. **smithay-clipboard:src/dnd/state.rs:481-527** — `send_dnd_request` looks the requested MIME up in `content.available()` (line 485), takes `content.as_bytes()` (line 498), marks the pipe non-blocking, and registers an event-loop writer (line 504) that writes until `written + n == contents.len()` (lines 507-511). On completion it returns `PostAction::Remove`, which drops the closure and therefore closes the fd **after** the full payload. This is not an early-close bug; P0b saw the complete URI arrive.

No hop drops, renames or reorders a MIME type. The only missing thing is what was never put into `ClipboardCopy::available()`.

## MIME types advertised

| MIME | Added in (crate) | path:line | Payload format |
|---|---|---|---|
| `text/plain` | cosmic-files | `src/clipboard.rs:27` | Raw UTF-8 path(s), CRLF between entries, no trailing CRLF (`:46-52`) |
| `text/plain;charset=utf-8` | cosmic-files | `src/clipboard.rs:28` | Same bytes as `text/plain` (`:97-99`) |
| `UTF8_STRING` | cosmic-files | `src/clipboard.rs:29` | Same bytes as `text/plain` (`:97-99`) |
| `text/uri-list` | cosmic-files | `src/clipboard.rs:31` | `Url::from_file_path(path).as_ref()` (percent-encoded `file://` URI), followed by CRLF **including a trailing CRLF** (`:63-68`) |
| `x-special/gnome-copied-files` | cosmic-files | `src/clipboard.rs:32` | `"copy"` or `"cut"`, then `\n` + URI per file, LF only, no trailing LF (`:36-40,70-71`) |
| `application/vnd.portal.filetransfer` | **not offered** | `src/clipboard.rs:27-33` is where it would go | — |
| `application/vnd.portal.files` | **not offered** | `src/clipboard.rs:27-33` is where it would go | — |

Excerpt (cosmic-files, GPL-3.0-only, `src/clipboard.rs:27-33`):

```rust
let available = vec![
    "text/plain".to_string(),
    "text/plain;charset=utf-8".to_string(),
    "UTF8_STRING".to_string(),
    "text/uri-list".to_string(),
    "x-special/gnome-copied-files".to_string(),
];
```

The matching drop side, `ClipboardPaste`, accepts only `x-special/gnome-copied-files` and `text/uri-list` (`cosmic-files:src/clipboard.rs:113-120`) and parses them with `TryFrom` (`cosmic-files:src/clipboard.rs:122-166`). That is for drops *into* cosmic-files; it is not the bug and it also doesn't accept the portal types.

## Actions

- libcosmic's `DndSource::new`/`with_id` default to `DndAction::Copy | DndAction::Move` (`libcosmic:src/widget/dnd_source.rs:46,61`). cosmic-files never calls `.action(...)`, so that is what it sends (`cosmic-files:src/tab.rs:4153-4162`).
- sctk offers both and calls `wl_data_source.set_actions` when the manager is v3 (`sctk:src/data_device_manager/mod.rs:86-88`). Ask is not requested.
- P0b observed the target selecting `move` (action 2) in all 8 rows, including COSMIC and Thunar. That is action negotiation working, not a cause of the failure.
- Drag-out content is always `ClipboardKind::Copy` (`cosmic-files:src/tab.rs:4161`), and `Message::DragEnd` only clears UI state (`cosmic-files:src/tab.rs:1275-1278`); cosmic-files deletes nothing on an outgoing move. Its `Move`→`Cut` mapping exists only for drops *into* cosmic-files (`cosmic-files:src/tab.rs:4307-4316`, `cosmic-files:src/app.rs:4880`).

## Portal

- **cosmic-files: absent.** A whole-tree search for `vnd.portal`, `FileTransfer`, `StartTransfer` and `ashpd` (Rust and Cargo files) returns zero hits. The drag list at `cosmic-files:src/clipboard.rs:27-33` is the complete offer set. There is also no `xdg-portal` feature in the libcosmic dependency (`cosmic-files:Cargo.toml:79-87`).
- **libcosmic: receiver only.** `file_transfer_send` exists (`libcosmic:src/command.rs:51-62`), but it is `#[cfg(xdg_portal)]` (`libcosmic:src/command.rs:50`) and nothing in libcosmic calls it. `FILE_TRANSFER_MIME` (`libcosmic:src/widget/dnd_destination.rs:33`) is inserted only into a *destination's* accepted list by `on_file_transfer` (`libcosmic:src/widget/dnd_destination.rs:242-249`) and consumed only when a drop arrives with that MIME (`libcosmic:src/widget/dnd_destination.rs:517-524`), at which point the app calls `file_transfer_receive` (`libcosmic:src/command.rs:67-72`). Feature switch: `libcosmic:Cargo.toml:108`.
- **smithay-clipboard / window_clipboard: absent.** No `vnd.portal`, `FileTransfer`, `StartTransfer`, `ashpd` or other portal reference in either tree.
- **Capture [observed].** P0b: COSMIC offered exactly the five types above; Zen read only `text/uri-list` (plus `text/plain`) and there were **no transfer calls**. Dolphin and Nautilus offered a portal MIME, called `StartTransfer`/`AddFiles`, and Zen's `RetrieveFiles` returned `/run/user/1000/doc/7fb4b484/sample.png`; their drags succeeded outside `~/Downloads`. Source and capture agree exactly.
- The upstream intent is already written down in cosmic-files#1422 ("advertise the relevant mime type, then … ashpd File Transfer") and built in WIP PR #1586.

## Upstream status

- **cosmic-files#830** (opened 2025-02-26, closed same day): drag-out to other applications fails; the reporter found no iced/libcosmic DnD bindings and offered to implement it. Closed by a member as a duplicate of cosmic-epoch#1175.
- **cosmic-epoch#3926** (opened 2026-10-05, open): drag from COSMIC Files to a browser does not drop and does not raise the window; no comments, no linked PR, no maintainer diagnosis.
- **cosmic-epoch#1175** (the duplicate target): long thread, mostly Xwayland/Electron cases; a member says DnD was implemented (cosmic-comp#1797). One comment gives the exact sandbox symptom: drag yields `/home/me/testdata/teapot.obj` while "Open with" yields `/run/user/1000/doc/...`.
- **cosmic-files#1422** (open): "Support File Transfer portal for drag-and-drop and copy-paste to sandboxed Flatpaks" — names the MIME types and the ashpd `FileTransfer` calls, and notes Chromium lacks portal support.
- **cosmic-files#1586** (open since 2026-01-30): "WIP File transfer portal", +137/−77 in `src/clipboard.rs`, one added dependency in `Cargo.toml`, plus Cargo.lock; adds both portal MIMEs and starts a read-only, autostop transfer then adds the files.
- A 2026-09-18 comment on #1586 reports a build at `61e5e1e` working into Flatpak Zen with only `filesystems=xdg-download`; `RetrieveFiles` returned `/run/user/1000/doc/...`. It also warns to advertise the portal types only when the portal is reachable, or a receiver can pick them and get `None` instead of the working URI.
- No PR is linked from #830 or #3926; #1586 is linked to #1422.
- #1175 contains the sandbox symptom but no portal diagnosis; #1422 names the portal cure. No maintainer has linked the two in #3926.

## Verdict: small / deep (with reasons, the proposed fix location, and what would confirm it)

**[inferred] Small.** The smallest correct fix is entirely inside **cosmic-files**, in `cosmic-files:src/clipboard.rs`: add `application/vnd.portal.filetransfer` and `application/vnd.portal.files` to `available()` (`:27-33`), and add an `as_bytes()` arm (`:95-104`) that opens each path as a file, starts a read-only/autostop `ashpd` `FileTransfer` transfer, adds the files, and returns the key bytes. Add `ashpd` to `cosmic-files/Cargo.toml` (0.12.3 is already in `Cargo.lock` via libcosmic; #1586 bumps to 0.13 and enables its `documents` feature). Minimal version: roughly 40–60 new lines in one file; the existing upstream WIP #1586 is +137/−77 there, much of it an unrelated refactor to lazy payload construction.

Why not deep:

- No toolkit or protocol work is needed. `window_clipboard` and `smithay-clipboard` already pass `content.available()` through verbatim and in order (`window_clipboard:dnd/src/platform/linux.rs:32-45`, `smithay-clipboard:src/dnd/state.rs:363-375`), and sctk already offers every string it is given (`sctk:src/data_device_manager/mod.rs:81-84`).
- The portal call is **not** on the drag-start path. `as_bytes()` is evaluated lazily when the receiver asks for that MIME, in `send_dnd_request` (`smithay-clipboard:src/dnd/state.rs:481-498`), so a blocking D-Bus call there does not need a new async path at drag start. #1586 does exactly this with `futures::executor::block_on`.
- It is one crate and one file; libcosmic's unused `file_transfer_send` even shows the intended API shape (`libcosmic:src/command.rs:51-62`), though cosmic-files needs a direct dependency because that helper is feature-gated off.

The one real caveat is the guard: only advertise the two portal types when the portal is reachable, otherwise a receiver that prefers them can get `None` and fall back to nothing instead of `text/uri-list` (the #1586 review comment). That is a few more lines in the same function, still one crate.

What would confirm it (P0b already supplies most of it):

- P0b already shows the decisive negative: COSMIC offered **only** the five non-portal types, Zen read only `text/uri-list`/`text/plain`, and no `org.freedesktop.portal.FileTransfer` calls occurred, while the portal-offering sources succeeded outside `~/Downloads` (`docs/research/phase0/P0b-results.md:11-45`).
- To confirm the fix, a patched build (or #1586) should offer both portal types alongside the existing five (either before or after `text/uri-list`; #1586 appends them), log `StartTransfer`+`AddFiles`, and make the same Flatpak Zen load `sample.png` from `~/gimbal-dnd-test`; Zen's `RetrieveFiles` should return a `/run/user/1000/doc/...` path.
- Negative control: with the portal types offered but the portal stopped/unavailable, the drag must still fall back to `text/uri-list` rather than fail.

## Surprises

- COSMIC's offered list matches the capture exactly, in the same order. None of libcosmic, window_clipboard, smithay-clipboard or sctk filters, renames or reorders anything; the whole bug is two absent strings plus the missing registration call.
- libcosmic already ships half of the feature (`file_transfer_send`, `FILE_TRANSFER_MIME`), but only for *receiving* drops and behind a feature cosmic-files doesn't enable.
- The open upstream fix (#1586) predates #3926 by about eight months and is linked to a different issue (#1422). #830, the issue named in the brief, was closed as a duplicate of a broad DnD thread and never diagnosed as a sandbox/portal problem.
- `move` was negotiated in every P0b row, including sources that never touch the portal. Action negotiation therefore looks healthy and tells you nothing about whether the payload will be readable.
- Thunar has the identical failure (plain `text/uri-list`, no portal), so this is a source-side pattern, not a COSMIC compositor bug.
- The `send` implementation writes asynchronously and closes the fd only after the final byte (`smithay-clipboard:src/dnd/state.rs:504-514`); the "fd closed too early" hypothesis is ruled out both by code and by Zen receiving the full URI.

## Not found / unsure (what you searched for)

- Searched cosmic-files (whole tree, `*.rs`/`*.toml`) for `vnd.portal`, `FileTransfer`, `StartTransfer`, `ashpd`: no hits. Searched libcosmic for the same plus `xdg_portal`; smithay-clipboard and window_clipboard for `vnd.portal`, `FileTransfer`, `StartTransfer`, `ashpd`, `portal`: only libcosmic's receiver-side code.
- Did not inspect xdg-desktop-portal-cosmic or ashpd internals; whether the backend accepts directories, and its exact key format, are out of scope. P0b shows the portal works for Dolphin/Nautilus in this session.
- Did not verify niri's advertised `wl_data_device_manager` version; the `action`/`dnd_finished` events in P0b imply v3, so `set_actions` was sent.
- Did not independently sniff `wl_data_source.offer`; the offered list is taken from P0b's capture column and matches the source exactly.
- Did not test grid view separately; both grid and list go through the same `DndSource` wrapper in `view_responsive` (`cosmic-files:src/tab.rs:4153`).
- Did not check whether other cosmic-files copy paths (Ctrl+C) should also gain the portal types; this brief is drag-out only. #1422/#1586 cover copy-paste too.
- Did not run any build or drag; no git state was changed (HEAD verified at `09bdd8251d5fd909e79fab5b8e6fb5b063a6e6c0`).

## Review (Claude, Oct 31, 2026)

**Verdict: accepted. The [inferred] "small" verdict matches the brief's definition and the
capture.** `~/cosmic-files` is clean at 09bdd8251d5f. Spot-checked in the exact sources the build
uses (`~/.cargo/git/checkouts/…/430f3d4`, `…/859b02c`, registry sctk 0.20.0): clipboard.rs:27–33,
tab.rs:4153 and :4161, Cargo.toml:79–87 (`winit`, no `xdg-portal`), dnd_source.rs:46, :50 and :61,
command.rs:50–52 (`#[cfg(xdg_portal)]`), smithay-clipboard dnd/state.rs:363, :368 and :375, and sctk
mod.rs:81–88. All correct. The call path has no gaps. Upstream claims were checked read-only with
`gh`: #830 was closed 2025-02-26; cosmic-epoch#3926 is open with 0 comments; #1586 is an open draft
from 2026-01-30 with head `61e5e1e`, at `src/clipboard.rs` +137/−77 (+154/−77 including Cargo.toml and
Cargo.lock); and the 2026-09-18 comment reporting it working into Flatpak Zen exists. Nothing was
posted.

Notes for P0h and P0i:

1. The Q6 section runs longer than the 8-line limit. That's fine: the extra lines are the useful
   ones, the #1586 history.
2. The "advertise the portal types only when the portal is reachable" caveat applies to Gimbal Files
   too. Put it in P0h's minimal set.
3. P0i choice for Kyle, after P0h: build #1586 (`61e5e1e`) on a local branch and rerun the two COSMIC
   rows, rather than writing our own patch. It's the same fix, already written upstream.
