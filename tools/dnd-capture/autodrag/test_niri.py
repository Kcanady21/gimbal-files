# SPDX-License-Identifier: GPL-3.0-or-later
"""Tests for niri.py — canned JSON reads via a fake runner, exact action
argv construction, and dry-run printing.  No real niri process is started
and no niri action is ever run (P0a2 hard limit 2).

Run:  python3 -I test_niri.py -v
"""

import copy
import io
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import niri as niri_mod
from niri import Niri, NiriError, enabled_outputs, focused_workspace_id, \
    readout_geometry, target_workspace, window_by_id, window_by_pid


# ---------------------------------------------------------------------------
# Real niri JSON, pasted verbatim (P0a2-r1 correction 1: no invented keys).
#
# Provenance: `niri msg --json workspaces` and `niri msg --json windows` on
# this machine, 2026-10-31, niri 26.04 ("Compositor version: 26.04").  Two
# outputs were enabled that day (DP-2 and DP-3, both 2560x1440).  The
# workspaces JSON has NO `is_empty` key — a workspace shows as empty via
# `active_window_id: null`.  On DP-2 the empty end workspace is id 6 (idx 2);
# DP-3's empty id 13 sits at a HIGHER idx (3), which is what makes the
# output filter testable with real data.
#
REAL_WORKSPACES = [
    {"id": 13, "idx": 3, "name": None, "output": "DP-3", "is_urgent": False,
     "is_active": False, "is_focused": False, "active_window_id": None},
    {"id": 11, "idx": 1, "name": None, "output": "DP-3", "is_urgent": False,
     "is_active": True, "is_focused": False, "active_window_id": 14},
    {"id": 1, "idx": 1, "name": None, "output": "DP-2", "is_urgent": False,
     "is_active": True, "is_focused": True, "active_window_id": 17},
    {"id": 6, "idx": 2, "name": None, "output": "DP-2", "is_urgent": False,
     "is_active": False, "is_focused": False, "active_window_id": None},
    {"id": 12, "idx": 2, "name": None, "output": "DP-3", "is_urgent": False,
     "is_active": False, "is_focused": False, "active_window_id": 27},
]

REAL_WINDOW = {
    "id": 7,
    "title": "◑ Mercury harness development (2)",
    "app_id": "com.mitchellh.ghostty",
    "pid": 38038,
    "workspace_id": 11,
    "is_focused": False,
    "is_floating": False,
    "is_urgent": False,
    "layout": {"pos_in_scrolling_layout": [2, 1],
               "tile_size": [1272.0, 1398.0],
               "window_size": [1272, 1398],
               "tile_pos_in_workspace_view": None,
               "window_offset_in_tile": [0.0, 0.0]},
    "focus_timestamp": {"secs": 72497, "nanos": 311282545},
}

# the workspace_id of every window in the same `niri msg --json windows`
# read, in file order (7 windows that day)
REAL_WINDOW_WS_IDS = [11, 11, 12, 11, 1, 11, 11]


def real_window(**overrides):
    """A copy of the real window shape with values overridden — same keys
    niri produces, never more."""
    w = copy.deepcopy(REAL_WINDOW)
    w.update(overrides)
    return w


def real_windows_on(ws_ids, start_id=900):
    return [real_window(id=start_id + i, pid=9000 + i, app_id="app.x",
                        title="window %d" % i, workspace_id=ws_id)
            for i, ws_id in enumerate(ws_ids)]


def win(id, pid=100, app_id="org.kde.dolphin", title="w", ws=2, floating=False,
        pos=None, size=(1120.0, 1462.0)):
    """Test window: real key shape (P0a2-r1), values chosen per test."""
    w = real_window(id=id, pid=pid, app_id=app_id, title=title,
                    workspace_id=ws, is_focused=False, is_floating=floating)
    w["layout"]["tile_size"] = [float(size[0]), float(size[1])]
    w["layout"]["window_size"] = [int(size[0]), int(size[1])]
    w["layout"]["tile_pos_in_workspace_view"] = \
        [float(pos[0]), float(pos[1])] if pos is not None else None
    return w


class FakeRunner(object):
    """Records argv, returns canned stdout per subcommand."""

    def __init__(self, replies=None):
        self.replies = replies or {}
        self.calls = []

    def __call__(self, argv):
        self.calls.append(list(argv))
        for key, val in self.replies.items():
            if key in argv:
                return 0, val
        return 0, ""


class TestReads(unittest.TestCase):
    def test_windows_parsed_from_fake_runner(self):
        r = FakeRunner({"--json": '[{"id": 7, "pid": 42}]'})
        n = Niri(runner=r)
        self.assertEqual(n.windows(), [{"id": 7, "pid": 42}])
        self.assertEqual(r.calls, [["niri", "msg", "--json", "windows"]])

    def test_enabled_outputs_counts_nonnull_logical(self):
        outs = {"DP-2": {"logical": {"width": 2256, "height": 1504}},
                "DP-3": {"logical": None}}
        self.assertEqual(sorted(enabled_outputs(outs)), ["DP-2"])

    def test_window_lookups(self):
        wins = [win(7, pid=42), win(9, pid=43, app_id="app.zen_browser.zen")]
        self.assertEqual(window_by_pid(wins, 43)["id"], 9)
        self.assertIsNone(window_by_pid(wins, 999))
        self.assertEqual(window_by_id(wins, 7)["id"], 7)

    def test_target_workspace_real_sample_dp2(self):
        # the "Done means" case: on the real 2026-10-31 output the empty end
        # workspace of DP-2 is id 6 (idx 2)
        tws = target_workspace(REAL_WORKSPACES,
                               real_windows_on(REAL_WINDOW_WS_IDS), {"DP-2"})
        self.assertIsNotNone(tws)
        self.assertEqual(tws["id"], 6)
        self.assertEqual(tws["idx"], 2)
        self.assertEqual(tws["output"], "DP-2")

    def test_target_workspace_real_sample_other_output(self):
        # DP-3's empty workspace id 13 sits at a HIGHER idx (3) than DP-2's —
        # with DP-3 enabled it wins; with DP-2 enabled it must be ignored
        tws = target_workspace(REAL_WORKSPACES,
                               real_windows_on(REAL_WINDOW_WS_IDS), {"DP-3"})
        self.assertEqual(tws["id"], 13)
        self.assertEqual(target_workspace(REAL_WORKSPACES,
                                          real_windows_on(REAL_WINDOW_WS_IDS),
                                          {"DP-2"})["id"], 6)

    def test_target_workspace_real_sample_no_filter(self):
        # without an output filter the highest empty idx overall wins (13/3)
        self.assertEqual(target_workspace(REAL_WORKSPACES,
                                          real_windows_on(REAL_WINDOW_WS_IDS))["id"], 13)

    def test_target_workspace_real_sample_all_occupied(self):
        # give id 6 a window: DP-2 has no empty workspace left -> None
        wins = real_windows_on(REAL_WINDOW_WS_IDS + [6])
        self.assertIsNone(target_workspace(REAL_WORKSPACES, wins, {"DP-2"}))

    def test_target_workspace_null_active_window_still_occupied(self):
        # active_window_id null but a window entry carries the workspace id:
        # not empty (the windows() cross-check)
        wins = real_windows_on([6])
        self.assertIsNone(target_workspace(REAL_WORKSPACES, wins, {"DP-2"}))

    def test_focused_workspace_id_real_sample(self):
        self.assertEqual(focused_workspace_id(REAL_WORKSPACES), 1)

    def test_readout_geometry_null_for_tiled(self):
        self.assertEqual(readout_geometry(win(7)), (None, None))
        self.assertEqual(readout_geometry(win(7, floating=True, pos=[40.0, 40.0])),
                         ((40.0, 40.0), (1120.0, 1462.0)))


class TestActions(unittest.TestCase):
    def test_place_builds_exact_argv(self):
        r = FakeRunner()
        n = Niri(runner=r)
        cmds = n.place(42, 40, 40, 1068, 1424)
        self.assertEqual([c[3:] for c in r.calls], [
            ["move-window-to-floating", "--id", "42"],
            ["move-floating-window", "--id", "42", "-x", "40", "-y", "40"],
            ["set-window-width", "--id", "42", "1068"],
            ["set-window-height", "--id", "42", "1424"],
        ])
        self.assertEqual(len(cmds), 4)
        # every action passes an explicit --id: nothing acts on the focused
        # window implicitly (hard limit 3)
        for c in r.calls:
            self.assertIn("--id", c)

    def test_move_window_to_workspace_uses_window_id_flag(self):
        r = FakeRunner()
        Niri(runner=r).move_window_to_workspace(9, 4, focus=False)
        self.assertEqual(r.calls[0][3:],
                         ["move-window-to-workspace", "--window-id", "9",
                          "--focus", "false", "4"])

    def test_failed_action_raises(self):
        r = FakeRunner()
        r.replies["x"] = None

        def failing(argv):
            return 1, ""

        n = Niri(runner=failing)
        with self.assertRaises(NiriError):
            n.focus_window(5)

    def test_focus_workspace_by_id_looks_up_index(self):
        # the CLI reference parser maps numerals to index, not id (niri-ipc
        # v26.04) — focus by id must translate through workspaces(); the
        # canned reply uses only keys niri produces (P0a2-r1: no is_empty)
        r = FakeRunner({"workspaces":
                        '[{"id": 6, "idx": 4, "name": null, "output": "DP-2",'
                        ' "is_urgent": false, "is_active": false,'
                        ' "is_focused": false, "active_window_id": null}]'})
        n = Niri(runner=r)
        n.focus_workspace_by_id(6)
        self.assertEqual(r.calls[-1], ["niri", "msg", "action", "focus-workspace", "4"])

    def test_focus_workspace_by_id_missing(self):
        n = Niri(runner=FakeRunner({"workspaces": "[]"}))
        with self.assertRaises(NiriError):
            n.focus_workspace_by_id(99)


class TestDryRun(unittest.TestCase):
    def test_dry_run_prints_instead_of_running(self):
        r = FakeRunner({"windows": "[]"})  # canned read for the dry-run
        buf = io.StringIO()
        printed = []
        n = Niri(dry_run=True, runner=r, printer=printed.append)
        n.place(42, 10, 20, 300, 200)
        # actions never ran
        self.assertEqual([c for c in r.calls if "action" in c], [])
        # the exact commands were printed
        self.assertEqual(len(printed), 4)
        self.assertIn("move-floating-window --id 42 -x 10 -y 20", printed[1])
        # reads still work under dry-run (read-only is allowed)
        n.windows()
        self.assertEqual(r.calls, [["niri", "msg", "--json", "windows"]])


if __name__ == "__main__":
    unittest.main(verbosity=2)
