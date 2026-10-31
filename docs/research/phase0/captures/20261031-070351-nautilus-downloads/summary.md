# Drag-and-drop capture: 20261031-070351-nautilus-downloads

Source app: nautilus — location: downloads — Zen opened the file: yes
Note: auto:true source=nautilus location=downloads file=sample.png folder=/home/kyle/Downloads/gimbal-dnd-test

## Source side (nautilus)

- Drag source: wl_data_source#54 — attempt 1 of 1 (chosen by wl_data_device.start_drag)
- Offers (in order): application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, application/vnd.portal.filetransfer, application/vnd.portal.files
- set_actions: copy|move|ask (7)
- send: text/uri-list (fd 40); application/vnd.portal.files (fd 40)
- action events: move; none; move (2, 0, 2)
- Ending: dnd_drop_performed+dnd_finished

## Zen side

- Drag offer: wl_data_offer#4278190081 (from the last wl_data_device.enter in the log)
- attempts Zen saw: 1
- drop after enter: yes
- drop count: 1
- enter: serial 549897 surface wl_surface#66 offer wl_data_offer#4278190081
- leave: serial None
- drop: serial None
- Offers on the drag offer: application/x-gtk-local-dnd, text/plain;charset=utf-8, text/uri-list, application/vnd.portal.filetransfer, application/vnd.portal.files
- source_actions: copy|move|ask (7)
- action: move (2)
- accept: serial 549897 nil; serial 549872 text/uri-list; serial 549897 nil; serial 549872 application/vnd.portal.files; serial 549897 nil; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files; serial 549897 application/vnd.portal.files
- set_actions: none; none (0, 0)
- receive (MIME Zen read): text/uri-list (fd 142); application/vnd.portal.files (fd 142)
- finish: yes

## Portal D-Bus (5 calls)

1. **GetMountPoint** — caller :1.1314 — returned: "/run/user/1000/doc"
2. **StartTransfer** — caller :1.1317 — args: "writable", "autostop" — returned: "80053700614124756536895358332849677152"
3. **AddFiles** — caller :1.1317 — args: "80053700614124756536895358332849677152"
4. **RetrieveFiles** — caller :1.1316 — args: "80053700614124756536895358332849677152" — returned: "/home/kyle/Downloads/gimbal-dnd-test/sample.png"
5. **TransferClosed** — caller :1.261 — args: "80053700614124756536895358332849677152"
