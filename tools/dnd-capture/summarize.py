# SPDX-License-Identifier: GPL-3.0-or-later
"""Reduce one drag-and-drop capture run to summary.md and summary.json.

Usage: python3 -I summarize.py <run-dir>

Reads <run-dir>/source.log, zen.log, dbus.log, result.txt and meta.txt (all
optional except source.log/zen.log), and writes <run-dir>/summary.json and
<run-dir>/summary.md.  Standard library only.

WAYLAND_DEBUG line formats handled (see NOTES.md for how these were chosen):

  libwayland (C apps, and cosmic-files which links libwayland at runtime):
    [  12345.678] {Default Queue}  -> wl_data_source#45.offer("text/uri-list")   request
    [  12345.678] wl_data_source#45.target("text/uri-list")                      event
    [  12345.678] wl_registry#2.global(17, "wl_data_device_manager", 3)          event

  wayland-backend rs (pure Rust client backend; logs when WAYLAND_DEBUG is
  "1" or "client", tagged [rs], separator '@', events marked '<-'):
    [  12345.678][rs] -> wl_data_source@45.offer("text/uri-list")                request
    [  12345.678][rs] <- wl_data_source@45.target("text/uri-list")               event

The separator may be '#' or '@' and a '{Queue Name}' prefix may appear on
either style; neither is assumed to be in a fixed column.

Object identity is scoped by LIFETIME, not by a connection counter: an
object runs from its creation message to its destroy request or the
matching wl_display.delete_id, and the same id number can appear in several
successive lifetimes.  The drag's source is the wl_data_source whose
lifetime contains the (last) start_drag; the Zen offer is the wl_data_offer
whose lifetime contains the (last) enter.  When a log holds several drag
attempts, the last one is summarized and the attempt counts are reported.
"""

import json
import os
import re
import sys

LINE_RE = re.compile(
    r"^\[\s*(?P<sec>\d+)\.(?P<usec>\d+)\]"     # [sec.usec] timestamp
    r"(?:\[rs\])?"                             # optional rust-backend tag
    r"\s*(?:\{(?P<queue>[^}]*)\})?"            # optional {queue name}
    r"\s*(?P<dir><-|->)?"                      # request/event marker
    r"\s*(?:\[discarded\])?"                   # rust discarded marker
    r"\s*(?P<iface>[A-Za-z0-9_]+)[#@](?P<oid>\d+)"
    r"\.(?P<method>[A-Za-z0-9_]+)"
    r"\((?P<args>.*)\)\s*$"
)

ACTION_BITS = ((1, "copy"), (2, "move"), (4, "ask"))


def decode_actions(mask):
    return [name for bit, name in ACTION_BITS if mask & bit]


def ref(kind, args):
    """First object reference of `kind` ("wl_data_source" etc.) in args."""
    m = re.search(kind + r"[@#](\d+)", args)
    return "%s#%s" % (kind, m.group(1)) if m else None


def nums_in(text):
    return re.findall(r"-?\d+", text)


def strings_in(text):
    return re.findall(r'"([^"]*)"', text)


class Msg(object):
    __slots__ = ("idx", "line", "sec", "usec", "queue", "request", "iface", "oid", "method", "args")

    def __init__(self, idx, m, line):
        self.idx = idx
        self.line = line  # 1-based line number in the log file
        self.sec = int(m.group("sec"))
        self.usec = int(m.group("usec"))
        self.queue = m.group("queue")
        self.request = m.group("dir") == "->"
        self.iface = m.group("iface")
        self.oid = int(m.group("oid"))
        self.method = m.group("method")
        self.args = m.group("args")

    @property
    def obj(self):
        return "%s#%d" % (self.iface, self.oid)


class Lifetime(object):
    """One life of an object id: born at `birth` (message index), dies at
    `death` (index of its destroy request or delete_id; None = still alive)."""

    __slots__ = ("iface", "oid", "birth", "death")

    def __init__(self, iface, oid, birth):
        self.iface = iface
        self.oid = oid
        self.birth = birth
        self.death = None

    def contains(self, idx):
        return self.birth <= idx and (self.death is None or idx < self.death)


# Creation arguments: "new id <iface>#N" — wl_display.get_registry
# (wayland.xml:55), wl_registry.bind (:137),
# wl_data_device_manager.create_data_source (:1044) and get_data_device
# (:1051) all create objects this way.
NEW_ID_RE = re.compile(r"new id ([A-Za-z0-9_]+)[#@](\d+)")


def build_lifetimes(msgs):
    """One pass over the log recording the lifetime of every object we can.

    Birth: a message whose args contain `new id <iface>#N`, or — for data
    offers — the wl_data_device.data_offer event (wayland.xml:936), whose id
    argument is the new offer (real libwayland prints it with the `new id`
    prefix, other loggers without; both accepted here).

    Death: the object's own `destroy` request (type="destructor":
    wl_data_offer.destroy wayland.xml:585, wl_data_source.destroy :738), or
    the wl_display.delete_id event (:101) — per the spec the id may only be
    reused once delete_id is received, which is exactly what lets the same
    number appear in several successive lifetimes.  As a safety net (the
    protocol never reuses a live id), a `new id` for an id whose lifetime is
    still open closes it and starts a fresh lifetime, so two objects can
    never have their messages merged.
    """
    live = {}  # oid -> current Lifetime
    out = []

    def create(iface, oid, idx):
        cur = live.get(oid)
        if cur is not None and cur.death is None:
            cur.death = idx  # open lifetime hit by a new id: close it here
        lt = Lifetime(iface, oid, idx)
        live[oid] = lt
        out.append(lt)

    for m in msgs:
        created_here = set()
        for iface, oid_s in NEW_ID_RE.findall(m.args):
            if iface == "unknown":  # `new id [unknown]#N` (bind): type not known
                continue
            oid = int(oid_s)
            create(iface, oid, m.idx)
            created_here.add((iface, oid))
        if m.iface == "wl_data_device" and m.method == "data_offer":
            # cover logs that print the offer without the `new id` prefix;
            # skip when the generic path above already created it
            offer = ref("wl_data_offer", m.args)
            if offer:
                oid = int(offer.split("#")[1])
                if ("wl_data_offer", oid) not in created_here:
                    create("wl_data_offer", oid, m.idx)
        if m.method == "destroy":
            cur = live.get(m.oid)
            if cur is not None and cur.death is None:
                cur.death = m.idx
        if m.iface == "wl_display" and m.method == "delete_id":
            for n in nums_in(m.args):
                cur = live.get(int(n))
                if cur is not None and cur.death is None:
                    cur.death = m.idx
    return out


def containing(lifetimes, iface, oid, idx):
    """The lifetime of iface#oid that is alive at message index `idx`."""
    found = None
    for lt in lifetimes:
        if lt.iface == iface and lt.oid == oid and lt.contains(idx):
            found = lt  # ids are unique while alive; a match is unambiguous
    return found


def lifetime_label(lt, msgs):
    """Human label that survives id reuse (P0a-r3 correction 2): the object
    id plus the span of its lifetime — from the creating message's line to
    its destroy/delete_id line, open-ended while still alive.  Line numbers
    refer to the raw log, so they can be checked by hand."""
    if lt.death is not None:
        return "%s#%d (lines %d–%d)" % (lt.iface, lt.oid, msgs[lt.birth].line,
                                        msgs[lt.death].line)
    return "%s#%d (from line %d)" % (lt.iface, lt.oid, msgs[lt.birth].line)


def parse_wayland_log(path):
    msgs = []
    with open(path, "r", errors="replace") as fh:
        for line_no, line in enumerate(fh, 1):
            m = LINE_RE.match(line)
            if m:
                msgs.append(Msg(len(msgs), m, line_no))
    return msgs


def read_meta(rundir):
    meta = {}
    path = os.path.join(rundir, "meta.txt")
    if os.path.exists(path):
        with open(path, errors="replace") as fh:
            for line in fh:
                if ":" in line:
                    k, v = line.split(":", 1)
                    meta[k.strip()] = v.strip()
    return meta


def read_result(rundir):
    result = {"zen_opened_file": None, "note": None, "raw_answer": None}
    path = os.path.join(rundir, "result.txt")
    if not os.path.exists(path):
        return result
    with open(path, errors="replace") as fh:
        lines = [l.rstrip("\n") for l in fh]
    for line in lines:
        if line.lower().startswith("zen_opened_file:"):
            answer = line.split(":", 1)[1].strip()
            result["raw_answer"] = answer
            low = answer.lower()
            if low in ("y", "yes"):
                result["zen_opened_file"] = True
            elif low in ("n", "no"):
                result["zen_opened_file"] = False
        elif line.lower().startswith("note:"):
            result["note"] = line.split(":", 1)[1].strip() or None
    return result


def summarize_source(msgs):
    """The drag's data source is the wl_data_source whose lifetime contains
    the last start_drag (wayland.xml:883 — start_drag carries the source as
    its first argument); any other data source in the log (clipboard,
    earlier attempts) is reported separately.  With several attempts, the
    LAST one is summarized."""
    lifetimes = build_lifetimes(msgs)
    start_drag_idx = [m.idx for m in msgs
                      if m.iface == "wl_data_device" and m.method == "start_drag"]
    source_msgs = [m for m in msgs if m.iface == "wl_data_source"]
    source_lts = [lt for lt in lifetimes if lt.iface == "wl_data_source"]

    # labels carry the lifetime's line span, so a reused id (f6: two
    # lifetimes of wl_data_source#40) stays distinguishable
    all_sources = [lifetime_label(lt, msgs) for lt in source_lts]

    drag_lt = None
    start_drag_info = None
    if start_drag_idx:
        sd = msgs[start_drag_idx[-1]]  # last attempt
        obj = ref("wl_data_source", sd.args)
        if obj:
            drag_lt = containing(lifetimes, "wl_data_source", int(obj.split("#")[1]), sd.idx)
        if drag_lt is None:
            # source ref missing or untracked: newest data source alive there
            for lt in lifetimes:
                if lt.iface == "wl_data_source" and lt.contains(sd.idx):
                    drag_lt = lt
        surfaces = re.findall(r"wl_surface[@#](\d+)", sd.args)
        start_drag_info = {
            "source": "wl_data_source#%d" % drag_lt.oid if drag_lt else obj,
            "origin_surface": "wl_surface#%s" % surfaces[0] if surfaces else None,
            "icon": ("wl_surface#%s" % surfaces[1] if len(surfaces) > 1
                     else "nil" if re.search(r"\bnil\b", sd.args) else None),
            "serial": nums_in(sd.args)[-1] if nums_in(sd.args) else None,
        }

    drag = []
    if drag_lt is not None:
        drag = [m for m in source_msgs
                if m.oid == drag_lt.oid
                and containing(lifetimes, "wl_data_source", m.oid, m.idx) is drag_lt]
    other = [lifetime_label(lt, msgs) for lt in source_lts if lt is not drag_lt]

    offers, targets, sends, actions, set_actions = [], [], [], [], []
    cancelled = drop_performed = finished = False
    for m in drag:
        if m.method == "offer":
            mime = strings_in(m.args)
            if mime:
                offers.append(mime[0])
        elif m.method == "target":
            t = strings_in(m.args)
            targets.append(t[0] if t else None)
        elif m.method == "send":
            mime = strings_in(m.args)
            n = nums_in(m.args)
            sends.append({"mime": mime[0] if mime else None, "fd": n[-1] if n else None})
        elif m.method == "action":
            n = nums_in(m.args)
            if n:
                actions.append(int(n[-1]))
        elif m.method == "set_actions":
            n = nums_in(m.args)
            set_actions.append({"args": [int(x) for x in n],
                                "decoded": [decode_actions(int(x)) for x in n]})
        elif m.method == "cancelled":
            cancelled = True
        elif m.method == "dnd_drop_performed":
            drop_performed = True
        elif m.method == "dnd_finished":
            finished = True

    drag_seen = start_drag_info is not None
    if not drag_seen:
        ending = "no drag seen"
    elif finished or drop_performed:
        ending = ("dnd_drop_performed+dnd_finished"
                  if (drop_performed and finished)
                  else "dnd_finished" if finished else "dnd_drop_performed")
    elif cancelled:
        ending = "cancelled"
    else:
        ending = "incomplete"

    return {
        "drag_seen": drag_seen,
        "drag_source_object": "wl_data_source#%d" % drag_lt.oid if drag_lt else None,
        "all_data_sources": all_sources,
        "other_sources_excluded": other,
        "start_drag_count": len(start_drag_idx),
        "attempt": ("attempt %d of %d" % (len(start_drag_idx), len(start_drag_idx))
                    if drag_seen else None),
        "start_drag": start_drag_info,
        "offers": offers,
        "set_actions": set_actions,
        "target": targets,
        "send": sends,
        "action": actions,
        "cancelled": cancelled,
        "dnd_drop_performed": drop_performed,
        "dnd_finished": finished,
        "ending": ending,
    }


def summarize_zen(msgs):
    """The drag's offer is the wl_data_offer whose lifetime contains the
    LAST wl_data_device.enter in the log (wayland.xml:949) — the last
    attempt that reached Zen, whichever side ended it.  "dropped" records
    whether a drop (:984) followed that enter.  The number of Zen-side
    attempts is the number of distinct offer lifetimes that got an enter;
    the caller compares it against start_drag_count (attempts_match).
    Selection/clipboard offers (wl_data_device.selection, :1002) are
    ignored."""
    lifetimes = build_lifetimes(msgs)
    dev = [m for m in msgs if m.iface == "wl_data_device"]

    drop_idx = [m.idx for m in dev if m.method == "drop"]
    enters, leaves, drops = [], [], []
    entered_lts = []
    enter_idx = None
    for m in dev:
        if m.method == "enter":
            n = nums_in(m.args)
            enters.append({"serial": n[0] if n else None,
                           "surface": ref("wl_surface", m.args),
                           "offer": ref("wl_data_offer", m.args)})
            enter_idx = m.idx  # keep the last enter in the log
            o = ref("wl_data_offer", m.args)
            if o:
                lt = containing(lifetimes, "wl_data_offer", int(o.split("#")[1]), m.idx)
                if lt is not None and lt not in entered_lts:
                    entered_lts.append(lt)
        elif m.method == "leave":
            n = nums_in(m.args)
            leaves.append({"serial": n[0] if n else None})
        elif m.method == "drop":
            n = nums_in(m.args)
            drops.append({"serial": n[0] if n else None})

    dropped = any(i > enter_idx for i in drop_idx) if enter_idx is not None else False

    offer_lt = None
    if enter_idx is not None:
        o = ref("wl_data_offer", msgs[enter_idx].args)
        if o:
            offer_lt = containing(lifetimes, "wl_data_offer", int(o.split("#")[1]), enter_idx)
        if offer_lt is None:
            for lt in lifetimes:
                if lt.iface == "wl_data_offer" and lt.contains(enter_idx):
                    offer_lt = lt  # newest offer alive at the enter

    offer_msgs = []
    if offer_lt is not None:
        offer_msgs = [m for m in msgs
                      if m.iface == "wl_data_offer" and m.oid == offer_lt.oid
                      and containing(lifetimes, "wl_data_offer", m.oid, m.idx) is offer_lt]

    offers, source_actions, action, accepts, set_actions, receives = [], [], [], [], [], []
    finish = False
    for m in offer_msgs:
        if m.method == "offer":
            s = strings_in(m.args)
            if s:
                offers.append(s[0])
        elif m.method == "source_actions":
            n = nums_in(m.args)
            if n:
                source_actions.append({"mask": int(n[-1]), "decoded": decode_actions(int(n[-1]))})
        elif m.method == "action":
            n = nums_in(m.args)
            if n:
                action.append({"mask": int(n[-1]), "decoded": decode_actions(int(n[-1]))})
        elif m.method == "accept":
            s = strings_in(m.args)
            n = nums_in(m.args)
            accepts.append({"serial": n[0] if n else None,
                            "mime": s[0] if s else None})
        elif m.method == "set_actions":
            n = nums_in(m.args)
            set_actions.append({"args": [int(x) for x in n],
                                "decoded": [decode_actions(int(x)) for x in n]})
        elif m.method == "receive":
            s = strings_in(m.args)
            n = nums_in(m.args)
            receives.append({"mime": s[0] if s else None, "fd": n[-1] if n else None})
        elif m.method == "finish":
            finish = True

    return {
        "drag_offer_object": ("wl_data_offer#%d" % offer_lt.oid) if offer_lt else None,
        "attempt_count": len(entered_lts),
        "dropped": dropped,
        "drop_count": len(drop_idx),
        "enter": enters,
        "leave": leaves,
        "drop": drops,
        "offers": offers,
        "source_actions": source_actions,
        "action": action,
        "accept": accepts,
        "set_actions": set_actions,
        "receive": receives,
        "finish": finish,
    }


DBUS_HEADER_RE = re.compile(
    r"^(?P<kind>method call|method return|signal|error)\s+time=(?P<time>\S+)\s+"
    r"sender=(?P<sender>\S+)\s+->\s+destination=(?P<dest>\S+)\s+serial=(?P<serial>\S+)"
    r"(?:\s+reply_serial=(?P<reply>\S+))?"
    r"(?:.*?path=(?P<path>[^;]*))?"
    r"(?:; interface=(?P<iface>[^;]*))?"
    r"(?:; member=(?P<member>\S+))?"
)


def summarize_dbus(path):
    """Parse dbus-monitor output.  run.sh pre-filters the capture to the
    document portal (FileTransfer + Documents interfaces, replies from the
    portal, and FileTransfer signals); entries not matching those shapes are
    dropped here as well."""
    if not path or not os.path.exists(path):
        return {"portal_calls": []}

    entries = []
    current = None
    with open(path, errors="replace") as fh:
        for line in fh:
            m = DBUS_HEADER_RE.match(line)
            if m:
                if current:
                    entries.append(current)
                kind = m.group("kind").replace(" ", "_")
                current = {"kind": kind,
                           "sender": m.group("sender"),
                           "destination": m.group("dest"),
                           "serial": m.group("serial"),
                           "reply_serial": m.group("reply"),
                           "interface": m.group("iface"),
                           "member": m.group("member"),
                           "body": []}
            elif current is not None and line.startswith("   "):
                current["body"].append(line.rstrip("\n"))
    if current:
        entries.append(current)

    def body_strings(e):
        return strings_in("\n".join(e["body"]))

    portal = []
    # Serial -> entry, so method_return entries can be attached to the
    # portal call they answer.
    by_serial = {}
    for e in entries:
        if (e["kind"] in ("method_call", "signal")
                and e["interface"] in ("org.freedesktop.portal.FileTransfer",
                                       "org.freedesktop.portal.Documents")):
            rec = {"kind": e["kind"],
                   "interface": e["interface"],
                   "member": e["member"],
                   "sender": e["sender"],
                   "destination": e["destination"],
                   "strings": body_strings(e),
                   "returned": None}
            if e["kind"] == "method_call":
                by_serial[(e["sender"], e["serial"])] = rec
            portal.append(rec)
        elif e["kind"] in ("method_return", "error") and e["reply_serial"] is not None:
            call = by_serial.get((e["destination"], e["reply_serial"]))
            if call is not None:
                if e["kind"] == "method_return":
                    call["returned"] = body_strings(e)
                else:
                    errs = body_strings(e)
                    call["returned"] = "ERROR: %s" % errs[0] if errs else "ERROR"

    return {"portal_calls": portal}


def summarize_rundir(rundir):
    def maybe(name):
        p = os.path.join(rundir, name)
        return p if os.path.exists(p) else None

    source_msgs = parse_wayland_log(maybe("source.log")) if maybe("source.log") else []
    zen_msgs = parse_wayland_log(maybe("zen.log")) if maybe("zen.log") else []

    src = summarize_source(source_msgs)
    zen = summarize_zen(zen_msgs)
    summary = {
        "run": os.path.basename(os.path.abspath(rundir)),
        "meta": read_meta(rundir),
        "source_app": read_meta(rundir).get("source"),
        "location": read_meta(rundir).get("location"),
        "drag_seen": src["drag_seen"],
        "attempts_match": src["start_drag_count"] == zen["attempt_count"],
        "result": read_result(rundir),
        "source_side": src,
        "zen_side": zen,
        "dbus": summarize_dbus(maybe("dbus.log")),
        "parsing_notes": {
            "source_messages": len(source_msgs),
            "zen_messages": len(zen_msgs),
            "source_log_present": maybe("source.log") is not None,
            "zen_log_present": maybe("zen.log") is not None,
            "dbus_log_present": maybe("dbus.log") is not None,
        },
    }
    return summary


def render_md(s):
    res = s["result"]
    src = s["source_side"]
    zen = s["zen_side"]
    db = s["dbus"]["portal_calls"]

    opened = {True: "yes", False: "no", None: "?"}[res["zen_opened_file"]]
    lines = []
    lines.append("# Drag-and-drop capture: %s" % s["run"])
    lines.append("")
    lines.append("Source app: %s — location: %s — Zen opened the file: %s" %
                 (s.get("source_app") or "?", s.get("location") or "?", opened))
    if not s.get("attempts_match", True):
        lines.append("source saw %d drags, Zen saw %d; check the raw logs" %
                     (src["start_drag_count"], zen["attempt_count"]))
    if res.get("note"):
        lines.append("Note: %s" % res["note"])
    lines.append("")
    lines.append("## Source side (%s)" % (s.get("source_app") or "source app"))
    lines.append("")
    if not src["drag_seen"]:
        lines.append("- Drag source: no drag seen (no start_drag in source.log)")
        if src["other_sources_excluded"]:
            lines.append("- Other data sources seen: %s" % "; ".join(src["other_sources_excluded"]))
    else:
        lines.append("- Drag source: %s — %s (chosen by wl_data_device.start_drag)" %
                     (src["drag_source_object"], src["attempt"]))
        if src["start_drag_count"] > 1:
            lines.append("- Attempts: %d start_drag(s); summarizing the last" % src["start_drag_count"])
        if src["other_sources_excluded"]:
            lines.append("- Other data sources ignored (not the drag): %s" %
                         "; ".join(src["other_sources_excluded"]))
        lines.append("- Offers (in order): %s" % (", ".join(src["offers"]) or "none"))
        if src["set_actions"]:
            dec = "; ".join("|".join(d) or "none" for d in src["set_actions"][0]["decoded"])
            lines.append("- set_actions: %s (%s)" % (dec, ", ".join(str(a) for a in src["set_actions"][0]["args"])))
        if src["target"]:
            lines.append("- target events: %s" % ", ".join(str(t) for t in src["target"]))
        if src["send"]:
            lines.append("- send: %s" % "; ".join("%s (fd %s)" % (x["mime"], x["fd"]) for x in src["send"]))
        if src["action"]:
            dec = "; ".join("|".join(decode_actions(a)) or "none" for a in src["action"])
            lines.append("- action events: %s (%s)" % (dec, ", ".join(str(a) for a in src["action"])))
        lines.append("- Ending: %s" % src["ending"])
    lines.append("")
    lines.append("## Zen side")
    lines.append("")
    if zen["drag_offer_object"] is None:
        lines.append("- Drag offer: no data offer received")
    else:
        lines.append("- Drag offer: %s (from the last wl_data_device.enter in the log)" %
                     zen["drag_offer_object"])
        lines.append("- attempts Zen saw: %d" % zen["attempt_count"])
        lines.append("- drop after enter: %s" % ("yes" if zen["dropped"] else "no"))
        lines.append("- drop count: %d" % zen["drop_count"])
        for e in zen["enter"]:
            lines.append("- enter: serial %s surface %s offer %s" %
                         (e["serial"], e["surface"], e["offer"]))
        for l in zen["leave"]:
            lines.append("- leave: serial %s" % l["serial"])
        for d in zen["drop"]:
            lines.append("- drop: serial %s" % d["serial"])
        lines.append("- Offers on the drag offer: %s" % (", ".join(zen["offers"]) or "none"))
        if zen["source_actions"]:
            dec = "; ".join("|".join(x["decoded"]) or "none" for x in zen["source_actions"])
            lines.append("- source_actions: %s (%s)" % (dec, ", ".join(str(x["mask"]) for x in zen["source_actions"])))
        if zen["action"]:
            dec = "; ".join("|".join(x["decoded"]) or "none" for x in zen["action"])
            lines.append("- action: %s (%s)" % (dec, ", ".join(str(x["mask"]) for x in zen["action"])))
        if zen["accept"]:
            lines.append("- accept: %s" % "; ".join("serial %s %s" % (a["serial"], a["mime"] or "nil") for a in zen["accept"]))
        if zen["set_actions"]:
            dec = "; ".join("|".join(d) or "none" for d in zen["set_actions"][0]["decoded"])
            lines.append("- set_actions: %s (%s)" % (dec, ", ".join(str(a) for a in zen["set_actions"][0]["args"])))
        if zen["receive"]:
            lines.append("- receive (MIME Zen read): %s" %
                         "; ".join("%s (fd %s)" % (r["mime"], r["fd"]) for r in zen["receive"]))
        lines.append("- finish: %s" % ("yes" if zen["finish"] else "no"))
    lines.append("")
    lines.append("## Portal D-Bus (%d calls)" % len(db))
    lines.append("")
    if db:
        for i, c in enumerate(db, 1):
            line = "%d. **%s** — caller %s" % (i, c["member"], c["sender"])
            if c["strings"]:
                line += " — args: %s" % ", ".join('"%s"' % x for x in c["strings"])
            if c["returned"]:
                line += " — returned: %s" % ", ".join('"%s"' % x for x in c["returned"])
            lines.append(line)
    else:
        lines.append("No portal FileTransfer/Documents traffic captured (expected when the")
        lines.append("dragged file lives inside the Flatpak sandbox mount).")
    lines.append("")
    return "\n".join(lines)


def main(argv):
    if len(argv) != 2:
        sys.stderr.write(__doc__)
        return 2
    rundir = argv[1]
    if not os.path.isdir(rundir):
        sys.stderr.write("not a directory: %s\n" % rundir)
        return 2

    s = summarize_rundir(rundir)

    with open(os.path.join(rundir, "summary.json"), "w") as fh:
        json.dump(s, fh, indent=2, sort_keys=True)
        fh.write("\n")
    with open(os.path.join(rundir, "summary.md"), "w") as fh:
        fh.write(render_md(s))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
