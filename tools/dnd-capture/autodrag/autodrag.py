# SPDX-License-Identifier: GPL-3.0-or-later
"""Automated drag runner (P0a2): one file-manager -> Zen drag, no human.

    python3 -I autodrag/autodrag.py run --source <app> --location <outside|downloads>
                [--file <name>[,<name>...]] [--stop-after <placement|select>]
                [--dry-run]

P1g adds the `gimbal` source (the GTK4 spike).  `--file` now takes one name
or a comma-separated list; with several names the app has already selected
them (`gimbal-spike --select`, passed by run.sh from GIMBAL_AUTODRAG_SELECT),
so the runner presses on the first name's row and never clicks — a click
would reset the selection to one item.

Real runs are attached to run.sh, which does the launching (P0a's launch
code, reused — not copied): run.sh --auto starts the D-Bus monitor, the
throwaway Zen and the source app exactly as in a manual capture, then calls
this script with the context in GIMBAL_* environment variables.  Invoked
without that context (and without --dry-run), this script simply execs
`run.sh --auto` so there is exactly one launch implementation.

Steps of a run (hard limits from the P0a2 brief; abort checks between
steps, button released in a finally and on SIGINT/SIGTERM/any exception):

  1. (run.sh) launch source app + throwaway Zen; here: wait for both
     windows in niri (source by PID, Zen by flatpak instance via
     /proc/<pid>/cgroup, falling back to a window id new after launch).
  2. place both on niri's empty end workspace — source left half, Zen right
     half, 40 px margin — floating at exact positions; verify against niri.
  3. locate the file item (AT-SPI by PID, fallback offset from apps.toml),
     click once, verify STATE_SELECTED ("unverified" without AT-SPI).  With
     several names the app preselects them, so there is no click: press on
     the first name's row and record selection: preselected.
  4. press, 12 px in 4 steps, travel to the center of Zen's content area in
     30 steps over ~600 ms, hold 400 ms, release.
  5. wait up to 3 s for Zen's title; classify y/n/other; write result.txt
     (P0a's format plus auto: true, raw title, selection, coordinates).
  6. close only what it started (run.sh's cleanup) and restore focus to
     Kyle's original workspace (by id, targeted by index — see niri.py).

Abort conditions (hard limit 6): focused window not ours, a started window
moved/resized, more than one output enabled, run longer than 20 s.
"""

import os
import re
import signal
import sys
import time
import tomllib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import locate
import niri as niri_mod
import vmouse as vmouse_mod

TIME_LIMIT_S = 20.0        # hard limit 6
WINDOW_WAIT_S = 15.0       # waiting for both windows to appear
POLL_S = 0.25
ZEN_APP_ID = "app.zen_browser.zen"

OK_EXIT, USAGE_EXIT, ABORT_EXIT = 0, 2, 3

ATTACH_ENV_VARS = ("GIMBAL_SRC_PID", "GIMBAL_ZEN_INSTANCE", "GIMBAL_RUNDIR",
                   "GIMBAL_FOLDER")

APPS = ("dolphin", "thunar", "nautilus", "cosmic", "gimbal")
LOCATIONS = ("outside", "downloads")

DEFAULT_DRYRUN_SIZE = (2560, 1440)  # observed 2026-10-31 (DP-2 logical)


class Abort(Exception):
    """Abort the run: release the button, stop, record aborted: <reason>."""

    def __init__(self, reason):
        super().__init__(reason)
        self.reason = reason


class StoppedEarly(Exception):
    """--stop-after reached: clean up as after a normal run."""

    def __init__(self, phase):
        super().__init__(phase)
        self.phase = phase


def load_config(path=None):
    with open(path or locate.config_path(), "rb") as fh:
        return tomllib.load(fh)


# ---------------------------------------------------------------------------
# pure helpers (unit-tested)
#

def geometry(out_w, out_h, margin):
    """Left/right halves with `margin` px around and between.  Returns
    (source_box, zen_box) as (x, y, w, h)."""
    w = (out_w - 3 * margin) // 2
    h = out_h - 2 * margin
    return ((margin, margin, w, h),
            (2 * margin + w, margin, w, h))


def split_files(value):
    """`--file` is one name or a comma-separated list (P1g).  Pure."""
    return [name.strip() for name in (value or "").split(",") if name.strip()]


def classify_title(before, after, file_name):
    """y if the title names the file, n if it did not change, other with
    the raw title otherwise.  Kept for the single-name callers/tests."""
    return classify_title_any(before, after, [file_name])[0]


def title_candidates(names, folder):
    """Every name whose presence in Zen's title counts as a successful drop:
    the given names, plus a symlink's target name (P1g).  A folder dropped
    on Zen shows a directory listing whose title contains the folder name —
    that is the folder name itself, so it is already covered here.  Pure
    apart from reading the symlink in `folder`."""
    candidates = []
    for name in names:
        if name not in candidates:
            candidates.append(name)
        path = os.path.join(folder, name)
        if os.path.islink(path):
            target = os.path.basename(os.path.realpath(path))
            if not target:
                target = os.path.basename(os.readlink(path).rstrip("/"))
            if target and target not in candidates:
                candidates.append(target)
    return candidates


def classify_title_any(before, after, candidates):
    """(verdict, matched name): y if the title contains any candidate, n if
    the title did not change, other otherwise.  `matched` is the first
    candidate found (recorded in result.txt), or None."""
    if after == before:
        return "n", None
    for name in candidates:
        if name in (after or ""):
            return "y", name
    return "other", None


def parse_source_intact(result_text):
    """The value of the `source_intact:` line run.sh appends after cleanup
    (P1g), or None when the line is missing or malformed.  The last such
    line wins.  Used by run-matrix-gimbal.sh's stop rule and unit-tested."""
    value = None
    for line in (result_text or "").splitlines():
        if line.startswith("source_intact:"):
            value = line.split(":", 1)[1].strip()
    return value if value in ("yes", "no") else None


def drop_point(zen_box, fraction):
    """Center of Zen's content area: a fraction of the window box (apps.toml
    [app.zen] drop_point_fraction; 0.5/0.55 aims below the toolbar).  In
    workspace-view coordinates — run it through view_to_screen before
    pointing the mouse at it."""
    x, y, w, h = zen_box
    return (int(x + w * fraction[0]), int(y + h * fraction[1]))


def view_to_screen(xy, cfg):
    """Workspace-view coordinates -> output (screen) coordinates:
    screen = view + view_origin_in_output (apps.toml [workspace]).  With the
    Noctalia bar/dock reserving space the view origin is not (0, 0) in
    output pixels; both the click point and the drop point go through this
    one function (P0a2-r1 correction 4)."""
    origin = (cfg.get("workspace", {}) or {}).get("view_origin_in_output") or [0, 0]
    return (xy[0] + origin[0], xy[1] + origin[1])


def working_area_offset(cfg):
    """Where niri's working area starts on the output.  move-floating-window
    positions are relative to the working area (below layer-shell exclusive
    zones such as the Noctalia bar), but niri reports
    tile_pos_in_workspace_view in output coordinates.  Live check, Oct 31:
    commanded y 40, read back y 74, and the window's top edge is at y 74 in
    a screenshot; the bar is 34 px tall."""
    off = (cfg.get("workspace", {}) or {}).get("working_area_offset") or [0, 0]
    return (off[0], off[1])


def output_box(box, offset):
    """A commanded (working-area) box in output coordinates."""
    return (box[0] + offset[0], box[1] + offset[1], box[2], box[3])


def wait_layout(n, state, note, clock, sleep, timeout_s=3.0):
    """verify_layout, retried until it passes or timeout_s runs out: niri
    applies the size asynchronously (live check, Oct 31: Zen still read
    1272x1398 right after set-window-width/height to 1220x1360)."""
    deadline = clock() + timeout_s
    while True:
        try:
            return verify_layout(n, state, note)
        except Abort:
            if clock() >= deadline:
                raise
            sleep(0.1)


def proc_cgroup(pid):
    try:
        with open("/proc/%d/cgroup" % pid, "r") as fh:
            return fh.read()
    except OSError:
        return ""


def find_zen_window(windows, instance=None, before_ids=(), cgroup_reader=None):
    """The throwaway Zen's window.  Hard limit 3 offers two ways: by PID
    (here: the window whose client process lives in the throwaway flatpak
    instance's cgroup scope — the instance id is part of the scope name,
    verified live 2026-10-30) or by a window id that is new after launch
    (before_ids) with a zen-ish app_id.  Kyle's own Zen window can never
    match either.

    P0a2-r1 correction 6: the new-window-id fallback also skips any
    candidate whose /proc/<pid>/cgroup names an
    app.zen_browser.zen-<n>.scope OTHER than ours — a window Kyle's own
    (or any other) Zen instance opens before ours is never picked, even
    when it appears first.  The fallback is therefore only reachable for
    windows whose cgroup cannot be read (or names our own scope)."""
    cgroup_reader = cgroup_reader or proc_cgroup
    if instance:
        scope = "app.zen_browser.zen-%s.scope" % instance
        for w in windows:
            pid = w.get("pid")
            if pid and scope in cgroup_reader(pid):
                return w
    for w in windows:
        if w.get("id") in before_ids:
            continue
        if "zen" not in (w.get("app_id") or "").lower():
            continue
        other = zen_scope_instance(cgroup_reader(w.get("pid")))
        if other is not None and str(other) != str(instance):
            continue
        return w
    return None


def zen_scope_instance(cgroup_text):
    """The <n> in an app-flatpak-app.zen_browser.zen-<n>.scope cgroup path,
    or None when the cgroup is unreadable or names no zen scope."""
    m = re.search(r"app\.zen_browser\.zen-(\d+)\.scope", cgroup_text or "")
    return m.group(1) if m else None


def plan_commands(n, src_id, zen_id, src_box, zen_box, ws_idx):
    """Issue (or in dry-run, print) the exact niri commands that put both
    windows on the target workspace: float, move, focus the workspace,
    position, size, focus the source."""
    cmds = []
    cmds += [n.move_window_to_floating(src_id),
             n.move_window_to_floating(zen_id)]
    cmds += [n.move_window_to_workspace(src_id, ws_idx, focus=False),
             n.move_window_to_workspace(zen_id, ws_idx, focus=False)]
    cmds += [n.focus_workspace(ws_idx)]
    for wid, box in ((src_id, src_box), (zen_id, zen_box)):
        x, y, w, h = box
        cmds += [n.move_floating_window(wid, x, y),
                 n.set_window_width(wid, w),
                 n.set_window_height(wid, h)]
    cmds += [n.focus_window(src_id)]
    return cmds


class RunState(object):
    """Bookkeeping shared by the abort checks and the cleanup."""

    def __init__(self):
        self.our_ids = set()
        self.expected_boxes = {}   # window id -> (x, y, w, h)
        self.kyle_ws_id = None
        self.switched_ws = False
        self.placed = False
        self.workspace_id = None


def abort_reason(n, state, t0, now):
    """Hard limit 6, checked between steps.  Returns a reason or None.
    The focused-window and moved/resized checks only apply once the windows
    are placed (before that, focus legitimately belongs to other windows)."""
    outs = niri_mod.enabled_outputs(n.outputs())
    if len(outs) > 1:
        return "more than one output enabled: %s" % ", ".join(sorted(outs))
    if now() - t0 > TIME_LIMIT_S:
        return "timeout: run exceeded %.0f s" % TIME_LIMIT_S
    if state.placed:
        fw = n.focused_window()
        if fw is None or fw.get("id") not in state.our_ids:
            return ("focused window is not one the runner started: %r"
                    % (fw.get("title") if fw else None))
        wins = n.windows()
        for wid, box in state.expected_boxes.items():
            w = niri_mod.window_by_id(wins, wid)
            if w is None:
                return "window %d disappeared" % wid
            pos, size = niri_mod.readout_geometry(w)
            if pos is not None and (round(pos[0]), round(pos[1])) != (box[0], box[1]):
                return "window %d moved: %r != %r" % (wid, pos, box[:2])
            if size is not None and (round(size[0]), round(size[1])) != (box[2], box[3]):
                return "window %d resized: %r != %r" % (wid, size, box[2:])
    return None


def wait_for_windows(n, src_pid, zen_instance, before_ids, clock, sleep,
                     deadline_s=WINDOW_WAIT_S):
    """Both windows present in niri: source by PID, Zen by instance."""
    deadline = clock() + deadline_s
    while True:
        wins = n.windows()
        src = niri_mod.window_by_pid(wins, src_pid)
        zen = find_zen_window(wins, zen_instance, before_ids)
        if src is not None and zen is not None:
            return src, zen
        if clock() >= deadline:
            missing = []
            if src is None:
                missing.append("source (pid %d)" % src_pid)
            if zen is None:
                missing.append("zen (instance %s)" % zen_instance)
            raise Abort("windows did not appear within %.0f s: %s"
                        % (deadline_s, ", ".join(missing)))
        sleep(POLL_S)


def verify_layout(n, state, note):
    """Check the placed layout against niri and abort on any mismatch
    (P0a2 step 2).  What niri does not report for floating windows
    (tile_pos_in_workspace_view may be null — live check) is recorded as a
    note, not a mismatch.  A None workspace_id (creation-on-demand path)
    skips the workspace comparison; position/size still apply."""
    wins = n.windows()
    problems = []
    for wid, box in state.expected_boxes.items():
        w = niri_mod.window_by_id(wins, wid)
        if w is None:
            problems.append("window %d not found" % wid)
            continue
        if not w.get("is_floating"):
            problems.append("window %d (%r) is not floating" % (wid, w.get("title")))
        if state.workspace_id is not None and \
                w.get("workspace_id") != state.workspace_id:
            problems.append("window %d on workspace %r, expected %r"
                            % (wid, w.get("workspace_id"), state.workspace_id))
        pos, size = niri_mod.readout_geometry(w)
        if pos is None:
            note("window %d reports no tile_pos_in_workspace_view (live check: "
                 "does a floating window report it?)" % wid)
        elif (round(pos[0]), round(pos[1])) != (box[0], box[1]):
            problems.append("window %d at %r, expected %r" % (wid, pos, box[:2]))
        if size is not None and (round(size[0]), round(size[1])) != (box[2], box[3]):
            problems.append("window %d size %r, expected %r" % (wid, size, box[2:]))
    if problems:
        raise Abort("layout mismatch: " + "; ".join(problems))


def fallback_item_point(app_cfg, item_name, window_pos, window_size):
    """The fallback point for one item when AT-SPI exposes no row.
    `item_offsets` is the per-file table (P1g, gimbal): a missing name or an
    unset value ([] or None) is unconfigured, not a click at (0, 0).  Apps
    without `item_offsets` keep the old `first_item_offset` behaviour."""
    offsets = app_cfg.get("item_offsets")
    if isinstance(offsets, dict):
        off = offsets.get(item_name)
        if not off or len(off) < 2 or off[0] is None or off[1] is None:
            return None
        return (window_pos[0] + int(off[0]), window_pos[1] + int(off[1]))
    return locate.fallback_point(app_cfg, window_pos, window_size)


def selection_point(cfg, app, item, item_name, window_pos, window_size):
    """The press/click point for the file item: AT-SPI window-relative
    center converted to screen coordinates, or the apps.toml fallback offset
    for that exact item, or None (-> abort rather than click blind)."""
    app_cfg = cfg.get("app", {}).get(app, {})
    if item is not None:
        corr = app_cfg.get("window_offset_correction") or [0, 0]
        rect = locate.item_screen_rect(item, window_pos, tuple(corr))
        return (rect[0] + rect[2] // 2, rect[1] + rect[3] // 2)
    return fallback_item_point(app_cfg, item_name, window_pos, window_size)


def perform_click(vm, point, settle_s, sleep, clock):
    """One click at `point`.  Returns the clock time of the release — the
    click_to_drag_gap is measured from it (P0a2-r1 correction 2)."""
    vm.move_to(*point)
    vm.press()
    vm.release()
    t_release = clock()
    sleep(settle_s)
    return t_release


def wait_click_gap(clock, sleep, t_release, cfg):
    """Wait until at least click_to_drag_gap_ms (apps.toml [drag], default
    1000) have passed since the selection click's release, so the selection
    click and the drag press cannot register as a double-click (which would
    open the file in an external app — a window the runner did not start).
    Measured with the clock: only the remaining amount is slept, so time
    already spent in the AT-SPI recheck counts toward the gap."""
    gap = cfg.get("drag", {}).get("click_to_drag_gap_ms", 1000) / 1000.0
    remaining = gap - (clock() - t_release)
    if remaining > 0:
        sleep(remaining)


def perform_drag(vm, cfg, from_xy, to_xy, sleep):
    """Press, 12 px in 4 steps past the drag threshold, at least 30 steps
    to the target over travel_ms, hold 400 ms, release (in finally: any
    exception releases the button — hard limit 5)."""
    drag_cfg = cfg.get("drag", {})
    threshold = drag_cfg.get("threshold_px", 12)
    tsteps = drag_cfg.get("threshold_steps", 4)
    nsteps = max(30, drag_cfg.get("travel_steps", 30))
    # the step interval is derived from travel_ms so the travel really does
    # take about that long (P0a2-r1 correction 4)
    interval = drag_cfg.get("travel_ms", 600) / 1000.0 / nsteps
    hold = drag_cfg.get("hold_ms", 400) / 1000.0

    vm.press()
    try:
        dx, dy = to_xy[0] - from_xy[0], to_xy[1] - from_xy[1]
        sx = 1 if dx >= 0 else -1
        sy = 1 if dy >= 0 else -1
        for i in range(1, tsteps + 1):
            vm.move_to(from_xy[0] + sx * threshold * i // tsteps,
                       from_xy[1] + sy * threshold * i // tsteps)
        for i in range(1, nsteps + 1):
            vm.move_to(from_xy[0] + dx * i // nsteps,
                       from_xy[1] + dy * i // nsteps)
            sleep(interval)
        sleep(hold)  # hold still over the target
    finally:
        vm.release()


def write_result(rundir, opened, note, extra_lines):
    """result.txt in P0a's format (zen_opened_file/note — summarize.py's
    read_result ignores unknown lines) plus the auto: fields."""
    lines = ["zen_opened_file: %s" % opened, "note: %s" % note]
    lines += extra_lines
    with open(os.path.join(rundir, "result.txt"), "w") as fh:
        fh.write("\n".join(lines) + "\n")


def attached_env(env=None):
    env = os.environ if env is None else env
    vals = {k: env.get(k) for k in ATTACH_ENV_VARS}
    return vals if all(vals.values()) else None


# ---------------------------------------------------------------------------
# the real run
#

def default_vm_factory(cfg):
    """The real pointer factory: builds the uinput device with the settle
    timings from apps.toml [vmouse] (P0a2-r1 correction 3)."""
    dev_cfg = cfg.get("vmouse", {})

    def factory(w, h):
        device = vmouse_mod.UinputDevice(
            device_settle_ms=dev_cfg.get("device_settle_ms", 500),
            pre_destroy_ms=dev_cfg.get("pre_destroy_ms", 100))
        return vmouse_mod.VMouse(w, h, device=device)

    return factory


def run_real(args, cfg, env=None, niri=None, vm_factory=None,
             locate_fn=None, clock=None, sleep=None, log=None):
    env = os.environ if env is None else env
    niri = niri if niri is not None else niri_mod.Niri()
    vm_factory = vm_factory if vm_factory is not None else default_vm_factory(cfg)
    locate_fn = locate_fn if locate_fn is not None else locate.locate_file_item
    clock = clock if clock is not None else time.monotonic
    sleep = sleep if sleep is not None else time.sleep
    log = log if log is not None else (lambda msg: print("[autodrag] %s" % msg, flush=True))

    src_pid = int(env["GIMBAL_SRC_PID"])
    zen_instance = env["GIMBAL_ZEN_INSTANCE"]
    rundir = env["GIMBAL_RUNDIR"]
    folder = env["GIMBAL_FOLDER"]
    before_ids = {int(x) for x in (env.get("GIMBAL_ZEN_WIN_BEFORE") or "").split(",")
                  if x.strip().isdigit()}

    file_names = split_files(args.file)
    if not file_names:
        raise Abort("--file needs at least one name")
    multi = len(file_names) > 1
    first_name = file_names[0]
    for name in file_names:
        if not os.path.exists(os.path.join(folder, name)):
            raise Abort("test file %s not found in %s" % (name, folder))
    # read symlink targets before the drag: a MOVE drop could remove the link
    title_names = title_candidates(file_names, folder)

    t0 = clock()
    state = RunState()
    vm = None
    opened = "aborted"
    selection = "n/a"
    title_before = title_after = None
    matched_name = None
    click_at = drop_at = None
    t_release = None
    aborted = None
    stopped = None

    def on_signal(signum, frame):
        raise Abort("signal %d" % signum)

    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, on_signal)

    try:
        out_w, out_h = output_size(niri)
        margin = cfg.get("layout", {}).get("margin", 40)
        wa = working_area_offset(cfg)
        # commanded boxes are in working-area coordinates; everything read
        # back from niri and every pointer position is in output coordinates
        src_box, zen_box = geometry(out_w - wa[0], out_h - wa[1], margin)
        src_out, zen_out = output_box(src_box, wa), output_box(zen_box, wa)

        # -- 1. windows ----------------------------------------------------
        src_win, zen_win = wait_for_windows(
            niri, src_pid, zen_instance, before_ids, clock, sleep)
        state.our_ids = {src_win["id"], zen_win["id"]}
        title_before = zen_win.get("title")
        log("windows: source id %s %r, zen id %s %r" %
            (src_win["id"], src_win.get("title"), zen_win["id"], title_before))

        # -- workspace: niri's empty workspace at the end -------------------
        state.kyle_ws_id = niri_mod.focused_workspace_id(niri.workspaces())
        log("Kyle's focused workspace: id %s (restored afterwards)" % state.kyle_ws_id)
        output_names = set(niri_mod.enabled_outputs(niri.outputs()))
        # P0a2-r1 correction 1: a workspace is empty when active_window_id
        # is null AND no window carries its id (niri has no is_empty key);
        # when none exists the run aborts — no reliance on niri creating a
        # workspace on demand, which would strand the two windows apart.
        ws = niri_mod.target_workspace(niri.workspaces(), niri.windows(),
                                       output_names)
        if ws is None:
            raise Abort("no empty workspace at the end of the enabled "
                        "output(s) %s" % ", ".join(sorted(output_names)))
        ws_idx, ws_id = ws["idx"], ws["id"]
        state.workspace_id = ws_id
        log("target workspace: id %s idx %d on %s" % (ws_id, ws_idx, ws["output"]))

        # -- 2. place + verify ----------------------------------------------
        log("placing windows on workspace idx %d: source %s, zen %s" %
            (ws_idx, src_box, zen_box))
        plan_commands(niri, src_win["id"], zen_win["id"], src_box, zen_box, ws_idx)
        state.switched_ws = True
        state.expected_boxes = {src_win["id"]: src_out, zen_win["id"]: zen_out}
        state.placed = True
        notes = []
        wait_layout(niri, state, notes.append, clock, sleep)
        for nline in notes:
            log(nline)
        # the window position for the AT-SPI math: prefer what niri reports
        # back for the floating source window, fall back to the commanded box
        src_now = niri_mod.window_by_id(niri.windows(), src_win["id"])
        rb_pos, rb_size = niri_mod.readout_geometry(src_now) if src_now else (None, None)
        window_pos = (round(rb_pos[0]), round(rb_pos[1])) if rb_pos else (src_out[0], src_out[1])
        reason = abort_reason(niri, state, t0, clock)
        if reason:
            raise Abort(reason)

        if args.stop_after == "placement":
            raise StoppedEarly("placement")

        # -- 3. locate + click/selection check --------------------------------
        item, app_found = locate_fn(src_pid, first_name)
        if item is None:
            log("AT-SPI: %s" % ("app found, item %r not exposed" % first_name
                                if app_found else "no application for pid %d" % src_pid))
        click_at = selection_point(cfg, args.source, item, first_name,
                                   window_pos, src_box[2:])
        if click_at is None:
            raise Abort("unconfigured: no AT-SPI item and no item_offsets/"
                        "first_item_offset for %s (%s) (apps.toml TODO(live))"
                        % (args.source, first_name))
        # the item math yields workspace-view coordinates (the window
        # position niri works with); the pointer needs output coordinates
        click_at = view_to_screen(click_at, cfg)

        if multi:
            # Several names: the app already selected them (gimbal-spike
            # --select, passed by run.sh).  A click would reset the selection
            # to one item, so skip it and just press on the first name's row.
            log("preselected %r; no selection click (press on %r at %s)"
                % (file_names, first_name, click_at))
            if item is not None:
                for name in file_names:
                    preset, _ = locate_fn(src_pid, name)
                    if preset is None or not preset.selected:
                        raise Abort("preselection check failed: %r did not report "
                                    "STATE_SELECTED" % name)
            selection = "preselected"
            log("selection: %s" % selection)
            reason = abort_reason(niri, state, t0, clock)
            if reason:
                raise Abort(reason)
            if args.stop_after == "select":
                raise StoppedEarly("select")
        else:
            log("clicking %r at %s (selection check follows)" % (first_name, click_at))
            vm = vm_factory(out_w, out_h)
            t_release = perform_click(vm, click_at,
                                      cfg.get("drag", {}).get("select_settle_ms", 300) / 1000.0,
                                      sleep, clock)

            if item is not None:
                recheck, _ = locate_fn(src_pid, first_name)
                if recheck is None or not recheck.selected:
                    raise Abort("selection check failed: %r did not report "
                                "STATE_SELECTED" % first_name)
                selection = "selected"
            else:
                selection = "unverified"
            log("selection: %s" % selection)
            reason = abort_reason(niri, state, t0, clock)
            if reason:
                raise Abort(reason)

            if args.stop_after == "select":
                raise StoppedEarly("select")

        # -- 4. the drag ------------------------------------------------------
        if multi:
            # no selection click happened, so there is no click/press gap to
            # keep (click_to_drag_gap_ms exists to stop a double-click).
            # perform_drag presses wherever the pointer is: move it onto the
            # first row and let the app see the motion first (live check,
            # Nov 1: without this the press never reached the spike).
            vm = vm_factory(out_w, out_h)
            vm.move_to(*click_at)
            sleep(cfg.get("drag", {}).get("select_settle_ms", 300) / 1000.0)
        else:
            # correction 2: the selection click and the drag press must not
            # register as a double-click, which would open the file in an
            # external viewer — a window the runner did not start
            wait_click_gap(clock, sleep, t_release, cfg)
        # correction 5: the title seen at window-appearance is stale (Zen is
        # still starting up); read it again right before the press
        zen_now = niri_mod.window_by_id(niri.windows(), zen_win["id"])
        title_before = (zen_now or {}).get("title")
        zen_cfg = cfg.get("app", {}).get("zen", {})
        drop_at = view_to_screen(
            drop_point(zen_out, zen_cfg.get("drop_point_fraction", [0.5, 0.55])),
            cfg)
        log("dragging %s -> %s" % (click_at, drop_at))
        perform_drag(vm, cfg, click_at, drop_at, sleep)
        reason = abort_reason(niri, state, t0, clock)
        if reason:
            raise Abort(reason)

        # -- 5. title wait + classification ------------------------------------
        title_after = wait_for_title(niri, zen_win["id"], title_before,
                                     cfg.get("drag", {}).get("title_wait_ms", 3000),
                                     t0, clock, sleep)
        # y if the title names ANY given file, or a symlink's target; a
        # folder drop's listing title contains the folder name (title_candidates)
        opened, matched_name = classify_title_any(
            title_before, title_after, title_names)

    except Abort as a:
        aborted = a.reason
    except StoppedEarly as s:
        stopped = s.phase
    except Exception as e:  # unexpected: release, restore, record, exit non-zero
        import traceback
        traceback.print_exc()
        aborted = "error: %s: %s" % (type(e).__name__, e)
    finally:
        # minor 7: once cleanup starts, a second Ctrl-C must not raise out
        # of vm.close() (that would skip the release or the destroy); the
        # previous handlers go back right after the close, so the --hold-s
        # wait and the workspace restore stay interruptible
        saved_handlers = {}
        for sig in (signal.SIGINT, signal.SIGTERM):
            saved_handlers[sig] = signal.signal(sig, signal.SIG_IGN)
        try:
            # hard limit 5: the button is always released, the device destroyed
            if vm is not None:
                vm.close()
        finally:
            for sig, handler in saved_handlers.items():
                signal.signal(sig, handler)
        if args.hold_s:
            # minor 8: keep the layout on screen for `niri msg --json windows`
            log("--hold-s: waiting %.1f s before cleanup (device destroyed; "
                "inspect now)" % args.hold_s)
            sleep(args.hold_s)
        cleanup_workspace(niri, state, log)

    if stopped is not None:
        write_result(rundir, "aborted",
                     "stopped after %s (intentional --stop-after)" % stopped,
                     result_extra(args, selection, click_at, drop_at,
                                  title_before, title_after, matched_name,
                                  "aborted: stop-after %s" % stopped))
        log("stopped after %s" % stopped)
        return ABORT_EXIT
    if aborted is not None:
        log("aborted: %s" % aborted)
        opened = "aborted"
    write_result(rundir, opened,
                 "auto:true source=%s location=%s file=%s folder=%s" %
                 (args.source, args.location, args.file, folder),
                 result_extra(args, selection, click_at, drop_at,
                              title_before, title_after, matched_name,
                              "aborted: %s" % aborted if aborted else None))
    log("result: zen_opened_file=%s selection=%s" % (opened, selection))
    return ABORT_EXIT if aborted is not None else OK_EXIT


def output_size(n):
    """The single enabled output's logical size.  More than one enabled
    output is itself an abort condition (hard limit 6)."""
    outs = niri_mod.enabled_outputs(n.outputs())
    if len(outs) != 1:
        raise Abort("expected exactly one enabled output, found %d: %s"
                    % (len(outs), ", ".join(sorted(outs))))
    logical = next(iter(outs.values()))["logical"]
    return logical["width"], logical["height"]


def wait_for_title(n, zen_id, before, timeout_ms, t0, clock, sleep,
                   settle_ms=500):
    """Wait up to timeout_ms for Zen's title to change.  The 20 s wall still
    applies (hard limit 6): if it passes here the button is already
    released, so the current title is classified and any timeout recorded
    by the caller's checks.

    P0a2-r1 correction 5: the first change is often a transient one (a
    page finishing loading), so after the first change the runner waits
    settle_ms and reads the title once more — the settled read is what
    gets classified."""
    deadline = min(clock() + timeout_ms / 1000.0, t0 + TIME_LIMIT_S)
    while clock() < deadline:
        w = niri_mod.window_by_id(n.windows(), zen_id)
        title = (w or {}).get("title")
        if title != before:
            sleep(settle_ms / 1000.0)
            w = niri_mod.window_by_id(n.windows(), zen_id)
            return (w or {}).get("title")
        sleep(0.1)
    return before


def cleanup_workspace(n, state, log):
    """Restore focus to Kyle's original workspace (by id).  Nothing else is
    touched: run.sh's cleanup closes the processes it started."""
    if state.switched_ws and state.kyle_ws_id is not None:
        try:
            n.focus_workspace_by_id(state.kyle_ws_id)
            log("focus restored to workspace id %s" % state.kyle_ws_id)
        except Exception as e:
            log("WARNING: could not restore workspace focus: %s" % e)


def result_extra(args, selection, click_at, drop_at, title_before, title_after,
                 matched_name, aborted_line):
    lines = ["auto: true",
             "source: %s" % args.source,
             "location: %s" % args.location,
             "file: %s" % args.file,
             "selection: %s" % selection,
             "click_at: %s" % (click_at,),
             "drop_at: %s" % (drop_at,),
             "title_before: %s" % title_before,
             "raw_title: %s" % title_after,
             "matched_name: %s" % matched_name]
    if aborted_line:
        lines.append(aborted_line)
    return lines


# ---------------------------------------------------------------------------
# dry run: runs nothing, prints the planned commands, coordinates, events
#

def run_dry(args, cfg, log=None):
    log = log if log is not None else (lambda msg: print(msg))
    out_w = args.width if args.width else DEFAULT_DRYRUN_SIZE[0]
    out_h = args.height if args.height else DEFAULT_DRYRUN_SIZE[1]
    margin = cfg.get("layout", {}).get("margin", 40)
    wa = working_area_offset(cfg)
    src_box, zen_box = geometry(out_w - wa[0], out_h - wa[1], margin)
    drag_cfg = cfg.get("drag", {})
    zen_cfg = cfg.get("app", {}).get("zen", {})
    drop_at = view_to_screen(
        drop_point(output_box(zen_box, wa),
                   zen_cfg.get("drop_point_fraction", [0.5, 0.55])), cfg)

    files = split_files(args.file)
    log("autodrag --dry-run: nothing is run, launched or moved")
    log("plan: source=%s location=%s file=%s" % (args.source, args.location, args.file))
    log("files: %r (%s)" % (files, "preselected, no selection click"
                            if len(files) > 1 else "one click, then drag"))
    log("output: %dx%d, margin %d, working area offset %s" % (out_w, out_h, margin, list(wa)))
    log("boxes (working-area coordinates, as commanded): source=(%d, %d, %d, %d) zen=(%d, %d, %d, %d)" %
        (src_box + zen_box))
    log("")
    log("planned niri commands (ids are read at runtime):")
    n = niri_mod.Niri(dry_run=True, printer=lambda cmd: log("  " + cmd))
    plan_commands(n, "<source-window-id>", "<zen-window-id>",
                  src_box, zen_box, "<empty-end-workspace-idx>")
    log("  # afterwards: verify layout via `niri msg --json windows`")
    log("  # on cleanup: niri msg action focus-workspace <Kyle's ws idx, looked up by id>")
    log("")
    log("planned pointer events (logical px -> ABS):")
    app_cfg = cfg.get("app", {}).get(args.source, {})
    fallback = app_cfg.get("first_item_offset")
    offsets = app_cfg.get("item_offsets")
    origin = (cfg.get("workspace", {}) or {}).get("view_origin_in_output") or [0, 0]
    log("  item point: located at runtime via AT-SPI (%s); first_item_offset: %s; "
        "item_offsets: %s"
        % ("apps.toml atspi_items=%s" % app_cfg.get("atspi_items"), fallback, offsets))
    log("  view->screen: + %s (view_origin_in_output, applied to click and drop)"
        % (origin,))
    log("  drop point: %s (zen box + fraction %s)" % (drop_at, zen_cfg.get("drop_point_fraction")))
    if len(files) > 1:
        log("  press:  move_to first name's row (%r); no selection click "
            "(a click would reset the selection)" % files[0])
    else:
        log("  click:  move_to <item point>; press (EV_KEY BTN_LEFT 1 + SYN); "
            "release (EV_KEY BTN_LEFT 0 + SYN)")
    log("  verify: %s" % ("each name already STATE_SELECTED via AT-SPI"
                          if len(files) > 1 else
                          "STATE_SELECTED via AT-SPI (unverified when no AT-SPI)"))
    if len(files) > 1:
        log("  gap:    none (no selection click, so no double-click risk)")
    else:
        log("  gap:    >= %s ms since the click release before the drag press "
            "(click_to_drag_gap_ms; no double-click)"
            % cfg.get("drag", {}).get("click_to_drag_gap_ms", 1000))
    log("  drag:   press (BTN_LEFT 1 + SYN)")
    for i in range(1, drag_cfg.get("threshold_steps", 4) + 1):
        d = drag_cfg.get("threshold_px", 12) * i // drag_cfg.get("threshold_steps", 4)
        log("    move_to item + (%+d, %+d)  # past the drag threshold" % (d, d))
    steps = max(30, drag_cfg.get("travel_steps", 30))
    log("  lerp(item, drop, t) = item + (drop - item) * t")
    for i in range(1, steps + 1):
        log("    move_to lerp(item, drop, %d/%d)  # ~%d ms per step" %
            (i, steps, drag_cfg.get("step_interval_ms", 20)))
    log("  hold %d ms, then release (EV_KEY BTN_LEFT 0 + SYN)" % drag_cfg.get("hold_ms", 400))
    log("")
    log("classify: title unchanged -> n; contains any of %r -> y "
        "(plus a symlink's target name); else other" % (files,))
    log("abort rules: focused window not ours | window moved/resized | "
        ">1 enabled output | run > %.0f s" % TIME_LIMIT_S)
    if args.stop_after:
        log("stop-after: %s (real runs stop there and clean up)" % args.stop_after)
    return OK_EXIT


# ---------------------------------------------------------------------------
# CLI
#

class Args(object):
    def __init__(self):
        self.source = None
        self.location = None
        self.file = "sample.png"
        self.stop_after = None
        self.dry_run = False
        self.width = 0
        self.height = 0
        self.hold_s = 0.0


def parse_args(argv):
    a = Args()
    if not argv or argv[0] != "run":
        raise SystemExit(__doc__)
    rest = argv[1:]
    i = 0
    while i < len(rest):
        arg = rest[i]
        if arg == "--source":
            i += 1; a.source = rest[i]
        elif arg == "--location":
            i += 1; a.location = rest[i]
        elif arg == "--file":
            i += 1; a.file = rest[i]
        elif arg == "--stop-after":
            i += 1; a.stop_after = rest[i]
        elif arg == "--hold-s":
            i += 1; a.hold_s = float(rest[i])
        elif arg == "--dry-run":
            a.dry_run = True
        elif arg == "--width":
            i += 1; a.width = int(rest[i])
        elif arg == "--height":
            i += 1; a.height = int(rest[i])
        else:
            raise SystemExit("unknown argument: %s" % arg)
        i += 1
    if a.source not in APPS:
        raise SystemExit("--source must be one of %s" % (APPS,))
    if a.location not in LOCATIONS:
        raise SystemExit("--location must be one of %s" % (LOCATIONS,))
    if not split_files(a.file):
        raise SystemExit("--file must name at least one file")
    if a.stop_after is not None and a.stop_after not in ("placement", "select"):
        raise SystemExit("--stop-after must be placement or select")
    if a.hold_s < 0:
        raise SystemExit("--hold-s must be >= 0")
    return a


def main(argv):
    args = parse_args(argv)
    cfg = load_config()
    if args.dry_run:
        return run_dry(args, cfg)
    if attached_env() is None:
        # Not attached and not a dry run: reuse P0a's launch code by exec'ing
        # run.sh --auto (there is exactly one launch implementation).  run.sh
        # calls back into this script with the GIMBAL_* context.
        run_sh = os.path.normpath(os.path.join(
            os.path.dirname(os.path.abspath(__file__)), "..", "run.sh"))
        env = dict(os.environ)
        env["GIMBAL_AUTODRAG_FILE"] = args.file
        # a multi-file run needs the spike's --select hook; set it from the
        # same list unless the caller already chose a different selection
        if len(split_files(args.file)) > 1 and not env.get("GIMBAL_AUTODRAG_SELECT"):
            env["GIMBAL_AUTODRAG_SELECT"] = args.file
        if args.stop_after:
            env["GIMBAL_AUTODRAG_STOP_AFTER"] = args.stop_after
        if args.hold_s:
            env["GIMBAL_AUTODRAG_HOLD_S"] = str(args.hold_s)
        print("[autodrag] not attached to run.sh; exec: run.sh --auto %s %s"
              % (args.source, args.location))
        os.execvpe("bash", ["bash", run_sh, "--auto", args.source, args.location], env)
    return run_real(args, cfg)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
