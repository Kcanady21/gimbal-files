# P0e: Nautilus / GTK4 drag-out notes

Pinned: nautilus 50.2.2 (c6592e9c7fce37ad685d0ba24720893955b7835d), gtk 4.22.4 (7f99ab1a26408b6499a18f353f081e3c0598ea5c), gtk4-rs docs version read: gdk4/gtk4 0.11.5 (docs.rs `latest`; tags exist upstream for 50.2.1 and 50.2.2 — used the exact 50.2.2 to match the installed 50.2.2)

Prefixes (clones under `"/home/kyle/Documents/03.) Code-Projects/gimbal-refs"`): `N:` = `nautilus-50.2.2`, `G:` = `gtk-4.22.4`. Tags: [read] = seen in source at the pinned tag; [observed] = matches the P0b capture; [inferred].

## Answers (one line each, Q1–Q7, each with its tag and path:line)

- **Q1** [read] The only file drag-out path is one `GtkDragSource` per view cell, created in `N:src/nautilus-list-base.c:891` and wired to `prepare` at `:894`; the grid view gets it free by subclassing (`N:src/nautilus-grid-view.c:35`), and `nautilus-dnd.c/.h` create no drag source at all.
- **Q2** [read] `prepare` returns a single typed provider — `gdk_content_provider_new_typed (GDK_TYPE_FILE_LIST, file_list)` at `N:src/nautilus-list-base.c:470` (`GSList<GFile>` built at `:455`); no union, no bytes, no MIME string anywhere in the drag path (union only for the clipboard, `N:src/nautilus-clipboard.c:318-321`).
- **Q3** [read] `actions = GDK_ACTION_ALL | GDK_ACTION_ASK` (`N:src/nautilus-list-base.c:443`), with `MOVE` cleared per file that cannot be deleted (`:459`) and applied via `gtk_drag_source_set_actions` (`:463`); ASK is advertised but unimplemented (`:896` TODO; `N:src/nautilus-dnd.c:85` body is `#if 0`, returns 0 at `:159`).
- **Q4** [read] For `GDK_TYPE_FILE_LIST` GTK 4.22.4 registers exactly two MIMEs, in this order: `text/plain;charset=utf-8` then `text/uri-list` (`G:gdk/gdkcontentserializer.c:1042,1047`), plus the two portal MIMEs added by `:1054`; `text/uri-list` = one percent-encoded `file://` URI per line, each terminated `\r\n` including the last (`:814,857`).
- **Q5** [read] The portal serializers are registered **once at startup, only if the portal is reachable**: `file_transfer_portal_register ()` from `gdk_content_init_serializers` (`G:gdk/gdkcontentserializer.c:966,1054`) → gated on `gdk_display_should_use_portal` + a `name has an owner` probe (`G:gdk/filetransferportal.c:607,583,618-621`); at serialize time it does `StartTransfer{writable,autostop}` → `AddFiles` (16-fd chunks, `O_PATH`) → writes the key, and requires `g_file_peek_path` (`:370,392`) so non-native `GFile`s are dropped.
- **Q6** [read] `G:gdk/wayland/gdkdrag-wayland.c:350` builds the `wl_data_source`: offers `application/x-gtk-local-dnd` first (`:376`) then **every** MIME the provider can serialize to (`:365,377-378`); `wl_data_source_set_actions` at `:415`, only when the manager version ≥ 3, mapping copy|link→COPY, MOVE→MOVE, ASK→ASK (`:97-108`).
- **Q7** [read] Verified in docs.rs gdk4/gtk4 0.11.5: `gdk4::FileList::from_array(&[File])` (feature `v4_8`), `impl From<FileList> for Value`, `gdk4::ContentProvider::for_value(&Value)`, `gtk4::DragSource::new()/set_actions/set_icon/connect_prepare` — see the two lists below.

## Drag-out call path (numbered: Nautilus → GDK content → Wayland)

1. `GtkGestureDrag` begins on the cell controller (`G:gtk/gtkdragsource.c:312` → `gtk_drag_source_drag_begin()` `:571`).
2. GTK emits `prepare`; Nautilus returns the `GdkFileList` provider (`N:src/nautilus-list-base.c:894` → `:405` → `:470`). Actions/icon were set inside the same callback (`:463,468`).
3. `gdk_drag_begin (surface, pointer, content, source->actions, dx, dy)` (`G:gtk/gtkdragsource.c:609`).
4. Wayland backend `_gdk_wayland_surface_drag_begin` (`G:gdk/wayland/gdkdrag-wayland.c:383`) → `gdk_wayland_drag_create_data_source` (`:350`): formats ∪ serializable MIMEs (`:365`, impl `G:gdk/gdkcontentserializer.c:496,506`), `wl_data_source_offer` per MIME (`:376-378`).
5. `wl_data_source_set_actions` if the bound `wl_data_device_manager` version ≥ `WL_DATA_SOURCE_SET_ACTIONS_SINCE_VERSION` (= 3, `/usr/include/wayland-client-protocol.h:2664`) (`:414-415`).
6. `wl_data_device_start_drag` (`:419-423`); nothing is serialized yet.
7. The receiver/compositor picks a MIME and requests it: `data_source_send` (`:262`) → `gdk_intern_mime_type` (`:274`; unknown → `close(fd)`, `:276-279`) → `gdk_drag_write_async` (`:283` → `G:gdk/gdkdrag.c:536`) → the registered serializer runs then (for the portal MIMEs, the D-Bus calls in the next section).
8. `data_source_target` / `dnd_drop_performed` / `dnd_finished` / `action` drive `GtkDragSource`'s `drag-end` (`G:gtk/gtkdragsource.c:489`) and Nautilus's post-drop behaviour.
   Offer order = registration order, because serializers are queued `g_queue_push_tail` (`G:gdk/gdkcontentserializer.c:428`) and the builder prepends then writes backwards (`G:gdk/gdkcontentformats.c:836,737,766`). [read]

## MIME types offered

| MIME | Added by (Nautilus / GTK4 serializer / portal) | path:line | Payload format |
|---|---|---|---|
| `application/x-gtk-local-dnd` | GDK Wayland backend (drag-only marker) | `G:gdk/wayland/gdkdrag-wayland.c:376`, define `G:gdk/wayland/gdkdrop-wayland.h:32` | no serializer; empty payload, used to spot in-process drags (`G:gdk/wayland/gdkseat-wayland.c:259`) |
| `text/plain;charset=utf-8` | GTK4 serializer | `G:gdk/gdkcontentserializer.c:1042` → `file_text_serializer :871` | absolute local paths joined by `\n`, **no** trailing newline; falls back to the URI if there is no path (`:893-896`) |
| `text/uri-list` | GTK4 serializer | `G:gdk/gdkcontentserializer.c:1047` → `file_uri_serializer :823` | each entry percent-encoded (`g_filename_to_uri :814`) and terminated `\r\n` **including the last** (`:857`); `content`/`http`/`https` kept as-is (`:794`), other non-native GFiles use `g_file_get_uri` |
| `application/vnd.portal.filetransfer` | portal (GTK4) | `G:gdk/filetransferportal.c:531` (list), `:525` (single `GFile`) | the portal transfer **key**, NUL-terminated (`strlen (key) + 1`, `:346-348`) |
| `application/vnd.portal.files` | portal (legacy alias) | `G:gdk/filetransferportal.c:559` (`:553`) | same serializer/payload as above; kept for GTK 4.6-era Flatpaks (`:549-552` FIXME) |
| `x-special/gnome-copied-files` | Nautilus, **clipboard only** | `N:src/nautilus-clipboard.c:332-341` | not offered on drags — see Surprises |

`G_TYPE_FILE` serializers exist too (`G:gdk/gdkcontentserializer.c:1032,1037`) but are skipped for a `FileList` provider because format matching is exact GType equality (`G:gdk/gdkcontentformats.c:490`). [read]

## Portal (registration condition, D-Bus calls, requirements)

- **When registered** [read]: at process start, not per drag — `gtk_init` → `do_pre_parse_initialization` (`G:gtk/gtkmain.c:558`) → `gdk_pre_parse` (`G:gdk/gdk.c:330,357`) → `gdk_content_init_serializers` (`G:gdk/gdkcontentserializer.c:966`) → `file_transfer_portal_register ()` (`:1054`, guarded `G_OS_UNIX && !__APPLE__`; same on the deserializer side `G:gdk/gdkcontentdeserializer.c:986`). One-shot (`static gboolean called`, `G:gdk/filetransferportal.c:600-604`).
- **Condition** [read]: `gdk_display_should_use_portal (display, NULL, 0)` (`:607`; impl `G:gdk/gdk.c:619-649` — `GDK_DEBUG=no-portals` off, per-portal disable, **sandboxed ⇒ TRUE**, else needs portals in the environment), then a sync proxy to bus `org.freedesktop.portal.Documents`, object `/org/freedesktop/portal/documents`, iface `org.freedesktop.portal.FileTransfer`, `DO_NOT_AUTO_START` (`:608-616`), and `finish_registration ()` only if that bus name has an owner (`:583,618-621`). So an unsandboxed app with no portal running simply never offers the portal MIMEs. [inferred]
- **At serialize time, in order** [read]: `portal_file_serializer` (`:355`) collects `g_file_get_path` per file → `StartTransfer` with options `writable` and `autostop=TRUE` (`:222-223,225`; `writable` passed `TRUE` by the serializer at `:409`) → reply key (`:178-186`) → `AddFiles` called repeatedly, ≤16 files per message, each opened `O_PATH | O_CLOEXEC` and sent as an fd in a `GUnixFDList` (`:95,119,150-158`) → the key string + NUL written as the payload (`:342-348`). Receiving side: `RetrieveFiles` (`:293`). GTK **never calls `StopTransfer`** (grep across `G:gdk` found nothing) — `autostop` closes it. [observed] P0b: `StartTransfer("writable","autostop")`, `AddFiles`, `RetrieveFiles` → `/run/user/1000/doc/…/sample.png`.
- **Requirements from the app** [read]: nothing to opt in, but the payload is path-based — a `GFile` without a local path is dropped with a debug line (`:370,392-397`), so a `gvfs`/`trash://`-style URI does not travel through the portal. Passing `FileList` of local-path files is the whole requirement.

## For gtk4-rs: free vs must-do

**(a) Free from just offering a `gdk::FileList`** [read, 0.11.5 docs + the GTK source above]:
`text/uri-list` and `text/plain;charset=utf-8` with correct CRLF/percent-encoding; both portal MIMEs with `StartTransfer`/`AddFiles`/key payload when the portal is reachable; the `x-gtk-local-dnd` marker; `wl_data_source.offer` for every serializable format and `set_actions` (incl. ASK→`WL_..._ASK`) plus the version fallback; lazy serialization only on request; drag icon if you call `set_icon`.
Names checked: `gdk4::FileList::from_array (&[File]) -> FileList` (feature `v4_8`), `gdk4::FileList::files() -> Vec<File>`, `impl From<FileList> for Value`, `gdk4::ContentProvider::for_value (value: &Value) -> ContentProvider`, `::for_bytes (mime_type: &str, bytes: &Bytes)`, `::new_union (providers: &[ContentProvider])`, `gtk4::DragSource::new() -> DragSource`, `set_actions (&self, actions: DragAction)`, `set_icon (Option<&impl IsA<Paintable>>, i32, i32)`, `connect_prepare<F: Fn(&Self, f64, f64) -> Option<ContentProvider>>`.

**(b) What the app must add itself** [read from Nautilus]:
build the selected `Vec<gdk::File>` from the selection model (`N:src/nautilus-list-base.c:445-460`); compute actions per drag and mask `MOVE` when a file cannot be deleted (`:443,459`); a drag icon `Paintable` for multi-selection (`N:src/nautilus-dnd.c:291`); an early `NULL` return to veto drags (`:414-416` cell-unbound, `:430` `dnd_disabled`); a real ASK UI — GTK gives you the wire action only (`G:gdk/wayland/gdkdrag-wayland.c:105`), Nautilus's dialog is dead code (`N:src/nautilus-dnd.c:85-159`); the move/delete-after-drop decision (`N:src/nautilus-dnd.c:161`); and any own MIME via `ContentProvider::new_union` + `for_bytes`, which is what Nautilus does for the clipboard only (`N:src/nautilus-clipboard.c:318-321,334`).

## Surprises

- **`x-special/gnome-icon-list` is gone.** Repo-wide grep for `icon-list`/`icon_list` in `N:` returned nothing; `nautilus-dnd.h` still exists but declares only drop-side helpers + `get_paintable_for_drag_selection` (`N:src/nautilus-dnd.h:21-32`). The old plan note is wrong for 50.2.2. [read]
- **Nautilus's drag offers no app-specific MIME at all** — one `GdkFileList`, and GTK decides the wire formats. All 5 types in the P0b capture come from GTK/the backend, not from Nautilus. [read] matches [observed]
- **`x-special/gnome-copied-files` is clipboard-only** (`N:src/nautilus-clipboard.c:332`), so a Nautilus *drag* is not distinguishable as "cut" — the negotiated action carries that. This explains why the P0b capture shows that MIME from COSMIC (which puts it on the DnD path) but never from Nautilus. [observed]
- **Portal registration is startup-time and probe-based, with `DO_NOT_AUTO_START`** — a late-starting portal after `gtk_init` will not be picked up for that process (`G:gdk/filetransferportal.c:610,618-621`). [read]
- **GTK always asks for a writable transfer** (`writable` = TRUE, `:409`) even for a plain copy, and relies on `autostop`. [read] P0b saw the same and noted the missing `StopTransfer` is harmless. [observed]
- **`text/uri-list` ends with a trailing `\r\n`** per entry (`:857`), while `text/plain;charset=utf-8` has **no** trailing `\n` (`:902-905`) — easy to get wrong when hand-rolling serializers. [read]
- **A `FileList` provider never picks up the `G_TYPE_FILE` serializers** (exact-match format test, `G:gdk/gdkcontentformats.c:490`), so single-file vs list type choice matters only for the portal/`GFile` variants. [read]
- **Clipboard does not offer `x-gtk-local-dnd`** — the marker is added in the drag path only (`G:gdk/wayland/gdkdrag-wayland.c:376` vs `G:gdk/wayland/gdkclipboard-wayland.c:199-208`). [read]
- Source vs capture: **no disagreement** on MIME set, order, portal options, or `move`. Zen picked the legacy `application/vnd.portal.files` even though GTK offers `…filetransfer` first (`:531` before `:559`), so receiver preference is not offer order — treat the capture as correct on that choice. [observed]

## Not found / unsure (what you searched for)

- `gtk_drag_source_new` repo-wide in `N:` → only `nautilus-list-base.c:891` and `nautilus-column-chooser.c:379`; `grep '"prepare"'` → the same two. So there is no view-level (background) drag source; sidebar reordering uses `gdk_drag_begin` directly with `GDK_ACTION_MOVE` and its own row type (`N:src/nautilus-sidebar.c:3291,3296`) — not a file drag-out path.
- Searched `G:` for a `filetransfer` serializer registered anywhere outside `filetransferportal.c` (only `gdkcontentserializer.c:1054` / `gdkcontentdeserializer.c:986`), and for `StopTransfer` (none).
- Unsure: whether `GDK_ACTION_ALL`-plus-ASK has any effect on non-Wayland backends — X11/win32/macOS paths were not read (brief is Wayland-only).
- Unsure: `gtk4::DragSource` `set_start_button` / `set_threshold` did not appear on the 0.11.5 page content I fetched; no other `for_typed`-style binding exists either — C names `gtk_drag_source_set_start_button`, `gdk_content_provider_new_typed` are the safe references.
- Unsure: no P0b capture line covers a *non*-native `GFile` (e.g. `trash:`), so the `g_file_peek_path` drop (`:370,392`) is source-only [read], unverified against the capture.
- No app code was written and no drags performed (brief: source reading only). Only this file was written.

## Review (Claude, Oct 31, 2026)

**Verdict: accepted.** Both clones are clean, and Nautilus is at tag 50.2.2 (the brief asked for
"50.2". Using the exact 50.2.2 tag was the right choice). Spot-checked and correct:
nautilus-list-base.c:443, :459, :470 and :891; gdkcontentserializer.c:857, :1042, :1047 and
:1054; filetransferportal.c:222–223, :409, :531, :559, :607 and :618–621; gdkdrag-wayland.c:376 and
:415. There's no `StopTransfer` anywhere in `gdk/`, as claimed. The gtk4-rs names were checked in the
gdk4 0.11.5 crate source, the current release: `FileList::from_array`, `files`,
`ContentProvider::for_value`, `for_bytes` and `new_union` all exist. The offer list matches the
capture exactly, in order.

Additions for P0h:

1. **An unsandboxed GTK4 app gets the portal types too.** `gdk_display_should_use_portal`
   (gdk.c:619–651) returns TRUE when sandboxed, and otherwise when `environment_has_portals()`, which
   is true on Kyle's desktop (Nautilus offered both). So Gimbal Files gets the whole fix just by
   offering a `gdk::FileList`. The only exception is a portal that starts after `gtk_init`, which is
   never picked up (Surprises, above).
2. **Never delete on `drag-end`.** GTK4 emits `drag-end` with `delete_data = TRUE` whenever a drop
   succeeds with MOVE (gtkdragsource.c:487–489), and Firefox reports MOVE for every Wayland drop
   (P0g). Nautilus never connects `drag-end` in its file views (there's no `drag-end`/`delete_data` in
   nautilus-list-base.c or nautilus-dnd.c), so nothing gets deleted. Gimbal Files must not treat
   `delete_data` as "move the file" when the drop target is another app. This goes with P0c review
   correction 2 (GTK3's `DELETE` request).
3. On "Zen picked `.files` though `.filetransfer` is offered first": P0g explains this. Firefox asks
   for `application/vnd.portal.files` first, whatever the offer order (nsDragService.cpp:696–698).
