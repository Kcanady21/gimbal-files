# SPDX-License-Identifier: GPL-3.0-or-later
"""niri IPC for the automated drag runner (P0a2).

Reads (always allowed, P0a2 hard limit 2):
    windows(), outputs(), workspaces(), focused_window()   # niri msg --json …

Actions (hard limit 2: the runner only issues actions with --id for windows
it started; with dry_run=True the exact command is printed instead of run):

  place(window_id, x, y, w, h) — float a window and put it at an exact
  logical position/size.  Exact action syntax read from the CLI on
  2026-10-30 (niri 26.04), recorded in NOTES-autodrag.md:

    niri msg action move-window-to-floating --id <ID>
        ("Move the focused window to the floating layout"; --id "Id of the
        window to move")  — used instead of toggle-window-floating because
        it is idempotent.
    niri msg action move-floating-window --id <ID> -x <X> -y <Y>
        ("Move the floating window on screen"; -x/-y "How to change the X
        position", default "+0")  — a plain number is an absolute
        SetFixed(position), a +/% suffix would be relative/proportional
        (PositionChange::FromStr in niri-ipc/src/lib.rs, v26.04: leading
        '+'/'-' -> AdjustFixed, plain -> SetFixed).
    niri msg action set-window-width --id <ID> <W>
    niri msg action set-window-height --id <ID> <H>
        (<CHANGE> positional; same parsing: plain = absolute pixels)

    niri msg action move-window-to-workspace --window-id <ID> --focus false <IDX>
        (flag is --window-id here, not --id; --focus true|false, default
        true — we keep focus where it is and switch explicitly)
    niri msg action focus-workspace <REFERENCE>
        ("Focus a workspace by reference (index or name)") — the CLI parses
        a numeral as an INDEX, not an id (WorkspaceReferenceArg::FromStr in
        niri-ipc/src/lib.rs v26.04), so focusing "by ID" means looking the
        id up in workspaces() and passing its idx (focus_workspace_by_id).
    niri msg action focus-window --id <ID>

Whether a floating window reports layout.tile_pos_in_workspace_view, and in
which coordinate space (output vs workspace view with the Noctalia bar
reserving space), cannot be determined without running an action — left as
the live check; the readback verification in autodrag.py adapts (TODO in
NOTES-autodrag.md).
"""

import json
import shlex
import subprocess


class NiriError(RuntimeError):
    pass


class Niri(object):
    """niri IPC client.  `runner` is injectable for the tests (a callable
    taking an argv list and returning (returncode, stdout)); `printer`
    receives the exact commands in dry-run mode."""

    def __init__(self, dry_run=False, runner=None, printer=None):
        self.dry_run = dry_run
        self._runner = runner
        self._printer = printer if printer is not None else (
            lambda cmd: print("niri:%s" % cmd))

    # -- plumbing -----------------------------------------------------------
    def _run(self, argv):
        """Run a read or an action.  Reads always execute; actions are
        replaced by a printed line in dry-run mode."""
        cmd = " ".join(shlex.quote(a) for a in argv)
        if self.dry_run and argv[2] == "action":
            self._printer(cmd)
            return ""
        if self._runner is not None:
            rc, out = self._runner(argv)
        else:
            p = subprocess.run(argv, capture_output=True, text=True)
            rc, out = p.returncode, p.stdout
        if rc != 0:
            raise NiriError("niri failed (%d): %s" % (rc, cmd))
        return out

    def _read_json(self, sub):
        return json.loads(self._run(["niri", "msg", "--json", sub]))

    def _action(self, *args):
        return self._run(["niri", "msg", "action"] + [str(a) for a in args])

    # -- reads ---------------------------------------------------------------
    def windows(self):
        return self._read_json("windows")

    def outputs(self):
        return self._read_json("outputs")

    def workspaces(self):
        return self._read_json("workspaces")

    def focused_window(self):
        return self._read_json("focused-window")

    # -- actions (all pass --id/--window-id; never act on the focused
    #    window implicitly) --------------------------------------------------
    def move_window_to_floating(self, window_id):
        return self._action("move-window-to-floating", "--id", window_id)

    def move_floating_window(self, window_id, x, y):
        # plain numbers: absolute position (SetFixed), see module docstring
        return self._action("move-floating-window", "--id", window_id,
                            "-x", x, "-y", y)

    def set_window_width(self, window_id, w):
        return self._action("set-window-width", "--id", window_id, w)

    def set_window_height(self, window_id, h):
        return self._action("set-window-height", "--id", window_id, h)

    def focus_window(self, window_id):
        return self._action("focus-window", "--id", window_id)

    def move_window_to_workspace(self, window_id, ws_index, focus=False):
        # --focus false: focus must not follow the window; the runner
        # switches workspaces explicitly (and aborts if focus is wrong)
        return self._action("move-window-to-workspace", "--window-id", window_id,
                            "--focus", "true" if focus else "false", ws_index)

    def focus_workspace(self, ws_index):
        return self._action("focus-workspace", ws_index)

    # -- composite helpers ----------------------------------------------------
    def place(self, window_id, x, y, w, h):
        """Float the window and put it at logical (x, y) with size (w, h).
        Returns the list of commands issued (for the run log)."""
        cmds = [
            self.move_window_to_floating(window_id),
            self.move_floating_window(window_id, x, y),
            self.set_window_width(window_id, w),
            self.set_window_height(window_id, h),
        ]
        return cmds

    def focus_workspace_by_id(self, ws_id):
        """Focus a workspace identified by its niri workspace id: looked up
        in workspaces(), targeted by index (the CLI reference parser only
        accepts index or name — see module docstring)."""
        for ws in self.workspaces():
            if ws.get("id") == ws_id:
                return self.focus_workspace(ws["idx"])
        raise NiriError("workspace id %r not found" % ws_id)


# ---------------------------------------------------------------------------
# pure helpers over the JSON shapes (unit-tested with canned data)
#

def enabled_outputs(outputs_json):
    """The enabled outputs.  niri 26.04 has no 'enabled' key: a disabled
    output has logical == null (observed 2026-10-30: DP-2 with logical,
    DP-3 with logical null)."""
    return {name: out for name, out in outputs_json.items()
            if out.get("logical") is not None}


def window_by_pid(windows_json, pid):
    for w in windows_json:
        if w.get("pid") == pid:
            return w
    return None


def window_by_id(windows_json, window_id):
    for w in windows_json:
        if w.get("id") == window_id:
            return w
    return None


def target_workspace(workspaces_json, windows_json, output_names=None):
    """The runner's target: the EMPTY workspace with the highest idx on the
    enabled output(s).  Returns None when there is none — the caller must
    abort rather than rely on niri creating a workspace on demand
    (P0a2-r1 correction 1).

    niri 26.04's workspaces JSON has no `is_empty` key (the real shape:
    id, idx, name, output, is_urgent, is_active, is_focused,
    active_window_id — see the sample pasted in test_niri.py), so a
    workspace counts as empty when its active_window_id is null AND no
    window in windows_json carries that workspace_id.

    `output_names` is the set of enabled output names (enabled_outputs());
    workspaces on other outputs are ignored — with two monitors the empty
    end workspace of the *other* output must never be targeted."""
    occupied = set(w.get("workspace_id") for w in windows_json)
    candidates = [ws for ws in workspaces_json
                  if ws.get("active_window_id") is None
                  and ws.get("id") not in occupied
                  and (output_names is None or ws.get("output") in output_names)]
    if not candidates:
        return None
    return max(candidates, key=lambda ws: ws["idx"])


def focused_workspace_id(workspaces_json):
    for ws in workspaces_json:
        if ws.get("is_focused"):
            return ws.get("id")
    return None


def readout_geometry(window):
    """(position, size) niri reports for a window, or (None, None) when the
    window is tiled and reports no position (tile_pos_in_workspace_view is
    null for tiled windows — observed 2026-10-30)."""
    layout = window.get("layout") or {}
    pos = layout.get("tile_pos_in_workspace_view")
    size = layout.get("tile_size")
    if pos is None or size is None:
        return None, None
    return tuple(pos), tuple(size)
