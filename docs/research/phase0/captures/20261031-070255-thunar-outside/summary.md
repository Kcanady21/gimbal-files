# Drag-and-drop capture: 20261031-070255-thunar-outside

Source app: thunar — location: outside — Zen opened the file: ?
Note: auto:true source=thunar location=outside file=sample.png folder=/home/kyle/gimbal-dnd-test

## Source side (thunar)

- Drag source: wl_data_source#41 — attempt 1 of 1 (chosen by wl_data_device.start_drag)
- Offers (in order): text/uri-list, text/uri-list, DELETE
- set_actions: copy|move (3)
- send: text/uri-list (fd 31)
- action events: move; none; move (2, 0, 2)
- Ending: dnd_drop_performed+dnd_finished

## Zen side

- Drag offer: wl_data_offer#4278190081 (from the last wl_data_device.enter in the log)
- attempts Zen saw: 1
- drop after enter: yes
- drop count: 1
- enter: serial 549553 surface wl_surface#66 offer wl_data_offer#4278190081
- leave: serial None
- drop: serial None
- Offers on the drag offer: text/uri-list, text/uri-list, DELETE
- source_actions: copy|move (3)
- action: move (2)
- accept: serial 549553 nil; serial 549527 text/uri-list; serial 549553 nil; serial 549553 text/uri-list; serial 549553 text/uri-list; serial 549553 text/uri-list; serial 549553 text/uri-list; serial 549553 text/uri-list; serial 549553 text/uri-list; serial 549553 text/uri-list; serial 549553 text/uri-list; serial 549553 text/uri-list; serial 549553 text/uri-list; serial 549553 text/uri-list
- set_actions: none; none (0, 0)
- receive (MIME Zen read): text/uri-list (fd 141)
- finish: yes

## Portal D-Bus (1 calls)

1. **GetMountPoint** — caller :1.1249 — returned: "/run/user/1000/doc"
