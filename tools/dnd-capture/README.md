# P0b: running the drag-and-drop captures — Kyle's instructions

This harness records what a working drag-out puts on the Wayland wire and on
the D-Bus document portal. Each run captures three logs, asks you two
questions, and reduces everything to a short `summary.md` / `summary.json`
pair. **You do the drags; the scripts do everything else.**

All commands below assume you are in this directory:

```bash
cd "/home/kyle/Documents/03.) Code-Projects/gimbal-files/tools/dnd-capture"
```

(Quote the path — the project folder name contains a space and parentheses.)

## One-time setup (already done, re-run only if needed)

- Test files exist at `~/gimbal-dnd-test/` and `~/Downloads/gimbal-dnd-test/`
  (same pair in both: `sample.png`, `sample.txt`). Re-create with:

  ```bash
  ./make-test-files.sh
  ```

  It never overwrites existing files.

- A throwaway Zen profile `gimbal-dnd` exists. `run.sh` recreates it
  automatically if it is missing. Your normal Zen profile is never touched and
  never needs to quit.

## Per run

Pick the source app and the location, then run:

```bash
./run.sh dolphin outside      # drag from ~/gimbal-dnd-test
./run.sh thunar outside
./run.sh nautilus outside
./run.sh cosmic outside

./run.sh dolphin downloads    # drag from ~/Downloads/gimbal-dnd-test (control)
# … and thunar/nautilus/cosmic × downloads likewise
```

Eight runs total: every app from `outside`, plus each app from `downloads`
(the control — Zen reads Downloads directly, no portal involved).

For each run, the script:

1. Starts the D-Bus monitor and launches Zen (throwaway profile, empty
   `about:blank` tab) and the source app, already opened at the test folder.
   Two new windows appear — tile them side by side (niri: `Mod+H` / `Mod+V`,
   or drag a window to a screen edge).
2. Waits for you. In the source window, **drag `sample.png` into the empty
   Zen tab and drop it**. (A later pass can repeat this with `sample.txt`.)
3. Back in the terminal, answer:
   - `Did Zen open the file? [y/n/other]` — what you saw, no guessing.
   - an optional free-text note.
4. Stops everything it started, writes `meta.txt`, and prints the two summary
   files.

## Before you start a run

- **Thunar and Nautilus must not be running.** They hand drags off to an
  already-running instance over D-Bus, the fresh process exits instantly and
  its Wayland log would be empty. `run.sh` refuses to start if either is up;
  quit with `thunar -q` / `nautilus -q`.
- **Dolphin and COSMIC Files don't care** — both verified to start a fresh
  process even when another instance is open.
- Your own Zen stays open the whole time; `--new-instance` runs the capture
  side by side with it. Only the throwaway instance is stopped afterwards.

## What lands where

```
docs/research/phase0/captures/<timestamp>-<source>-<location>/
  ├── source.log.gz   # WAYLAND_DEBUG=1 stderr of the source app
  ├── zen.log.gz      # WAYLAND_DEBUG=1 (+ MOZ_LOG) stderr of Zen
  ├── dbus.log.gz     # dbus-monitor, filtered to the document portal
  ├── meta.txt        # app/Zen/niri versions, date, module names
  ├── result.txt      # your y/n answer and note
  ├── summary.md      # short human-readable reduction
  └── summary.json    # same fields, for P0h's table
```

Raw logs are gzipped and `.gitignore`d (captures/.gitignore ignores
`*.log.gz`); only `meta.txt`, `result.txt` and the summaries get committed.

To re-reduce a run after tweaking the parser (logs must be gunzipped first):

```bash
gunzip <run-dir>/*.log.gz
python3 -I summarize.py <run-dir>
gzip -f <run-dir>/*.log
```

## Automated run (`--auto`)

`autodrag/` (P0a2) performs the drag a person would perform, so a capture
run needs no human at the wheel.  The launch, capture and cleanup are
**unchanged** — only the "press Enter" step and the y/n question are
replaced:

```bash
./run.sh --auto dolphin outside        # one automated capture run
./run-matrix.sh                        # all 8 rows of the P0b matrix
```

What `--auto` does per run: waits for both windows, puts them on niri's
empty end workspace (source left half, Zen right half, 40 px margin,
floating at exact positions), locates `sample.png` through AT-SPI, clicks
it and checks the item reports `STATE_SELECTED` (aborting when it does
not), then presses, drags into Zen's content area in 30 steps over ~600 ms,
holds 400 ms and releases.  It classifies Zen's title (y / n / other) and
writes `result.txt` itself: P0a's format plus `auto: true`, the raw title,
the selection check and the coordinates used.  A run that hits a safety
rule (focused window not ours, a placed window moved or resized, more than
one enabled output, longer than 20 s) releases the button, writes
`aborted: <reason>` into `result.txt`, and makes run.sh exit 3.

`run-matrix.sh` runs the 8 rows in P0b's order (all apps × outside, then ×
downloads), waits 2 s between runs, and stops the whole matrix at the first
`aborted` row.

For stepping through a run one phase at a time (the live check):

```bash
GIMBAL_AUTODRAG_STOP_AFTER=placement ./run.sh --auto dolphin outside
GIMBAL_AUTODRAG_STOP_AFTER=select    ./run.sh --auto dolphin outside
# add GIMBAL_AUTODRAG_HOLD_S=20 to keep the layout on screen for 20 s
# (device destroyed, cleanup waits) before the workspace is restored
```

`--stop-after placement` stops after the windows are placed and verified;
`--stop-after select` stops after the selection click.  Both clean up
(after each: `python3 -I autodrag/autodrag.py run … --dry-run` prints the
whole plan without touching anything).

Safety properties: the runner only ever issues niri actions with `--id`
for windows it launched this run (source by PID, the throwaway Zen by its
flatpak instance / a window id new after launch); the virtual uinput device
declares only ABS_X/ABS_Y and BTN_LEFT (it cannot type); the button is
released in a `finally` and on SIGINT/SIGTERM/any exception.  Details,
calibration knobs and the live-check TODOs: `NOTES-autodrag.md`.

## Notes and known sharp edges

- The MOZ_LOG module for GTK drag logging defaults to `WidgetDrag`
  (`GIMBAL_MOZ_LOG_MODULE=<name> ./run.sh …` overrides it). An invalid module
  name fails **silently** — verified: no warning is printed — so the
  `zen.log` MOZ_LOG section can be empty without any error. P0g confirms the
  exact module name from Firefox source.
- COSMIC Files is started with `--no-daemon` on purpose: it otherwise
  daemonizes when launched from a terminal (parent exits, real process
  escapes the PID cleanup).
- If the summary says `Ending: incomplete`, `no drag seen (no start_drag in
  source.log)` or `Zen opened the file: ?`, the drag did not complete (or
  never started) — rerun that combination.
- If the summary says `source saw N drags, Zen saw M; check the raw logs`,
  the two sides disagree on how many attempts happened (e.g. one side logged
  an attempt the other never saw) — `gunzip` the raw logs and look before
  trusting that run's summary.
- To abort a run early: `Ctrl+C` — the script stops only what it started:
  the source app and D-Bus monitor by PID, and the throwaway Zen by its
  flatpak instance ID (`flatpak kill <instance-id>`; never by app ID, which
  would also stop your own Zen). If the throwaway instance somehow survives
  cleanup, run.sh says so loudly, prints the exact `flatpak kill` command,
  and exits non-zero.
