# Drag-and-drop capture: 20261101-114515-gimbal-outside

Source app: gimbal — location: outside — Zen opened the file: yes
Note: auto:true source=gimbal location=outside file=sample.png,sample.txt folder=/home/kyle/gimbal-dnd-test-gimbal

## Source side (gimbal)

- Drag source: wl_data_source#53 — attempt 1 of 1 (chosen by wl_data_device.start_drag)
- Offers (in order): application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, application/vnd.portal.filetransfer, application/vnd.portal.files
- set_actions: copy|move (3)
- send: text/uri-list (fd 22); application/vnd.portal.files (fd 22)
- action events: copy; none; move (1, 0, 2)
- Ending: dnd_drop_performed+dnd_finished

## Zen side

- Drag offer: wl_data_offer#4278190082 (from the last wl_data_device.enter in the log)
- attempts Zen saw: 1
- drop after enter: yes
- drop count: 1
- enter: serial 240944 surface wl_surface#66 offer wl_data_offer#4278190082
- leave: serial None
- drop: serial None
- Offers on the drag offer: application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, application/vnd.portal.filetransfer, application/vnd.portal.files
- source_actions: copy|move (3)
- action: move (2)
- accept: serial 240944 nil; serial 240921 text/uri-list; serial 240944 nil; serial 240921 application/vnd.portal.files; serial 240944 nil; serial 240944 application/vnd.portal.files; serial 240944 application/vnd.portal.files; serial 240944 application/vnd.portal.files; serial 240944 application/vnd.portal.files; serial 240944 application/vnd.portal.files; serial 240944 application/vnd.portal.files; serial 240944 application/vnd.portal.files; serial 240944 application/vnd.portal.files; serial 240944 application/vnd.portal.files; serial 240944 application/vnd.portal.files; serial 240944 application/vnd.portal.files; serial 240944 application/vnd.portal.files
- set_actions: none; none (0, 0)
- receive (MIME Zen read): text/uri-list (fd 142); application/vnd.portal.files (fd 142)
- finish: yes

## Portal D-Bus (5 calls)

1. **GetMountPoint** — caller :1.662 — returned: "/run/user/1000/doc"
2. **StartTransfer** — caller :1.665 — args: "writable", "autostop" — returned: "1657233408874126045117274075126858720883"
3. **AddFiles** — caller :1.665 — args: "1657233408874126045117274075126858720883"
4. **RetrieveFiles** — caller :1.664 — args: "1657233408874126045117274075126858720883" — returned: "/run/user/1000/doc/c8a42b07/sample.png", "/run/user/1000/doc/845cdb52/sample.txt"
5. **TransferClosed** — caller :1.37 — args: "1657233408874126045117274075126858720883"
