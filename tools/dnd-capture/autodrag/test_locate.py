# SPDX-License-Identifier: GPL-3.0-or-later
"""Tests for locate.py — the tree search and the coordinate composition
run entirely on fake nodes; no AT-SPI/gi is imported (P0a2 hard limit 1).

Run:  python3 -I test_locate.py -v
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from locate import (SpiItem, fallback_point, find_item_by_name,
                    item_screen_rect, spi_item_from_node)


class FakeNode(object):
    """The node protocol: name/role_name/children/extents_window/state."""

    def __init__(self, name, role="unknown", rect=(0, 0, 0, 0),
                 children=(), selected=False, selectable=False):
        self._name = name
        self._role = role
        self._rect = rect
        self._children = list(children)
        self._selected = selected
        self._selectable = selectable

    def name(self):
        return self._name

    def role_name(self):
        return self._role

    def children(self):
        return self._children

    def extents_window(self):
        return self._rect

    def state_selected(self):
        return self._selected

    def state_selectable(self):
        return self._selectable


def sample_tree(png_rect=(312, 88, 135, 92), png_selected=False):
    return FakeNode("dolphin", "application", children=[
        FakeNode("gimbal-dnd-test — Dolphin", "frame", children=[
            FakeNode("", "split pane", children=[
                FakeNode("", "scroll pane", children=[
                    FakeNode("sample.txt", "list item", (164, 88, 135, 92)),
                    FakeNode("sample.png", "list item", png_rect,
                             selected=png_selected, selectable=True),
                ]),
            ]),
        ]),
    ])


class TestTreeSearch(unittest.TestCase):
    def test_finds_item_by_exact_name(self):
        node = find_item_by_name(sample_tree(), "sample.png")
        self.assertIsNotNone(node)
        self.assertEqual(node.role_name(), "list item")
        self.assertEqual(node.extents_window(), (312, 88, 135, 92))

    def test_exact_name_only(self):
        # a node whose name merely contains the target must not match
        tree = FakeNode("app", children=[FakeNode("sample.png.bak", "list item")])
        self.assertIsNone(find_item_by_name(tree, "sample.png"))

    def test_missing_returns_none(self):
        self.assertIsNone(find_item_by_name(sample_tree(), "nope.txt"))

    def test_budget_bounds_the_search(self):
        deep = current = FakeNode("root")
        for i in range(100):
            nxt = FakeNode("child%d" % i)
            current._children.append(nxt)
            current = nxt
        current._name = "sample.png"
        self.assertIsNone(find_item_by_name(deep, "sample.png", budget=10))
        self.assertIsNotNone(find_item_by_name(deep, "sample.png", budget=500))

    def test_snapshot_into_item(self):
        tree = sample_tree(png_selected=True)
        item = spi_item_from_node(find_item_by_name(tree, "sample.png"))
        self.assertIsInstance(item, SpiItem)
        self.assertEqual(item.name, "sample.png")
        self.assertTrue(item.selected)
        self.assertTrue(item.selectable)
        self.assertEqual(item.center(), (312 + 67, 88 + 46))


class TestCoordinateComposition(unittest.TestCase):
    def test_window_relative_plus_window_pos(self):
        item = SpiItem("sample.png", "list item", (312, 88, 135, 92))
        # niri placed the window at (40, 40): the item's screen rect
        self.assertEqual(item_screen_rect(item, (40, 40)),
                         (352, 128, 135, 92))

    def test_decoration_offset_case(self):
        # GTK3 CSD: the AT-SPI window origin sits inside a 24 px shadow
        # border around the tile, so the correction is (-24, -24)
        item = SpiItem("sample.png", "list item", (312, 88, 135, 92))
        rect = item_screen_rect(item, (40, 40), (-24, -24))
        self.assertEqual(rect, (40 - 24 + 312, 40 - 24 + 88, 135, 92))

    def test_center_is_inside_the_rect(self):
        item = SpiItem("sample.png", "list item", (312, 88, 135, 92))
        rect = item_screen_rect(item, (1148, 40))
        cx, cy = rect[0] + rect[2] // 2, rect[1] + rect[3] // 2
        self.assertTrue(rect[0] <= cx < rect[0] + rect[2])
        self.assertTrue(rect[1] <= cy < rect[1] + rect[3])


class TestFallback(unittest.TestCase):
    def test_fallback_offset_from_window_origin(self):
        cfg = {"first_item_offset": (60, 120)}
        self.assertEqual(fallback_point(cfg, (40, 40), (1068, 1424)), (100, 160))

    def test_unconfigured_returns_none(self):
        self.assertIsNone(fallback_point({}, (40, 40), (1068, 1424)))
        self.assertIsNone(fallback_point({"first_item_offset": None}, (40, 40), (1, 1)))


if __name__ == "__main__":
    unittest.main(verbosity=2)
