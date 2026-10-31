# Drag and drop: what works and why (Phase 0)

Captured 2026-10-31 (data runs at 06:51 and 07:02–07:04 EDT). Versions from the captures' `meta.txt`:
Zen 1.24b (throwaway profile `gimbal-dnd`, `MOZ_LOG` module `WidgetDrag`), Dolphin 25.12.3, Thunar
4.20.7 (Xfce 4.20), GNOME Nautilus 50.2.2, cosmic-files git 09bdd8251d5f, niri 26.04. P0g gives
Zen's Firefox base as 159.0.

Notes: [P0c Thunar](phase0/P0c-thunar-notes.md) · [P0d Dolphin](phase0/P0d-dolphin-notes.md) · [P0e Nautilus/GTK4](phase0/P0e-nautilus-notes.md) ·
[P0f COSMIC](phase0/P0f-cosmic-notes.md) · [P0g Zen/Firefox](phase0/P0g-zen-receiver-notes.md) · [P0b capture matrix](phase0/P0b-results.md). Raw captures: `phase0/captures/`.

## Summary

A working drag into sandboxed Zen from outside its mounts offers `text/uri-list` plus `application/vnd.portal.files` and/or
`application/vnd.portal.filetransfer`, answered through a real `org.freedesktop.portal.FileTransfer` transfer
[observed: Dolphin, Nautilus]. COSMIC Files offers neither portal type and never calls
`StartTransfer`/`AddFiles`, so Zen gets only a plain URI and fails outside `~/Downloads` [observed]. The action is not
the problem: every row negotiated `move`, because Firefox rewrites its preference to MOVE on Wayland [observed + read].
Gimbal Files must offer a `gdk::FileList` with copy|move actions, letting GTK4 add both portal types itself, and must
never delete a source file when a drop reports "move" [read].

## What each source offered

Every row ended `dnd_drop_performed` + `dnd_finished`, Zen `finish` = true, and Zen negotiated action `move` (mask 2).
`065118` is the earlier live Dolphin run; P0b says it counts and matches the matrix's Dolphin `outside` row.

| Source | Location | MIME types offered (order) | Actions | Zen read (receive) | Portal calls | Result |
|---|---|---|---|---|---|---|
| Dolphin 25.12.3 (KF6/Qt), `065118` | outside | `text/uri-list`, `application/x-kde4-urilist`, `application/vnd.portal.filetransfer`, `application/x-kde-source-id` | copy, move | `text/uri-list`, `application/vnd.portal.filetransfer` | GetMountPoint; StartTransfer(`autostop`); AddFiles; RetrieveFiles → `/run/user/1000/doc/…`; StopTransfer; TransferClosed | y |
| Dolphin 25.12.3 (KF6/Qt), `070243` | outside | `text/uri-list`, `application/x-kde4-urilist`, `application/vnd.portal.filetransfer`, `application/x-kde-source-id` | copy, move | `text/uri-list`, `application/vnd.portal.filetransfer` | GetMountPoint; StartTransfer(`autostop`); AddFiles; RetrieveFiles → `/run/user/1000/doc/…`; StopTransfer; TransferClosed | y |
| Dolphin 25.12.3 (KF6/Qt), `070328` | downloads | `text/uri-list`, `application/x-kde4-urilist`, `application/vnd.portal.filetransfer`, `application/x-kde-source-id` | copy, move | `text/uri-list`, `application/vnd.portal.filetransfer` | GetMountPoint; StartTransfer(`autostop`); AddFiles; RetrieveFiles → real path; StopTransfer; TransferClosed | y |
| Thunar 4.20.7 (GTK3), `070255` | outside | `text/uri-list`, `text/uri-list`, `DELETE` | copy, move | `text/uri-list` | GetMountPoint only | other: "Problem loading page" |
| Thunar 4.20.7 (GTK3), `070340` | downloads | `text/uri-list`, `text/uri-list`, `DELETE` | copy, move | `text/uri-list` | GetMountPoint only | y |
| Nautilus 50.2.2 (GTK4), `070306` | outside | `application/x-gtk-local-dnd`, `text/plain;charset=utf-8`, `text/uri-list`, `application/vnd.portal.filetransfer`, `application/vnd.portal.files` | copy, move, ask | `text/uri-list`, `application/vnd.portal.files` | GetMountPoint; StartTransfer(`writable`, `autostop`); AddFiles; RetrieveFiles → `/run/user/1000/doc/…`; TransferClosed | y |
| Nautilus 50.2.2 (GTK4), `070351` | downloads | `application/x-gtk-local-dnd`, `text/plain;charset=utf-8`, `text/uri-list`, `application/vnd.portal.filetransfer`, `application/vnd.portal.files` | copy, move, ask | `text/uri-list`, `application/vnd.portal.files` | GetMountPoint; StartTransfer(`writable`, `autostop`); AddFiles; RetrieveFiles → real path; TransferClosed | y |
| COSMIC Files 09bdd825, `070317` | outside | `text/plain`, `text/plain;charset=utf-8`, `UTF8_STRING`, `text/uri-list`, `x-special/gnome-copied-files` | copy, move | `text/uri-list`, `text/plain` | GetMountPoint only | other: "Problem loading page" |
| COSMIC Files 09bdd825, `070402` | downloads | `text/plain`, `text/plain;charset=utf-8`, `UTF8_STRING`, `text/uri-list`, `x-special/gnome-copied-files` | copy, move | `text/uri-list`, `text/plain` | GetMountPoint only | y |

"Actions" is the source's `set_actions` from summary.json. Portal calls are every call in `dbus.log`; StartTransfer option *names*, not values, are captured (see gaps).

## Who adds what

Each cell is `app`, `toolkit`, `—` (not offered) or `unknown`.

| MIME | Thunar/GTK3 | Dolphin/KF6/Qt | Nautilus/GTK4 | COSMIC | Needed by Zen? |
|---|---|---|---|---|---|
| `text/uri-list` | app[^c] | toolkit[^d] | toolkit[^e] | app[^f] | yes[^g] |
| `application/vnd.portal.filetransfer` | —[^c] | toolkit[^d] | toolkit[^e] | —[^f] | yes[^g] |
| `application/vnd.portal.files` | —[^c] | —[^d] | toolkit[^e] | —[^f] | yes (preferred)[^g] |
| `text/plain;charset=utf-8` | —[^c] | —[^d] | toolkit[^e] | app[^f] | no[^g] |
| `text/plain` | —[^c] | —[^d] | —[^e] | app[^f] | no[^g] |
| `UTF8_STRING` | —[^c] | —[^d] | —[^e] | app[^f] | no[^g] |
| `x-special/gnome-copied-files` | —[^c] | —[^d] | —[^e] | app[^f] | no[^g] |
| `application/x-kde4-urilist` | —[^c] | toolkit[^d] | —[^e] | —[^f] | no[^g] |
| `application/x-kde-source-id` | —[^c] | toolkit[^d] | —[^e] | —[^f] | no[^g] |
| `application/x-gtk-local-dnd` | —[^c] | —[^d] | toolkit[^e] | —[^f] | no[^g] |
| `DELETE` | toolkit[^c2] | —[^d] | —[^e] | —[^f] | no[^g] |

[^c]: P0c, "MIME types offered".
[^c2]: P0c, Review (Claude), correction 1: GTK3 re-offers the source targets plus `DELETE` after `start_drag`.
[^d]: P0d, "MIME types offered"; portal rows, "Portal export".
[^e]: P0e, "MIME types offered"; portal rows, "Portal".
[^f]: P0f, "MIME types advertised"; portal rows, "Portal".
[^g]: P0g, "Flavor preference order" and "Minimal set to offer".

## Minimal set for Gimbal Files

- `text/uri-list`, with percent-encoded `file://` URIs, one per line, each ending `\r\n`. GTK4
  provides this from a `gdk::FileList` [observed: present and read in all nine rows; read: P0e].
- `application/vnd.portal.files` and `application/vnd.portal.filetransfer`, answered by a real
  `StartTransfer` + `AddFiles` session. GTK4 provides both from the same `FileList` [observed:
  Nautilus; read: P0e]. Only one is strictly needed; Firefox checks `.files` first [read: P0g].
- Actions: advertise copy and move. Zen will still report MOVE [observed: all nine rows; read: P0g].
- `text/plain`, `UTF8_STRING`, `x-special/gnome-copied-files`, the KDE types,
  `application/x-gtk-local-dnd` and `DELETE` are extras, not requirements [read: P0g; P0e].

Three required items from the reviews:

1. **When the portal types are offered at all.** GTK4 registers the portal serializers once at
   startup, only when the portal is reachable (probe of `org.freedesktop.portal.Documents`), and an
   unsandboxed app qualifies when `environment_has_portals()` is true — it is on Kyle's desktop
   [read: P0e Review 1]. If the portal is not reachable, do not advertise the portal types: a receiver
   that prefers them can pick one and get nothing instead of the working URI [read: P0f Review 2].
   With a `gdk::FileList`, GTK4 enforces both points; Gimbal must not add the MIMEs by hand. A portal
   that starts after `gtk_init` is never picked up [read: P0e].
2. **The move/delete rule.** Firefox rewrites its action to MOVE on every Wayland drop, so every
   source sees "move" [observed: all nine rows; read: P0g Review 1]. GTK3 then asks the source to
   delete via a `DELETE` selection request [read: P0c Review 2], and GTK4 emits `drag-end` with
   `delete_data = TRUE` [read: P0e Review 2]. Thunar refuses, and no source deleted anything in the
   captures — both test folders still held both files afterwards [observed: P0b]. Gimbal Files must
   not connect a delete action to `drag-end` and must never delete on an outgoing move [inferred].
3. **What Dolphin does differently.** Dolphin exports to the portal on every drag, regardless of
   target, and gives symlinks no portal export [read: P0d Review 3]. Do not copy this. Follow
   Nautilus/GTK4 instead: nothing is exported until the receiver requests a MIME, and only files with
   a local path are exported [read: P0e, drag-out path step 7 and "Portal"]. The inputs do not say
   what GTK4 does with a symlink; that is a gap.

## Why COSMIC Files fails

COSMIC's drag offers five types and none is a portal type: `text/plain`, `text/plain;charset=utf-8`, `UTF8_STRING`,
`text/uri-list`, `x-special/gnome-copied-files` [observed: `070317`, `070402`; read: `cosmic-files:src/clipboard.rs:27-33`].
Zen therefore reads only `text/uri-list` (and `text/plain`) and makes no `StartTransfer`/`AddFiles` call [observed]. From
`~/gimbal-dnd-test` the plain URI `file:///home/kyle/gimbal-dnd-test/sample.png` is outside the Flatpak's mounts, Zen's
`nsIFile::Exists()` check fails, and the tab loads the URI as a page — "Problem loading page" [observed: `070317`; read:
P0g]. From `~/Downloads/gimbal-dnd-test` the same URI works because the sandbox mounts `~/Downloads` [observed: `070402`].
The source confirms it: cosmic-files has no portal code, libcosmic's FileTransfer helper is receive-side and behind a
feature cosmic-files does not enable, and smithay-clipboard/window_clipboard pass the offer list through verbatim
[read: P0f]. COSMIC's failure is the same source-side pattern as Thunar's, not a compositor bug [observed + read].

## Small fix or deep fix?

**Small.** The fix is one crate and one file, `cosmic-files:src/clipboard.rs`: add
`application/vnd.portal.filetransfer` and `application/vnd.portal.files` to the list in
`available()` (`:27-33`), and add an `as_bytes()` arm (`:95-104`) that starts a read-only/autostop
`ashpd` FileTransfer transfer, adds the files, and returns the key bytes. Add `ashpd` to
`cosmic-files/Cargo.toml`. Guard advertisement on the portal being reachable. That is roughly 40–60
new lines [inferred: P0f; accepted in P0f Review]. No toolkit or protocol change is needed because
every layer already passes `available()` through unchanged [read: P0f]. The upstream WIP PR #1586
(`61e5e1e`, +137/−77 in that file) is the same fix [read: P0f Review].

## Disagreements and gaps

- **Thunar's offer list.** P0c's body says one target (`text/uri-list`) and that GTK3 adds no MIME on
  the DnD path; the capture shows `text/uri-list`, `text/uri-list`, `DELETE` [observed: `070255`,
  `070340`]. P0c Review 1 corrects this; the report follows the capture.
- **StartTransfer option values.** summary.json records option *names* only. P0b's
  `StartTransfer("autostop")` for Dolphin is therefore ambiguous; P0d says the value is false, and
  Nautilus passes `writable` and `autostop` (P0e says both true). No capture shows the values.
- **`application/x-kde-cutselection`.** The Phase 0 plan expected it as a Dolphin drag extra; P0d
  finds it clipboard-only and no capture shows it in a drag [read + observed].
- **Missing evidence.** No capture covers a symlink, a non-native `GFile` (e.g. `trash:`), a
  directory, or a multi-file drag; none covers a non-sandboxed Zen; none runs the negative control
  with the portal stopped (proposed in P0f); and no capture tests a receiver other than Zen.
- **Offer order** comes from P0b's capture column, not an independent sniff [read: P0f].
- **`065118`** is outside the eight-run matrix; P0b says it counts and matches the Dolphin `outside` row, so it is a ninth row.

## Effect on Phase 1

- Drag source: build a `gdk::FileList` from the selection, convert it to a `Value`, and return
  `gdk::ContentProvider::for_value(&value)` from `connect_prepare`; set actions `COPY | MOVE`. Do not add
  `text/uri-list` or the portal MIMEs by hand — GTK4 adds them, and only offers the portal types when the
  portal was reachable at startup [read: P0e].
- Do not connect `drag-end` to a delete/move operation; ignore `delete_data` [read: P0e Review 2].
- Add acceptance tests: drag from `~/Documents` (outside `~/Downloads`), not only `~/Downloads`; drag a symlink; drag
  several files and a folder; with the portal stopped, confirm `~/Downloads` still works via `text/uri-list`; confirm
  `RetrieveFiles` returns `/run/user/1000/doc/…`; confirm the source files still exist after a Zen drop.

## Review (Claude, Oct 31, 2026)

**Verdict: accepted. Phase 0 is done.** Every row of "What each source offered" matches its
summary.json: offers in order, `set_actions`, Zen's action and receives, portal calls, ending and
result. That's all nine rows, and `065118` is correctly counted as the ninth. The minimal set tags
each item [observed] or [read]/[inferred], and all three review items are there. The "small" verdict
agrees with P0f and the captures. The report is 145 lines, within the limit, and P0h wrote only this
file. It didn't touch the plan.

One addition: ignoring `delete_data` costs Gimbal Files nothing for its own moves. In Nautilus and
Thunar the *drop* side performs a move between folders (P0c "Actions", P0e (b)), so the source never
needs to delete anything after a drag.

### Plan edits proposed to Kyle (all seven applied Oct 31, at his request)

To `Rust File Manager Plan for Claude Code.md`:

1. Context, "Reference behavior": Thunar fails too. It works only from `~/Downloads` and fails from
   other folders, the same way COSMIC Files does. Only Dolphin and Nautilus work from anywhere.
2. Decisions table, COSMIC row: the cause is known. Two portal MIME types and the FileTransfer call
   are missing from `src/clipboard.rs`. It's a small fix, and upstream draft PR
   pop-os/cosmic-files#1586 is that fix.
3. Phase 0: mark it done, and link this report.
4. Phase 1 prototype: the `DragSource` returns `gdk::ContentProvider::for_value` of a `gdk::FileList`
   with actions COPY|MOVE. Don't add MIME types by hand. Never delete on `drag-end`.
5. Phase 1 acceptance tests: drag into Zen from `~/Documents`, not only `~/Downloads`; check the
   source file still exists after a Zen drop; drag several files and a folder; drag a symlink. Run
   the Zen rows with the P0a2 runner (`run.sh --auto`, with a Gimbal row added) instead of by hand.
6. Phase 1, "patched COSMIC Files": that's a local build of #1586 (`61e5e1e`). It's the P0i
   decision.
7. Rules for agents: "An outgoing drag never deletes or moves the source; moves are done by the drop
   side."
