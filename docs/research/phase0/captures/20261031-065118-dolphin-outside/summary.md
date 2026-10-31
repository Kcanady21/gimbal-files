# Drag-and-drop capture: 20261031-065118-dolphin-outside

Source app: dolphin — location: outside — Zen opened the file: yes
Note: auto:true source=dolphin location=outside file=sample.png folder=/home/kyle/gimbal-dnd-test

## Source side (dolphin)

- Drag source: wl_data_source#39 — attempt 1 of 1 (chosen by wl_data_device.start_drag)
- Offers (in order): text/uri-list, application/x-kde4-urilist, application/vnd.portal.filetransfer, application/x-kde-source-id
- set_actions: copy|move (3)
- send: text/uri-list (fd 49); application/vnd.portal.filetransfer (fd 49)
- action events: copy; none; move (1, 0, 2)
- Ending: dnd_drop_performed+dnd_finished

## Zen side

- Drag offer: wl_data_offer#4278190082 (from the last wl_data_device.enter in the log)
- attempts Zen saw: 1
- drop after enter: yes
- drop count: 1
- enter: serial 543650 surface wl_surface#66 offer wl_data_offer#4278190082
- leave: serial None
- drop: serial None
- Offers on the drag offer: text/uri-list, application/x-kde4-urilist, application/vnd.portal.filetransfer, application/x-kde-source-id
- source_actions: copy|move (3)
- action: move (2)
- accept: serial 543650 nil; serial 543616 text/uri-list; serial 543650 nil; serial 543616 application/vnd.portal.filetransfer; serial 543650 nil; serial 543650 application/x-kde-source-id; serial 543650 application/x-kde-source-id; serial 543650 application/x-kde-source-id; serial 543650 application/x-kde-source-id; serial 543650 application/x-kde-source-id; serial 543650 application/x-kde-source-id; serial 543650 application/x-kde-source-id; serial 543650 application/x-kde-source-id; serial 543650 application/x-kde-source-id; serial 543650 application/x-kde-source-id; serial 543650 application/x-kde-source-id; serial 543650 application/x-kde-source-id
- set_actions: none; none (0, 0)
- receive (MIME Zen read): text/uri-list (fd 145); application/vnd.portal.filetransfer (fd 145)
- finish: yes

## Portal D-Bus (6 calls)

1. **GetMountPoint** — caller :1.1151 — returned: "/run/user/1000/doc"
2. **StartTransfer** — caller :1.1154 — args: "autostop" — returned: "169421091064013600573210775999417796461"
3. **AddFiles** — caller :1.1154 — args: "169421091064013600573210775999417796461"
4. **RetrieveFiles** — caller :1.1153 — args: "169421091064013600573210775999417796461" — returned: "/run/user/1000/doc/7fb4b484/sample.png"
5. **StopTransfer** — caller :1.1154 — args: "169421091064013600573210775999417796461"
6. **TransferClosed** — caller :1.261 — args: "169421091064013600573210775999417796461"
