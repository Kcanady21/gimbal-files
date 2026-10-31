# NOTES — P0a capture harness

Every claim is tagged **[read]** (source/doc on disk), **[observed]** (ran it
on this machine and watched) or **[inferred]** (reasoning, marked when it's
load-bearing). Date of all observations: 2026-10-30.

## Environment as found

- niri 26.04 (`niri msg version` → `Compositor version: 26.04 (unknown commit)`),
  Wayland socket `/run/user/1000/wayland-1`. **[observed]**
- Zen is Flatpak `app.zen_browser.zen` 1.24b and **was running** (PID 48894)
  during all of the checks below; Kyle's main Zen was never stopped. **[observed]**
- Dolphin was already running (PID 130898) during the checks; left running. **[observed]**
- Nautilus / Thunar / cosmic-files were **not** running at the start. **[observed]**
- `pytest` is not installed, so the tests are plain `unittest` (the brief
  allows either). **[observed]**
- Versions at capture time: dolphin 25.12.3, nautilus 50.2.2, thunar 4.20.7,
  Zen 1.24b, cosmic-files git 09bdd8251d5f (Kyle's build, today). **[observed]**

## D-Bus monitor: dbus-monitor chosen over busctl monitor

- Both tools work **without root** on the session bus here — verified live:
  each attached and captured a real
  `org.freedesktop.portal.FileTransfer.StartTransfer` call made from a
  separate process. **[observed]**
- Chosen: **`dbus-monitor --session`**, for two reasons **[observed]**:
  - Match rules can capture replies by *sender* — the run needs the values
    **returned** by the portal (`StartTransfer`'s transfer key,
    `RetrieveFiles`' file list), and replies are matched to calls via
    `reply_serial` in summarize.py. `busctl monitor`'s match syntax plus its
    multi-line binary dump format made that needlessly hard to parse.
  - dbus-monitor's text format is one header line + indented args, which
    `summarize.py` parses directly.
- The exact match set used in run.sh:
  `method_call` on `org.freedesktop.portal.FileTransfer` and
  `org.freedesktop.portal.Documents`, `method_return`/`error` **from**
  `org.freedesktop.portal.Documents`, and FileTransfer signals
  (`TransferClosed`). **[observed]** (the reply matches were verified to fire.)

## Portal topology — correction to the brief's assumption

- `org.freedesktop.portal.FileTransfer` is **not** on the
  `org.freedesktop.portal.Desktop` object. It is exported by
  **xdg-document-portal** on bus name `org.freedesktop.portal.Documents`,
  object path `/org/freedesktop/portal/documents`, next to the Documents
  interface. Verified by introspecting all portal objects. **[observed]**
- Interface (introspected): `StartTransfer(a{sv} → s)`,
  `AddFiles(s, h, a{sv})`, `RetrieveFiles(s, a{sv} → as)`,
  `StopTransfer(s)`, signal `TransferClosed(s)`, `version = 1`. **[observed]**
- Portal transfer sessions are per-caller: `StopTransfer` from a second
  connection → `Access denied`. Harmless for us; noted so a failed
  StopTransfer in a capture isn't mistaken for a bug. **[observed]**

## Zen

- **Throwaway profile created**: `flatpak run app.zen_browser.zen
  -CreateProfile gimbal-dnd` → registered as `Profile2` /
  `bt9lefdm.gimbal-dnd` in `~/.var/app/app.zen_browser.zen/.zen/profiles.ini`
  while Kyle's Zen was running; his profiles untouched (still `Default=1`).
  **[observed]**
- **`--new-instance` works**: `flatpak run --env=… app.zen_browser.zen
  --new-instance -P gimbal-dnd about:blank` started a **separate sandbox**
  while his Zen was open — `flatpak ps` showed two `app.zen_browser.zen`
  rows, and the new process tree had its own main process
  (`/app/zen/zen … --new-instance -P gimbal-dnd about:blank`). No quit
  required. **[observed]** (Verified twice, plus two full run.sh smoke tests.)
- `WAYLAND_DEBUG=1` passes through `flatpak run --env=…` and produces full
  libwayland traffic in zen.log (≈10k lines in a ~15 s run, ≈1300 wl_ lines
  in the smoke tests). **[observed]**
- **MOZ_LOG module `WidgetDrag` is unverified.** With
  `MOZ_LOG=NotARealModuleXYZ:5`, Zen prints **no warning at all** — an
  invalid module fails silently, so an empty MOZ_LOG section in zen.log does
  not distinguish a wrong name from "no drag happened". No `WidgetDrag` lines
  appear during plain startup (no drag = nothing to log). **[observed]**
  → P0g must confirm the module name from Firefox/Zen source;
  `GIMBAL_MOZ_LOG_MODULE=… ./run.sh …` overrides without editing anything.
- Cleanup detail that cost one leak: `kill -TERM` on the **flatpak wrapper**
  PID does **not** stop the sandboxed browser (flatpak ps keeps listing it).
  Killing the **inner** `/app/zen/zen … -P gimbal-dnd` main process takes the
  whole tree down cleanly. run.sh resolves that inner PID via
  `pgrep -f '^/app/zen/zen .*-P gimbal-dnd'` — that cmdline exists only on
  the throwaway instance, so Kyle's Zen can never match. **[observed]**

## Source apps: fresh-process behavior (the brief's key question)

Empirically tested by launching two instances and watching which processes
survived; the logged instance ran with `WAYLAND_DEBUG=1`.

| app | single-instance hand-off? | fresh-process method found | evidence |
|---|---|---|---|
| dolphin | no — second process ran alongside Kyle's (two `dolphin` processes coexisted) | none needed; plain launch is already a fresh process | **[observed]** |
| thunar | **yes** — second `thunar <dir>` exited instantly (0 log lines), window opened in the first instance | none in `--help` (`-w/--window`, `--daemon`, `-q/--quit` only); README tells Kyle to quit first; run.sh refuses when `org.xfce.FileManager` has an owner | **[observed]** + **[read]** of `thunar --help` |
| nautilus | **yes** — same behavior, second process died before logging a single Wayland line | none in `--help` (`-w/--new-window`, `-q/--quit`, `-s/--select`); same refusal in run.sh (`org.gnome.Nautilus`) | **[observed]** + **[read]** of `nautilus --help` |
| cosmic | no — two processes coexisted | none needed; additionally started with **`--no-daemon`** (see below) | **[observed]** |

- Dolphin registers its D-Bus name with the PID baked in
  (`org.kde.dolphin-130898`), consistent with it being multi-instance.
  **[observed]**
- run.sh detects a running Thunar/Nautilus with
  `org.freedesktop.DBus.NameHasOwner` on the session bus (verified true/false
  behavior). It never quits Kyle's apps. **[observed]**

### cosmic-files daemonize trap

- `~/cosmic-files/src/lib.rs:177` sets
  `daemonize = std::io::stdin().is_terminal()` and `lib.rs:219-229` then
  `fork::daemon(true, true)` — parent exits with code 0. **[read]**
- Consequence: from an interactive terminal the launched process **forks
  into the background and the `$!` child exits**, escaping PID-based
  cleanup. (My test runs from a non-tty shell did not daemonize, which is why
  the two-process test worked.) run.sh therefore passes **`--no-daemon`**,
  which `lib.rs:181` handles explicitly. **[read]** + **[inferred]** for the
  interactive case (not exercised with a tty here).
- cosmic-files also has **no `--version`**: every argument that isn't a known
  flag is parsed as a path/URL (`lib.rs:180-217`); `--help`/`--version`
  launch the app (verified live — the `--help` probe opened a window, killed
  by PID immediately). meta.txt records the git commit of `~/cosmic-files`
  instead. **[read]** + **[observed]**

## Does the pure-Rust wayland-backend honor WAYLAND_DEBUG?

The brief's question, answered from source:

- Yes. `wayland-backend` (both 0.3.11 and 0.3.17 — `src/debug.rs` is
  byte-identical between them) enables client logging when `WAYLAND_DEBUG` is
  `"1"` or `"client"` (`debug.rs:12-14`), unconditionally in
  `rs/client_impl/mod.rs:170` and `sys/client_impl/mod.rs:294` — **not
  feature-gated** (checked the crate's feature list). **[read]**
- The rs backend's format differs from libwayland's: timestamps tagged
  `[rs]`, separator `@` instead of `#`, requests `->`, events **`<-`**
  (`debug.rs:20-57`). The parser handles it (fixtures f2 and the unit tests
  cover `[rs]`/`@`/`<-`/`[discarded]`). **[read]**
- **But the cosmic-files binary Kyle runs does not currently log in that
  format.** Under `WAYLAND_DEBUG=1` it emits plain libwayland `#`-style
  lines, and `/proc/<pid>/maps` of a live instance showed
  `libwayland-client.so.0.24.0` mapped in (dlopen'd — invisible to `ldd`).
  **[observed]** So *something* in cosmic-files' tree uses libwayland directly
  (not identified further — open item). Practical consequence: the COSMIC
  capture works either way, because the parser accepts both formats.
- cosmic-files' log shows more than one Wayland connection inside one
  process. The tell is a repeated id, not a growing one: `wl_registry#2`
  appears twice in the run below (id numbering starts near 1 on every
  connection, so two registries numbered 2 mean two connections; the
  increasing ids `#2`, `#32`, `#77` by themselves are not evidence — one
  connection can allocate increasing ids just the same). Lines without a
  `{queue}` tag are events on unnamed queues; they do not mark a different
  connection either. Because ids repeat across the log, they are ambiguous
  on their own; since P0a-r1 the parser scopes each object by its lifetime
  (creation line to destroy/delete_id) instead of guessing connections —
  see "P0a-r1 changes" at the end of this file. **[observed]**

## Real WAYLAND_DEBUG lines (home paths removed; none of these had any)

From the dolphin run (libwayland, queue names in braces, `#`):

```
[1840479.301] {Default Queue}  -> wl_display#1.sync(new id wl_callback#3)
[1840479.386] {Display Queue} wl_display#1.delete_id(3)
[1840479.392] {Default Queue} wl_registry#2.global(1, "wl_compositor", 6)
[1840479.396] {Default Queue}  -> wl_registry#2.bind(1, "wl_compositor", 6, new id [unknown]#4)
[1840479.430] {Default Queue} wl_registry#2.global(17, "wl_data_device_manager", 3)
```

From the Zen run (same shape — flatpak passes the debug through):

```
[1330243.730] {Default Queue}  -> wl_display#1.get_registry(new id wl_registry#2)
[1330243.882] {Default Queue} wl_registry#2.global(1, "wl_compositor", 6)
[1330243.886] {Default Queue}  -> wl_registry#2.bind(1, "wl_compositor", 3, new id [unknown]#4)
```

From the cosmic-files run (note: `wl_registry#2` appears twice — the mark of
a second connection; the first line simply has no `{queue}` tag):

```
[1702139.187] wl_registry#2.global(17, "wl_data_device_manager", 3)          # unnamed queue, earlier connection
[1702205.100] {Default Queue} wl_registry#2.global(17, "wl_data_device_manager", 3)  # later connection, same id number
```

Real dbus-monitor lines captured during the monitor test (caller was my
probe, `:1.746`; portal is `:1.261`):

```
method call time=1793407831.255934 sender=:1.746 -> destination=org.freedesktop.portal.Documents serial=2 path=/org/freedesktop/portal/documents; interface=org.freedesktop.portal.FileTransfer; member=StartTransfer
   array [
   ]
method return time=1793407831.256775 sender=:1.261 -> destination=:1.746 serial=73 reply_serial=2
   string "231357448812495671314748954923190749594"
```

## Fixtures — protocol basis (cited from /usr/share/wayland/wayland.xml)

- `wl_data_offer` interface: line 521 — request `receive` (563), request
  `accept` (542), request `finish` (601), request `set_actions` (620), event
  `offer` (591), event `source_actions` (660), event `action` (671).
- `wl_data_source` interface: line 714 — request `offer` (729), request
  `set_actions` (791), events `target` (744), `send` (754), `cancelled`
  (764), `dnd_drop_performed` (811), `dnd_finished` (825), `action` (836).
- `wl_data_device` interface: line 869 — request `start_drag` (883), events
  `data_offer` (936), `enter` (949), `leave` (964), `motion` (972), `drop`
  (984), `selection` (1002).
- `wl_data_device_manager`: line 1030.
- Fixture details derived from these lines: `set_actions` masks decode as
  1=copy, 2=move, 4=ask (documented in the XML action summaries); events on
  the data source (`target`/`send`/`cancelled`/…) are compositor→client and
  are logged without `->`, while client requests carry `->`.
- f1 (success + clipboard source), f2 (cancelled drag, `@`/`[rs]`/`<-`
  format, no dbus.log), f3 (success + portal calls). Each fixture carries
  `meta.txt`, `result.txt` and `expected-summary.json`; the clipboard offer
  in f1's zen.log (`wl_data_offer#33` via `selection`) must be ignored by the
  parser, and f1's dbus.log contains only a Notifications call that must be
  filtered out.

## Parser decisions (summarize.py)

- The drag's data source on the source side = the `wl_data_source` whose id
  appears in `wl_data_device.start_drag` (wayland.xml:883); any other source
  (clipboard, f1's `wl_data_source#21`) is reported separately and excluded.
  **[read]**-based.
- The drag's offer on the Zen side = the offer id in the **last
  `wl_data_device.enter` before `drop`** (falls back to last enter when the
  drag is cancelled). `selection` events are ignored. Object ids are printed
  in the summary, as the brief requires.
- `summary.json` deliberately contains **no wall-clock timestamps** — only
  order, object ids, MIME types, masks and flags — so fixtures can be
  compared byte-for-byte against expected JSON and P0h gets stable input.
  **[inferred]** (choice).
- meta.txt is written **before** summarize.py runs (the brief lists it as
  step 7, after summarize) so summary.json can embed the versions; ordering
  within the run otherwise follows the brief. Deviation recorded here.
- Both D-Bus monitors were verified working before dbus-monitor was picked;
  the run.sh match set is the one that fired during the live probe.

## Tests

`python3 -I test_summarize.py -v` from this directory (22 tests):

```
Ran 22 tests in 0.210s

OK
```

(Also green without `-I`: `python3 -m unittest test_summarize`.) Coverage:
every line format variant (`#`/`@`, `[rs]`, `{queue}`, `[discarded]`,
brief's exact example line), each fixture against its frozen
`expected-summary.json`, plus targeted assertions (clipboard source excluded,
cancelled ending, portal call order StartTransfer→AddFiles→RetrieveFiles→
StopTransfer, RetrieveFiles return paths, missing dbus.log tolerated).

Two bugs were found and fixed while getting here: the D-Bus kind mismatch
(`"method call"` vs `"method_call"`) and a `set -e` unsafe `[ … ] && break`
in run.sh.

## End-to-end smoke tests

run.sh was executed twice for real (no drag performed): `dolphin outside` and
`thunar downloads`, answers piped in. Both produced complete run dirs (meta,
result, summary.md/json, gzipped logs with 1300+ / 400+ wayland lines each),
correct refusal when Thunar was already running (exit 1, message printed),
and **no leaked processes** after cleanup (checked `flatpak ps` / `pgrep`).
Kyle's Zen and Dolphin were running the whole time and were untouched. The
two smoke-test run dirs were deleted afterwards so P0b starts from a clean
captures/ (the `.gitignore` run.sh wrote there remains).

## Open items

1. **`WidgetDrag` MOZ_LOG module name** — unconfirmed here (fails silently if
   wrong); P0g confirms from Firefox source. Override with
   `GIMBAL_MOZ_LOG_MODULE` meanwhile.
2. **Which cosmic-files dependency dlopens libwayland** (and when, if ever,
   the rs `[rs]` format shows up in practice) — unidentified; parser handles
   both, so captures are safe either way.
3. **`wl_data_device.enter` argument types**: the XML marks x/y as `fixed`;
   libwayland prints them as decimals and the fixture uses integers — the
   parser only reads the offer id from enter args, so both are fine, but a
   real capture may show `120.00000`-style numbers nobody needs to parse.
4. Real drags will show whether Zen/portal calls appear that fixtures didn't
   anticipate (e.g. `RetrieveFiles` called by the portal backend rather than
   Zen — the monitor records whichever caller appears; summaries name it).
5. The smoke tests ran as my capture runs but could not exercise an actual
   drag; every parser/fixture claim above rests on the wayland.xml protocol
   text plus the live line formats, per the brief's "cited, not recalled".

## Review (Claude, Oct 30, 2026)

**Verdict: accepted with corrections.** The harness design is sound and the safety behavior is good:
it used a throwaway profile, `--new-instance` worked alongside Kyle's Zen, cleanup only killed PIDs it
started, and it refused to run against an already-running Thunar or Nautilus. Two problems block P0b:
correction 1 (fixed by Claude) and correction 2 (for the agent).

What I checked:
- Files written: only the Output paths, plus the Zen profile the brief allowed. `~/cosmic-files` is
  still clean at `09bdd825`.
- `python3 -I test_summarize.py`: 22 tests OK.
- Citations: all 26 `wayland.xml` line numbers, `~/cosmic-files/src/lib.rs:177/181/219-229`, and
  wayland-backend 0.3.17 `debug.rs:12-14`, `rs/client_impl/mod.rs:170` and `sys/client_impl/mod.rs:294`
  are all correct.
- Live no-drag run of `run.sh dolphin downloads`, after correction 1: the run directory was complete,
  meta.txt was right, nothing leaked (Kyle's Zen, Dolphin and dbus-monitor counts were unchanged), and
  source.log had 1041 `wl_` lines. That run directory has been removed.

Corrections:

1. **Fixed by Claude: neither script had a `#!` line.** `./run.sh`, the command the README gives,
   ran under `dash` and failed at line 150 (`Syntax error: "(" unexpected`) on the bash arrays. The
   smoke tests must have used `bash run.sh`. I added `#!/usr/bin/env bash` as line 1 of both `run.sh`
   and `make-test-files.sh`, so the SPDX line is now line 2.
2. **For the agent: the parser loses the drag's offers when a registry is created mid-drag.** The
   connection counter goes up on every `wl_display#1.get_registry`, so a registry created between
   `create_data_source`/`offer` and `start_drag` puts the offers in a different "connection" from
   `start_drag`. Mesa's Vulkan WSI does create registries when it sets up a surface, so this can
   plausibly happen with the drag-icon surface in wgpu apps such as COSMIC. Result on
   `fixtures/f4-registry-mid-drag/`: `offers: []`, `set_actions: []`, and the drag's own source listed
   under `other_sources_excluded`. The heuristic is also wrong for real multiple connections, because
   their lines interleave in time, so "most recent get_registry" doesn't identify a connection.
   **Fix:** remove the connection counter. Scope each object by its lifetime instead: from the message
   that creates it (`new id <iface>#N`, or for offers the `wl_data_device.data_offer` event) to its
   `destroy` request or `wl_display.delete_id(N)`. The drag source is the `wl_data_source#N` whose
   lifetime contains `start_drag`. The Zen offer is the `wl_data_offer#N` whose lifetime contains the
   `enter` used. Write `expected-summary.json` for f4: offers `["text/uri-list",
   "text/plain;charset=utf-8"]`, set_actions `[3]` → copy+move, ending `dnd_drop_performed+dnd_finished`.
   Keep f1–f3 green.
3. **For the agent: more than one attempt in a run.** The source side uses the first `start_drag` and
   the Zen side uses the first `drop`, so if Kyle retries, the two sides can describe different
   attempts. Count `start_drag` and `drop` in the summary, summarize the **last** attempt on both sides,
   and add a fixture with one cancelled drag followed by one successful drag.
4. **For the agent: make the no-drag case explicit.** With no `start_drag`, summary.md shows
   "Drag source: ?" and "Ending: incomplete". Write "no drag seen (no start_drag in source.log)", and
   in summary.json set `"drag_seen": false`.
5. **For the agent: NOTES wording.** "Object numbering restarts: wl_registry#2, #32, #77" isn't
   evidence: those are increasing IDs, which three registries on one connection would also give. The
   real sign of a second connection is `wl_registry#2` appearing twice (the cosmic sample lines). Also,
   lines without a `{queue}` tag are events on unnamed queues; they don't mark a different connection.
   Reword this section.
6. **Open item 2 answered (Claude):** cosmic-files' `wayland-backend` 0.3.17 depends on `wayland-sys`
   (Cargo.lock), so it uses the **sys** backend, which drives libwayland through dlopen. libwayland
   prints the debug log. The `[rs]` format only appears for discarded sends
   (`sys/client_impl/mod.rs:596-599`, `debug.rs` `print_send_message`). Record this; no code change.

Not a defect, but noted: the zen-side `GetMountPoint` call at Zen startup shows up in dbus.log. That's
fine, and P0h should ignore it.

## P0a-r1 changes

Corrections 2, 3, 4 and 5 from the review above were applied. `run.sh` was
**not** run (parser-only fixes, per the r1 brief). Correction 1 (shebangs) and
item 6 were Claude's; untouched.

### Correction 2 — connection counter removed, objects scoped by lifetime

`summarize.py` no longer counts connections or guesses them from
`wl_display#1.get_registry` (that heuristic failed f4 and was wrong for real
multi-connection logs anyway). Every parsed message now carries its index in
the file, and a single lifetime pass (`build_lifetimes`) tracks each object
id from birth to death:

- **Birth** — the message that creates the object, i.e. any message with a
  `new id <iface>#N` argument: `wl_display.get_registry` (wayland.xml:55),
  `wl_registry.bind` (:137), `wl_data_device_manager.create_data_source`
  (:1044), `wl_data_device_manager.get_data_device` (:1051); and, for data
  offers, the `wl_data_device.data_offer` event (:936), whose id argument is
  the new offer (libwayland prints it as `new id wl_data_offer#N`, other
  loggers as a bare `wl_data_offer#N` — both accepted).
- **Death** — the object's own `destroy` request (`type="destructor"`:
  `wl_data_offer.destroy` wayland.xml:585, `wl_data_source.destroy` :738),
  or the `wl_display.delete_id` event (:101). The spec text of `delete_id`
  ("when the client receives this event, it will know that it can safely
  reuse the object ID") is what makes several lifetimes per id number
  possible; the parser therefore keeps a *list* of lifetimes per id and
  resolves a reference to the lifetime that contains the referencing
  message (`containing`).

Selection, per the review: the drag's source is the `wl_data_source#N` whose
lifetime contains the (last) `start_drag`; the Zen offer is the
`wl_data_offer#N` whose lifetime contains the `enter` used (the last enter
before the last drop, or the last enter if no drop). f4's
`expected-summary.json` was written from the rewritten parser's output and
then checked against the review's required values: offers
`["text/uri-list", "text/plain;charset=utf-8"]`, `set_actions` args `[3]`
(decoded copy|move), ending `dnd_drop_performed+dnd_finished`, with the
mid-drag `wl_registry#41` correctly ignored and the empty zen.log tolerated.

JSON fields that existed only for the counter are gone:
`drag_source_connection`, `start_drag.connection`, `enter[].connection`,
`drag_offer_connection`, and `parsing_notes.source_connections` /
`zen_connections`. **Consequence to flag:** removing those fields changes the
summary schema, so f1–f3's `expected-summary.json` had to be regenerated —
the r1 brief's Output list did not name them, but leaving them frozen would
have broken the "f1–f5 all pass" requirement. Only the three
`expected-summary.json` files were regenerated (from the reviewed parser's
output, then re-checked by eye against the old values); every other file in
f1–f3, and all of Claude's f4 files except `expected-summary.json`, are
untouched.

### Correction 3 — several attempts in one run

`start_drag` and `drop` are now counted and reported
(`source_side.start_drag_count`, `zen_side.drop_count`), and **the last
attempt is summarized on both sides**: the source side takes the last
`start_drag` and its source's lifetime; the Zen side searches for the enter
inside the last attempt's window (after the previous drop). New fixture
`fixtures/f5-retry/` holds one cancelled drag followed by one successful
drag in the same logs, and — because the source destroys the first attempt's
objects and `delete_id` frees the numbers — Zen's second data offer **reuses
id 60**, so the fixture only passes if lifetime scoping actually works.

### Correction 4 — the no-drag case is explicit

With no `start_drag` in source.log, summary.json gains a top-level
`"drag_seen": false` (and `"drag_seen": true` otherwise), `ending` becomes
`"no drag seen"`, and summary.md prints
`no drag seen (no start_drag in source.log)` instead of the old
`Drag source: ?` / `Ending: incomplete`. Covered by `TestNoDrag`, which
builds a two-line source.log without `start_drag` and runs the real script.

### Correction 5 — NOTES wording fixed

The multi-connection bullet under the wayland-backend section above was
rewritten: repeated ids (`wl_registry#2` twice) are the evidence for a second
connection, increasing ids are not, and queue-less lines are unnamed queues,
not connections. The sample-line captions were corrected to match. The
connection-counter description that bullet contained is obsolete and is
replaced by this section.

### Tests

`python3 -I test_summarize.py -v` (from `tools/dnd-capture/`), full output:

```
test_drag_source_is_not_the_clipboard (__main__.TestFixtureF1SuccessClipboard.test_drag_source_is_not_the_clipboard) ... ok
test_markdown (__main__.TestFixtureF1SuccessClipboard.test_markdown) ... ok
test_matches_expected_json (__main__.TestFixtureF1SuccessClipboard.test_matches_expected_json) ... ok
test_non_portal_dbus_filtered (__main__.TestFixtureF1SuccessClipboard.test_non_portal_dbus_filtered) ... ok
test_offers_and_ending (__main__.TestFixtureF1SuccessClipboard.test_offers_and_ending) ... ok
test_result (__main__.TestFixtureF1SuccessClipboard.test_result) ... ok
test_zen_side (__main__.TestFixtureF1SuccessClipboard.test_zen_side) ... ok
test_cancelled_ending (__main__.TestFixtureF2Cancelled.test_cancelled_ending) ... ok
test_matches_expected_json (__main__.TestFixtureF2Cancelled.test_matches_expected_json) ... ok
test_missing_dbus_log_is_not_fatal (__main__.TestFixtureF2Cancelled.test_missing_dbus_log_is_not_fatal) ... ok
test_result (__main__.TestFixtureF2Cancelled.test_result) ... ok
test_matches_expected_json (__main__.TestFixtureF3Portal.test_matches_expected_json) ... ok
test_portal_calls_in_order (__main__.TestFixtureF3Portal.test_portal_calls_in_order) ... ok
test_transfer_key_and_retrieved_paths (__main__.TestFixtureF3Portal.test_transfer_key_and_retrieved_paths) ... ok
test_zen_received_uri_list (__main__.TestFixtureF3Portal.test_zen_received_uri_list) ... ok
test_drag_seen (__main__.TestFixtureF4RegistryMidDrag.test_drag_seen) ... ok
test_matches_expected_json (__main__.TestFixtureF4RegistryMidDrag.test_matches_expected_json) ... ok
test_offers_survive_the_mid_drag_registry (__main__.TestFixtureF4RegistryMidDrag.test_offers_survive_the_mid_drag_registry) ... ok
test_zen_log_empty_is_tolerated (__main__.TestFixtureF4RegistryMidDrag.test_zen_log_empty_is_tolerated) ... ok
test_last_attempt_is_summarized (__main__.TestFixtureF5Retry.test_last_attempt_is_summarized) ... ok
test_markdown_shows_attempts (__main__.TestFixtureF5Retry.test_markdown_shows_attempts) ... ok
test_matches_expected_json (__main__.TestFixtureF5Retry.test_matches_expected_json) ... ok
test_zen_offer_is_the_reused_id_second_lifetime (__main__.TestFixtureF5Retry.test_zen_offer_is_the_reused_id_second_lifetime) ... ok
test_creation_via_new_id_and_death_via_delete_id (__main__.TestLifetimes.test_creation_via_new_id_and_death_via_delete_id) ... ok
test_data_offer_without_new_id_prefix_still_creates (__main__.TestLifetimes.test_data_offer_without_new_id_prefix_still_creates) ... ok
test_id_reuse_starts_new_lifetime (__main__.TestLifetimes.test_id_reuse_starts_new_lifetime) ... ok
test_at_separator_rs_backend_event (__main__.TestLineParser.test_at_separator_rs_backend_event) ... ok
test_at_separator_rs_backend_request (__main__.TestLineParser.test_at_separator_rs_backend_request) ... ok
test_brief_example_hash_no_queue (__main__.TestLineParser.test_brief_example_hash_no_queue) ... ok
test_discarded_marker (__main__.TestLineParser.test_discarded_marker) ... ok
test_libwayland_event_no_arrow (__main__.TestLineParser.test_libwayland_event_no_arrow) ... ok
test_queue_prefix (__main__.TestLineParser.test_queue_prefix) ... ok
test_queue_prefix_on_request (__main__.TestLineParser.test_queue_prefix_on_request) ... ok
test_no_drag_seen (__main__.TestNoDrag.test_no_drag_seen) ... ok

----------------------------------------------------------------------
Ran 34 tests in 0.319s

OK
```

### README

The troubleshooting line was updated: a run with no completed drag now says
"no drag seen (no start_drag in source.log)" rather than showing
`Ending: incomplete` for a run that never started one. No other README
changes — the summary's overall shape is unchanged.

## Review of P0a-r1 (Claude, Oct 30, 2026)

**Verdict: accepted with one correction (r2).** Lifetime scoping is the right design and is
implemented cleanly.

What I checked:
- Files: only the r1 Output paths changed. Regenerating f1–f3's expected JSON was justified by the
  schema change and recorded honestly. f4's input files are byte-identical to mine, the first review
  section is unchanged, and the shebangs are still there.
- I compared every fixture's expected JSON against its input logs by hand: drag source, offers,
  excluded sources, ending, the Zen offer, receive, finish, and the D-Bus calls. All f1–f5 values match
  the logs, so the snapshots aren't just echoing the parser.
- `python3 -I test_summarize.py`: 34 tests OK.
- Live no-drag run of `./run.sh dolphin downloads`: it printed "no drag seen (no start_drag in
  source.log)" with `drag_seen: false`, and nothing leaked. That run directory has been removed.

Correction for r2:

1. **The two sides can still describe different attempts.** The Zen side picks its window from drops
   ("after the previous drop, up to the last drop"). So if a successful drag is followed by a cancelled
   one, the source summarizes attempt 2 and Zen summarizes attempt 1. On `fixtures/f6-success-then-cancel/`
   (inputs written by Claude), the source side reports `attempt 2 of 2`, offers `[text/uri-list,
   application/x-second-attempt]`, `cancelled`; the Zen side reports `wl_data_offer#4278190080`, offers
   `[text/uri-list]`, `receive text/uri-list`, `finish: true`.
   **Fix:** the Zen side uses the **last `enter` in the log** (the last attempt that reached Zen), and
   reports separately whether a `drop` followed that enter. Count Zen-side attempts as the number of
   distinct offer lifetimes that got an `enter`. When that count differs from `start_drag_count`, add
   `"attempts_match": false` to summary.json and a line to summary.md: "source saw N drags, Zen saw
   M; check the raw logs". Expected for f6 on the Zen side: offer `wl_data_offer#4278190081`, offers
   `[text/uri-list, application/x-second-attempt]`, no receive, `finish: false`, dropped false,
   `attempts_match: true`.
2. **Robustness: a `new id` always starts a new lifetime.** Today, a `new id` for an ID whose current
   lifetime has no recorded death is ignored, so two objects' messages merge. The protocol only reuses
   an ID once it's free, so a `new id` should close any open lifetime for that ID and start a fresh one.
   Add a unit test for this.

Known limitation (no code change): if one process has two Wayland connections (cosmic-files seems to),
the same ID can be alive on both at once, and a libwayland log has nothing to tell them apart. The
P0b review checks each COSMIC raw log by hand for this.

Minor, optional: f3 is the "portal" fixture, but its source offers `application/x-moz-file` instead of
`application/vnd.portal.filetransfer`. The parser test is still valid; the MIME would just be more
realistic.

## P0a-r2 changes

Both corrections from "Review of P0a-r1" were applied; the f3 MIME note was
**not** (it is optional, and f3's `source.log` is not in the r2 Output list —
changing fixture inputs is out of scope for a parser round). `run.sh` was not
run, per the r2 rules.

### Correction 1 — Zen summarizes the last enter, and attempt counts are compared

`summarize_zen` no longer windows events by drops (that made a
success-then-cancel run describe attempt 1 on the Zen side and attempt 2 on
the source side). Now:

- The drag's offer is the `wl_data_offer` whose lifetime contains the
  **last `enter` in the log** (wayland.xml:949) — the last attempt that
  reached Zen, whichever side ended it.
- `zen_side.dropped` (new boolean) records whether a `drop`
  (wayland.xml:984) followed that enter.
- `zen_side.attempt_count` (new) is the number of **distinct offer
  lifetimes that got an `enter`** — the Zen-side attempt count per the
  review.
- Top-level `attempts_match` (new) is `start_drag_count == attempt_count`;
  when it is false, summary.md carries the line
  `source saw N drags, Zen saw M; check the raw logs`.

On `fixtures/f6-success-then-cancel/` (inputs by Claude) this produces
exactly the review's expected values: source side unchanged
(`wl_data_source#40`, `attempt 2 of 2`, offers
`[text/uri-list, application/x-second-attempt]`, `cancelled`); Zen side
`wl_data_offer#4278190081`, offers
`[text/uri-list, application/x-second-attempt]`, no receive,
`finish: false`, `dropped: false`, `attempts_match: true`. f4 (empty
zen.log) now honestly reports `attempts_match: false` with the warning line,
since Zen saw 0 of the source's 1 drag.

### Correction 2 — a `new id` always starts a new lifetime

`build_lifetimes` previously ignored a `new id` for an id whose current
lifetime had no recorded death, which could merge two distinct objects'
messages. Since the protocol never reuses a live id (the id is only
reusable after `delete_id`, wayland.xml:101), a `new id` arriving for an id
with an open lifetime now **closes that lifetime at that message and starts
a fresh one** — same for a repeated `data_offer` event on a live offer.
Covered by `TestLifetimes.test_new_id_with_open_lifetime_starts_new_lifetime`.
One implementation bug surfaced and was fixed while doing this: a
`data_offer` event printed *with* the `new id` prefix matched both the
generic creation path and the data_offer path, creating the lifetime twice;
the data_offer path now skips ids the generic path already created in the
same message (`test_id_reuse_starts_new_lifetime` guards the count).

### Schema changes and regenerated expected JSON

Three fields were added, so the schema changed and **all six** expected
summaries were regenerated from the parser output and re-checked against the
review's stated values:

- `zen_side.attempt_count` (new)
- `zen_side.dropped` (new)
- `attempts_match`, top level (new)

No fields were removed this round. Files changed:
`fixtures/f{1,2,3,4,5}-*/expected-summary.json` (additions only) and
`fixtures/f6-success-then-cancel/expected-summary.json` (new, mine to write;
the other f6 files are Claude's and untouched).

### README

One bullet added to "Notes and known sharp edges": a summary containing
`source saw N drags, Zen saw M; check the raw logs` means the two sides saw
different attempt counts and the raw logs need a look. No other README
changes.

### Tests

`python3 -I test_summarize.py -v` (from `tools/dnd-capture/`), full output:

```
test_mismatch_detected_and_warned (__main__.TestAttemptsMismatch.test_mismatch_detected_and_warned) ... ok
test_drag_source_is_not_the_clipboard (__main__.TestFixtureF1SuccessClipboard.test_drag_source_is_not_the_clipboard) ... ok
test_markdown (__main__.TestFixtureF1SuccessClipboard.test_markdown) ... ok
test_matches_expected_json (__main__.TestFixtureF1SuccessClipboard.test_matches_expected_json) ... ok
test_non_portal_dbus_filtered (__main__.TestFixtureF1SuccessClipboard.test_non_portal_dbus_filtered) ... ok
test_offers_and_ending (__main__.TestFixtureF1SuccessClipboard.test_offers_and_ending) ... ok
test_result (__main__.TestFixtureF1SuccessClipboard.test_result) ... ok
test_zen_side (__main__.TestFixtureF1SuccessClipboard.test_zen_side) ... ok
test_cancelled_ending (__main__.TestFixtureF2Cancelled.test_cancelled_ending) ... ok
test_matches_expected_json (__main__.TestFixtureF2Cancelled.test_matches_expected_json) ... ok
test_missing_dbus_log_is_not_fatal (__main__.TestFixtureF2Cancelled.test_missing_dbus_log_is_not_fatal) ... ok
test_result (__main__.TestFixtureF2Cancelled.test_result) ... ok
test_matches_expected_json (__main__.TestFixtureF3Portal.test_matches_expected_json) ... ok
test_portal_calls_in_order (__main__.TestFixtureF3Portal.test_portal_calls_in_order) ... ok
test_transfer_key_and_retrieved_paths (__main__.TestFixtureF3Portal.test_transfer_key_and_retrieved_paths) ... ok
test_zen_received_uri_list (__main__.TestFixtureF3Portal.test_zen_received_uri_list) ... ok
test_drag_seen (__main__.TestFixtureF4RegistryMidDrag.test_drag_seen) ... ok
test_matches_expected_json (__main__.TestFixtureF4RegistryMidDrag.test_matches_expected_json) ... ok
test_offers_survive_the_mid_drag_registry (__main__.TestFixtureF4RegistryMidDrag.test_offers_survive_the_mid_drag_registry) ... ok
test_zen_log_empty_is_tolerated (__main__.TestFixtureF4RegistryMidDrag.test_zen_log_empty_is_tolerated) ... ok
test_last_attempt_is_summarized (__main__.TestFixtureF5Retry.test_last_attempt_is_summarized) ... ok
test_markdown_shows_attempts (__main__.TestFixtureF5Retry.test_markdown_shows_attempts) ... ok
test_matches_expected_json (__main__.TestFixtureF5Retry.test_matches_expected_json) ... ok
test_zen_offer_is_the_reused_id_second_lifetime (__main__.TestFixtureF5Retry.test_zen_offer_is_the_reused_id_second_lifetime) ... ok
test_attempts_match (__main__.TestFixtureF6SuccessThenCancel.test_attempts_match) ... ok
test_matches_expected_json (__main__.TestFixtureF6SuccessThenCancel.test_matches_expected_json) ... ok
test_source_side_unchanged (__main__.TestFixtureF6SuccessThenCancel.test_source_side_unchanged) ... ok
test_zen_side_is_the_last_enter (__main__.TestFixtureF6SuccessThenCancel.test_zen_side_is_the_last_enter) ... ok
test_creation_via_new_id_and_death_via_delete_id (__main__.TestLifetimes.test_creation_via_new_id_and_death_via_delete_id) ... ok
test_data_offer_without_new_id_prefix_still_creates (__main__.TestLifetimes.test_data_offer_without_new_id_prefix_still_creates) ... ok
test_id_reuse_starts_new_lifetime (__main__.TestLifetimes.test_id_reuse_starts_new_lifetime) ... ok
test_new_id_with_open_lifetime_starts_new_lifetime (__main__.TestLifetimes.test_new_id_with_open_lifetime_starts_new_lifetime) ... ok
test_at_separator_rs_backend_event (__main__.TestLineParser.test_at_separator_rs_backend_event) ... ok
test_at_separator_rs_backend_request (__main__.TestLineParser.test_at_separator_rs_backend_request) ... ok
test_brief_example_hash_no_queue (__main__.TestLineParser.test_brief_example_hash_no_queue) ... ok
test_discarded_marker (__main__.TestLineParser.test_discarded_marker) ... ok
test_libwayland_event_no_arrow (__main__.TestLineParser.test_libwayland_event_no_arrow) ... ok
test_queue_prefix (__main__.TestLineParser.test_queue_prefix) ... ok
test_queue_prefix_on_request (__main__.TestLineParser.test_queue_prefix_on_request) ... ok
test_no_drag_seen (__main__.TestNoDrag.test_no_drag_seen) ... ok

----------------------------------------------------------------------
Ran 40 tests in 0.385s

OK
```

## Review of P0a-r2 (Claude, Oct 30, 2026)

**Verdict: the parser is accepted. run.sh has one blocking cleanup bug (r3).**

Parser (accepted):
- Only the r2 Output paths changed. f6's input files are byte-identical to Claude's, and the earlier
  review sections are unchanged (the md5 of the first one matches).
- f1–f5 expected JSON: every value that existed in r1 is unchanged. Only `dropped`, `attempt_count` and
  `attempts_match` were added. f4's `attempts_match: false` is correct, because its Zen log is empty.
- f6 matches every required value: Zen side `wl_data_offer#4278190081`, offers `[text/uri-list,
  application/x-second-attempt]`, no receive, `finish: false`, `dropped: false`, `attempts_match:
  true`; the source side is still `attempt 2 of 2`, `cancelled`.
- `create()` closes an open lifetime when a `new id` reuses its number. The tests for that and for the
  mismatch warning exist and pass. `python3 -I test_summarize.py`: 40 tests OK.

Correction for r3:

1. **run.sh leaked the throwaway Zen.** In this review's live no-drag run, the `gimbal-dnd` sandbox
   stayed alive after run.sh exited (flatpak instance `135129854`, browser PID 698244, with the profile
   `bt9lefdm.gimbal-dnd` open). That browser process's command line was just `/app/zen/zen`, without
   `-P gimbal-dnd`. Zen re-launched itself inside the sandbox, so `find_zen_inner` found nothing, and
   killing the flatpak wrapper PID didn't stop the sandbox. The two earlier live runs didn't hit this.
   The exact trigger isn't known (possibly the Zen Flatpak update on Oct 30 at 14:34, since Kyle's
   running Zen is still the older build). Claude stopped it with `flatpak kill 135129854`; Kyle's
   instance `2943722691` wasn't touched.
   **Fix:** before launch, snapshot `flatpak ps --columns=instance,application`. After launch, take
   the new **instance ID** for `app.zen_browser.zen` (not the PID). In cleanup, run
   `flatpak kill <instance-id>`. **Pass only a numeric instance ID, never the app ID**, because
   `flatpak kill app.zen_browser.zen` would also stop Kyle's Zen. Guard this with a check that the
   value is all digits and appears in the before/after difference. After cleanup, check
   `flatpak ps` again. If the instance is still listed, print a loud warning with the instance ID and
   exit non-zero. Keep the PID kills for the source app and dbus-monitor as they are.
2. **Minor:** in f6, `other_sources_excluded` lists `wl_data_source#40`, which is the same label as
   the drag source, because the ID was reused. Label lifetimes so the two can be told apart, for
   example `wl_data_source#40 (attempt 1)` or `(lines 3–10)`.

## P0a-r3 changes

Both corrections from "Review of P0a-r2" were applied. Per the r3 rules,
`run.sh` and `flatpak kill` were **not** executed (Kyle's Zen is running;
Claude does the live check). Read-only checks actually run this round:
`flatpak ps --columns=instance,application` (works; Kyle's Zen is instance
`2943722691`) and `flatpak kill --help` (usage `flatpak kill INSTANCE`).
**[observed]**

### Correction 1 — the throwaway Zen is stopped by flatpak instance ID

The r2 leak: Zen re-launched itself inside the sandbox with the bare cmdline
`/app/zen/zen` (no `-P gimbal-dnd`), so `find_zen_inner` matched nothing and
TERM to the flatpak wrapper did not stop the sandbox. run.sh now:

- snapshots `flatpak ps --columns=instance,application` **before** the
  launch (`.zen-inst-before`), then after the launch polls the same list and
  takes the new **instance ID** for `app.zen_browser.zen` from the
  before/after `comm -13` difference — the instance ID, not the PID;
- guards it (`zen_instance_killable` + a membership re-check): only an
  all-numeric id that appears in the after-snapshot and **not** in the
  before-snapshot is accepted; anything else aborts the run with
  `refusing instance id '…'`. **`flatpak kill` is never called with
  `app.zen_browser.zen`** — that would stop Kyle's Zen too;
- in cleanup, runs `flatpak kill <instance-id>` (guarded again) alongside
  the unchanged PID kills for the source app, the flatpak wrapper and the
  D-Bus monitor;
- after cleanup, re-checks `flatpak ps --columns=instance`: if the instance
  is still listed it prints
  `ERROR: throwaway Zen instance <id> is still running after cleanup.` plus
  the exact manual `flatpak kill <id>` command and exits 1;
- keeps `find_zen_inner` **for logging only** (it prints the browser PID when
  the cmdline still carries the profile flag) — cleanup no longer depends on
  it, since the leaked browser had no matching cmdline.

`bash -n run.sh` passes; line 1 is still `#!/usr/bin/env bash`. The old
`ZEN_APP_PID`/`.zen-pids-*` discovery is gone (instance ID replaces it).

### Correction 2 — reused ids get labeled lifetimes

`summarize.py` now records each parsed message's **file line number**, and
`all_data_sources` / `other_sources_excluded` label every data-source
lifetime with its line span, e.g. f6 now reports
`all_data_sources: ["wl_data_source#40 (lines 3–9)", "wl_data_source#40
(from line 11)"]` and `other_sources_excluded:
["wl_data_source#40 (lines 3–9)"]` — the excluded entry is no longer the
same bare label as the drag source. `drag_source_object` stays the bare
object id (the P0a brief requires stating the object IDs). Chose line spans
over the review's `(attempt N)` suggestion **[inferred]**: line numbers are
objective, point straight into the raw log Kyle can grep, and never mislabel
a clipboard source (which belongs to no attempt). Label format:
`(lines N–M)` when the lifetime ended (destroy/delete_id line), `(from line
N)` while still alive at end of log.

**Regenerated expected files (label format affects them):** all six —
`fixtures/f{1,2,3,4,5,6}-*/expected-summary.json`. Only
`all_data_sources` / `other_sources_excluded` strings changed; no other
values moved (verified against the r2 snapshots). Claude's f6 input files
are untouched.

### README

The abort/cleanup bullet now describes the instance-ID cleanup and the
loud-failure path. No other README changes.

### Tests

`python3 -I test_summarize.py -v` (from `tools/dnd-capture/`), full output:

```
test_mismatch_detected_and_warned (__main__.TestAttemptsMismatch.test_mismatch_detected_and_warned) ... ok
test_drag_source_is_not_the_clipboard (__main__.TestFixtureF1SuccessClipboard.test_drag_source_is_not_the_clipboard) ... ok
test_markdown (__main__.TestFixtureF1SuccessClipboard.test_markdown) ... ok
test_matches_expected_json (__main__.TestFixtureF1SuccessClipboard.test_matches_expected_json) ... ok
test_non_portal_dbus_filtered (__main__.TestFixtureF1SuccessClipboard.test_non_portal_dbus_filtered) ... ok
test_offers_and_ending (__main__.TestFixtureF1SuccessClipboard.test_offers_and_ending) ... ok
test_result (__main__.TestFixtureF1SuccessClipboard.test_result) ... ok
test_zen_side (__main__.TestFixtureF1SuccessClipboard.test_zen_side) ... ok
test_cancelled_ending (__main__.TestFixtureF2Cancelled.test_cancelled_ending) ... ok
test_matches_expected_json (__main__.TestFixtureF2Cancelled.test_matches_expected_json) ... ok
test_missing_dbus_log_is_not_fatal (__main__.TestFixtureF2Cancelled.test_missing_dbus_log_is_not_fatal) ... ok
test_result (__main__.TestFixtureF2Cancelled.test_result) ... ok
test_matches_expected_json (__main__.TestFixtureF3Portal.test_matches_expected_json) ... ok
test_portal_calls_in_order (__main__.TestFixtureF3Portal.test_portal_calls_in_order) ... ok
test_transfer_key_and_retrieved_paths (__main__.TestFixtureF3Portal.test_transfer_key_and_retrieved_paths) ... ok
test_zen_received_uri_list (__main__.TestFixtureF3Portal.test_zen_received_uri_list) ... ok
test_drag_seen (__main__.TestFixtureF4RegistryMidDrag.test_drag_seen) ... ok
test_matches_expected_json (__main__.TestFixtureF4RegistryMidDrag.test_matches_expected_json) ... ok
test_offers_survive_the_mid_drag_registry (__main__.TestFixtureF4RegistryMidDrag.test_offers_survive_the_mid_drag_registry) ... ok
test_zen_log_empty_is_tolerated (__main__.TestFixtureF4RegistryMidDrag.test_zen_log_empty_is_tolerated) ... ok
test_last_attempt_is_summarized (__main__.TestFixtureF5Retry.test_last_attempt_is_summarized) ... ok
test_markdown_shows_attempts (__main__.TestFixtureF5Retry.test_markdown_shows_attempts) ... ok
test_matches_expected_json (__main__.TestFixtureF5Retry.test_matches_expected_json) ... ok
test_zen_offer_is_the_reused_id_second_lifetime (__main__.TestFixtureF5Retry.test_zen_offer_is_the_reused_id_second_lifetime) ... ok
test_attempts_match (__main__.TestFixtureF6SuccessThenCancel.test_attempts_match) ... ok
test_matches_expected_json (__main__.TestFixtureF6SuccessThenCancel.test_matches_expected_json) ... ok
test_reused_id_lifetimes_are_labeled (__main__.TestFixtureF6SuccessThenCancel.test_reused_id_lifetimes_are_labeled) ... ok
test_source_side_unchanged (__main__.TestFixtureF6SuccessThenCancel.test_source_side_unchanged) ... ok
test_zen_side_is_the_last_enter (__main__.TestFixtureF6SuccessThenCancel.test_zen_side_is_the_last_enter) ... ok
test_creation_via_new_id_and_death_via_delete_id (__main__.TestLifetimes.test_creation_via_new_id_and_death_via_delete_id) ... ok
test_data_offer_without_new_id_prefix_still_creates (__main__.TestLifetimes.test_data_offer_without_new_id_prefix_still_creates) ... ok
test_id_reuse_starts_new_lifetime (__main__.TestLifetimes.test_id_reuse_starts_new_lifetime) ... ok
test_lifetime_labels_carry_line_spans (__main__.TestLifetimes.test_lifetime_labels_carry_line_spans) ... ok
test_new_id_with_open_lifetime_starts_new_lifetime (__main__.TestLifetimes.test_new_id_with_open_lifetime_starts_new_lifetime) ... ok
test_at_separator_rs_backend_event (__main__.TestLineParser.test_at_separator_rs_backend_event) ... ok
test_at_separator_rs_backend_request (__main__.TestLineParser.test_at_separator_rs_backend_request) ... ok
test_brief_example_hash_no_queue (__main__.TestLineParser.test_brief_example_hash_no_queue) ... ok
test_discarded_marker (__main__.TestLineParser.test_discarded_marker) ... ok
test_libwayland_event_no_arrow (__main__.TestLineParser.test_libwayland_event_no_arrow) ... ok
test_queue_prefix (__main__.TestLineParser.test_queue_prefix) ... ok
test_queue_prefix_on_request (__main__.TestLineParser.test_queue_prefix_on_request) ... ok
test_no_drag_seen (__main__.TestNoDrag.test_no_drag_seen) ... ok

----------------------------------------------------------------------
Ran 42 tests in 0.407s

OK
```

## Review of P0a-r3 (Claude, Oct 30, 2026)

**Verdict: accepted. P0a is done.** The harness is ready for P0b and for P0a2 to build on.

What I checked:
- Only the r3 Output paths changed. The shebang is still line 1, `bash -n` passes, and the earlier
  review sections are unchanged.
- `flatpak kill` appears once in the cleanup path, and it's only reached through
  `zen_instance_killable` with an ID taken from the before/after `flatpak ps` difference. The app ID is
  never passed. After cleanup, `flatpak ps` is checked and a surviving instance makes the script exit 1.
- f6's excluded source is now labeled `wl_data_source#40 (lines 3–9)`, so it can be told apart from
  the drag source. `python3 -I test_summarize.py`: OK.
- **Live:** one no-drag `run.sh <app> downloads` for each of dolphin, thunar, cosmic and nautilus,
  with Kyle's Zen open. All four exited 0 and left only Kyle's instance (`2943722691`). There were no
  leftover `gimbal-dnd`, `cosmic-files --no-daemon` or dbus-monitor processes. The source logs had
  461–2099 Wayland messages and the Zen logs 676–1473. The run directories have been removed.
- In all four runs the browser PID was reported as "unknown". The sandboxed Zen now always re-launches
  itself with a bare command line, so the r2 cleanup would have leaked on every run. The
  instance-ID approach was necessary, not just a hardening.

Minor, folded into P0a2 (which edits run.sh anyway), not blocking:
1. If some other `flatpak run app.zen_browser.zen` starts during the 20 s launch window (for example
   Kyle clicks a link in another app), `comm` could pick that short-lived instance, and the real
   throwaway would then go unchecked. Pick the new instance whose `child-pid` command line
   (`/proc/<child-pid>/cmdline`, the bwrap line) contains `-P gimbal-dnd`.
2. If the guard refuses an ID, clear `ZEN_INSTANCE` before `die`, so the EXIT trap can never kill a
   refused ID. Today the only refusable IDs are non-numeric, which cleanup already skips, so this
   can't happen now. It's just there so a later edit can't open it up.
