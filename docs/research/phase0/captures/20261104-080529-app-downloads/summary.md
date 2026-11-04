# Drag-and-drop capture: 20261104-080529-app-downloads

Source app: app — location: downloads — Zen opened the file: yes
Note: auto:true source=gimbal location=downloads file=sample.png folder=/home/kyle/Downloads/gimbal-dnd-test-gimbal

## Source side (app)

- Drag source: wl_data_source#69 — attempt 1 of 1 (chosen by wl_data_device.start_drag)
- Offers (in order): application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, application/vnd.portal.filetransfer, application/vnd.portal.files
- set_actions: copy|move (3)
- send: text/uri-list (fd 29); application/vnd.portal.files (fd 29)
- action events: none; move (0, 2)
- Ending: dnd_drop_performed+dnd_finished

## Zen side

- Drag offer: wl_data_offer#4278190080 (from the last wl_data_device.enter in the log)
- attempts Zen saw: 1
- drop after enter: yes
- drop count: 1
- enter: serial 210799 surface wl_surface#66 offer wl_data_offer#4278190080
- leave: serial None
- drop: serial None
- Offers on the drag offer: application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, application/vnd.portal.filetransfer, application/vnd.portal.files
- source_actions: copy|move (3)
- action: move (2)
- accept: serial 210799 nil; serial 210775 text/uri-list; serial 210799 nil; serial 210775 application/vnd.portal.files; serial 210799 nil; serial 210799 application/vnd.portal.files; serial 210799 application/vnd.portal.files; serial 210799 application/vnd.portal.files; serial 210799 application/vnd.portal.files; serial 210799 application/vnd.portal.files; serial 210799 application/vnd.portal.files; serial 210799 application/vnd.portal.files; serial 210799 application/vnd.portal.files; serial 210799 application/vnd.portal.files; serial 210799 application/vnd.portal.files; serial 210799 application/vnd.portal.files; serial 210799 application/vnd.portal.files; serial 210799 application/vnd.portal.files
- set_actions: none; none (0, 0)
- receive (MIME Zen read): text/uri-list (fd 142); application/vnd.portal.files (fd 142)
- finish: yes

## Portal D-Bus (5 calls)

1. **GetMountPoint** — caller :1.335 — returned: "/run/user/1000/doc"
2. **StartTransfer** — caller :1.338 — args: "writable", "autostop" — returned: "160400939498114028329882466219688609997"
3. **AddFiles** — caller :1.338 — args: "160400939498114028329882466219688609997"
4. **RetrieveFiles** — caller :1.337 — args: "160400939498114028329882466219688609997" — returned: "/home/kyle/Downloads/gimbal-dnd-test-gimbal/sample.png"
5. **TransferClosed** — caller :1.12 — args: "160400939498114028329882466219688609997"
