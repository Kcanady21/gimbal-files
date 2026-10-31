# P0g: Zen / Firefox drop-receiving notes

Zen 1.24b, Firefox base 159.0, source tag `FIREFOX_159_0_RELEASE` (commit `9a2dc53c369c7ae5172018b35988e1d2ffeff402`, sparse clone at `"/home/kyle/Documents/03.) Code-Projects/gimbal-refs/firefox-159.0"`), Zen GTK drag patches: no `nsDragService*` patch found; one patch touches `widget/gtk/nsDragSessionSource.cpp` (`src/widget/gtk/nsDragSessionSource-cpp.patch`) but only swaps the drag-image alpha for a `nsZenDragAndDrop` opacity setting — flavor negotiation is untouched. [read]

Firefox version from the Flatpak install (read-only):
`platform.ini` `[Build] Milestone=159.0`, `application.ini` `[Gecko] MinVersion=MaxVersion=159.0`. [observed]

All `path:line` below are `widget/gtk/…` at commit `9a2dc53c` unless noted.

## Answers (Q1–Q6)

1. **Receive path.** `nsDragSession::GetData()` at `nsDragService.cpp:611` walks the transferable's
   flavors and fills each via a conversion cascade (`nsDragService.cpp:687-745`); availability is
   reported by `nsDragSession::IsDataFlavorSupported()` at `nsDragService.cpp:756-853`; the Wayland
   read itself happens in `nsDragSessionWayland::DragDataReceived()`
   (`nsDragServiceWayland.cpp:167-231`). [read]
2. **Preference order.** For a URL/file flavor Firefox requests, in order: `application/vnd.portal.files`
   → `application/vnd.portal.filetransfer` → `text/uri-list` → `_NETSCAPE_URL` → the requested MIME
   directly → `text/uri-list` again for `application/x-moz-file` (`nsDragService.cpp:694-745`,
   constants at `:58-64`). [read]
3. **Portal.** Both portal MIMEs are read as opaque keys and passed to
   `FileTransferPortal::RetrieveFilesSync()` → D-Bus `org.freedesktop.portal.FileTransfer.RetrieveFiles`
   (`nsDragServiceWayland.cpp:156-163`, `FileTransferPortal.cpp:369-390`). Only-`text/uri-list` drops
   from outside the sandbox fail the `nsIFile::Exists()` check in `DragData::Export()`
   (`nsDragService.cpp:182-190`) and the tab then loads the URI as a page — the "Problem loading
   page" P0b captured. [read] + [inferred]
4. **Actions.** Firefox requires no particular action; on Wayland it advertises COPY|MOVE as allowed
   but always reports MOVE as preferred — it deliberately rewrites COPY→MOVE before
   `wl_data_offer_set_actions()` to avoid a status feedback lock (`nsDragServiceWayland.cpp:100-104`,
   `RetrievalContextWayland.cpp:176-191`). [read]
5. **Logging.** `LazyLogModule gWidgetDragLog("WidgetDrag");` at `widget/gtk/nsAppShell.cpp:56`. [read]
6. **Minimal set.** Sandboxed: `text/uri-list` + `application/vnd.portal.files` +
   `application/vnd.portal.filetransfer`, backed by the FileTransfer portal, advertising copy|move.
   Non-sandboxed: `text/uri-list` alone. [inferred] — consistent with P0b.

## Receive path (numbered)

1. GDK/compositor schedules the drop: `nsDragSessionWayland::ScheduleDropEvent()`
   (`nsDragServiceWayland.cpp:131-142`) queues an `eDragTaskDrop` task; the offer's MIME list was
   already collected by the `data_offer_offer` listener (`RetrievalContextWayland.cpp:73-77`).
2. The browser front-end asks the drag session which flavors it can accept:
   `nsDragSession::IsDataFlavorSupported()` (`nsDragService.cpp:756`). It answers "yes" not only for
   MIMEs actually present but for conversions: `text/plain` from `text/plain;charset=utf-8`
   (`:812-819`), `text/x-moz-url`/`application/x-moz-file` from `text/uri-list` (`:822-830`),
   `_NETSCAPE_URL` (`:832-838`), and both portal MIMEs → url/file (`:840-850`). [read]
3. The front-end then pulls data per flavor: `nsDragSession::GetData()` (`nsDragService.cpp:611`)
   → `GetDragData()` (`:871`, caches per-atom) → `GetDragDataImpl()` reads from GDK/Wayland.
4. On Wayland the raw read lands in `nsDragSessionWayland::DragDataReceived()`
   (`nsDragServiceWayland.cpp:167`). Portal MIMEs take the branch at `:174-231`, everything else is
   parsed by `GetURIs()` (`:146-151`, `g_uri_list_extract_uris`) or kept as text/image bytes.
5. `DragData::Export()` (`nsDragService.cpp:162-236`) converts to what the transferable asked for:
   file flavors must resolve to an existing `nsIFile` (`:182-190`) or the export fails;
   `text/uri-list` is exported as `text/x-moz-url` via `ConvertToMozURL()` (`:291-295`).
6. Item counting happens earlier and with a *different* order: `GetNumDropItems()` probes
   `text/uri-list`, then both portal MIMEs, then `text/x-moz-url` (`nsDragService.cpp:585-590`) —
   probe order only; actual reads follow the GetData cascade. [read]

## Flavor preference order

Constants (`nsDragService.cpp:58-64`): [read]

- `gTextUriListType = "text/uri-list"` (`:58`)
- `gPortalFile = "application/vnd.portal.files"` (`:62`)
- `gPortalFileTransfer = "application/vnd.portal.filetransfer"` (`:63-64`)
- `gMozUrlType = "_NETSCAPE_URL"` (`:55`), `gTextPlainUTF8Type = "text/plain; charset=utf-8"` (`:59`)

Read order inside `GetData()` when the transferable asks for `text/x-moz-url` or
`application/x-moz-file` (`nsDragService.cpp:692-745`): [read]

1. `sPortalFileAtom` (`application/vnd.portal.files`, `:696`)
2. `sPortalFileTransferAtom` (`application/vnd.portal.filetransfer`, `:698`) — comment at `:692-693`:
   "Try portals first since text/uri-list URIs in sandboxed environments (Flatpak/Snap) may point to
   inaccessible file paths."
3. `sTextUriListTypeAtom` (`text/uri-list`, `:706`) converted to `text/x-moz-url`
4. `sMozUrlTypeAtom` (`_NETSCAPE_URL`, `:713`)
5. the requested flavor verbatim (`:725`)
6. `text/uri-list` once more, converted to `application/x-moz-file` (`:730-745`)

So: portal.files before portal.filetransfer before `text/uri-list` before `text/plain`. This matches
P0b: Zen preferred `application/vnd.portal.files` over `…filetransfer` on the Nautilus row. [observed]

## Portal handling

- Both portal MIMEs carry the FileTransfer session key, not file data. `DragDataReceived()` takes
  their bytes and calls `GetURIsFromPortal()` → `FileTransferPortal::RetrieveFilesSync(key)`
  (`nsDragServiceWayland.cpp:156-163,227`). [read]
- `RetrieveFilesSync()` is a synchronous D-Bus call to `org.freedesktop.portal.FileTransfer.RetrieveFiles`
  and converts the returned paths back to `file://` URIs (`FileTransferPortal.cpp:369-392`,
  `ConvertToURIs` at `:391`). The async variant `RetrieveFiles()` exists too (`:331-368`) but the
  drag path uses the sync one. [read]
- Nothing in the receiver calls `StopTransfer`; P0b's observation that "autostop" closes the transfer
  for sources that skip it is a portal-side behavior, not Firefox's. Firefox is only the
  RetrieveFiles consumer. [inferred]
- If the drop carries only `text/uri-list` pointing outside the sandbox: portals are absent, so
  GetData falls to the `text/uri-list` conversion. `DragData::Export()` resolves each URI with
  `GetFileFromUri()` (`nsDragService.cpp:111-127`) and then checks `file->Exists()`; inside the
  sandbox that fails and export returns false with "uri … not reachable/not found" (`:182-190`).
  The tab then treats the dropped `text/x-moz-url` as a page load of `file:///home/kyle/…`, which
  the sandbox can't read → the "Problem loading page" error page P0b captured for Thunar/COSMIC,
  while the same URI under `~/Downloads` works. Source and capture agree. [read] + [observed]

## Minimal set to offer (sandboxed / non-sandboxed)

All [inferred], pending no further capture; consistent with every P0b row.

Sandboxed Zen (any folder), a file must be offered with:

1. `application/vnd.portal.files` (checked first) and/or `application/vnd.portal.filetransfer`,
   answered through a real `org.freedesktop.portal.FileTransfer` session (StartTransfer + AddFiles;
   RetrieveFiles must return a readable `/run/user/1000/doc/…` path).
2. `text/uri-list` as well — it is what `IsDataFlavorSupported` advertises as the universal
   conversion source (`nsDragService.cpp:822-830`) and the fallback for non-sandboxed targets.
3. Actions: advertise copy (and move if the source supports it). Firefox's own preference arrives as
   MOVE (`nsDragServiceWayland.cpp:100-104`); the source must not delete on drop — P0b confirmed no
   source ever deletes despite the negotiated `move`. [observed]

Non-sandboxed Firefox/Zen:

- `text/uri-list` alone is sufficient (converted to `text/x-moz-url`/`application/x-moz-file`);
  `text/plain` is not needed for file drops.

## Surprises

- Firefox lies about the action on Wayland: it rewrites its COPY preference to MOVE before
  `wl_data_offer_set_actions()` to avoid a gdk_drag_status ↔ selected_action feedback loop
  (`nsDragServiceGtk.cpp:91-102`, `nsDragServiceWayland.cpp:96-104`,
  `RetrievalContextWayland.cpp:183-191`). This exactly explains P0b's "negotiated action was move (2)
  in all 8 rows" even though sources advertised copy|move. [read] + [observed]
- The item-count probe (`GetNumDropItems`, `nsDragService.cpp:585-590`) prefers `text/uri-list`
  first while the actual read prefers portals first — two different orders in one file. [read]
- `DragData::Export()` checks `file->Exists()` (`nsDragService.cpp:184-190`): the receiver catches
  invisible paths itself instead of failing later in content. [read]
- Firefox 159 splits the old `nsDragService.cpp` into `nsDragService{,Gtk,Wayland}.cpp` +
  `nsDragSessionSource.cpp`; the flavor logic lives in `nsDragService.cpp` (nsDragSession methods).
  [read]
- `DragDataReceived()` carries a workaround for GTK issue 6563: portal flavors receive a numeric id
  for local files which GTK's `gtk_selection_data_get_uris` resolves
  (`nsDragServiceWayland.cpp:201-215`). [read]
- Zen's only GTK drag patch (`src/widget/gtk/nsDragSessionSource-cpp.patch`, branch `dev`) just makes
  the drag-image alpha configurable via `nsZenDragAndDrop` (`src/zen/drag-and-drop/`). [observed]

## Not found / unsure

- No patch touching `widget/gtk/nsDragService*` in zen-browser/desktop. Searched the `dev` branch via
  the GitHub contents API: `src/` top level, `src/widget/`, `src/widget/gtk/`, `src/zen/`,
  `src/external-patches/`. The recursive tree API returned 404, so this was a directory-by-directory
  walk, not a full-tree grep — a patch hidden in a subdirectory I didn't list would be missed, but
  the widget directories are fully enumerated above. [observed]
- Whether Zen's `external-patches/firefox` changes drag code: the directory exists; I only checked
  `src/widget` overlays and `src/zen`. Not inspected file-by-file. [observed]
- No X11-specific portal behavior checked; Zen here runs Wayland (niri), and the brief only needs
  the Wayland path. `nsDragServiceGtk.cpp` shows the same COPY→MOVE rewrite for the GTK3 context
  path, so the action finding likely holds on X11 too. [inferred]

## Review (Claude, Oct 31, 2026)

**Verdict: accepted.** The Firefox base comes from the installed Flatpak (`files/zen/platform.ini`:
`Milestone=159.0`, confirmed by Claude), not guessed from Zen's version. `FIREFOX_159_0_RELEASE`
resolves to `9a2dc53c…`, which matches the clean sparse clone. Spot-checked and correct:
nsDragService.cpp:58–64, :692–699, :706 and :182–190; nsDragServiceWayland.cpp:96–104 and
:156–163; FileTransferPortal.cpp:369; nsAppShell.cpp:56. **The `MOZ_LOG` module is `WidgetDrag`,
which is P0a's default already, so no change is needed.** The receive side explains every P0b row: the
portal types are read first, `.files` is preferred over `.filetransfer`, and plain `file://` URIs
outside the sandbox fail the `Exists()` check.

Notes for P0h:

1. The COPY→MOVE rewrite (nsDragServiceWayland.cpp:99–103) is why every P0b row negotiated MOVE. Put
   it next to P0c review correction 2 and P0e review addition 2. Together they mean a source sees MOVE
   on every drop into Firefox, so a source that deletes on MOVE would delete the user's file. "The
   source must not delete" is a requirement on Gimbal Files, not something P0b observed about all
   sources: GTK3 does ask Thunar to delete, and Thunar refuses.
2. The Zen-patch search was a directory walk, not a full-tree grep, as the notes say. That's enough
   here, because the receive path behaves exactly as the stock Firefox source predicts in all 8 rows.
