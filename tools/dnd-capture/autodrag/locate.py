# SPDX-License-Identifier: GPL-3.0-or-later
"""Find the test file's on-screen position through AT-SPI (P0a2).

The runner launches each source app at the test folder, then locates the
item whose accessible name is the file name (e.g. "sample.png") inside the
AT-SPI application registered by that process.  On Wayland, AT-SPI cannot
give true screen coordinates, so item extents are read with
Atspi.CoordType.WINDOW (window-relative) and the niri window position is
added:

    screen = niri_window_pos + window_offset_correction + item_window_pos

`window_offset_correction` (apps.toml, per app) covers the decoration
difference between the AT-SPI "window" origin and niri's window position —
observed 2026-10-30, tiled windows: nautilus (GTK4) frame == niri tile_size
(1120x1462 == 1120x1462), thunar (GTK3) frame is 48x48 px LARGER than the
tile (CSD shadow border), dolphin (Qt) frame 6x33 px SMALLER (SSD
decorations).  The corrections are calibrated live (TODO in
NOTES-autodrag.md); the selection check aborts the run when a click misses.

AT-SPI availability observed 2026-10-30 (session org.a11y.Status.IsEnabled
== false; recorded in NOTES-autodrag.md):
  dolphin          registers only with QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1;
                   with it, file items are exposed (role "list item")
  thunar           registers, but the file view exposes no items
  nautilus         registers, but the tree is only app->frame->panel->panel
  cosmic-files     no AT-SPI application at all
Apps without a locatable item use the apps.toml fallback offset instead
(null until calibrated live; a null offset with no AT-SPI item aborts the
run rather than clicking blind).

The gi import is lazy (inside the GI bridge classes) so the unit tests run
hermetically with fake nodes.
"""

import os.path

# window-relative coordinate type — the only meaningful one on Wayland
COORD_WINDOW = "window"


class SpiItem(object):
    """A located item: window-relative rect + what AT-SPI knows about it."""

    def __init__(self, name, role, window_rect, selectable=None, selected=None):
        self.name = name
        self.role = role
        self.window_rect = window_rect  # (x, y, w, h), CoordType.WINDOW
        self.selectable = selectable
        self.selected = selected

    def center(self):
        x, y, w, h = self.window_rect
        return (x + w // 2, y + h // 2)


def item_screen_rect(item, window_pos, correction=(0, 0)):
    """Window-relative item rect -> screen rect: niri window position plus
    the per-app decoration correction plus the item's window-relative pos.
    Pure; tested including a decoration offset case."""
    ix, iy, iw, ih = item.window_rect
    cx, cy = correction
    wx, wy = window_pos
    return (wx + cx + ix, wy + cy + iy, iw, ih)


# ---------------------------------------------------------------------------
# toolkit-independent tree search over a node protocol
#
# A node provides: name() -> str, role_name() -> str, children() -> [node],
# extents_window() -> (x, y, w, h), state_selected() -> bool.
# The GI bridge below adapts Atspi.Accessibles to it; the tests use fakes.
#

def find_item_by_name(root, target, budget=5000):
    """Depth-first search under `root` for a node whose name equals
    `target` (the file name).  Bounded by `budget` nodes.  Returns the node
    or None.  Pure; operates on the node protocol."""
    stack = [root]
    seen = 0
    while stack and seen < budget:
        node = stack.pop()
        seen += 1
        try:
            name = node.name()
        except Exception:
            continue
        if name == target:
            return node
        try:
            kids = list(node.children())
        except Exception:
            kids = []
        stack.extend(reversed(kids))
    return None


def spi_item_from_node(node):
    """Snapshot a protocol node into a SpiItem."""
    return SpiItem(
        name=node.name(),
        role=node.role_name(),
        window_rect=tuple(node.extents_window()),
        selectable=node.state_selectable(),
        selected=node.state_selected(),
    )


# ---------------------------------------------------------------------------
# GI bridge (real AT-SPI).  Never exercised by the agent or the tests.
#

class AtspiNode(object):
    """Adapts an Atspi.Accessible to the node protocol."""

    def __init__(self, acc):
        self._acc = acc

    def name(self):
        return self._acc.get_name() or ""

    def role_name(self):
        return self._acc.get_role_name()

    def children(self):
        n = self._acc.get_child_count()
        return [AtspiNode(self._acc.get_child_at_index(i)) for i in range(n)]

    def extents_window(self):
        r = self._acc.get_extents(AtspiCoordType().WINDOW)
        return (r.x, r.y, r.width, r.height)

    def state_selected(self):
        return self._acc.get_state_set().contains(AtspiStateType().SELECTED)

    def state_selectable(self):
        return self._acc.get_state_set().contains(AtspiStateType().SELECTABLE)


class AtspiCoordType(object):
    """Lazy Atspi.CoordType lookup (kept behind a callable so importing
    this module never requires gi)."""

    def __init__(self):
        import gi  # noqa: deferred import
        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi  # noqa: deferred import
        self._ = Atspi.CoordType

    def __getattr__(self, item):
        return getattr(self._, item)


class AtspiStateType(object):
    def __init__(self):
        import gi  # noqa: deferred import
        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi  # noqa: deferred import
        self._ = Atspi.StateType

    def __getattr__(self, item):
        return getattr(self._, item)


def find_app_by_pid(pid, budget=100):
    """The AT-SPI desktop child whose process id is `pid`, or None.
    Atspi.Accessible.get_process_id() exists and was verified live
    (2026-10-30).  None also covers 'the app exposes no AT-SPI'."""
    import gi  # noqa: deferred import
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi  # noqa: deferred import
    desktop = Atspi.get_desktop(0)
    n = desktop.get_child_count()
    for i in range(min(n, budget)):
        app = desktop.get_child_at_index(i)
        try:
            if app.get_process_id() == pid:
                return AtspiNode(app)
        except Exception:
            continue
    return None


def locate_file_item(pid, file_name, budget=5000):
    """One-stop locate: AT-SPI app by pid -> search for the file item ->
    SpiItem snapshot.  Returns (item, app_found) where app_found says
    whether the process registered an AT-SPI application at all."""
    app = find_app_by_pid(pid)
    if app is None:
        return None, False
    node = find_item_by_name(app, file_name, budget=budget)
    if node is None:
        return None, True
    return spi_item_from_node(node), True


# ---------------------------------------------------------------------------
# fallback path (no usable AT-SPI item)
#

def fallback_point(app_cfg, window_pos, window_size):
    """The click point when AT-SPI has no item: the configured offset from
    the window's top-left to the first item in the view.  Returns None when
    unconfigured — the caller must abort rather than click blind."""
    off = app_cfg.get("first_item_offset")
    if not off:
        return None
    return (window_pos[0] + off[0], window_pos[1] + off[1])


def config_path():
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), "apps.toml")
