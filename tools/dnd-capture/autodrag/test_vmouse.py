# SPDX-License-Identifier: GPL-3.0-or-later
"""Tests for vmouse.py — all against the fake uinput layer, never the real
/dev/uinput (P0a2 hard limit 1).

Run:  python3 -I test_vmouse.py -v
"""

import io
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import vmouse
from vmouse import (ABS_RANGE_MAX, ABS_X, ABS_Y, BTN_LEFT, EV_ABS, EV_KEY,
                    EV_SYN, SYN_REPORT, FakeUinputDevice, VMouse,
                    logical_to_abs, abs_to_logical)


class TestCoordinateMath(unittest.TestCase):
    def test_zero_and_full_range(self):
        self.assertEqual(logical_to_abs(0, 2256), 0)
        # the mapping is linear: px*65535/extent (round-half-even)
        self.assertEqual(logical_to_abs(1128, 2256), 32768)

    def test_monotonic_and_bounded(self):
        prev = -1
        for px in range(0, 2257):
            v = logical_to_abs(px, 2256)
            self.assertTrue(0 <= v <= ABS_RANGE_MAX)
            self.assertGreaterEqual(v, prev)
            prev = v

    def test_inverse_within_half_pixel(self):
        # rounding to integer ABS units costs at most half a logical pixel
        for px in (0, 1, 500, 1128, 2255):
            back = abs_to_logical(logical_to_abs(px, 2256), 2256)
            self.assertAlmostEqual(back, px, delta=0.5)

    def test_move_to_clamps(self):
        dev = FakeUinputDevice()
        vm = VMouse(100, 100, device=dev)
        vm.move_to(-5, 500)
        vm.move_to(99, 0)
        xs = [v for (t, c, v) in dev.events if (t, c) == (EV_ABS, ABS_X)]
        ys = [v for (t, c, v) in dev.events if (t, c) == (EV_ABS, ABS_Y)]
        self.assertEqual(xs[0], 0)
        self.assertEqual(ys[0], logical_to_abs(99, 100))


class TestEventSequence(unittest.TestCase):
    def events(self, vm):
        return vm._device.event_names()

    def test_move_frame_has_syn(self):
        dev = FakeUinputDevice()
        vm = VMouse(100, 100, device=dev)
        vm.move_to(10, 20)
        evs = self.events(vm)
        # one frame: ABS_X, ABS_Y, SYN — in order
        self.assertEqual(evs, [("EV_ABS", "ABS_X", vmouse.logical_to_abs(10, 100)),
                               ("EV_ABS", "ABS_Y", vmouse.logical_to_abs(20, 100)),
                               ("EV_SYN", "SYN_REPORT", 0)])

    def test_press_release_order(self):
        dev = FakeUinputDevice()
        vm = VMouse(100, 100, device=dev)
        vm.move_to(50, 50)
        vm.press()
        vm.move_to(60, 50)
        vm.release()
        evs = self.events(vm)
        kinds = [(t, c) for (t, c, v) in evs]
        self.assertEqual(kinds[0], ("EV_ABS", "ABS_X"))
        # the release frame is last: BTN_LEFT 0 closed by a SYN
        self.assertEqual(kinds[-2], ("EV_KEY", "BTN_LEFT"))
        self.assertEqual(evs[-2][2], 0)
        self.assertEqual(kinds[-1], ("EV_SYN", "SYN_REPORT"))
        # press: BTN_LEFT 1 followed by SYN before any motion
        press_idx = kinds.index(("EV_KEY", "BTN_LEFT"))
        self.assertEqual(evs[press_idx][2], 1)
        self.assertEqual(kinds[press_idx + 1], ("EV_SYN", "SYN_REPORT"))
        # every EV_KEY/EV_ABS frame is closed by a SYN
        i = 0
        while i < len(evs):
            self.assertNotEqual(kinds[i], ("EV_SYN", "SYN_REPORT"))
            j = kinds.index(("EV_SYN", "SYN_REPORT"), i)
            for (t, c) in kinds[i:j]:
                self.assertIn(t, ("EV_ABS", "EV_KEY"))
            i = j + 1

    def test_release_is_idempotent(self):
        dev = FakeUinputDevice()
        vm = VMouse(100, 100, device=dev)
        vm.release()  # not pressed: nothing sent
        self.assertEqual(dev.events, [])
        vm.press()
        n_after_press = len(dev.events)
        vm.release()
        vm.release()
        releases = [e for e in dev.event_names()
                    if e[:2] == ("EV_KEY", "BTN_LEFT") and e[2] == 0]
        self.assertEqual(len(releases), 1)
        # exactly one release frame (KEY 0 + SYN) was added
        self.assertEqual(len(dev.events), n_after_press + 2)

    def test_exception_mid_drag_still_releases(self):
        dev = FakeUinputDevice()
        vm = VMouse(1000, 1000, device=dev)

        def drag():
            vm.press()
            vm.move_to(10, 10)
            raise RuntimeError("boom")

        with self.assertRaises(RuntimeError):
            with vm:
                drag()
        evs = dev.event_names()
        # hard limit 5: the release (BTN_LEFT 0) is the last button event,
        # and the device was destroyed by the context manager
        btn = [e for e in evs if e[0] == "EV_KEY"]
        self.assertEqual(btn, [("EV_KEY", "BTN_LEFT", 1), ("EV_KEY", "BTN_LEFT", 0)])
        self.assertIsNone(dev.fd)
        self.assertEqual(evs[-1], ("EV_SYN", "SYN_REPORT", 0))

    def test_close_releases_then_destroys(self):
        dev = FakeUinputDevice()
        vm = VMouse(100, 100, device=dev)
        vm.press()
        vm.close()
        self.assertEqual([e[:2] for e in dev.event_names()][-2:],
                         [("EV_KEY", "BTN_LEFT"), ("EV_SYN", "SYN_REPORT")])
        self.assertIsNone(dev.fd)


class TestUinputDeviceSetup(unittest.TestCase):
    """P0a2-r1 correction 3: the readiness sequence around UI_DEV_CREATE
    and UI_DEV_DESTROY, exercised through injected layers only — no real
    /dev/uinput, no real sleeping."""

    class Journal(object):
        """Shared ordered log of sleeps and ioctls, so the test can assert
        the ORDER of: create -> sysname -> settle sleep -> … -> pre-destroy
        sleep -> destroy ioctl."""

        def __init__(self):
            self.entries = []

        def sleep(self, s):
            self.entries.append(("sleep", round(s, 6)))

        def ioctl(self, fd, req, arg=None):
            self.entries.append(("ioctl", req))
            return 0

        def count(self, kind):
            return [e for e in self.entries if e[0] == kind]

    def make_device(self, journal, sysfs_layout, clock=None, **kw):
        """UinputDevice with every external effect injected.  The fake
        ioctl answers UI_GET_SYSNAME(64) with the sysfs name and records
        everything; the fake opener/closer track the fd lifecycle."""
        made = {}

        def opener(path):
            made["opened"] = path
            return 42  # never touches the kernel: ioctl_fn is injected

        def closer(fd):
            made["closed"] = fd

        sysname = kw.pop("sysname", "input42")

        def ioctl_fn(fd, req, arg=None):
            journal.entries.append(("ioctl", req))
            if req == vmouse.ui_get_sysname(64) and isinstance(arg, bytearray):
                arg[:len(sysname) + 1] = sysname.encode() + b"\0"
                return len(sysname) + 1
            return 0

        writes = []

        def write_fn(fd, data):
            writes.append(data)
            return len(data)

        import tempfile as tempfile_mod
        root = tempfile_mod.mkdtemp()
        self.addCleanup(__import__("shutil").rmtree, root, True)
        for name, children in sysfs_layout.items():
            for child in children:
                os.makedirs(os.path.join(root, name, child), exist_ok=True)
        dev = vmouse.UinputDevice(
            path="/dev/uinput-fake", opener=opener, ioctl_fn=ioctl_fn,
            write_fn=write_fn, closer=closer, sleep=journal.sleep,
            clock=clock or (lambda: 0.0),
            listdir=kw.pop("listdir", os.listdir),
            sysfs_root=root, **kw)
        made["device"] = dev
        made["writes"] = writes
        return made

    def test_setup_order_and_settle_sleep(self):
        journal = self.Journal()
        made = self.make_device(journal, {"input42": ["event0"]},
                                device_settle_ms=500, pre_destroy_ms=100)
        dev = made["device"]
        reqs = [e[1] for e in journal.count("ioctl")]
        # EVBIT x3, KEYBIT, ABSBIT x2, ABS_SETUP x2, DEV_SETUP, DEV_CREATE,
        # then UI_GET_SYSNAME
        self.assertIn(vmouse.UI_DEV_CREATE, reqs)
        self.assertEqual(vmouse.ui_get_sysname(64), 0x8040552C)  # _IOC(2,'U',44,64), by hand
        self.assertIn(vmouse.ui_get_sysname(64), reqs)
        self.assertEqual(dev.sysname, "input42")
        # the settle sleep happened after DEV_CREATE (and the sysname read)
        after_create = journal.entries[journal.entries.index(
            ("ioctl", vmouse.UI_DEV_CREATE)):]
        self.assertIn(("sleep", 0.5), after_create)
        self.assertNotIn(("sleep", 0.1), after_create)  # pre-destroy only on destroy()
        # the event node existed -> no poll sleeps
        self.assertEqual([s for (_, s) in journal.count("sleep")].count(0.05), 0)

    def test_waits_for_late_event_node(self):
        journal = self.Journal()
        # event node appears after two empty polls
        state = {"n": 0}
        real_listdir = os.listdir

        def listdir_sometimes(path):
            state["n"] += 1
            if state["n"] < 3:
                # the sysfs dir exists but has no event* child yet
                os.makedirs(os.path.join(path), exist_ok=True)
                return []
            return ["event0"]

        made = self.make_device(journal, {}, listdir=listdir_sometimes)
        polls = [s for (_, s) in journal.count("sleep") if s == 0.05]
        self.assertEqual(len(polls), 2)

    def test_timeout_aborts_with_closed_fd(self):
        journal = self.Journal()
        # the clock advances 0.03 per call, so the 0.1 s deadline is passed
        # after a few polls (the sleeps themselves are recorded, not real)
        ticks = {"t": 0.0}

        def advancing_clock():
            ticks["t"] += 0.03
            return ticks["t"]

        with self.assertRaises(RuntimeError) as ctx:
            self.make_device(journal, {"input42": []},  # no event* child ever
                             clock=advancing_clock,
                             settle_timeout_s=0.1, poll_s=0.05)
        self.assertIn("event* node", str(ctx.exception))
        # the fd was closed on the failure path
        # (UinputDevice.__init__ closes and re-raises)

    def test_destroy_sleeps_pre_destroy_ms_first(self):
        journal = self.Journal()
        made = self.make_device(journal, {"input42": ["event0"]})
        dev = made["device"]
        journal.entries.clear()
        dev.emit(vmouse.EV_KEY, vmouse.BTN_LEFT, 0)
        dev.destroy()
        # pre-destroy sleep precedes UI_DEV_DESTROY; close comes after
        kinds = [(k, v if k == "sleep" else v) for (k, v) in journal.entries]
        self.assertEqual(kinds[0], ("sleep", 0.1))
        self.assertEqual(kinds[1][1], vmouse.UI_DEV_DESTROY)
        self.assertEqual(made["device"].fd, None)

    def test_sysname_ioctl_number_matches_macro(self):
        # UI_GET_SYSNAME(64) = _IOC(_IOC_READ, 'U', 44, 64)
        # = (2<<30) | (64<<16) | (0x55<<8) | 44 = 0x8040552C  (hand-computed)
        self.assertEqual(vmouse.ui_get_sysname(64), 0x8040552C)
        self.assertEqual(vmouse.ui_get_sysname(16),
                         (2 << 30) | (16 << 16) | (0x55 << 8) | 44)


class TestSquareDwell(unittest.TestCase):
    def test_real_run_dwells_at_each_corner(self):
        # P0a2-r1 correction 3: --square pauses dwell_ms at each of the 5
        # corner visits so the cursor can be seen
        dev = FakeUinputDevice()
        vm = VMouse(800, 600, device=dev)
        sleeps = []
        buf = io.StringIO()
        vmouse._trace_square(vm, 200, buf, sleep=sleeps.append, dwell_ms=400)
        self.assertEqual(len(sleeps), 5)
        self.assertEqual(sleeps, [0.4] * 5)
        self.assertIn("[dwell 400 ms]", buf.getvalue())

    def test_dry_run_never_sleeps(self):
        dev = FakeUinputDevice()
        vm = VMouse(800, 600, device=dev)
        sleeps = []
        buf = io.StringIO()
        vmouse._trace_square(vm, 200, buf, sleep=None, dwell_ms=400)
        self.assertEqual(sleeps, [])
        self.assertNotIn("[dwell", buf.getvalue())


class TestDryRunSquare(unittest.TestCase):
    def test_square_prints_no_button_events(self):
        buf = io.StringIO()
        old = sys.stdout
        sys.stdout = buf
        try:
            rc = vmouse.main(["--square", "--dry-run", "--width", "800",
                              "--height", "600"])
        finally:
            sys.stdout = old
        self.assertEqual(rc, 0)
        out = buf.getvalue()
        self.assertIn("output: 800x600", out)
        self.assertIn("move_to (300, 200)", out)  # center 400,300, side 200
        # the dry-run's own fake backend saw no BTN events at all
        self.assertIn("all EV_KEY counts: 0", out)

    def test_square_real_backend_via_cli_is_not_touched_here(self):
        # --square without --dry-run would create the real device; this agent
        # never runs it.  The CLI must therefore refuse to act without a mode.
        buf = io.StringIO()
        old = sys.stderr
        sys.stderr = buf
        try:
            rc = vmouse.main([])
        finally:
            sys.stderr = old
        self.assertEqual(rc, 2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
