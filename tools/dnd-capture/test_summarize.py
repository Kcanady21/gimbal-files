# SPDX-License-Identifier: GPL-3.0-or-later
"""Tests for summarize.py against the hand-written fixtures in fixtures/.

Run from this directory with either:
    python3 -I test_summarize.py -v
    python3 -m unittest test_summarize -v

(`python3 -I -m unittest ...` does not work: isolated mode does not put this
directory on sys.path, so unittest cannot find the module.  pytest is not
installed on this machine, so this is plain unittest.)
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
FIXTURES = os.path.join(HERE, "fixtures")
SUMMARIZE = os.path.join(HERE, "summarize.py")

# isolated mode (-I) does not put the script's directory on sys.path
if HERE not in sys.path:
    sys.path.insert(0, HERE)
import summarize  # noqa: E402


def run_summarize_dir(work):
    """Run summarize.py on an existing directory; return (json, markdown)."""
    proc = subprocess.run([sys.executable, "-I", SUMMARIZE, work],
                          capture_output=True, text=True, timeout=60)
    if proc.returncode != 0:
        raise AssertionError("summarize.py failed on %s:\n%s%s"
                             % (work, proc.stdout, proc.stderr))
    with open(os.path.join(work, "summary.json")) as fh:
        got = json.load(fh)
    md_path = os.path.join(work, "summary.md")
    md = None
    if os.path.exists(md_path):
        with open(md_path) as fh:
            md = fh.read()
    return got, md


def run_summarize(fixdir):
    """Copy a fixture to a temp dir (named like the fixture, so the `run`
    field matches), run summarize.py there, return the JSON and markdown."""
    tmp = tempfile.mkdtemp(prefix="gimbal-dnd-test-")
    work = os.path.join(tmp, os.path.basename(fixdir.rstrip("/")))
    try:
        shutil.copytree(fixdir, work)
        os.remove(os.path.join(work, "expected-summary.json"))
        return run_summarize_dir(work)
    finally:
        shutil.rmtree(tmp)


def load_expected(name):
    with open(os.path.join(FIXTURES, name, "expected-summary.json")) as fh:
        return json.load(fh)


class TestLineParser(unittest.TestCase):
    """The parser accepts every WAYLAND_DEBUG shape we know about."""

    def _parse(self, line):
        import tempfile as tf
        with tf.NamedTemporaryFile("w", suffix=".log", delete=False) as fh:
            fh.write(line + "\n")
            path = fh.name
        try:
            msgs = summarize.parse_wayland_log(path)
        finally:
            os.unlink(path)
        self.assertEqual(len(msgs), 1, "line did not parse: %r" % line)
        return msgs[0]

    def test_brief_example_hash_no_queue(self):
        m = self._parse('[  12345.678]  -> wl_data_source#45.offer("text/uri-list")')
        self.assertTrue(m.request)
        self.assertEqual(m.obj, "wl_data_source#45")
        self.assertEqual(m.method, "offer")
        self.assertEqual(m.args, '"text/uri-list"')

    def test_libwayland_event_no_arrow(self):
        m = self._parse('[12345.678] wl_registry#2.global(17, "wl_data_device_manager", 3)')
        self.assertFalse(m.request)
        self.assertEqual(m.iface, "wl_registry")

    def test_queue_prefix(self):
        m = self._parse('[1330243.871] {Display Queue} wl_display#1.delete_id(3)')
        self.assertEqual(m.queue, "Display Queue")
        self.assertFalse(m.request)

    def test_queue_prefix_on_request(self):
        m = self._parse('[1000021.000] {Default Queue}  -> wl_data_device#7.start_drag(wl_data_source#45, wl_surface#30, nil, 1234)')
        self.assertTrue(m.request)
        self.assertEqual(m.method, "start_drag")

    def test_at_separator_rs_backend_request(self):
        m = self._parse('[  2000010.050][rs] -> wl_data_source@30.offer("text/uri-list")')
        self.assertTrue(m.request)
        self.assertEqual(m.obj, "wl_data_source#30")  # normalised to # in .obj

    def test_at_separator_rs_backend_event(self):
        m = self._parse('[  2000025.000][rs] <- wl_data_source@30.cancelled()')
        self.assertFalse(m.request)
        self.assertEqual(m.method, "cancelled")

    def test_discarded_marker(self):
        m = self._parse('[  2000025.000][rs] -> [discarded] wl_data_source@30.set_actions(1)')
        self.assertTrue(m.request)


class TestLifetimes(unittest.TestCase):
    """Object identity follows creation -> destroy/delete_id (P0a-r1)."""

    def _log(self, lines):
        fd, path = tempfile.mkstemp(suffix=".log")
        with os.fdopen(fd, "w") as fh:
            fh.write("\n".join(lines) + "\n")
        try:
            return summarize.build_lifetimes(summarize.parse_wayland_log(path))
        finally:
            os.unlink(path)

    def test_creation_via_new_id_and_death_via_delete_id(self):
        lt = self._log([
            '[1.0]  -> wl_data_device_manager#5.create_data_source(new id wl_data_source#30)',
            '[2.0] wl_data_source#30.target("text/uri-list")',
            '[3.0] wl_display#1.delete_id(30)',
            '[4.0] wl_data_source#30.offer("after death")',
        ])
        self.assertEqual([(l.iface, l.oid, l.birth, l.death) for l in lt],
                         [("wl_data_source", 30, 0, 2)])

    def test_id_reuse_starts_new_lifetime(self):
        lt = self._log([
            '[1.0] wl_data_device#9.data_offer(new id wl_data_offer#60)',
            '[2.0]  -> wl_data_offer#60.destroy()',
            '[3.0] wl_display#1.delete_id(60)',
            '[4.0] wl_data_device#9.data_offer(new id wl_data_offer#60)',
            '[5.0] wl_data_offer#60.offer("second life")',
        ])
        self.assertEqual(len(lt), 2)
        self.assertEqual([l.birth for l in lt], [0, 3])
        self.assertEqual([l.death for l in lt], [1, None])
        # the second life contains index 4, the first does not
        self.assertIsNone(summarize.containing(lt, "wl_data_offer", 60, 1))
        self.assertIs(summarize.containing(lt, "wl_data_offer", 60, 4), lt[1])

    def test_data_offer_without_new_id_prefix_still_creates(self):
        lt = self._log([
            '[1.0] wl_data_device#9.data_offer(wl_data_offer#67)',
            '[2.0] wl_data_offer#67.offer("text/uri-list")',
        ])
        self.assertEqual([(l.iface, l.oid, l.birth) for l in lt],
                         [("wl_data_offer", 67, 0)])

    def test_new_id_with_open_lifetime_starts_new_lifetime(self):
        # P0a-r2 correction 2: the protocol never reuses a live id, so a
        # `new id` whose id still has an open lifetime must close that
        # lifetime and start a fresh one, never merge the two objects.
        lt = self._log([
            '[1.0]  -> wl_data_device_manager#5.create_data_source(new id wl_data_source#30)',
            '[2.0]  -> wl_data_source#30.offer("first object")',
            '[3.0]  -> wl_data_device_manager#5.create_data_source(new id wl_data_source#30)',
            '[4.0]  -> wl_data_source#30.offer("second object")',
        ])
        self.assertEqual(len(lt), 2)
        self.assertEqual([(l.birth, l.death) for l in lt], [(0, 2), (2, None)])
        self.assertIs(summarize.containing(lt, "wl_data_source", 30, 1), lt[0])
        self.assertIs(summarize.containing(lt, "wl_data_source", 30, 3), lt[1])
        # the second object's lifetime starts at the creation line itself
        self.assertFalse(lt[0].contains(2))
        self.assertTrue(lt[1].contains(2))

    def test_lifetime_labels_carry_line_spans(self):
        # P0a-r3 correction 2: labels distinguish reused ids via the
        # lifetime's line span in the raw log
        import tempfile as tf
        lines = [
            '[1.0]  -> wl_data_device_manager#5.create_data_source(new id wl_data_source#30)',
            '[2.0]  -> wl_data_source#30.offer("a")',
            '[3.0]  -> wl_data_source#30.destroy()',
            '[4.0] wl_display#1.delete_id(30)',
            '[5.0]  -> wl_data_device_manager#5.create_data_source(new id wl_data_source#31)',
        ]
        with tf.NamedTemporaryFile("w", suffix=".log", delete=False) as fh:
            fh.write("\n".join(lines) + "\n")
            path = fh.name
        try:
            msgs = summarize.parse_wayland_log(path)
            lt = summarize.build_lifetimes(msgs)
        finally:
            os.unlink(path)
        self.assertEqual(summarize.lifetime_label(lt[0], msgs),
                         "wl_data_source#30 (lines 1\u20133)")
        self.assertEqual(summarize.lifetime_label(lt[1], msgs),
                         "wl_data_source#31 (from line 5)")


class TestFixtureF1SuccessClipboard(unittest.TestCase):
    """Successful drag with a clipboard source mixed in (# style, queues)."""

    def setUp(self):
        self.summary, self.md = run_summarize(os.path.join(FIXTURES, "f1-success-clipboard"))

    def test_matches_expected_json(self):
        self.assertEqual(self.summary, load_expected("f1-success-clipboard"))

    def test_drag_source_is_not_the_clipboard(self):
        src = self.summary["source_side"]
        self.assertEqual(src["drag_source_object"], "wl_data_source#45")
        self.assertEqual(src["other_sources_excluded"],
                         ["wl_data_source#21 (from line 6)"])

    def test_offers_and_ending(self):
        src = self.summary["source_side"]
        self.assertEqual(src["offers"], ["text/uri-list", "text/plain;charset=utf-8"])
        self.assertEqual(src["set_actions"][0]["decoded"], [["copy", "move"]])
        self.assertEqual(src["send"], [{"mime": "text/uri-list", "fd": "42"}])
        self.assertEqual(src["ending"], "dnd_drop_performed+dnd_finished")

    def test_zen_side(self):
        zen = self.summary["zen_side"]
        self.assertEqual(zen["drag_offer_object"], "wl_data_offer#67")
        self.assertEqual(zen["offers"], ["text/uri-list", "text/plain;charset=utf-8"])
        self.assertEqual(zen["receive"], [{"mime": "text/uri-list", "fd": "7"}])
        self.assertTrue(zen["finish"])

    def test_non_portal_dbus_filtered(self):
        self.assertEqual(self.summary["dbus"]["portal_calls"], [])

    def test_result(self):
        self.assertEqual(self.summary["result"]["zen_opened_file"], True)

    def test_markdown(self):
        self.assertIn("wl_data_source#45", self.md)
        self.assertIn("dnd_drop_performed+dnd_finished", self.md)


class TestFixtureF2Cancelled(unittest.TestCase):
    """Cancelled drag (@ style, [rs] tag, <- events, no dbus.log)."""

    def setUp(self):
        self.summary, self.md = run_summarize(os.path.join(FIXTURES, "f2-cancelled"))

    def test_matches_expected_json(self):
        self.assertEqual(self.summary, load_expected("f2-cancelled"))

    def test_cancelled_ending(self):
        self.assertEqual(self.summary["source_side"]["ending"], "cancelled")
        self.assertTrue(self.summary["source_side"]["cancelled"])
        self.assertFalse(self.summary["zen_side"]["finish"])
        self.assertEqual(self.summary["zen_side"]["receive"], [])
        self.assertEqual(len(self.summary["zen_side"]["leave"]), 1)

    def test_missing_dbus_log_is_not_fatal(self):
        self.assertFalse(self.summary["parsing_notes"]["dbus_log_present"])
        self.assertEqual(self.summary["dbus"]["portal_calls"], [])

    def test_result(self):
        self.assertEqual(self.summary["result"]["zen_opened_file"], False)


class TestFixtureF3Portal(unittest.TestCase):
    """Successful drag with document-portal traffic in dbus.log."""

    def setUp(self):
        self.summary, self.md = run_summarize(os.path.join(FIXTURES, "f3-portal-success"))

    def test_matches_expected_json(self):
        self.assertEqual(self.summary, load_expected("f3-portal-success"))

    def test_portal_calls_in_order(self):
        calls = self.summary["dbus"]["portal_calls"]
        self.assertEqual([c["member"] for c in calls],
                         ["StartTransfer", "AddFiles", "RetrieveFiles", "StopTransfer"])
        for c in calls:
            self.assertEqual(c["sender"], ":1.900")

    def test_transfer_key_and_retrieved_paths(self):
        calls = {c["member"]: c for c in self.summary["dbus"]["portal_calls"]}
        self.assertEqual(calls["StartTransfer"]["returned"], ["42"])
        self.assertEqual(calls["AddFiles"]["strings"], ["42"])
        self.assertEqual(calls["RetrieveFiles"]["returned"],
                         ["/home/kyle/.var/app/app.zen_browser.zen/doc/2f1a/gimbal-dnd-test/sample.png"])

    def test_zen_received_uri_list(self):
        self.assertEqual(self.summary["zen_side"]["receive"][0]["mime"], "text/uri-list")
        self.assertEqual(self.summary["source_side"]["ending"], "dnd_drop_performed+dnd_finished")


class TestFixtureF4RegistryMidDrag(unittest.TestCase):
    """A registry created between the offers and start_drag must not hide
    the drag's offers (the removed connection counter failed here)."""

    def setUp(self):
        self.summary, self.md = run_summarize(os.path.join(FIXTURES, "f4-registry-mid-drag"))

    def test_matches_expected_json(self):
        self.assertEqual(self.summary, load_expected("f4-registry-mid-drag"))

    def test_offers_survive_the_mid_drag_registry(self):
        src = self.summary["source_side"]
        self.assertEqual(src["offers"], ["text/uri-list", "text/plain;charset=utf-8"])
        self.assertEqual(src["set_actions"], [{"args": [3], "decoded": [["copy", "move"]]}])
        self.assertEqual(src["ending"], "dnd_drop_performed+dnd_finished")
        self.assertEqual(src["drag_source_object"], "wl_data_source#40")
        self.assertEqual(src["other_sources_excluded"], [])

    def test_zen_log_empty_is_tolerated(self):
        self.assertEqual(self.summary["zen_side"]["drag_offer_object"], None)
        self.assertEqual(self.summary["zen_side"]["receive"], [])
        self.assertEqual(self.summary["parsing_notes"]["zen_messages"], 0)

    def test_drag_seen(self):
        self.assertTrue(self.summary["drag_seen"])
        self.assertEqual(self.summary["source_side"]["start_drag_count"], 1)


class TestFixtureF5Retry(unittest.TestCase):
    """One cancelled drag, then one successful drag, in the same logs; the
    second Zen offer reuses id 60 after delete_id."""

    def setUp(self):
        self.summary, self.md = run_summarize(os.path.join(FIXTURES, "f5-retry"))

    def test_matches_expected_json(self):
        self.assertEqual(self.summary, load_expected("f5-retry"))

    def test_last_attempt_is_summarized(self):
        src = self.summary["source_side"]
        self.assertEqual(src["start_drag_count"], 2)
        self.assertEqual(src["attempt"], "attempt 2 of 2")
        self.assertEqual(src["drag_source_object"], "wl_data_source#31")
        self.assertEqual(src["offers"], ["text/uri-list", "text/plain;charset=utf-8"])
        self.assertEqual(src["send"], [{"mime": "text/uri-list", "fd": "44"}])
        self.assertFalse(src["cancelled"])
        self.assertEqual(src["ending"], "dnd_drop_performed+dnd_finished")
        self.assertEqual(src["other_sources_excluded"],
                         ["wl_data_source#30 (lines 3\u20139)"])

    def test_zen_offer_is_the_reused_id_second_lifetime(self):
        zen = self.summary["zen_side"]
        self.assertEqual(zen["drag_offer_object"], "wl_data_offer#60")
        self.assertEqual(zen["drop_count"], 1)
        self.assertEqual(zen["offers"], ["text/uri-list", "text/plain;charset=utf-8"])
        self.assertEqual(zen["receive"], [{"mime": "text/uri-list", "fd": "12"}])
        self.assertEqual(zen["accept"], [{"serial": "950", "mime": "text/uri-list"}])
        self.assertTrue(zen["finish"])
        # first attempt's artifacts (set_actions(1,1), single offer) excluded
        self.assertEqual(zen["set_actions"], [{"args": [1, 3],
                                               "decoded": [["copy"], ["copy", "move"]]}])

    def test_markdown_shows_attempts(self):
        self.assertIn("attempt 2 of 2", self.md)
        self.assertIn("start_drag(s)", self.md)


class TestFixtureF6SuccessThenCancel(unittest.TestCase):
    """Successful drag then a cancelled one (inputs by Claude): the source
    summarizes the last attempt (cancelled), Zen summarizes the last enter
    (the cancelled attempt, offer id from the client-allocated range), and
    the attempt counts agree."""

    def setUp(self):
        self.summary, self.md = run_summarize(os.path.join(FIXTURES, "f6-success-then-cancel"))

    def test_matches_expected_json(self):
        self.assertEqual(self.summary, load_expected("f6-success-then-cancel"))

    def test_source_side_unchanged(self):
        src = self.summary["source_side"]
        self.assertEqual(src["drag_source_object"], "wl_data_source#40")
        self.assertEqual(src["attempt"], "attempt 2 of 2")
        self.assertEqual(src["offers"], ["text/uri-list", "application/x-second-attempt"])
        self.assertEqual(src["ending"], "cancelled")

    def test_reused_id_lifetimes_are_labeled(self):
        # P0a-r3 correction 2: both lifetimes of wl_data_source#40 stay
        # distinguishable (the id is reused after delete_id)
        src = self.summary["source_side"]
        self.assertEqual(src["all_data_sources"],
                         ["wl_data_source#40 (lines 3\u20139)",
                          "wl_data_source#40 (from line 11)"])
        self.assertEqual(src["other_sources_excluded"],
                         ["wl_data_source#40 (lines 3\u20139)"])

    def test_zen_side_is_the_last_enter(self):
        zen = self.summary["zen_side"]
        self.assertEqual(zen["drag_offer_object"], "wl_data_offer#4278190081")
        self.assertEqual(zen["offers"], ["text/uri-list", "application/x-second-attempt"])
        self.assertEqual(zen["receive"], [])
        self.assertFalse(zen["finish"])
        self.assertFalse(zen["dropped"])
        self.assertEqual(zen["attempt_count"], 2)

    def test_attempts_match(self):
        self.assertTrue(self.summary["attempts_match"])
        self.assertNotIn("check the raw logs", self.md)


class TestAttemptsMismatch(unittest.TestCase):
    """When the sides see different attempt counts (2 start_drags, 1 enter):
    attempts_match is false and summary.md carries the warning line."""

    def test_mismatch_detected_and_warned(self):
        tmp = tempfile.mkdtemp(prefix="gimbal-dnd-test-mismatch-")
        try:
            with open(os.path.join(tmp, "source.log"), "w") as fh:
                fh.write(
                    '[100.000]  -> wl_data_device_manager#5.get_data_device(new id wl_data_device#7, wl_seat#6)\n'
                    '[101.000]  -> wl_data_device_manager#5.create_data_source(new id wl_data_source#30)\n'
                    '[102.000]  -> wl_data_device#7.start_drag(wl_data_source#30, wl_surface#9, nil, 1)\n'
                    '[103.000]  -> wl_data_device_manager#5.create_data_source(new id wl_data_source#31)\n'
                    '[104.000]  -> wl_data_device#7.start_drag(wl_data_source#31, wl_surface#9, nil, 2)\n')
            with open(os.path.join(tmp, "zen.log"), "w") as fh:
                fh.write(
                    '[101.500] wl_data_device#9.data_offer(new id wl_data_offer#50)\n'
                    '[102.500] wl_data_device#9.enter(1, wl_surface#55, 0, 0, wl_data_offer#50)\n')
            summary, md = run_summarize_dir(tmp)
            self.assertFalse(summary["attempts_match"])
            self.assertEqual(summary["source_side"]["start_drag_count"], 2)
            self.assertEqual(summary["zen_side"]["attempt_count"], 1)
            self.assertIn("source saw 2 drags, Zen saw 1; check the raw logs", md)
        finally:
            shutil.rmtree(tmp)


class TestNoDrag(unittest.TestCase):
    """A run whose source.log never calls start_drag (correction 4)."""

    def test_no_drag_seen(self):
        tmp = tempfile.mkdtemp(prefix="gimbal-dnd-test-nodrag-")
        try:
            with open(os.path.join(tmp, "source.log"), "w") as fh:
                fh.write('[100.000] {Default Queue}  -> wl_display#1.get_registry(new id wl_registry#2)\n'
                         '[100.100]  -> wl_data_device_manager#5.get_data_device(new id wl_data_device#7, wl_seat#6)\n')
            open(os.path.join(tmp, "zen.log"), "w").close()
            summary, md = run_summarize_dir(tmp)
            self.assertFalse(summary["drag_seen"])
            self.assertEqual(summary["source_side"]["start_drag_count"], 0)
            self.assertEqual(summary["source_side"]["ending"], "no drag seen")
            self.assertEqual(summary["source_side"]["drag_source_object"], None)
            self.assertIn("no drag seen (no start_drag in source.log)", md)
        finally:
            shutil.rmtree(tmp)


if __name__ == "__main__":
    unittest.main(verbosity=2)
