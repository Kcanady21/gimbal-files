# Drag-and-drop capture: 20261031-070402-cosmic-downloads

Source app: cosmic — location: downloads — Zen opened the file: yes
Note: auto:true source=cosmic location=downloads file=sample.png folder=/home/kyle/Downloads/gimbal-dnd-test

## Source side (cosmic)

- Drag source: wl_data_source#125 — attempt 1 of 1 (chosen by wl_data_device.start_drag)
- Offers (in order): text/plain, text/plain;charset=utf-8, UTF8_STRING, text/uri-list, x-special/gnome-copied-files
- set_actions: copy|move (3)
- send: text/uri-list (fd 58); text/plain (fd 58)
- action events: move; none; move (2, 0, 2)
- Ending: dnd_drop_performed+dnd_finished

## Zen side

- Drag offer: wl_data_offer#4278190081 (from the last wl_data_device.enter in the log)
- attempts Zen saw: 1
- drop after enter: yes
- drop count: 1
- enter: serial 549988 surface wl_surface#66 offer wl_data_offer#4278190081
- leave: serial None
- drop: serial None
- Offers on the drag offer: text/plain, text/plain;charset=utf-8, UTF8_STRING, text/uri-list, x-special/gnome-copied-files
- source_actions: copy|move (3)
- action: move (2)
- accept: serial 549988 nil; serial 549963 text/uri-list; serial 549988 nil; serial 549988 x-special/gnome-copied-files; serial 549988 x-special/gnome-copied-files; serial 549988 x-special/gnome-copied-files; serial 549988 x-special/gnome-copied-files; serial 549963 text/plain; serial 549988 x-special/gnome-copied-files; serial 549988 x-special/gnome-copied-files; serial 549988 x-special/gnome-copied-files; serial 549988 x-special/gnome-copied-files; serial 549988 x-special/gnome-copied-files; serial 549988 x-special/gnome-copied-files; serial 549988 x-special/gnome-copied-files; serial 549988 x-special/gnome-copied-files; serial 549988 x-special/gnome-copied-files
- set_actions: none; none (0, 0)
- receive (MIME Zen read): text/uri-list (fd 145); text/plain (fd 142)
- finish: yes

## Portal D-Bus (1 calls)

1. **GetMountPoint** — caller :1.1322 — returned: "/run/user/1000/doc"
