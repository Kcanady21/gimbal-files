# Drag-and-drop capture: 20261101-114702-dolphin-downloads

Source app: dolphin — location: downloads — Zen opened the file: yes
Note: auto:true source=dolphin location=downloads file=sample.png folder=/home/kyle/Downloads/gimbal-dnd-test

## Source side (dolphin)

- Drag source: wl_data_source#51 — attempt 1 of 1 (chosen by wl_data_device.start_drag)
- Offers (in order): text/uri-list, application/x-kde4-urilist, application/vnd.portal.filetransfer, application/x-kde-source-id
- set_actions: copy|move (3)
- send: text/uri-list (fd 52); application/vnd.portal.filetransfer (fd 52)
- action events: copy; none; move (1, 0, 2)
- Ending: dnd_drop_performed+dnd_finished

## Zen side

- Drag offer: wl_data_offer#4278190082 (from the last wl_data_device.enter in the log)
- attempts Zen saw: 1
- drop after enter: yes
- drop count: 1
- enter: serial 241549 surface wl_surface#66 offer wl_data_offer#4278190082
- leave: serial None
- drop: serial None
- Offers on the drag offer: text/uri-list, application/x-kde4-urilist, application/vnd.portal.filetransfer, application/x-kde-source-id
- source_actions: copy|move (3)
- action: move (2)
- accept: serial 241549 nil; serial 241524 text/uri-list; serial 241549 nil; serial 241524 application/vnd.portal.filetransfer; serial 241549 nil; serial 241549 application/x-kde-source-id; serial 241549 application/x-kde-source-id; serial 241549 application/x-kde-source-id; serial 241549 application/x-kde-source-id; serial 241549 application/x-kde-source-id; serial 241549 application/x-kde-source-id; serial 241549 application/x-kde-source-id; serial 241549 application/x-kde-source-id; serial 241549 application/x-kde-source-id; serial 241549 application/x-kde-source-id; serial 241549 application/x-kde-source-id; serial 241549 application/x-kde-source-id
- set_actions: none; none (0, 0)
- receive (MIME Zen read): text/uri-list (fd 142); application/vnd.portal.filetransfer (fd 142)
- finish: yes

## Portal D-Bus (6 calls)

1. **GetMountPoint** — caller :1.760 — returned: "/run/user/1000/doc"
2. **StartTransfer** — caller :1.763 — args: "autostop" — returned: "482248033726888154714500944320547359799"
3. **AddFiles** — caller :1.763 — args: "482248033726888154714500944320547359799"
4. **RetrieveFiles** — caller :1.762 — args: "482248033726888154714500944320547359799" — returned: "/home/kyle/Downloads/gimbal-dnd-test/sample.png"
5. **StopTransfer** — caller :1.763 — args: "482248033726888154714500944320547359799"
6. **TransferClosed** — caller :1.37 — args: "482248033726888154714500944320547359799"
