# Phase 1 spike results (P1h)

Acceptance run of the Phase 1 toolkit spike, Nov 1, 2026. Claude ran the automated drag rows;
Kyle ran the hands-on checklist. The verdict at the end is for Kyle to approve.

## Versions

| Part | Version |
|---|---|
| GTK | 4.22.4 |
| libadwaita | 1.9.1 |
| gtk4-rs / libadwaita-rs | 0.11.5 / 0.9.2 |
| gdk4-wayland, wayland-client, wayland-protocols (look programs only, P1k) | 0.11.5, 0.31.15, 0.32.13 |
| Rust | 1.95.0 (rustup toolchain used for fmt/clippy and builds) |
| niri | 26.04 |
| Zen (Flatpak, receiver) | 1.24b |
| Phase 0 sources | Dolphin 25.12.3, Nautilus 50.2.2, Thunar 4.20.7, COSMIC Files (git 09bdd825) |
| Spike commit | `f4ed884` (last change under `spike/src`) |
| `gimbal-spike` sha256 | `d690f6bc0a8883afa980988bc9c2ca3993be39b0ebe37da3e877e3309915d0cb` |
| `look-header` sha256 | `5942a15811ca0a2a7bf1a98027b6b781bd81ae9bb6418d7557ea61ea5487bb9b` |
| `look-body` sha256 | `7c47d233bc9796075dc605a8014ff6ad7f282861f759c0ac772a14e576590ab2` |

## Automated rows (Claude)

Captures are in `docs/research/phase0/captures/20261101-1144*` and `20261101-1145*`/`-1146*`/`-1147*`,
committed in `a594a1e`.

**Calibration.** A run stopped after the select step (`20261101-114427-gimbal-outside`) still finds
and selects `sample.png` through AT-SPI, so the spike needs no pixel offsets.

**Spike → Zen (`run-matrix-gimbal.sh`).** Zen opened the dropped item in every row, and the source
was intact afterwards in every row.

| Item | From `~/gimbal-dnd-test-gimbal` (outside the sandbox) | From `~/Downloads/gimbal-dnd-test-gimbal` |
|---|---|---|
| `sample.png` | y, intact | y, intact |
| `sample-dir` | y, intact | y, intact |
| `sample-link.png` | y, intact | y, intact |
| `sample.png` + `sample.txt` | y, intact | y, intact |

**Phase 0 rerun (`run-matrix.sh`).** Same results as Oct 31 (`P0b-results.md`), and every source intact.

| Source | Outside | Downloads |
|---|---|---|
| Dolphin | y | y |
| Thunar | other ("Problem loading page", as in Phase 0) | y |
| Nautilus | y | y |
| COSMIC Files | other ("Problem loading page", as in Phase 0) | y |

## Hands-on rows (Kyle)

| # | Test | Result | Notes |
|---|---|---|---|
| 0 | Basics (Ctrl+T, Ctrl+W, Ctrl+N, open folder, Up, double-click a file) | pass | |
| 1 | Drag `sample.txt` into Dolphin and Thunar | pass | |
| 2 | Drag from Dolphin into the spike | pass | |
| 3 | Drag between two spikes | pass* | Between two windows on the **same** folder nothing happens. Expected: the spike skips a drop onto the folder the file is already in. Different folders work. |
| 4 | Open in New Window | pass | |
| 5 | 100k folder | pass* | Narrowing stops at 2 columns. Expected: the spike's header sets the window's minimum width, which still leaves room for 2 cells (P1e's notes call it the 2-column minimum); the one-column floor was proven in `look-body`. Kyle also found no way to delete: neither the Delete key nor a right-click menu. Deleting was not in the spike; it goes on the Phase 2 feature list. |
| 6 | Noctalia theme change, spike | pass | Changed live, no restart. |
| 7 | `look-header` against the sketch | **fail** | See "Look findings" below. In Noctalia's colours; pills reorder, select and deselect; Ctrl+= / Ctrl+- rescale. |
| 8 | `look-body` against the sketch | **fail** | See "Look findings" below. |
| 9 | Live scale and theme in both look programs | **fail** | Colours didn't follow a Noctalia change; Ctrl+= / Ctrl+- do nothing (in `look-body`). |
| 10 | Transparency and blur | pass | |

### Why row 9 failed (checked in the code)

- **Colours:** the live theme reload (P1f's `theme.rs`, file monitors on `noctalia.css` and
  `noctalia-extensions.css`) is wired only into the spike, which is why row 6 passes. The two look
  programs pick up Noctalia's colours when they start (row 7: "in the Noctalia colours") and never
  reload them. Wiring the same reload in is a known, small change.
- **Keys:** only `look-header` has the Ctrl+= / Ctrl+- handler; `look-body` has only its dev-bar
  slider.

## Look findings (rows 7 and 8)

Sorted into plain fixes, which need no decision, and design changes, which need Kyle's answer (see
"Open design questions"). None of them is something GTK or libadwaita prevents.

**Header, fixes:**
- Closing the search field: during the collapse, part of its outline slides behind the back,
  forward and up buttons before it disappears. The animation needs to clip or fade, not slide under.
- The accordion's tab list ("N tabs") has no way to close a tab or group: add a ✕ next to each
  entry.

**Header, design changes:** path bar recentring (Q1), outline weight (Q2), float/close button size
and search icon size (Q3).

**Body, fixes:**
- The text preview should wrap lines when the preview panel shrinks.
- The close buttons' rings are ovals; they should be circles with the ✕ centred.
- A collapsed sidebar's expand button should stay exactly where the collapse button was.
- Save, Save As and Discard use the wrong icons (see Q8 for which ones).
- `look-body` needs Ctrl+= / Ctrl+- and the live theme reload (row 9).

**Body, design changes:** padding between split tiles and between a tile's pill and its contents
(Q4), the three-tile layout (Q5), pill tops level with the side panels (Q6), the sidebar's type and
icon hierarchy and section pill size (Q7), icons from the mockup (Q8), focus ring and selection
contrast (Q9), and reordering pinned folders (Q10).

## Findings that matter for Phase 2

- **Drag and drop (P1c, P1g):** a GTK4 `FileList` drag gives Zen the portal MIME types, so drag-out
  works from any folder. That was the key deal-breaker, and it passes. A drop onto the folder the
  file is already in is skipped.
- **Large folders (P1e):** 100,000 files in a `GridView`:
  - Opening takes 438–444 ms (was 719–757 ms before the fix).
  - Navigating into the folder by double-click takes 389–431 ms (was 2.0–2.1 s).
  - A 3.3 s scrollbar drag gives 201 frames: median gap 15 ms, longest 34 ms.
  - Peak RSS with one tab is 233 MB.

  The fix was capping `max-columns` from the viewport width, because GTK keeps 32 × max-columns
  cells and gives each one an AT-SPI context once the grid has focus. Phase 2 rules:
  - Every list or grid caps max-columns from its width; accessibility is never turned off.
  - The cap is applied **outside** allocation, from an idle. A `queue_resize` from inside
    `size_allocate` is ignored (`gtkwidget.c:3709–3712`), so a cap set from the `page-size` notify
    is lost after a rebuild (P1j-r5).
- **Theme (P1f):** Noctalia's two CSS files are reloaded live through file monitors, always
  reloading the main file. `@accent_color` is a real libadwaita colour that Noctalia overrides,
  so the look spec's outline role works. Nothing Noctalia writes was found to be ignored.
- **Libadwaita styles the look prototypes had to undo (P1i, P1j, P1k).** The list is short:
  - `gridview > child { padding: 3px }`: set to 0. It added 6 px to every column.
  - The default opaque backgrounds of `gridview`, `listview`, `textview`, `scrolledwindow`,
    `viewport` and `windowhandle`, cleared under our own classes so the translucent window shows.
  - `all: unset` on our pills and icon buttons, so libadwaita's button styling doesn't fight the
    design.

  Everything else is our own `gl-` classes on top of libadwaita's stylesheet.
- **GTK details worth keeping:**
  - Never unparent a widget during a drag (it cancels the gesture); raise it with `insert_after`.
  - A child drag inside a `GtkWindowHandle` must claim its sequence once past the drag threshold,
    or the window moves instead.
  - Never feed a measured width back into a minimum size.
  - A `ScrolledWindow` with policy `Never` passes its child's minimum width up to the window.
- **Transparency (P1k):** GTK 4.22 has no blur API. The app asks for blur itself through
  `ext-background-effect-v1`, which niri 26.04 supports. niri also needs
  `draw-border-with-background false` for Gimbal's app IDs, or it paints the focus ring behind
  the window. GimbalOS can ship that rule in `/etc/niri/config.kdl`; Phase 2 could instead tell
  niri through `xdg-decoration`.
- **Testing:**
  - Test look programs as floating windows; niri's tiling hides minimum-size bugs.
  - The virtual mouse now also does right-click, Ctrl, Shift and Escape.
  - A virtual device with keys delays button events by about 60 ms, so drags use the pointer-only
    device.

## Verdict (for Kyle to approve)

**GTK4 + libadwaita stays the toolkit.**

- **Drag-out to Zen from any folder:** passes, 8 of 8 rows, and the Phase 0 sources are
  unchanged.
- **The 100k folder:** passes. It opens in under half a second and scrolls smoothly.
- **The look:** rows 7 and 8 failed, but on design polish and new preferences, not on toolkit
  limits. Every item has a known fix in our own CSS and layout code. Only three libadwaita
  defaults had to be undone. The colours come from the theme. The live reload works in the spike
  and is only missing from the look programs (row 9).

## Proposed plan edits (not applied; Kyle decides)

1. **Add a Phase 1 look-polish round** before Phase 2: `P1l` for the header and `P1m` for the body.
   They would apply the fixes above and Kyle's answers to the open questions, add the live theme
   reload and Ctrl+= / Ctrl+- to both look programs, then repeat rows 7–9. The prototypes are
   the design reference for Phase 2 and cheap to change. The alternative is to carry these items
   into Phase 2's UI work.
2. **Phase 2 architecture:** one shared look module for the app: tokens, stylesheet, theme reload
   and blur. The two prototypes each copied `style.rs`, and only the spike had the theme reload.
3. **Phase 2 rules:** cap max-columns from width outside allocation; never unparent during a drag;
   claim drags inside the window handle past the threshold; no measured minimum sizes.
4. **Feature list additions:**
   - delete to Trash, with the Delete key and a right-click menu (row 5);
   - reorder pinned folders by dragging (Q10);
   - close buttons in the accordion's tab list;
   - Gimbal's own icon set from the mockup (Q8).
5. **Distribution:** ship the niri rule for Gimbal's app IDs in GimbalOS, and look at
   `xdg-decoration` in Phase 2 so other niri setups don't need the rule.

## Open design questions (for Kyle; Claude's proposal in each)

- **Q1 — Path bar.** The Oct 31 decision was "the path bar stretches into all free header space".
  Your note says it should recentre when there is space (tabs collapsed, tabs or groups closed).
  *Proposal:* give the path bar a natural width (the sketch's 960 px × scale) and centre it in the
  space between the tab strip and the trash button. When space runs short, it shrinks to its
  320 px minimum first, and only then do the tabs collapse.
- **Q2 — Outline weight.** All outlines are too thick. *Proposal:* the 6 px outlines (pills, path
  bar, search field, side panels) become 3 px; the 3 px outlines (section pills, tiles, trash
  button, ✕ rings) become 2 px.
- **Q3 — Window buttons and search icon.** *Proposal:* float and close go from 24 to 32 px; the
  search icon from 44 to 36 px.
- **Q4 — Split padding.** *Proposal:* the gap between tiles goes from 40 to 24 px (`gap-l`); the gap
  between a tile's pill and its frame goes from 24 to 8 px (`gap-s`).
- **Q5 — Three tiles.** *Proposal:* tiles go in two columns, and a column with fewer tiles makes
  them taller. With 3 tiles, the left column has 2 stacked and the right column has 1 at full
  height. With 5, the right column's 2 tiles share its height. Rows beyond 4 tiles still scroll as
  before.
- **Q6 — Pill tops.** *Proposal:* move the split view up so the top edge of the first row of pills
  is level with the top edge of the side panels, and everything below moves up with it.
- **Q7 — Sidebar hierarchy.** *Proposal:*
  - Pinned rows go from 28 to 22 px text and 32 to 24 px icons.
  - Section pills ("Recent Directories", "Devices", "Remote") go from 28 to 22 px text, 40 to 32 px
    tall.
  - The items under them (directory names, drive names) go from 20 to 22 px.
  - This puts the panel titles (32) on top, then section pills, then rows.
- **Q8 — Icons from the mockup.** *Proposal:* Claude extracts the mockup's own icons from the
  VectorCraft file as SVGs: the folder, the file, Save (floppy), Save As (floppy with pencil) and
  Discard (page with ✕). The look programs use them instead of Adwaita's icons, coloured from the
  theme. This replaces "icons from the theme" for these five.
- **Q9 — Focus and selection contrast.** *Proposal:* the focus ring becomes the full accent colour
  (now 50 % alpha), and selected fills mix in 50 % accent (now 30 %).
- **Q10 — Reordering pinned folders.** *Proposal:* drag a pinned row to reorder it, with the same
  follow-and-slide feel as the tab pills, in this round.

## Kyle's answers (Nov 1, 2026)

- **Q1:** Keep the path bar stretching into all free header space. The bug is that when the pills
  condense into the "N tabs" accordion, or tabs and groups close, the path bar doesn't move back
  to the left; it stays squeezed against the right. A fix, not a design change (P1l).
- **Q2:** Yes: 6 px outlines become 3 px, and 3 px outlines become 2 px.
- **Q3:** All three are 32 px: float, close (and minimize where shown) and the search icon.
- **Q4:** Yes: 24 px between tiles, 8 px between a tile's pill and its frame.
- **Q5:** Try the proposal (a column with fewer tiles makes them taller); Kyle judges it on screen.
- **Q6:** Yes.
- **Q7:** Yes.
- **Q8:** Asked whether the mockup's icons can follow the theme. Yes: GTK 4.22's icon engine ties
  each shape's fill or stroke to a named colour slot (`foreground`, `accent`, `success`,
  `warning`, `error`), and `-gtk-icon-palette` remaps the last three per widget from CSS, so each
  part of a multi-tone icon can take a theme colour and follow it live. Claude extracted the
  folder, Save, Save As and Discard icons from the VectorCraft file
  (`gimbal-files-prompts/phase1/icons/`); the mockup has no separate file icon (every grid item in
  it is a folder), so files keep the theme's icon. The colour mapping is in `look-style-spec.md`.
- **Q9:** Yes.
- **Q10:** Yes.

On the strength of these answers, the look-polish briefs `P1l` (header) and `P1m` (body) are written. The verdict and plan edits 1–5 still wait for Kyle's approval.
