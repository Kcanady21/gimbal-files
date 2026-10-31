# P0b results: the capture matrix (Claude, Oct 31, 2026)

All 8 rows were run with `tools/dnd-capture/run-matrix.sh` (automated drags, P0a2), between 07:02 and
07:04 on Oct 31, 2026. Each row drags `sample.png` into an empty tab of a throwaway Zen (Flatpak
`app.zen_browser.zen` 1.24b, profile `gimbal-dnd`). niri 26.04, single output DP-2 at 2560×1440. The
throwaway Zen and the source app were cleaned up after every run, and both test folders still hold
both files afterwards.

## Results

| source | `outside` (~/gimbal-dnd-test) | `downloads` (~/Downloads/gimbal-dnd-test) | what the source offers |
|---|---|---|---|
| Dolphin 25.12.3 (KF6/Qt) | **y** | y | text/uri-list, application/x-kde4-urilist, **application/vnd.portal.filetransfer**, application/x-kde-source-id |
| Nautilus 50.2.2 (GTK4) | **y** | y | application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, **application/vnd.portal.filetransfer**, **application/vnd.portal.files** |
| Thunar 4.20.7 (GTK3) | **other: "Problem loading page"** | y | text/uri-list (twice), DELETE |
| COSMIC Files (git 09bdd825) | **other: "Problem loading page"** | y | text/plain, text/plain;charset=utf-8, UTF8_STRING, text/uri-list, x-special/gnome-copied-files |

`y` means Zen's title became "sample.png (PNG Image, 16 × 16 pixels) — Zen Browser". Every drop
completed at the protocol level: `dnd_drop_performed` + `dnd_finished`, Zen called `finish`, and the
negotiated action was `move` (2) in all 8 rows.

## What Zen read, and the portal

- **Dolphin:** Zen received `text/uri-list` and `application/vnd.portal.filetransfer`. Dolphin then
  called StartTransfer (options: `autostop`, which KCoreAddons sets to **false** per P0d; the capture
  summary lists option names only) and AddFiles, and Zen called RetrieveFiles, which returned
  `/run/user/1000/doc/7fb4b484/sample.png` for `outside` and the real path for `downloads`.
- **Nautilus:** Zen preferred `application/vnd.portal.files` over `…filetransfer`. The transfer used
  StartTransfer("writable", "autostop"), and RetrieveFiles returned the same document-portal path for
  `outside`.
- **Thunar and COSMIC:** Zen read only `text/uri-list` (plus `text/plain` from COSMIC), and there were
  no transfer calls. That URI, `file:///home/kyle/gimbal-dnd-test/sample.png`, appears in zen.log
  (9 times in each row). The sandbox can't see that path, which fits Zen's "Problem loading page"
  title. From `downloads` the same URI works, because the
  sandbox mounts `~/Downloads`.

## Verdict on the hypothesis

**It holds.** A drag into sandboxed Zen works from outside `~/Downloads` exactly when the source offers
a portal MIME type (`application/vnd.portal.filetransfer` or `application/vnd.portal.files`) and
registers the file with the FileTransfer portal. A source that offers only a plain URI fails outside
the sandbox's mounts. That's COSMIC Files' bug, and Thunar has it too.

For Gimbal Files: offer `application/vnd.portal.files` and `application/vnd.portal.filetransfer`
alongside `text/uri-list`, and answer them through `org.freedesktop.portal.FileTransfer`. Nautilus is
the model to follow.

Worth a look in P0g/P0h:

- The `move` action. The sources advertised copy|move, and Zen settled on move, but no source deleted
  anything. Gimbal Files must never treat a drop into a browser as a move.
- Nautilus doesn't call StopTransfer, but Dolphin does. Dolphin passes `autostop: false` and stops the
  transfer itself when its QMimeData is destroyed (P0d). Nautilus passes `writable` and `autostop`
  (values not shown in the capture; P0e should confirm them).

## Which captures count

The data runs are the 8 matrix rows:

```
20261031-070243-dolphin-outside    20261031-070328-dolphin-downloads
20261031-070255-thunar-outside     20261031-070340-thunar-downloads
20261031-070306-nautilus-outside   20261031-070351-nautilus-downloads
20261031-070317-cosmic-outside     20261031-070402-cosmic-downloads
```

`20261031-065118-dolphin-outside` is an earlier successful Dolphin run from the live check. Its result
is the same as row 1. Every other run from 06:47 to 06:53 is a calibration run with an `aborted:` line
in its result.txt, and none of them count.
