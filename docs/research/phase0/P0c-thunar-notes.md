# P0c: Thunar drag-out notes
Pinned: thunar-4.20.7 (df0e054250e098fce8daefad80e550a94d792839), gtk 3.24.52 (6a0b360d473f7c546314738c0c8dd9829eb9d3c2)

Tags: [read] = verified in the cloned source at the pinned tag; [observed] = observed directly
(clone state, `git describe`); [inferred] = conclusion drawn from the code, not literally present.
`path:line` without a prefix is relative to that repo's root. Thunar excerpts are from GPL-2.0-or-later
code, GTK excerpts from LGPL-2.1-or-later code (per the headers of the cited files).

## Answers (one line each, Q1–Q7, each with its tag and path:line)

1. [read] Drag-out starts in `thunar/thunar-standard-view.c`: left-button drags via `gtk_drag_source_set` (thunar-standard-view.c:1066) driven by GTK's drag gesture (gtkdragsource.c:100), right-button drags via `gtk_drag_begin_with_coordinates` (thunar-standard-view.c:3010) after a threshold check (3003); both funnel through `gtk_drag_begin_internal` (gtkdnd.c:1707) → `gdk_drag_begin_from_point` (gdkwindow.c:11477) → `_gdk_wayland_window_drag_begin` (gdkdnd-wayland.c:521).
2. [read] Drag-out offers exactly one target, `text/uri-list` (`drag_targets`, thunar-standard-view.c:503–506); no `application/x-xfce-…`, and `XdndDirectSave0`/`_NETSCAPE_URL`/`application/octet-stream` exist only in the drop-target table (509–515).
3. [read] Thunar builds a `char**` of URIs with `thunar_g_file_list_to_stringv` (thunar-gio-extensions.c:1269–1294) and GTK3 serializes it as `uri` + `"\r\n"` per entry — CRLF-separated with a trailing CRLF, converted to ASCII (gtkselection.c:1883–1892).
4. [read] Actions advertised on drag-out are COPY|MOVE|LINK (thunar-standard-view.c:1066), plus ASK on right-drag (3010–3012); the Copy/Move/Ask *semantics* (`thunar_dnd_ask` thunar-dnd.c:64–68, `thunar_dnd_perform` thunar-dnd.c:208–247, smart move-vs-copy thunar-file.c:2085–2129) are all drop-side — the source only advertises the action mask.
5. [read] On Wayland, `_gdk_wayland_window_drag_begin` offers every target atom as a MIME via `wl_data_source_offer` (gdkdnd-wayland.c:547–553); GTK3 adds no MIME of its own on the DnD path — it invents `application/gtk+-local-dnd-<pid>` only when the target list is empty (gdkdnd-wayland.c:556–562), and maps STRING/UTF8_STRING only for clipboard/primary selections (gdkselection-wayland.c:1821–1826).
6. [read] GTK3 3.24.52 *does* ship FileTransfer-portal support (`gtk/filetransferportal.c`, doc'd "Since 3.24.37" gtkselection.c:1859–1860), but it is not used for Thunar drags: portal targets enter an offer only via `gtk_target_list_add_uri_targets` (gtkselection.c:529–535) or if the destination requests those atoms, and Thunar never calls it — its offer list is the hand-written one-entry table.
7. [read] GTK3 does for the app: uri-list payload serialization (gtkselection.c:1876–1905), `wl_data_source` creation/offers (gdkdnd-wayland.c:543–553), peer-request → `drag-data-get` plumbing (gtkdnd.c:2684–2734, gdkselection-wayland.c:1061–1103); the app must do: target declaration, URI-list building (incl. the native-path preference, thunar-gio-extensions.c:1281–1291), right-drag ASK initiation, drag icon, and all drop-side action logic [inferred for GTK4: P0e confirms what GTK4's API actually carries over].

## Drag-out call path (numbered, path:line and what happens at each step)

Left-button drag — one implementation shared by icon, compact and details views:

1. thunar/thunar-standard-view.c:1038 — `view = gtk_bin_get_child (...)`: the real ExoIconView/GtkTreeView is the drag source, so icon/compact/details get identical DnD from one init. [read]
2. thunar-standard-view.c:1066 — `gtk_drag_source_set (view, GDK_BUTTON1_MASK, drag_targets, 1, GDK_ACTION_COPY|MOVE|LINK)`; signals `drag-begin/data-get/data-delete/end` connected at 1067–1070. [read]
3. gtk/gtkdragsource.c:68–114 — GTK's button/motion handler feeds a `GtkGestureDrag`; past `gtk_drag_check_threshold` (85) it calls `gtk_drag_begin_internal` (100) with the site's target list (stored verbatim by `gtk_drag_source_set`, gtkdragsource.c:192) and `site->actions`. [read]
4. gtk/gtkdnd.c:1707–1828 — `gtk_drag_begin_internal`: computes suggested/possible actions (1736), flattens the target list to GdkAtoms (1801–1808), calls `gdk_drag_begin_from_point` (1828). [read]
5. gdk/gdkwindow.c:11476–11484 — dispatches to the window impl's `drag_begin` vfunc. [read]
6. gdk/wayland/gdkdnd-wayland.c:521–565 — `_gdk_wayland_window_drag_begin`: copies targets (537), gets a `wl_data_source` (543–545), and calls `wl_data_source_offer (data_source, mimetype)` for each atom (547–553). [read]
7. gdk/wayland/gdkselection-wayland.c:1061–1103 — when the peer requests a MIME, the `data_source_send` event receives an fd and routes to `gdk_wayland_selection_request_target` for the DND selection (1099–1102). [read]
8. gtk/gtkdnd.c:2684–2734 — `gtk_drag_selection_get` emits `drag-data-get` on the source widget (2725). [read]
9. thunar-standard-view.c:3706–3745 — `drag-begin` handler snapshots the selection into `drag_g_file_list` (3717–3720) and sets the drag icon surface (3737); thunar-standard-view.c:3750–3766 — `drag-data-get` handler: `thunar_g_file_list_to_stringv` (3762) → `gtk_selection_data_set_uris` (3763). [read]

Right-button drag:

1. thunar-standard-view.c:4229–4265 — `thunar_standard_view_queue_popup` arms a `MAX(225, gtk-menu-popup-delay)` timer on button-3 press and hooks button-release/motion on the real view (4262–4263). [read]
2. thunar-standard-view.c:2992–3019 — motion past the threshold (3003) cancels the timer and calls `gtk_drag_begin_with_coordinates (view, target_list, GDK_ACTION_COPY|MOVE|LINK|ASK, 3, event, -1, -1)` (3009–3012); if the timer expires instead, the context menu pops (2973–2988, 4256). [read]
3. gtk/gtkdnd.c:1983–1995 — `gtk_drag_begin_with_coordinates` → same `gtk_drag_begin_internal` funnel as above. [read]

## MIME types offered

| MIME | Added by (Thunar / GTK3) | path:line | Payload format |
|---|---|---|---|
| `text/uri-list` | Thunar (`drag_targets`) | thunar-standard-view.c:505 | CRLF-separated `file://` URIs, trailing CRLF, ASCII-only (gtkselection.c:1883–1892) [read] |
| `application/gtk+-local-dnd-<pid>` | GTK3, only when a drag starts with an empty target list | gdkdnd-wayland.c:556–562 | n/a — never offered by Thunar (its list is non-empty) [read] |
| `application/vnd.portal.filetransfer` / `application/vnd.portal.files` | GTK3, only if the app calls `gtk_target_list_add_uri_targets` | gtkselection.c:529–535 | transfer key string from StartTransfer+AddFiles (gtkselection.c:1939–1950) [read] — not in Thunar's offer [read] |

Drop-only targets, for contrast (not drag-out): `application/octet-stream`, `XdndDirectSave0`, `_NETSCAPE_URL`
(thunar-standard-view.c:509–515) [read]. Other Thunar drag sources: location buttons (thunar-location-button.c:269,
LINK action) and the bulk renamer (thunar-renamer-dialog.c:639, MOVE) — separate `drag_targets` tables [read].

### Payload excerpt (GTK3, LGPL-2.1-or-later) — gtkselection.c:1883–1892

```c
      list = g_string_new (NULL);
      for (i = 0; uris[i]; i++)
	{
	  g_string_append (list, uris[i]);
	  g_string_append (list, "\r\n");
	}

      result = g_convert (list->str, list->len,
			  "ASCII", "UTF-8",
			  NULL, &length, NULL);
```

### URI build excerpt (Thunar, GPL-2.0-or-later) — thunar-gio-extensions.c:1281–1290

```c
      /* Prefer native paths for interoperability. */
      gchar *path = g_file_get_path (G_FILE (lp->data));
      if (path == NULL)
        {
          uris[n++] = g_file_get_uri (G_FILE (lp->data));
        }
      else
        {
          uris[n++] = g_filename_to_uri (path, NULL, NULL);
          g_free (path);
        }
```

Percent-encoding of reserved/non-ASCII characters is done by `g_filename_to_uri`/`g_file_get_uri` —
GLib, not in these clones, so [inferred] from GLib API semantics, not read here.

## Actions

- Source side advertises only: left-drag COPY|MOVE|LINK (thunar-standard-view.c:1066), right-drag COPY|MOVE|LINK|ASK (3010–3012). [read]
- ASK exists to support *right-button* drags (doc comment thunar-standard-view.c:4213–4227). [read]
- Drop-side resolution when Thunar receives a drop: `thunar_standard_view_get_dest_actions` → `thunar_file_accepts_drop` (thunar-standard-view.c:2299; thunar-file.c:2036); ASK wins if offered (thunar-file.c:2061–2062); move preferred over copy when source parent is writable and same filesystem (thunar-file.c:2088–2129); finally `gdk_drag_status` (thunar-standard-view.c:2362). [read]
- `GDK_ACTION_ASK` is resolved by `thunar_dnd_ask`'s Copy/Move/Link menu at receive time (thunar-standard-view.c:3490–3492), then `thunar_dnd_perform` dispatches copy/move/link jobs (thunar-dnd.c:231–247). [read]
- All of that is drop-side; a drag to a browser exercises none of it — the browser picks from the advertised mask. [inferred]

## Portal / sandbox

- GTK3 3.24.52 has `gtk/filetransferportal.c` (LGPL): D-Bus proxy to `org.freedesktop.portal.FileTransfer` at `/org/freedesktop/portal/documents` on the session bus (filetransferportal.c:471–480); `file_transfer_portal_supported()` = that bus name has an owner (490–496). [read]
- Export = `StartTransfer` then `AddFiles` with `O_PATH` fds in chunks of 16 (filetransferportal.c:221–223, 253–255, 311–316, chunking comment 96–98, `open(..., O_PATH|O_CLOEXEC)` at 120/280); `writable=TRUE, autostop=TRUE` options (218–219). [read]
- Trigger on the source side: the *requested target* must be `application/vnd.portal.filetransfer` or `application/vnd.portal.files` (`portal_filetransfer_atom`/`portal_files_atom`, gtkselection.c:345–346, 367–368) and the portal must be running; then `gtk_selection_data_set_uris` converts the URIs to paths and replaces the payload with the transfer key (gtkselection.c:1906–1955). Symmetric receive side in `gtk_selection_data_get_uris` (2003–2025). [read]
- Portal targets are *never added automatically* to a drag: `gtk_drag_begin_internal` offers exactly the app's target list (gtkdnd.c:1801–1808), and `gtk_target_list_add_uri_targets` — the one function that appends the portal atoms (gtkselection.c:529–535) — is never called anywhere in Thunar (`grep -rn "add_uri_targets|include_uri" thunar/*.c` → no matches) [observed]. Thunar has zero occurrences of "portal" in its source [observed].
- Consequence: a Thunar drag-out advertises only `text/uri-list` with plain `file://` URIs; the GTK3 portal path is dormant unless the *destination* requests a portal atom that was never offered. A GTK3-based destination that calls `gtk_target_list_add_uri_targets` on its dest list can still pull the portal protocol, but not from Thunar as source. How Zen (Firefox-based, own DnD stack) imports out-of-sandbox paths is outside these sources — open item. [inferred]
- `gdk_should_use_portal` (gdk.c:479–495, `GTK_USE_PORTAL`) is used only for the settings portal in this tree (gdkscreen-wayland.c:722), not for DnD. [read]

## Surprises

- The GTK3 portal DnD support landed in the 3.24 *stable* series (3.24.37 doc note, gtkselection.c:1859–1860) — an installed libgtk-3 from Debian carries sandbox-bridge code, even though Thunar never activates it.
- Thunar prefers *native paths* over URIs when building the uri-list (thunar-gio-extensions.c:1281), then re-encodes via `g_filename_to_uri` — the interop choice is made in the payload builder, not at drop time.
- `gtk_selection_data_set_uris` converts the payload to ASCII and fails the whole target if any URI survives as non-ASCII (gtkselection.c:1890–1904) — safe only because the URI builders percent-encode. [read]
- Right-button drag is a real drag with ASK, not a context-menu shortcut: the 225 ms timer decides drag vs. menu (thunar-standard-view.c:4241–4264). [read]
- The one-target `drag_targets` table means Thunar's offer order is trivially stable; `gtk_target_list_add_table` actually prepends entries right-to-left (gtkselection.c:546–562), so order matters if we ever add targets in Gimbal. [read]

## Not found / unsure (what you searched for)

- `application/x-xfce-*` drag target: not found; searched `grep -rn '"application/' thunar/*.c` — only file-chooser filter mime types (thunar-chooser-dialog.c:919–952). [observed]
- Any Thunar portal involvement: `grep -rin portal thunar/` → no matches. [observed]
- Portal atoms in gdk/: `grep -rn "portal" gdk/wayland/*.c gdk/*.c` → settings portal only (gdk.c:479–495, gdkscreen-wayland.c:75, 722). [observed]
- GTK-side auto-add of portal targets to drags: searched gtkdnd.c, gtkdragsource.c, gtkdragdest.c, gdkdnd-wayland.c for `portal|vnd` — none; only `gtk_target_list_add_uri_targets`/manual adds can introduce them. [read]
- How Zen/Firefox consumes Thunar's `text/uri-list` under its Flatpak sandbox (xdg-document-portal fuse, its own portal D-Bus calls, etc.): not answerable from these two repos; needs P0 on the browser side or runtime observation (out of scope, no drags allowed here). [unsure]
- Whether the installed libgtk-3 binary matches the 3.24.52 tag beyond the apt candidate version: assumed per the brief. [unsure]

## Review (Claude, Oct 31, 2026)

**Verdict: accepted with corrections.** These corrections win over the text above. Spot-checked
and correct at the pinned tags: thunar-standard-view.c:503–506, :1066, :3009–3012, :3762–3763,
thunar-gio-extensions.c:1281, gtkselection.c:529–535, :1858–1860, gdkdnd-wayland.c:556–562 and
filetransferportal.c:218–219. Both clones are clean at the recorded commits. The notes don't use
P0b-results.md, and the capture contradicts Q2 and Q5:

1. **Thunar's offer is `text/uri-list`, `text/uri-list`, `DELETE`, not one target** [observed in
   captures/20261031-070255-thunar-outside/source.log.gz]. `wl_data_source.offer("text/uri-list")`
   is sent before `start_drag`, as described in the call path (gdkdnd-wayland.c:547–553). After
   `start_drag`, GTK3 offers the source's targets again plus `DELETE`:
   `gtk_drag_source_check_selection` (gtkdnd.c:2536) adds each target and then `DELETE`
   (gtkdnd.c:2570–2573) through `gtk_selection_add_target` → `gdk_wayland_selection_add_targets`
   (gtkselection.c:973) → `wl_data_source_offer` (gdkselection-wayland.c:1826) [read]. So Q5's
   "GTK3 adds no MIME of its own on the DnD path" is wrong. GTK3 adds `DELETE`, an X11-era
   internal target, and the Wayland backend offers it as if it were a MIME type.
2. **A drop that ends as a move makes GTK3 ask the source to delete.** In `data_source_dnd_finished`,
   `context->action == GDK_ACTION_MOVE` emits a `DELETE` selection request
   (gdkselection-wayland.c:1163–1167), which reaches the app as `drag-data-delete`. Thunar's handler
   only stops the default ExoIconView/GtkTreeView handler and deletes nothing
   (thunar-standard-view.c:3768–3775) [read]. That's why the 8 P0b drops, which all negotiated
   `move`, left the files in place [observed]. **For Gimbal Files: an outgoing drag must never delete
   the source when the drop reports "move" to a browser.** Q4 and the Actions section should be read
   with this in mind.
3. Q6 is correct and matches the capture: there's no portal type and no transfer calls, so Zen gets a
   plain `file://` URI it can't open outside `~/Downloads` [observed, P0b-results.md].
