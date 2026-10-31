# P0d: Dolphin drag-out notes
Pinned: dolphin v25.12.3 (5e0a9f6477424dcb3fdb8cdf1c9d5ebcba36424d), kcoreaddons v6.24.0 (50d360736c399502fedf203e95482b0d0e5a3ea2), kio v6.24.0 (c347b3cbc49480ba8977b75f1240d40b84a53c82), qtwayland v6.10.2 (4360cb966b57f51174f4dfdbb416ff48de068e24)

Q6's client-side Qt code is not in qtwayland v6.10.2: its `src/client` holds only `CMakeLists.txt` and `configure.cmake`, no drag sources (qtwayland:src/client/CMakeLists.txt:1-7) [observed]. The code is in qtbase v6.10.2 (000d6c62f7880bb8d3054724e8da0b8ae244130e), cited below as `qtbase:` [read].

## Answers (one line each, Q1–Q7, each with its tag and path:line)
- **Q1.** `KItemListController::startDragging()` creates the `QDrag` (dolphin:src/kitemviews/kitemlistcontroller.cpp:1415, :1434); `mouseMoveEvent()` reaches it at :639 after `QApplication::startDragDistance()` (:628); `draganddrophelper.cpp:37` is the drop/destination side only. [read]
- **Q2.** See table: Dolphin adds no MIME itself; KCoreAddons adds `text/uri-list` and `x-kde4-urilist` (kcoreaddons:src/lib/io/kurlmimedata.cpp:52, :55), then portal export adds `application/vnd.portal.filetransfer` and `application/x-kde-source-id` (:393-394); there is no `text/plain` format and no `x-kde-cutselection` on drag. [read]
- **Q3.** `KUrlMimeData::setUrls` is confirmed at kcoreaddons:src/lib/io/kurlmimedata.cpp:49; it writes the most-local list through `QMimeData::setUrls` to `text/uri-list` (:52) and the real URLs through `uriListData` to `application/x-kde4-urilist` (:55, :37-46); Dolphin supplies both lists (dolphin:src/kitemviews/kfileitemmodel.cpp:327, :330, :334). [read]
- **Q4.** `exportUrlsToPortal` is called unconditionally at drag start (dolphin:src/kitemviews/kitemlistcontroller.cpp:1430), gated only by portal availability (kcoreaddons:src/lib/io/kurlmimedata.cpp:285, :109-114) and by all URLs being local non-symlinks (:288-319); it calls `StartTransfer(autostop=false)` (:326), `AddFiles` in batches of 16 (:345, :353), later `StopTransfer` when the `QMimeData` dies (:386), and adds `application/vnd.portal.filetransfer` (:393) plus `application/x-kde-source-id` (:394). [read]
- **Q5.** Dolphin calls `drag->exec(Qt::MoveAction | Qt::CopyAction | Qt::LinkAction, Qt::CopyAction)` (dolphin:src/kitemviews/kitemlistcontroller.cpp:1443): supported actions are Move/Copy/Link and the default is Copy; Qt drops Link on Wayland (qtbase:src/plugins/platforms/wayland/qwaylanddatadevice.cpp:352-365). [read]
- **Q6.** `QWaylandDrag::startDrag` (qtbase:src/plugins/platforms/wayland/qwaylanddnd.cpp:29, :38) calls `QWaylandDataDevice::startDrag` (qtbase:.../qwaylanddatadevice.cpp:85), which constructs `QWaylandDataSource` (qtbase:.../qwaylanddatasource.cpp:22-31), offering every `QMimeData` format via `offer()` (:28-31), then calls `set_actions` (qtbase:.../qwaylanddatadevice.cpp:110); no rename, but Qt adds a placeholder or `text/plain;charset=utf-8` only in those cases (:98-105) and re-serializes `text/uri-list` on send (qtbase:.../shared/qwaylandmimehelper.cpp:37-42). [read]
- **Q7.** A GTK4 app must reproduce the most-local `text/uri-list` payload (verify GTK), optionally `application/x-kde4-urilist` for KDE consumers, and the FileTransfer portal dance plus `application/vnd.portal.filetransfer`; it must choose Copy|Move actions and a Copy default, while Qt's placeholder/utf8 additions and Link dropping are Qt-side, and `x-kde-cutselection` is not a drag format. [inferred]

## Drag-out call path (numbered, repo:path:line and what happens at each step)
1. dolphin:src/kitemviews/kitemlistcontroller.cpp:610, :628, :639 — `mouseMoveEvent()` sees the left button held and the manhattan distance at least `startDragDistance()`, then calls `startDragging()`. [read]
2. dolphin:src/kitemviews/kitemlistcontroller.cpp:1415-1426 — `startDragging()` takes the selection and asks the model for a `QMimeData`. [read]
3. dolphin:src/kitemviews/kfileitemmodel.cpp:300-334 — `createMimeData()` allocates `QMimeData`, collects `item.url()` and `item.mostLocalUrl()`, and calls `KUrlMimeData::setUrls()`. [read]
4. kcoreaddons:src/lib/io/kurlmimedata.cpp:52, :55 — `text/uri-list` gets the most-local URLs; `application/x-kde4-urilist` gets the real KIO URLs. [read]
5. dolphin:src/kitemviews/kitemlistcontroller.cpp:1430 — `KUrlMimeData::exportUrlsToPortal(data)` runs before the drag object exists. [read]
6. kcoreaddons:src/lib/io/kurlmimedata.cpp:285-326, :353, :386, :393-394 — portal availability is checked, `StartTransfer`/`AddFiles` register the files, `StopTransfer` is deferred to `QMimeData` destruction, and the portal key plus source-id are put into the MIME data. [read]
7. dolphin:src/kitemviews/kitemlistcontroller.cpp:1434-1441 — `new QDrag(QApplication::activeWindow())`, `setMimeData(data)`, `setPixmap()`, `setHotSpot()`. [read]
8. dolphin:src/kitemviews/kitemlistcontroller.cpp:1443 — `drag->exec(Move|Copy|Link, Copy)` starts the platform drag; the return value is ignored. [read]
9. qtbase:src/gui/kernel/qdrag.cpp:225-252 — `QDrag::exec()` records `supported_actions` and transforms the default action (Copy stays Copy). [read]
10. qtbase:src/plugins/platforms/wayland/qwaylanddnd.cpp:29-38 — `QWaylandDrag::startDrag()` calls the data device with the drag's MIME data, supported actions and icon window. [read]
11. qtbase:src/plugins/platforms/wayland/qwaylanddatadevice.cpp:85-110 — `QWaylandDataDevice::startDrag()` guards empty data and `text/plain`, constructs `QWaylandDataSource`, and calls `set_actions` if the protocol version is at least 3. [read]
12. qtbase:src/plugins/platforms/wayland/qwaylanddatasource.cpp:22-31 — the data-source constructor gets `QInternalMimeData::formatsHelper()` and emits one `offer(format)` per format, in `QMimeData::formats()` order. [read]
13. qtbase:src/plugins/platforms/wayland/qwaylanddatadevice.cpp:167 — `start_drag()` passes the source object, origin surface, icon surface and serial to the compositor. [read]
14. qtbase:src/plugins/platforms/wayland/qwaylanddatasource.cpp:44-98 — later `send` serializes requested data, `action` maps the compositor's action, and `drop`/`finished` update Qt. [read]
15. dolphin:src/views/draganddrophelper.cpp:37 — `DragAndDropHelper::dropUrls()` runs on the receiving side and calls `KIO::drop`; it is not part of drag-out. [read]

## MIME types offered
| MIME | Added by (Dolphin / KIO / KCoreAddons / Qt) | path:line | Payload format |
|---|---|---|---|
| `text/uri-list` | KCoreAddons, via `QMimeData::setUrls` | kcoreaddons:src/lib/io/kurlmimedata.cpp:52 | Most-local URL list; Qt writes each `QUrl::toEncoded()` plus CRLF (qtbase:src/plugins/platforms/wayland/shared/qwaylandmimehelper.cpp:37-42) [read] |
| `application/x-kde4-urilist` | KCoreAddons | kcoreaddons:src/lib/io/kurlmimedata.cpp:55, :37-46 | Real KIO URLs, each `toEncoded()` plus CRLF [read] |
| `application/vnd.portal.filetransfer` | KCoreAddons (portal export) | kcoreaddons:src/lib/io/kurlmimedata.cpp:393 | Transfer key as `QFile::encodeName(transferId)` [read] |
| `application/x-kde-source-id` | KCoreAddons (portal export) | kcoreaddons:src/lib/io/kurlmimedata.cpp:394, :160-162 | Session D-Bus base service name, UTF-8 [read] |
| `text/plain` | Not offered | qtbase:src/corelib/kernel/qmimedata.cpp:108-124 | No format entry; `text()` synthesizes display strings with `\n` only if read [read] |
| `application/x-kde-cutselection` | KIO, clipboard cut only | kio:src/widgets/paste.cpp:256-259; dolphin:src/views/dolphinview.cpp:841 | ASCII `"1"` or `"0"`; not present in a drag [read] |
| `application/vnd.portal.files` | Not offered | no occurrence in kcoreaddons/kio/dolphin at these tags | n/a [observed] |
| `application/x-qt-avoid-empty-placeholder` | Qt, only when the MIME data has no formats | qtbase:src/plugins/platforms/wayland/qwaylanddatadevice.cpp:98-99 | ASCII `"1"`; not triggered by Dolphin [read] |

## Portal export (trigger, D-Bus calls in order, MIME added)
- **Trigger.** `KItemListController::startDragging()` calls `KUrlMimeData::exportUrlsToPortal(data)` on every drag, before `QDrag` construction (dolphin:src/kitemviews/kitemlistcontroller.cpp:1430); there is no target/sandbox check and no opt-in flag. [read]
- **Guards.** It returns false unless `org.freedesktop.portal.Documents` is activatable (kcoreaddons:src/lib/io/kurlmimedata.cpp:285, :109-114); every URL must be a local file and not a symlink (:288-319), or a remote URL with `KCOREADDONS_FUSE_REDIRECT=1` and KIOFuse available (:293-311). [read]
- **Input.** It exports `mimeData->urls()`, i.e. the `text/uri-list` (most-local) list, not `application/x-kde4-urilist` (:288). [read]
- **D-Bus order.** On `org.freedesktop.portal.Documents` at `/org/freedesktop/portal/documents`, interface `org.freedesktop.portal.FileTransfer`: (1) `StartTransfer({"autostop": false})` returns the key (:321-326; xml:8-12); (2) `AddFiles(key, fds, {})` once per batch of at most 16, each call waited for (:345-378; xml:13-19); (3) `StopTransfer(key)` on `QMimeData` destruction, or via the failure scope guard (:327-329, :384-387; xml:26-28). [read]
- **Also watched.** `TransferClosed(key)` deletes the interface object (:389-391; xml:29-31); `RetrieveFiles` is the receiving side's call, implemented at :134, not called by Dolphin. [read]
- **MIME added.** `application/vnd.portal.filetransfer` = transfer key (:393), then `application/x-kde-source-id` = `QDBusConnection::sessionBus().baseService()` (:394, :160-162). It does not add `application/vnd.portal.files`. [read]

## Actions
- Dolphin advertises `Qt::MoveAction | Qt::CopyAction | Qt::LinkAction` and passes `Qt::CopyAction` as the default (dolphin:src/kitemviews/kitemlistcontroller.cpp:1443); the result of `exec()` is not inspected. [read]
- `QDrag::exec` stores those as `supported_actions` and `default_action`; with an explicit Copy default it does not fall back to Move (qtbase:src/gui/kernel/qdrag.cpp:225-252). [read]
- Wayland gets `set_actions(COPY|MOVE)` only: `dropActionsToWl` maps Copy and Move/TargetMove but has no Link mapping (qtbase:src/plugins/platforms/wayland/qwaylanddatadevice.cpp:352-365), and `set_actions` is only sent for protocol version 3 or newer (:110). [read]
- Incoming `wl_data_source.action` maps only Move and Copy; anything else becomes `Qt::IgnoreAction` (qtbase:src/plugins/platforms/wayland/qwaylanddatasource.cpp:76-86). [read]

## Surprises
- **Q6 lives in qtbase now.** qtwayland v6.10.2's `src/client` is only a feature module (qtwayland:src/client/CMakeLists.txt:1-7), while the data-source code is qtbase:src/plugins/platforms/wayland/qwaylanddatasource.cpp:22-31 [observed]; citing the old qtwayland path would have been wrong. [read]
- **Portal export is not target-aware.** Dolphin exports at every drag start, even to another local app; the only real conditions are portal availability, local regular files, and non-symlinks (kcoreaddons:src/lib/io/kurlmimedata.cpp:285-319). [read]
- **Symlinks get no portal export.** An explicitly symlinked URL is rejected outright (:314-318), so such a drag falls back to plain `text/uri-list` for sandboxed targets. [read]
- **`autostop` is false, not true.** The option is present but disabled (:326), and KDE stops the transfer only when the `QMimeData` dies (:384-387); P0b's shorthand “StartTransfer("autostop")” should not be read as enabling autostop. [read; P0b-results.md:24 observed]
- **No `text/plain` despite the comment.** `KUrlMimeData::setUrls` says it sets `text/uri-list` and `text/plain` (kcoreaddons:src/lib/io/kurlmimedata.cpp:52), but Qt 6.10.2's `setUrls` writes only `text/uri-list` (qtbase:src/corelib/kernel/qmimedata.cpp:356-360); `text/plain` exists only as an on-read fallback (:108-124). P0b's capture also saw no `text/plain` from Dolphin [observed P0b-results.md:13]. [read]
- **KDE does not offer `application/vnd.portal.files`.** Only `application/vnd.portal.filetransfer`; the `.files` type seen in P0b came from Nautilus [observed P0b-results.md:14], and it has no occurrence in the three KDE repos. [read]
- **Dolphin's Link action never reaches Wayland.** Qt's mapping silently omits it (qtbase:.../qwaylanddatadevice.cpp:363-364), so link is only a Qt-level hint. [read]
- **Dolphin ignores the drop result.** `drag->exec(...)` at :1443 is a statement, so the drag-start path itself does not delete or move anything based on the negotiated action. [read]

## Not found / unsure (what you searched for)
- Searched `vnd.portal.files` across `kcoreaddons/src`, `kio/src` and `dolphin/src`: no hit at these tags; only `application/vnd.portal.filetransfer` exists in KCoreAddons. [observed]
- Searched `exportUrlsToPortal` across `kio/src`: no hit; the implementation is KCoreAddons-only. [observed]
- Searched `x-kde-cutselection` across the three KDE repos: only the KIO clipboard helpers (kio:src/widgets/paste.cpp:256-259) and Dolphin's clipboard-cut call (dolphin:src/views/dolphinview.cpp:840-841); nothing in `startDragging()`. [read]
- Searched `QDrag`, `startDragging` and `createMimeData` in `dolphin/src/views/draganddrophelper.cpp`: none; that file only handles incoming drops (`dropUrls`, :37). [read]
- qtwayland v6.10.2 has no `qwaylanddatasource.cpp` or `qwaylanddatadevice.cpp`; its `src/client` has only build files (qtwayland:src/client/CMakeLists.txt:1-7), so Q6 uses qtbase v6.10.2. The qtbase commit is `000d6c62f7880bb8d3054724e8da0b8ae244130e`, tag `v6.10.2`; I read the files at that tag, not from a distro patch stack. [observed]
- I did not inspect Qt's compositor-side code, Zen, or run a drag; the brief forbids app code and drags, and P0e owns GTK4 behavior. [inferred]
- `application/vnd.portal.files` behavior and whether GDK already exports portal types remain open for P0e/P0b; this note only establishes that the KDE stack does not. [inferred]

## Review (Claude, Oct 31, 2026)

**Verdict: accepted.** Spot-checked and correct: kurlmimedata.cpp:52, :55, :285, :314–318, :324–326
and :393–394, and kitemlistcontroller.cpp:1415, :1430 and :1443. The KDE clones are clean at the
recorded commits. The offer list matches the capture exactly, in order (P0b-results.md).

Notes:

1. qtbase v6.10.2 isn't cloned in `gimbal-refs`. The agent read it online. The tag peels to
   `000d6c62f7880bb8d3054724e8da0b8ae244130e` as recorded (checked with `git ls-remote`), and
   qwaylanddatasource.cpp:22–31, qwaylanddatadevice.cpp:98–110, :352–365 and qmimedata.cpp:356–360
   match the files at that tag [read by Claude]. Clone it into `gimbal-refs` before anyone cites it
   again.
2. The `autostop` point is right. Dolphin calls `StartTransfer` with `autostop: false` and stops the
   transfer itself when the QMimeData is destroyed. The capture summary prints only option *names*,
   so P0b-results.md's "StartTransfer("autostop")" was ambiguous. That file is now corrected.
3. Worth carrying into P0h: Dolphin exports to the portal on **every** drag, regardless of target, and
   gives symlinks no portal export (kurlmimedata.cpp:314–318). Gimbal Files needs a policy for both.
