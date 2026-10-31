# SPDX-License-Identifier: GPL-3.0-or-later
"""Tests for autodrag.py — abort rules, title classification, geometry,
result.txt, --stop-after and the whole-run --dry-run output.  Everything
runs on fakes: a stateful FakeNiri, a FakeVMouse (no uinput), a fake locate,
a fake clock.  No niri action is executed (P0a2 hard limits 1-2).

Run:  python3 -I test_autodrag.py -v
"""

import contextlib
import copy
import io
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import autodrag
from autodrag import (ABORT_EXIT, OK_EXIT, Args, RunState, abort_reason,
                      classify_title, drop_point, find_zen_window, geometry,
                      perform_drag, run_dry, run_real)
from locate import SpiItem


# ---------------------------------------------------------------------------
# fakes
#

class FakeNiri(object):
    """Stateful fake: the action methods mutate the canned window/workspace
    state the way niri would, so verify_layout and the abort checks exercise
    real readbacks.  Reads return the live dicts; actions are recorded.

    The Zen window's title is driven by two hooks (P0a2-r1 correction 5
    tests): `title_after_actions`/`title_late` switches the title once n
    actions have been issued (models a change during placement), and
    `title_timeline` (list of (switch_time, title)) switches on the fake
    clock (models the first change + the settled read)."""

    def __init__(self, windows, outputs, workspaces, focused_id=None,
                 clock=None):
        self.windows_list = windows
        self.outputs_map = outputs
        self.workspaces_list = workspaces
        self.focused_id = focused_id
        self.actions = []
        self.clock = clock if clock is not None else FakeClock()
        self.zen_title_static = None
        self.title_after_actions = None
        self.title_late = None
        self.title_timeline = []

    # reads
    def effective_zen_title(self):
        t = self.zen_title_static
        if self.title_after_actions is not None \
                and len(self.actions) >= self.title_after_actions:
            t = self.title_late
        for (switch_at, title) in self.title_timeline:
            if self.clock.t >= switch_at:
                t = title
        return t

    def windows(self):
        if self.zen_title_static is not None:
            for w in self.windows_list:
                if w["app_id"] == "app.zen_browser.zen":
                    w["title"] = self.effective_zen_title()
        return self.windows_list

    def outputs(self):
        return self.outputs_map

    def workspaces(self):
        return self.workspaces_list

    def focused_window(self):
        for w in self.windows_list:
            if w["id"] == self.focused_id:
                return w
        return None

    def _win(self, wid):
        for w in self.windows_list:
            if w["id"] == wid:
                return w
        return None

    def _ws_by_idx(self, idx):
        for w in self.workspaces_list:
            if w["idx"] == idx:
                return w
        return None

    # actions (recorded, then applied)
    def move_window_to_floating(self, wid):
        self.actions.append(("move-window-to-floating", wid))
        self._win(wid)["is_floating"] = True

    def move_floating_window(self, wid, x, y):
        self.actions.append(("move-floating-window", wid, x, y))
        # niri reads positions back in output coordinates, commands are
        # relative to the working area (live check, Oct 31)
        ox, oy = getattr(self, "wa_offset", (0, 0))
        self._win(wid)["layout"]["tile_pos_in_workspace_view"] = [float(x + ox), float(y + oy)]

    def set_window_width(self, wid, w):
        self.actions.append(("set-window-width", wid, w))
        self._win(wid)["layout"]["tile_size"][0] = float(w)

    def set_window_height(self, wid, h):
        self.actions.append(("set-window-height", wid, h))
        self._win(wid)["layout"]["tile_size"][1] = float(h)
        self._win(wid)["layout"]["window_size"][1] = int(h)

    def focus_window(self, wid):
        self.actions.append(("focus-window", wid))
        self.focused_id = wid

    def move_window_to_workspace(self, wid, ws_idx, focus=False):
        self.actions.append(("move-window-to-workspace", wid, ws_idx, focus))
        target = self._ws_by_idx(ws_idx)
        self._win(wid)["workspace_id"] = target["id"] if target else 10 ** 9

    def focus_workspace(self, ws_idx):
        self.actions.append(("focus-workspace", ws_idx))
        for w in self.workspaces_list:
            w["is_focused"] = (w["idx"] == ws_idx)

    def focus_workspace_by_id(self, ws_id):
        self.actions.append(("focus-workspace-by-id", ws_id))
        for w in self.workspaces_list:
            w["is_focused"] = (w["id"] == ws_id)

    # test helpers
    def move_window(self, wid, x, y):
        self._win(wid)["layout"]["tile_pos_in_workspace_view"] = [float(x), float(y)]


class FakeVMouse(object):
    def __init__(self, w, h):
        self.w, self.h = w, h
        self.moves = []
        self.presses = 0
        self.releases = 0
        self.closed = False

    def move_to(self, x, y):
        self.moves.append((x, y))

    def press(self):
        self.presses += 1

    def release(self):
        self.releases += 1

    def close(self):
        # like VMouse.close: a pending press is released, then destroyed
        if self.presses > self.releases:
            self.releases += 1
        self.closed = True


class TimedVMouse(FakeVMouse):
    """Records the (fake) clock time of every press and release — the
    click_to_drag_gap assertions read these."""

    def __init__(self, w, h, clock):
        super().__init__(w, h)
        self._clock = clock
        self.press_at = []
        self.release_at = []

    def press(self):
        self.press_at.append(self._clock())
        super().press()

    def release(self):
        self.release_at.append(self._clock())
        super().release()


class FakeClock(object):
    """clock() advances by `step` on every call; sleep() advances by the
    slept amount."""

    def __init__(self, step=0.0):
        self.t = 0.0
        self.step = step

    def __call__(self):
        self.t += self.step
        return self.t

    def sleep(self, s):
        self.t += s


def std_outputs():
    return {"DP-2": {"logical": {"x": 0, "y": 0, "width": 2256, "height": 1504,
                                 "scale": 1.0}},
            "DP-3": {"logical": None}}


# P0a2-r1 correction 1: fixtures carry only keys niri really produces.  The
# shape comes from the real sample pasted in test_niri.py
# (REAL_WORKSPACES / REAL_WINDOW, `niri msg --json …` from 2026-10-31,
# niri 26.04 — no `is_empty` key); values here are remapped to the fake
# window ids below.  DP-3 entries are dropped because these runs are
# single-output.
def std_workspaces():
    return [{"id": 1, "idx": 1, "name": None, "output": "DP-2",
             "is_urgent": False, "is_active": True, "is_focused": True,
             "active_window_id": 100},
            {"id": 6, "idx": 2, "name": None, "output": "DP-2",
             "is_urgent": False, "is_active": False, "is_focused": False,
             "active_window_id": None}]


def std_windows(src_title="gimbal-dnd-test — Dolphin",
                zen_title="Zen Browser"):
    # same keys as the real window sample in test_niri.py; values ours
    return [
        {"id": 100, "pid": 5000, "app_id": "org.kde.dolphin", "title": src_title,
         "workspace_id": 1, "is_focused": True, "is_floating": False,
         "is_urgent": False,
         "layout": {"pos_in_scrolling_layout": [7, 1],
                    "tile_size": [1120.0, 1462.0], "window_size": [1120, 1462],
                    "tile_pos_in_workspace_view": None,
                    "window_offset_in_tile": [0.0, 0.0]},
         "focus_timestamp": {"secs": 72497, "nanos": 311282545}},
        {"id": 200, "pid": 6000, "app_id": "app.zen_browser.zen",
         "title": zen_title, "workspace_id": 1, "is_focused": False,
         "is_floating": False, "is_urgent": False,
         "layout": {"pos_in_scrolling_layout": [1, 1],
                    "tile_size": [1120.0, 1462.0], "window_size": [1120, 1462],
                    "tile_pos_in_workspace_view": None,
                    "window_offset_in_tile": [0.0, 0.0]},
         "focus_timestamp": {"secs": 72525, "nanos": 81172550}},
    ]


def attach_env(rundir, folder):
    return {"GIMBAL_SRC_PID": "5000", "GIMBAL_ZEN_INSTANCE": "777",
            "GIMBAL_RUNDIR": rundir, "GIMBAL_FOLDER": folder,
            "GIMBAL_ZEN_WIN_BEFORE": "1,2,3"}


def make_args(**kw):
    a = Args()
    a.source = kw.get("source", "dolphin")
    a.location = kw.get("location", "outside")
    a.file = kw.get("file", "sample.png")
    a.stop_after = kw.get("stop_after")
    a.dry_run = False
    a.hold_s = kw.get("hold_s", 0.0)
    return a


class Locater(object):
    """Fake locate: controllable selected state, counts queries."""

    def __init__(self, selected=True, found=True):
        self.selected = selected
        self.found = found
        self.queries = 0

    def __call__(self, pid, name):
        self.queries += 1
        if not self.found:
            return None, False
        return SpiItem(name, "list item", (312, 88, 135, 92),
                       selectable=True, selected=self.selected), True


CFG = autodrag.load_config()
# The coordinate assertions below were written for a zero working-area
# offset; the live offset (apps.toml, [0, 34]) has its own test.
CFG.setdefault("workspace", {})["working_area_offset"] = [0, 0]


# ---------------------------------------------------------------------------
# pure helpers
#

class TestGeometry(unittest.TestCase):
    def test_halves_with_40px_margin(self):
        src, zen = geometry(2256, 1504, 40)
        self.assertEqual(src, (40, 40, 1068, 1424))
        self.assertEqual(zen, (1148, 40, 1068, 1424))
        # the boxes plus margins tile the output exactly
        self.assertEqual(src[0] + src[2] + 40 + zen[2] + 40, 2256)

    def test_drop_point_fraction(self):
        _, zen = geometry(2256, 1504, 40)
        self.assertEqual(drop_point(zen, [0.5, 0.55]), (1148 + 534, 40 + 783))


class TestTitleClassification(unittest.TestCase):
    def test_y_when_title_names_the_file(self):
        self.assertEqual(classify_title("Zen Browser", "sample.png (PNG Image, 64x64) — Zen", "sample.png"), "y")

    def test_n_when_title_unchanged(self):
        self.assertEqual(classify_title("about:blank — Zen", "about:blank — Zen", "sample.png"), "n")

    def test_other_for_a_changed_title_without_the_name(self):
        self.assertEqual(classify_title("about:blank — Zen", "Hmm. We're having trouble — Zen", "sample.png"), "other")

    def test_txt_file(self):
        self.assertEqual(classify_title("a", "sample.txt — Zen", "sample.txt"), "y")


class TestZenWindowDiscovery(unittest.TestCase):
    def test_by_instance_cgroup(self):
        wins = std_windows()
        real = autodrag.proc_cgroup
        try:
            autodrag.proc_cgroup = lambda pid: \
                "0::/user.slice/.../app-flatpak-app.zen_browser.zen-777.scope" \
                if pid == 6000 else ""
            self.assertEqual(find_zen_window(wins, "777", {1, 2, 3})["id"], 200)
            # Kyle's Zen window (id 3, old, same app_id) never matches
            wins.append({"id": 3, "pid": 7, "app_id": "app.zen_browser.zen",
                         "title": "Kyle's", "workspace_id": 2, "is_floating": False,
                         "layout": {}})
            self.assertEqual(find_zen_window(wins, "777", {1, 2, 3})["id"], 200)
        finally:
            autodrag.proc_cgroup = real

    def test_by_new_window_id_fallback(self):
        wins = std_windows()
        wins.append({"id": 300, "pid": 9, "app_id": "app.zen_browser.zen",
                     "title": "x", "workspace_id": 9, "is_floating": False,
                     "layout": {}})
        self.assertEqual(find_zen_window(wins, None, {100, 200, 300 - 100})["id"], 300)

    def test_fallback_skips_kyles_new_window(self):
        # P0a2-r1 correction 6: a new window of ANOTHER Zen instance (Kyle's,
        # cgroup scope …-2943722691.scope) that appears before ours is never
        # picked, although its id is new and its app_id is zen
        real = autodrag.proc_cgroup
        try:
            autodrag.proc_cgroup = lambda pid: (
                "0::/app.slice/app-flatpak-app.zen_browser.zen-2943722691.scope"
                if pid == 7000 else
                "0::/app.slice/app-flatpak-app.zen_browser.zen-777.scope"
                if pid == 6000 else "")
            kyles = std_windows()[0]
            kyles.update({"id": 300, "pid": 7000, "title": "Kyle's new tab"})
            wins = [kyles] + std_windows()  # Kyle's window first = earlier poll
            self.assertEqual(find_zen_window(wins, "777", {100})["id"], 200)
        finally:
            autodrag.proc_cgroup = real

    def test_fallback_only_when_cgroup_unreadable(self):
        # ours is unreadable (the only fallback-eligible case); Kyle's
        # readable other-scope window is still skipped
        def reader(pid):
            if pid == 7000:
                return ("0::/app.slice/"
                        "app-flatpak-app.zen_browser.zen-2943722691.scope")
            return ""  # our pid's cgroup cannot be read
        kyles = std_windows()[0]
        kyles.update({"id": 300, "pid": 7000, "title": "Kyle's new tab"})
        wins = [kyles] + std_windows()
        self.assertEqual(find_zen_window(wins, "777", {100},
                                         cgroup_reader=reader)["id"], 200)

    def test_zen_scope_instance_parses(self):
        self.assertEqual(autodrag.zen_scope_instance(
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/"
            "app-flatpak-app.zen_browser.zen-2943722691.scope"), "2943722691")
        self.assertIsNone(autodrag.zen_scope_instance(""))
        self.assertIsNone(autodrag.zen_scope_instance("0::/user.slice/session.slice"))

    def test_none_when_no_match(self):
        self.assertIsNone(find_zen_window(std_windows(), "888", {100, 200}))


class TestAbortRules(unittest.TestCase):
    def make_placed_state(self):
        st = RunState()
        st.our_ids = {100, 200}
        st.expected_boxes = {100: (40, 40, 1068, 1424), 200: (1148, 40, 1068, 1424)}
        st.placed = True
        st.workspace_id = 6
        return st

    @staticmethod
    def make_floating(n):
        for w in n.windows_list:
            w["is_floating"] = True
            w["workspace_id"] = 6
            w["layout"]["tile_pos_in_workspace_view"] = [40.0, 40.0] \
                if w["id"] == 100 else [1148.0, 40.0]
            w["layout"]["tile_size"] = [1068.0, 1424.0]

    def test_two_enabled_outputs_abort(self):
        outs = {"DP-2": {"logical": {"width": 1}}, "DP-3": {"logical": {"width": 1}}}
        n = FakeNiri(std_windows(), outs, std_workspaces(), focused_id=100)
        reason = abort_reason(n, self.make_placed_state(), 0.0, lambda: 1.0)
        self.assertIn("more than one output", reason)

    def test_timeout_abort(self):
        n = FakeNiri(std_windows(), std_outputs(), std_workspaces(), focused_id=100)
        reason = abort_reason(n, self.make_placed_state(), 0.0, lambda: 21.0)
        self.assertIn("timeout", reason)

    def test_focus_change_abort(self):
        n = FakeNiri(std_windows(), std_outputs(), std_workspaces(), focused_id=14)
        reason = abort_reason(n, self.make_placed_state(), 0.0, lambda: 1.0)
        self.assertIn("focused window is not one the runner started", reason)

    def test_moved_window_abort(self):
        n = FakeNiri(std_windows(), std_outputs(), std_workspaces(), focused_id=100)
        self.make_floating(n)
        st = self.make_placed_state()
        self.assertIsNone(abort_reason(n, st, 0.0, lambda: 1.0))
        n.move_window(100, 41.0, 40.0)
        reason = abort_reason(n, st, 0.0, lambda: 1.0)
        self.assertIn("window 100 moved", reason)

    def test_resized_window_abort(self):
        n = FakeNiri(std_windows(), std_outputs(), std_workspaces(), focused_id=100)
        self.make_floating(n)
        st = self.make_placed_state()
        n.windows_list[0]["layout"]["tile_size"] = [900.0, 1424.0]
        self.assertIn("resized", abort_reason(n, st, 0.0, lambda: 1.0))

    def test_happy_state_no_reason(self):
        n = FakeNiri(std_windows(), std_outputs(), std_workspaces(), focused_id=100)
        st = self.make_placed_state()
        self.make_floating(n)
        self.assertIsNone(abort_reason(n, st, 0.0, lambda: 1.0))


# ---------------------------------------------------------------------------
# whole-run integration on fakes
#

class RunHarness(object):
    """Bundles the fakes for run_real and the temp run dir."""

    def __init__(self, selected=True, found=True, clock_step=0.0):
        self.tmp = tempfile.TemporaryDirectory()
        self.rundir = os.path.join(self.tmp.name, "run")
        os.makedirs(self.rundir)
        self.folder = os.path.join(self.tmp.name, "folder")
        os.makedirs(self.folder)
        open(os.path.join(self.folder, "sample.png"), "w").close()
        self.clock = FakeClock(step=clock_step)
        self.niri = FakeNiri(std_windows(), std_outputs(), std_workspaces(),
                             focused_id=100, clock=self.clock)
        self.vms = []
        self.logs = []
        self.locater = Locater(selected=selected, found=found)

    def vm_factory(self, w, h):
        vm = TimedVMouse(w, h, self.clock)
        self.vms.append(vm)
        return vm

    def run(self, **kw):
        args = make_args(**kw)
        return run_real(args, kw.get("cfg", CFG),
                        env=attach_env(self.rundir, self.folder),
                        niri=self.niri, vm_factory=self.vm_factory,
                        locate_fn=self.locater, clock=self.clock,
                        sleep=self.clock.sleep,
                        log=lambda msg: self.logs.append(msg))

    def result(self):
        with open(os.path.join(self.rundir, "result.txt")) as fh:
            return fh.read()

    def cleanup(self):
        self.tmp.cleanup()


class TestRunReal(unittest.TestCase):
    def setUp(self):
        self.h = RunHarness()
        self.addCleanup(self.h.cleanup)

    def test_successful_run_end_to_end(self):
        # the title changes to the opened-file title only after the drop:
        # the timeline switch (fake clock 2.5 s) sits after the drag press
        # (settle 0.3 + gap 1.0 = 1.3 s), so the pre-press re-read still
        # sees the initial title and the settled read classifies "y"
        self.h.niri.zen_title_static = "Zen Browser"
        self.h.niri.title_timeline = [(2.5, "sample.png (PNG Image, 64x64) — Zen")]
        rc = self.h.run()
        self.assertEqual(rc, OK_EXIT)
        res = self.h.result()
        self.assertIn("zen_opened_file: y", res)
        self.assertIn("auto: true", res)
        self.assertIn("selection: selected", res)
        self.assertIn("raw_title: sample.png (PNG Image, 64x64) — Zen", res)
        self.assertIn("click_at: (419, 174)", res)   # 40 + (312+67), 40 + (88+46)
        self.assertIn("drop_at: (1682, 823)", res)
        vm = self.h.vms[0]
        # click (2 frames) + drag press/release: the drag's release came
        # from perform_drag's finally, close() released nothing pending
        self.assertEqual(vm.presses, 2)
        self.assertEqual(vm.releases, 2)
        # 4 threshold moves + 30 travel moves after the click move
        self.assertEqual(len(vm.moves), 1 + 4 + 30)
        # cleanup: focus restored to Kyle's workspace by id
        self.assertIn(("focus-workspace-by-id", 1), self.h.niri.actions)
        # both windows were placed floating on the empty workspace idx 2
        self.assertIn(("move-window-to-workspace", 100, 2, False), self.h.niri.actions)
        self.assertIn(("move-window-to-workspace", 200, 2, False), self.h.niri.actions)
        self.assertEqual(self.h.niri._win(100)["is_floating"], True)


    def test_working_area_offset_live_calibration(self):
        # live check, Oct 31: commands are relative to the working area
        # (below the 34 px bar), readback and pointer are in output coords
        cfg = copy.deepcopy(CFG)
        cfg["workspace"]["working_area_offset"] = [0, 34]
        self.h.niri.wa_offset = (0, 34)
        self.h.niri.zen_title_static = "Zen Browser"
        self.h.niri.title_timeline = [(2.5, "sample.png (PNG Image, 64x64) — Zen")]
        rc = self.h.run(cfg=cfg)
        self.assertEqual(rc, OK_EXIT)
        res = self.h.result()
        self.assertNotIn("aborted:", res)
        # commanded: y 40, height 1504 - 34 - 80 = 1390
        self.assertIn(("move-floating-window", 100, 40, 40), self.h.niri.actions)
        self.assertIn(("set-window-height", 100, 1390), self.h.niri.actions)
        # click from the output-coordinate readback (40, 74) + item center
        self.assertIn("click_at: (419, 208)", res)
        # drop: zen output box (1148, 74, 1068, 1390) at [0.5, 0.55]
        self.assertIn("drop_at: (1682, 838)", res)
    def test_selection_failure_aborts_before_drag(self):
        self.h.locater.selected = False  # stays unselected after the click
        rc = self.h.run()
        self.assertEqual(rc, ABORT_EXIT)
        res = self.h.result()
        self.assertIn("zen_opened_file: aborted", res)
        self.assertIn("aborted: selection check failed", res)
        vm = self.h.vms[0]
        self.assertEqual(vm.presses, 1)   # the selection click only
        self.assertEqual(vm.releases, 1)  # and it was released

    def test_focus_change_between_steps_aborts(self):
        def break_focus_after_placement():
            original = self.h.niri.windows

            def windows():
                # once both windows are floating, steal focus to Kyle's window
                if all(w["is_floating"] for w in self.h.niri.windows_list):
                    self.h.niri.focused_id = 14
                return original()
            self.h.niri.windows = windows
        break_focus_after_placement()
        rc = self.h.run()
        self.assertEqual(rc, ABORT_EXIT)
        self.assertIn("focused window is not one the runner started",
                      self.h.result())
        # no pointer activity at all: the abort happened before the click
        self.assertEqual(self.h.vms, [])

    def test_window_moved_after_placement_aborts(self):
        original = self.h.niri.move_floating_window

        def move_then_shift(wid, x, y):
            original(wid, x, y)
            if wid == 200:
                self.h.niri.move_window(100, 55.0, 40.0)
        self.h.niri.move_floating_window = move_then_shift
        rc = self.h.run()
        self.assertEqual(rc, ABORT_EXIT)
        self.assertIn("layout mismatch: window 100 at", self.h.result())

    def test_two_outputs_abort_immediately(self):
        # output_size() aborts before anything else runs; the between-steps
        # abort_reason check with the same rule is covered in TestAbortRules
        self.h.niri.outputs_map = {"DP-2": {"logical": {"width": 1}},
                                   "DP-3": {"logical": {"width": 1}}}
        rc = self.h.run()
        self.assertEqual(rc, ABORT_EXIT)
        self.assertIn("expected exactly one enabled output, found 2",
                      self.h.result())

    def test_timeout_aborts_mid_run(self):
        h = RunHarness(clock_step=5.0)
        self.addCleanup(h.cleanup)
        rc = h.run()
        self.assertEqual(rc, ABORT_EXIT)
        self.assertIn("aborted: timeout", h.result())

    def test_missing_window_aborts(self):
        self.h.niri.windows_list = self.h.niri.windows_list[:1]  # zen never appears
        rc = self.h.run()
        self.assertEqual(rc, ABORT_EXIT)
        self.assertIn("zen (instance 777)", self.h.result())

    def test_unconfigured_fallback_aborts_without_clicking(self):
        h = RunHarness(found=False)
        self.addCleanup(h.cleanup)
        cfg = {"layout": {"margin": 40}, "drag": {},
               "app": {"cosmic": {"atspi_items": False}}}  # no first_item_offset
        rc = run_real(make_args(source="cosmic"), cfg,
                      env=attach_env(h.rundir, h.folder), niri=h.niri,
                      vm_factory=h.vm_factory, locate_fn=h.locater,
                      clock=h.clock, sleep=h.clock.sleep, log=lambda m: None)
        self.assertEqual(rc, ABORT_EXIT)
        self.assertIn("unconfigured", h.result())
        self.assertEqual(h.vms, [])

    def test_fallback_offset_is_used_when_configured(self):
        h = RunHarness(found=False)
        self.addCleanup(h.cleanup)
        cfg = {"layout": {"margin": 40}, "drag": {},
               "app": {"cosmic": {"atspi_items": False,
                                  "first_item_offset": [60, 120]}}}
        rc = run_real(make_args(source="cosmic"), cfg,
                      env=attach_env(h.rundir, h.folder), niri=h.niri,
                      vm_factory=h.vm_factory, locate_fn=h.locater,
                      clock=h.clock, sleep=h.clock.sleep, log=lambda m: None)
        self.assertEqual(rc, OK_EXIT)
        self.assertIn("selection: unverified", h.result())
        # click at window origin + offset: (40+60, 40+120)
        self.assertIn("click_at: (100, 160)", h.result())

    def test_stop_after_placement(self):
        rc = self.h.run(stop_after="placement")
        self.assertEqual(rc, ABORT_EXIT)
        res = self.h.result()
        self.assertIn("aborted: stop-after placement", res)
        self.assertEqual(self.h.vms, [])  # never created before placement+verify
        self.assertIn(("focus-workspace-by-id", 1), self.h.niri.actions)

    def test_stop_after_select(self):
        rc = self.h.run(stop_after="select")
        self.assertEqual(rc, ABORT_EXIT)
        self.assertIn("aborted: stop-after select", self.h.result())
        vm = self.h.vms[0]
        self.assertEqual(vm.presses, 1)  # the selection click, no drag

    def test_no_empty_workspace_aborts(self):
        # P0a2-r1 correction 1: no empty end workspace -> abort; no reliance
        # on niri creating a workspace on demand (that stranded the two
        # windows on different workspaces)
        wss = [w for w in std_workspaces() if w["id"] != 6]
        self.h.niri.workspaces_list = wss
        rc = self.h.run()
        self.assertEqual(rc, ABORT_EXIT)
        self.assertIn("aborted: no empty workspace at the end", self.h.result())
        # nothing was moved
        self.assertEqual([a for a in self.h.niri.actions
                          if a[0].startswith("move-")], [])

    def test_kyles_zen_window_is_never_picked(self):
        # P0a2-r1 correction 6, run level: Kyle's new Zen window (another
        # instance's cgroup scope) exists from the first poll and sorts
        # before ours; the runner must still find and place OUR window
        real = autodrag.proc_cgroup
        try:
            autodrag.proc_cgroup = lambda pid: (
                "0::/app.slice/app-flatpak-app.zen_browser.zen-2943722691.scope"
                if pid == 7000 else
                "0::/app.slice/app-flatpak-app.zen_browser.zen-777.scope"
                if pid == 6000 else "")
            kyles = std_windows()[0]
            kyles.update({"id": 300, "pid": 7000, "title": "Kyle's new tab"})
            self.h.niri.windows_list.insert(0, kyles)
            rc = self.h.run()
        finally:
            autodrag.proc_cgroup = real
        self.assertEqual(rc, OK_EXIT)
        floated = [a[1] for a in self.h.niri.actions
                   if a[0] == "move-window-to-floating"]
        self.assertEqual(sorted(floated), [100, 200])
        self.assertNotIn(300, floated)

    def test_drag_releases_on_exception(self):
        # an exception inside perform_drag must still release the button
        class BoomVM(FakeVMouse):
            def move_to(self, x, y):
                raise RuntimeError("mid-drag")

        def boom_factory(w, h):
            vm = BoomVM(w, h)
            self.h.vms.append(vm)
            return vm

        self.h.vm_factory = boom_factory
        rc = self.h.run()
        self.assertEqual(rc, ABORT_EXIT)
        self.assertIn("error: RuntimeError", self.h.result())
        vm = self.h.vms[0]
        self.assertEqual(vm.releases, vm.presses)  # nothing left pressed
        self.assertTrue(vm.closed)

    def test_click_gap_held_with_atspi_recheck(self):
        # P0a2-r1 correction 2: the drag press comes at least
        # click_to_drag_gap_ms (1000) after the selection click's release,
        # with the AT-SPI recheck in between (which itself takes no fake
        # clock time — the gap is measured, not stacked on fixed sleeps)
        rc = self.h.run()
        self.assertEqual(rc, OK_EXIT)
        vm = self.h.vms[0]
        self.assertEqual(vm.presses, 2)
        gap = vm.press_at[1] - vm.release_at[0]
        self.assertGreaterEqual(gap, 1.0)
        self.assertLessEqual(gap, 1.05)  # measured, not padded

    def test_click_gap_held_without_atspi_recheck(self):
        # same rule on the unverified path (no AT-SPI item, fallback offset)
        h = RunHarness(found=False)
        self.addCleanup(h.cleanup)
        cfg = {"layout": {"margin": 40}, "drag": {},
               "app": {"cosmic": {"atspi_items": False,
                                  "first_item_offset": [60, 120]}}}
        rc = run_real(make_args(source="cosmic"), cfg,
                      env=attach_env(h.rundir, h.folder), niri=h.niri,
                      vm_factory=h.vm_factory, locate_fn=h.locater,
                      clock=h.clock, sleep=h.clock.sleep,
                      log=lambda m: h.logs.append(m))
        self.assertEqual(rc, OK_EXIT)
        vm = h.vms[0]
        gap = vm.press_at[1] - vm.release_at[0]
        self.assertGreaterEqual(gap, 1.0)
        self.assertLessEqual(gap, 1.05)

    def test_view_origin_shifts_click_and_drop(self):
        # P0a2-r1 correction 4: view_origin_in_output must move both the
        # click point and the drop point (here +10 px vertically)
        cfg = copy.deepcopy(CFG)
        cfg["workspace"]["view_origin_in_output"] = [0, 10]
        self.h.run(cfg=cfg)
        res = self.h.result()
        self.assertIn("click_at: (419, 184)", res)   # 174 + 10
        self.assertIn("drop_at: (1682, 833)", res)   # 823 + 10

    def test_title_reread_before_press(self):
        # P0a2-r1 correction 5a: the title changes while Zen starts up
        # (here: after 5 placement actions); the pre-press re-read must use
        # the new title, so a drag that did nothing classifies "n", not
        # "other"
        self.h.niri.zen_title_static = "Zen Browser"
        self.h.niri.title_after_actions = 5
        self.h.niri.title_late = "Start Page — Zen Browser"
        rc = self.h.run()
        self.assertEqual(rc, OK_EXIT)
        res = self.h.result()
        self.assertIn("zen_opened_file: n", res)
        self.assertIn("title_before: Start Page — Zen Browser", res)

    def test_transient_title_is_settled(self):
        # P0a2-r1 correction 5b: the first title change is transient; after
        # it, the runner waits 500 ms and classifies the settled read
        self.h.niri.zen_title_static = "Zen Browser"
        self.h.niri.title_timeline = [(2.5, "sample.png — Zen"),
                                      (2.95, "sample.png (PNG Image, 64x64) — Zen")]
        rc = self.h.run()
        self.assertEqual(rc, OK_EXIT)
        # first change read at fake-clock 2.6 (poll loop), settled read at
        # 3.1 -> the settled title is what raw_title carries
        self.assertIn("raw_title: sample.png (PNG Image, 64x64) — Zen",
                      self.h.result())
        self.assertIn("zen_opened_file: y", self.h.result())

    def test_hold_s_waits_before_cleanup(self):
        # P0a2-r1 minor 8: --hold-s sleeps between destroying the device
        # and the workspace restore
        rc = self.h.run(hold_s=1.5)
        self.assertEqual(rc, OK_EXIT)
        self.assertTrue(any("--hold-s: waiting 1.5 s" in m for m in self.h.logs))
        # without hold the run consumes settle 0.3 + gap 0.7 + travel 0.6 +
        # hold 0.4 + title polls; with hold 1.5 s more — compare clocks
        h2 = RunHarness()
        self.addCleanup(h2.cleanup)
        h2.niri.zen_title_static = self.h.niri.zen_title_static
        h2.run()
        self.assertAlmostEqual(self.h.clock.t - h2.clock.t, 1.5, places=6)

    def test_sigint_handler_restored_after_close(self):
        # P0a2-r1 minor 7: SIG_IGN guards vm.close(); the handler is back
        # once the close is done (the hold/cleanup stay interruptible)
        rc = self.h.run()
        self.assertEqual(rc, OK_EXIT)
        import signal as signal_mod
        self.assertIsNot(signal_mod.getsignal(signal_mod.SIGINT),
                         signal_mod.SIG_IGN)
        self.assertIsNot(signal_mod.getsignal(signal_mod.SIGTERM),
                         signal_mod.SIG_IGN)


class TestDragMotion(unittest.TestCase):
    def test_press_threshold_travel_hold_release(self):
        vm = FakeVMouse(2256, 1504)
        sleeps = []
        perform_drag(vm, {"drag": {}}, (444, 174), (1682, 823),
                     sleep=lambda s: sleeps.append(s))
        self.assertEqual(vm.presses, 1)
        self.assertEqual(vm.releases, 1)
        moves = vm.moves
        # 4 threshold steps, 3 px each (no click move here: the drag starts
        # directly at the item point)
        self.assertEqual(moves[0:4], [(447, 177), (450, 180), (453, 183), (456, 186)])
        # 30 travel steps, ending exactly on the drop point
        self.assertEqual(len(moves), 4 + 30)
        self.assertEqual(moves[-1], (1682, 823))
        # ~600 ms of 20 ms steps plus the 400 ms hold
        self.assertAlmostEqual(sum(sleeps), 30 * 0.02 + 0.4)

    def test_exception_releases(self):
        vm = FakeVMouse(2256, 1504)

        class Boom(object):
            def press(self):
                vm.presses += 1

            def move_to(self, x, y):
                raise RuntimeError("x")

            def release(self):
                vm.releases += 1

        with self.assertRaises(RuntimeError):
            perform_drag(Boom(), {"drag": {}}, (0, 0), (100, 100), sleep=lambda s: None)
        self.assertEqual((vm.presses, vm.releases), (1, 1))

    def test_travel_ms_drives_step_interval(self):
        # P0a2-r1 correction 4: travel_ms (not a fixed 20 ms) sets the pace
        vm = FakeVMouse(2256, 1504)
        sleeps = []
        perform_drag(vm, {"drag": {"travel_ms": 900}}, (0, 0), (100, 100),
                     sleep=lambda s: sleeps.append(s))
        self.assertAlmostEqual(sum(sleeps), 0.9 + 0.4)
        self.assertAlmostEqual(max(sleeps[:-1]), 0.9 / 30)  # per-step interval


# ---------------------------------------------------------------------------
# dry run
#

class TestDryRun(unittest.TestCase):
    def test_whole_run_dry_run_output(self):
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            rc = autodrag.main(["run", "--source", "dolphin", "--location",
                                "outside", "--file", "sample.png", "--dry-run"])
        self.assertEqual(rc, OK_EXIT)
        out = buf.getvalue()
        # the plan contains the exact niri commands, coordinates and events
        self.assertIn("niri msg action move-window-to-floating --id", out)
        self.assertIn("move-floating-window --id", out)
        self.assertIn("move-window-to-workspace --window-id", out)
        self.assertIn("focus-workspace", out)
        # default dry-run size: DP-2 observed 2026-10-31 (2560x1440), with
        # the live working-area offset [0, 34] from apps.toml: height
        # 1440 - 34 - 80 = 1326, drop y = 74 + int(1326 * 0.55) = 803
        self.assertIn("working area offset [0, 34]", out)
        self.assertIn("source=(40, 40, 1220, 1326) zen=(1300, 40, 1220, 1326)", out)
        self.assertIn("drop point: (1910, 803)", out)
        self.assertIn("press (EV_KEY BTN_LEFT 1 + SYN)", out)
        self.assertIn("move_to lerp(item, drop, 30/30)", out)
        self.assertIn("hold 400 ms", out)
        self.assertIn("gap:    >= 1000 ms", out)
        self.assertIn("abort rules", out)
        return out

    def test_dry_run_stop_after(self):
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            rc = autodrag.main(["run", "--source", "thunar", "--location",
                                "downloads", "--file", "sample.png",
                                "--stop-after", "select", "--dry-run"])
        self.assertEqual(rc, OK_EXIT)
        self.assertIn("stop-after: select", buf.getvalue())

    def test_dry_run_never_launches_anything(self):
        # main() with --dry-run must return before the run.sh exec path
        # (no GIMBAL_* env in this test process — if it tried to attach it
        # would exec run.sh; reaching here proves it did not)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            rc = autodrag.main(["run", "--source", "nautilus", "--location",
                                "outside", "--file", "sample.png", "--dry-run"])
        self.assertEqual(rc, OK_EXIT)

    def test_parse_args_rejects_bad_input(self):
        with self.assertRaises(SystemExit):
            autodrag.parse_args(["run", "--source", "vim"])
        with self.assertRaises(SystemExit):
            autodrag.parse_args(["run", "--source", "dolphin", "--location", "tmp"])
        with self.assertRaises(SystemExit):
            autodrag.parse_args(["run", "--source", "dolphin", "--location",
                                 "outside", "--stop-after", "middle"])

class TestWaitLayout(unittest.TestCase):
    def test_retries_until_the_size_settles(self):
        calls = []

        def flaky(n, state, note):
            calls.append(1)
            if len(calls) < 3:
                raise autodrag.Abort("layout mismatch: still resizing")

        clock = FakeClock()
        orig = autodrag.verify_layout
        autodrag.verify_layout = flaky
        try:
            autodrag.wait_layout(None, None, None, clock, clock.sleep)
        finally:
            autodrag.verify_layout = orig
        self.assertEqual(len(calls), 3)

    def test_gives_up_after_the_timeout(self):
        def never(n, state, note):
            raise autodrag.Abort("layout mismatch: wrong place")

        clock = FakeClock()
        orig = autodrag.verify_layout
        autodrag.verify_layout = never
        try:
            with self.assertRaises(autodrag.Abort):
                autodrag.wait_layout(None, None, None, clock, clock.sleep)
        finally:
            autodrag.verify_layout = orig
        self.assertGreaterEqual(clock(), 3.0)

if __name__ == "__main__":
    unittest.main(verbosity=2)

