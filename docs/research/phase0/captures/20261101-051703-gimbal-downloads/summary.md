# Drag-and-drop capture: 20261101-051703-gimbal-downloads

Source app: gimbal — location: downloads — Zen opened the file: yes
Note: auto:true source=gimbal location=downloads file=sample.png folder=/home/kyle/Downloads/gimbal-dnd-test-gimbal

## Source side (gimbal)

- Drag source: wl_data_source#53 — attempt 1 of 1 (chosen by wl_data_device.start_drag)
- Offers (in order): application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, application/vnd.portal.filetransfer, application/vnd.portal.files
- set_actions: copy|move (3)
- send: text/uri-list (fd 23); application/vnd.portal.files (fd 23)
- action events: copy; none; move (1, 0, 2)
- Ending: dnd_drop_performed+dnd_finished

## Zen side

- Drag offer: wl_data_offer#4278190081 (from the last wl_data_device.enter in the log)
- attempts Zen saw: 1
- drop after enter: yes
- drop count: 1
- enter: serial 2571594 surface wl_surface#66 offer wl_data_offer#4278190081
- leave: serial None
- drop: serial None
- Offers on the drag offer: application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, application/vnd.portal.filetransfer, application/vnd.portal.files
- source_actions: copy|move (3)
- action: move (2)
- accept: serial 2571594 nil; serial 2571569 text/uri-list; serial 2571594 nil; serial 2571569 application/vnd.portal.files; serial 2571594 nil; serial 2571594 application/vnd.portal.files; serial 2571594 application/vnd.portal.files; serial 2571594 application/vnd.portal.files; serial 2571594 application/vnd.portal.files; serial 2571594 application/vnd.portal.files; serial 2571594 application/vnd.portal.files; serial 2571594 application/vnd.portal.files; serial 2571594 application/vnd.portal.files; serial 2571594 application/vnd.portal.files; serial 2571594 application/vnd.portal.files; serial 2571594 application/vnd.portal.files; serial 2571594 application/vnd.portal.files
- set_actions: none; none (0, 0)
- receive (MIME Zen read): text/uri-list (fd 142); application/vnd.portal.files (fd 142)
- finish: yes

## Portal D-Bus (5 calls)

1. **GetMountPoint** — caller :1.2065 — returned: "/run/user/1000/doc"
2. **StartTransfer** — caller :1.2068 — args: "writable", "autostop" — returned: "1708492479154230698215892887162624426077"
3. **AddFiles** — caller :1.2068 — args: "1708492479154230698215892887162624426077"
4. **RetrieveFiles** — caller :1.2067 — args: "1708492479154230698215892887162624426077" — returned: "/home/kyle/Downloads/gimbal-dnd-test-gimbal/sample.png"
5. **TransferClosed** — caller :1.261 — args: "1708492479154230698215892887162624426077"
