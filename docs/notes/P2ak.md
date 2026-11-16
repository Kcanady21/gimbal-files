# P2ak: smaller header icons, Minimize before Maximize

Tags: **[observed]** for a command or its output, **[read]** for source, and
**[inferred]** for anything that needs a real key, pointer or live feel.

Starts from `39ee372` (P2aj) on `main`. Nothing here launches the app: the
evidence is the generated stylesheet, the fit maths and its tests.

## What I built

- **One new token (`look/tokens.rs`).** `ICON_HEADER_SMALL: f64 = 24.0`, with
  the doc comment that names the five glyphs it sizes. One token is enough —
  Refresh and the window controls read the same at 24 px, so the brief's
  "or two tokens, if you find they need to differ" case did not come up.
  `ICON_WINDOW` is **retired**, not kept at 32: every one of its uses was a
  header glyph (`.gl-icon-button.winbtn`, `.float image`, `.close image`,
  `.gl-trash`, the `right` term in `header_fits`, and the Minimize estimate in
  `header.rs`) and all six moved to the new token, so a kept constant would be
  a value nothing draws. `ICON_NAV` and `ICON_SEARCH` keep 32 px and every
  other use they have. `spike/src/bin/look-header/style.rs` still declares its
  own `ICON_WINDOW`; the spike is a frozen prototype with its own copy of the
  constants and the brief does not list it, so it is untouched.
- **Five smaller glyphs (`look/css.rs`).** `.gl-icon-button.winbtn`,
  `.gl-icon-button.float image`, `.gl-icon-button.close image`,
  `.gl-field .gl-icon-button` (the Refresh button inside the path field) and
  `.gl-trash` now take the small token: 24 px at scale 1, 12 px at 0.5. The
  local binding in the stylesheet generator is `ismall` where it was `iwin`.
  Padding, radius, hover, active and focus-ring rules are exactly as they
  were, and nothing else was re-specified.
- **The Trash box is unchanged.** `min-width` / `min-height` stay
  `TRASH - 2 * LINE_THIN` = 44 px inside a 2 px border, 48 px outside, so it
  still lines up with the 50 px path and search fields. Only the glyph inside
  it drops from 32 to 24, which takes the slack from 8 px to 10 px a side.
- **Window buttons shrink with their glyphs.** `.gl-icon-button.winbtn`
  already had `min-width` / `min-height` equal to the icon size and `padding: 0
  GAP_S`, so both follow the token and the outer box goes 72 → 64 px. That is
  the only change to the right edge's width (see the maths below).
- **Minimize before Float / Maximize (`header.rs`).** The `then` block that
  builds and appends Minimize now runs before the Float button is built.
  Handlers, tooltips, accessible names, `valign` and the `winbtn` / `float` /
  `close` classes are byte-identical — only two statements swapped places.
  Order is now Trash, Minimize, Float / Maximize, Close where Minimize shows
  (Mango, and anywhere `NIRI_SOCKET` is unset), and Trash, Float, Close under
  niri, as before.
- **The two width estimates share one function (`look/css.rs`).** New
  `window_button_width(scale)` returns the outer width of one drawn window
  control: glyph + 2 × `GAP_S` padding + `GAP_L` margin. `header_fits`'s
  `right` term is `TRASH + GAP_L + 2 * window_button_width(scale)` (algebraically
  the same sum as before with a different token: it already charged 2 margins,
  they were just written as one `2 * GAP_L`), and `Header::update_layout`'s
  `desktop` term is `window_button_width(scale)` when `controls.minimize`. The
  per-button term is now written once instead of twice, so the collapsed-strip
  maths cannot disagree with what is drawn — which it could before: `header.rs`
  rounded `(ICON_WINDOW + 2 * GAP_S + GAP_L) * scale` as one value while
  `header_fits` rounded each token, and only the shared function fixes that.

## The right edge, at scale 1

| Desktop | Before | After | Δ |
| --- | --- | --- | --- |
| niri (Trash, Float, Close) | 216 px | 200 px | −16 |
| Mango (Trash, Minimize, Float, Close) | 288 px | 264 px | −24 |

One button is `glyph + 2 × 8 + 24`: 72 px before, 64 px after. The Trash box
contributes 48 + 24 in both columns. The freed width goes to the path field,
so `header_fits(1600, 2, 1.0).path_width` is 396 px where it was 380 px, and
with two pills the header keeps stretching down to 1524 px wide instead of
1540 px (762 instead of 770 at 0.5 scale). **[observed]**

## Tests

- `the_small_header_glyphs_and_the_right_edge_agree` (new, `look/css.rs`). Pins
  the three rules I changed at both scales — `winbtn`, `.gl-field
  .gl-icon-button` and `.gl-trash`, the last one checking the 44 px box *and*
  the 24 px glyph in one substring — asserts `.gl-search-glyph` is still
  32 px, and checks the right-edge sum (64 per button, 200 px without
  Minimize, 264 px with) against `header_fits` for the same width.
- `float_icon_is_symbolic_and_maps_the_front_fill` and
  `close_icon_is_symbolic_with_foreground_strokes` updated from 32 px / 16 px
  to 24 px / 12 px. The SVG assertions in both — viewBox, two `d="M "` paths,
  two `gpa:stroke="foreground"` paths, no literal colours — are untouched.
- `header_fits_stretches_then_collapses` updated: 1524..=2560 all fit,
  1600 → 396, 1524 → 320, 1523 collapses, 800 at 0.5 → 198, 762 → 160, 761
  collapses.
- `tokens_lost_nothing_in_the_merge` swaps `ICON_WINDOW` for
  `ICON_HEADER_SMALL` in the header-only list and adds `assert_eq!` on 24.0
  plus a loop that keeps it strictly below `ICON_NAV`, `ICON_SEARCH` and the
  Trash box's 44 px inside. **[observed]**

The order is not covered by a test: it is the `row.append` sequence in
`Header::new`, and there is no pure function to call. Wrapping it in one would
mean a second copy of the order that the real code does not go through, so per
the brief this is left to the live check. `platform::controls(niri)` — already
pure and tested — is the only branch in it, and it decides whether Minimize
exists at all, not where it sits.

## Five gates

**[observed]** `./scripts/check.sh` from the repo root: exit 0, all five steps
pass — `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, `cargo test --workspace`, `cargo build --workspace --release`,
`cargo deny check` (advisories, bans, licenses, sources all ok).

Totals: **1014 passed, 0 failed, 25 ignored** across the workspace, i.e. P2aj's
accepted 1013/25 plus the one new test. `clippy::assertions_on_constants`
rejected a plain `assert!(ICON_HEADER_SMALL < ICON_NAV)` in the token test (a
const-folded comparison), so those two comparisons are the value-pair loop the
same test already uses for its other tokens.

## Keyboard focus order

Item 4 is a confirmation, not a change: nothing in the header overrides the
focus chain (no `set_focus_chain`, and the only `set_can_focus` in the file is
on the path entry when the Trash view is active, nowhere near the window
controls), and `gtk::Box` walks its children in `append` order for Tab. The
same `Rc<Header>` fields (`minimize`, `float`, `close`) are wired by
`connect_clicked` later, so the handlers moved with their widgets. **[read]**
for "nothing overrides it"; whether Tab visibly walks Trash → Minimize → Float
→ Close is **[inferred]** until Kyle presses it.

## The icon art at 24 px

`float.svg` (28.08 × 25.12) and `close.svg` (28.08 square) are both stroked at
3 user units, so the stroke is 10.7 % of the glyph width whatever size GTK
draws it — the relative weight is unchanged, only the device pixels shrink:
3.42 px at 32 becomes 2.56 px at 24. That is still thicker than the 2 px
`LINE_THIN` outline the header draws around the Trash box, so nothing looks
hairline-faint; if it reads heavy in person the fix is a redraw, and per the
brief the art is Kyle's, so I have not touched either file. Float's aspect
ratio means it draws 24 × 21.5 in its 24 px box; it was 32 × 28.6 before, so
the same centring applies. **[inferred]** from the SVGs and the stylesheet.

## Live checks (Claude and Kyle, debug build, scratch XDG dirs)

**[inferred]** until someone runs them. Nothing here is verified by launching
the app.

1. Refresh, Trash, Minimize, Float / Maximize and Close are visibly smaller
   than the four nav arrows and the search glyph, centred in their boxes, and
   the Trash border box is exactly as big as before (level with the path and
   search fields).
2. The order is Trash, Minimize, Float / Maximize, Close on Mango.
3. Each button still does what it did: Minimize minimizes, Float / Maximize
   maximizes and restores, Close closes, Trash opens the Trash, Refresh
   reloads the folder.
4. Hover and keyboard focus rings look right on all five: the hover fill and
   the 2 px accent ring are the same size as the shrunken box, and Tab walks
   them in the new order.
5. Narrow the window until the header collapses: no clipping or overlap at the
   right edge, and the path field keeps its 320 px minimum while the pills go
   to the strip.
6. Light and dark themes.
7. Worth a look while it is open: the search glyph inside the collapsed search
   field is still 32 px while Refresh inside the path field is now 24 px (the
   brief keeps the search glyph at 32). If that asymmetry bothers Kyle,
   `.gl-search-glyph` is one token away from matching.

## Files changed

- `crates/fm-ui/src/look/tokens.rs`
- `crates/fm-ui/src/look/css.rs`
- `crates/fm-ui/src/header.rs`
- `docs/notes/P2ak.md`

No new dependencies, no new `unsafe`, no new GTK calls, and nothing outside
the brief's Output list. `README.md` needs no entry: it documents settings and
behaviour, and it never mentions the header's controls or their sizes, so
nothing there went stale.

## Claude's review (Nov 16, 2026)

The return matches the brief. The order change is only the append order in
`header.rs`, so it was left to the live check, as the brief allows. Both width
estimates now share `window_button_width`, and the search button sits outside
`.gl-field`, so the smaller field-button rule doesn't reach it. **[read]**

### Change after Kyle's live check

1. **No outline on the Trash box (`crates/fm-ui/src/look/css.rs`).** Kyle
   asked for the box's permanent outline to go. `.gl-trash` drops its
   border and keeps its full `TRASH` size (48 px at scale 1), so the header
   layout and `header_fits` are unchanged. The hover background and focus
   ring stay. The size tests now assert 48 / 24 px; the token test compares
   against `TRASH`. **[observed]**

### Gates

`scripts/check.sh`: exit 0; 1014 passed, 25 ignored. **[observed]**

### Live checks (Kyle, Nov 16, scratch XDG dirs, `/tmp/gimbal-p2ak`)

All passed: the five glyphs are smaller and centred; the order is Trash,
Minimize, Float / Maximize, Close on Mango; every button works; hover and
focus rings look right; the collapsed header doesn't clip; light and dark are
fine. The Trash button without its outline looks right. **[observed]**
