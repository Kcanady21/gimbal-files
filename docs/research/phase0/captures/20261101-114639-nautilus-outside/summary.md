# Drag-and-drop capture: 20261101-114639-nautilus-outside

Source app: nautilus — location: outside — Zen opened the file: yes
Note: auto:true source=nautilus location=outside file=sample.png folder=/home/kyle/gimbal-dnd-test

## Source side (nautilus)

- Drag source: wl_data_source#75 — attempt 1 of 1 (chosen by wl_data_device.start_drag)
- Offers (in order): application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, application/vnd.portal.filetransfer, application/vnd.portal.files
- set_actions: copy|move|ask (7)
- send: text/uri-list (fd 34); application/vnd.portal.files (fd 34)
- action events: move; none; move (2, 0, 2)
- Ending: dnd_drop_performed+dnd_finished

## Zen side

- Drag offer: wl_data_offer#4278190081 (from the last wl_data_device.enter in the log)
- attempts Zen saw: 1
- drop after enter: yes
- drop count: 1
- enter: serial 241413 surface wl_surface#66 offer wl_data_offer#4278190081
- leave: serial None
- drop: serial None
- Offers on the drag offer: application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, application/vnd.portal.filetransfer, application/vnd.portal.files
- source_actions: copy|move|ask (7)
- action: move (2)
- accept: serial 241413 nil; serial 241388 text/uri-list; serial 241413 nil; serial 241388 application/vnd.portal.files; serial 241413 nil; serial 241413 application/vnd.portal.files; serial 241413 application/vnd.portal.files; serial 241413 application/vnd.portal.files; serial 241413 application/vnd.portal.files; serial 241413 application/vnd.portal.files; serial 241413 application/vnd.portal.files; serial 241413 application/vnd.portal.files; serial 241413 application/vnd.portal.files; serial 241413 application/vnd.portal.files; serial 241413 application/vnd.portal.files; serial 241413 application/vnd.portal.files; serial 241413 application/vnd.portal.files
- set_actions: none; none (0, 0)
- receive (MIME Zen read): text/uri-list (fd 142); application/vnd.portal.files (fd 142)
- finish: yes

## Portal D-Bus (5 calls)

1. **GetMountPoint** — caller :1.734 — returned: "/run/user/1000/doc"
2. **StartTransfer** — caller :1.737 — args: "writable", "autostop" — returned: "183978891178632150727884522130577055197"
3. **AddFiles** — caller :1.737 — args: "183978891178632150727884522130577055197"
4. **RetrieveFiles** — caller :1.736 — args: "183978891178632150727884522130577055197" — returned: "/run/user/1000/doc/c8d47624/sample.png"
5. **TransferClosed** — caller :1.37 — args: "183978891178632150727884522130577055197"
