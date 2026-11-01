# Drag-and-drop capture: 20261101-114559-gimbal-downloads

Source app: gimbal — location: downloads — Zen opened the file: yes
Note: auto:true source=gimbal location=downloads file=sample.png,sample.txt folder=/home/kyle/Downloads/gimbal-dnd-test-gimbal

## Source side (gimbal)

- Drag source: wl_data_source#53 — attempt 1 of 1 (chosen by wl_data_device.start_drag)
- Offers (in order): application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, application/vnd.portal.filetransfer, application/vnd.portal.files
- set_actions: copy|move (3)
- send: text/uri-list (fd 23); application/vnd.portal.files (fd 23)
- action events: copy; none; move (1, 0, 2)
- Ending: dnd_drop_performed+dnd_finished

## Zen side

- Drag offer: wl_data_offer#4278190082 (from the last wl_data_device.enter in the log)
- attempts Zen saw: 1
- drop after enter: yes
- drop count: 1
- enter: serial 241210 surface wl_surface#66 offer wl_data_offer#4278190082
- leave: serial None
- drop: serial None
- Offers on the drag offer: application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, application/vnd.portal.filetransfer, application/vnd.portal.files
- source_actions: copy|move (3)
- action: move (2)
- accept: serial 241210 nil; serial 241187 text/uri-list; serial 241210 nil; serial 241187 application/vnd.portal.files; serial 241210 nil; serial 241210 application/vnd.portal.files; serial 241210 application/vnd.portal.files; serial 241210 application/vnd.portal.files; serial 241210 application/vnd.portal.files; serial 241210 application/vnd.portal.files; serial 241210 application/vnd.portal.files; serial 241210 application/vnd.portal.files; serial 241210 application/vnd.portal.files; serial 241210 application/vnd.portal.files; serial 241210 application/vnd.portal.files; serial 241210 application/vnd.portal.files; serial 241210 application/vnd.portal.files
- set_actions: none; none (0, 0)
- receive (MIME Zen read): text/uri-list (fd 145); application/vnd.portal.files (fd 145)
- finish: yes

## Portal D-Bus (5 calls)

1. **GetMountPoint** — caller :1.690 — returned: "/run/user/1000/doc"
2. **StartTransfer** — caller :1.693 — args: "writable", "autostop" — returned: "330891181906846580218187931364239210281"
3. **AddFiles** — caller :1.693 — args: "330891181906846580218187931364239210281"
4. **RetrieveFiles** — caller :1.692 — args: "330891181906846580218187931364239210281" — returned: "/home/kyle/Downloads/gimbal-dnd-test-gimbal/sample.png", "/home/kyle/Downloads/gimbal-dnd-test-gimbal/sample.txt"
5. **TransferClosed** — caller :1.37 — args: "330891181906846580218187931364239210281"
