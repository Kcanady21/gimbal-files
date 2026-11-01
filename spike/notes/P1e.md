# P1e notes: a folder with 100,000 files

Brief: `phase1/P1e-large-folders.md`. Files written: `src/listing.rs`, `tools/make-big-dir.sh`,
this file. Nothing else was touched.

## What I built

- `tools/make-big-dir.sh [COUNT] [DIR]` (defaults `100000`, `/tmp/gimbal-spike-100k`): makes `COUNT`
  empty files from 10 stems x 10 extensions (`note-…`, `Report-…`, `image-…` in `.txt .md .rs .py
  .png .jpg .pdf .json .csv .gz`), every 5000th name long and spacey to force ellipsization, plus 6
  subfolders that hold the first 600 files. Names come from the index only, so it is idempotent.
  It resolves `DIR` with `realpath -m` (which collapses `..`) and refuses anything that is not under
  `/tmp/gimbal-spike-`. 41 lines.
- `src/listing.rs`: `ListView` -> `gtk::GridView` (icon 64 px above a centred, end-ellipsized, 2-line
  name in a ~128 px cell), `max-columns`/`min-columns` set, incremental sorting on, the attribute
  list trimmed, and the P1b review item 2 leak fixed with weak references. 228 lines (< 240).

The generated folder: 100,000 files total, **99,400 of them in the root** plus 6 subfolders of 100
(99,406 items is what the view reports for the root, since `.` is not listed) **[observed]**.
It took 0.3 s to make. `/tmp/gimbal-spike-100k` and `/tmp/gimbal-spike-p1e-selftest` are left in
place for the live check; they are tmpfs, so they cost no disk.

## The table

Every run: `GIMBAL_SPIKE_LOG=1 ./target/release/gimbal-spike /tmp/gimbal-spike-100k`, 9 s, then
`VmHWM` read from `/proc/<pid>/status` and the process stopped with `kill -TERM <pid>`.
"first items" is seconds from process start (the log prefix); it is the first batch landing **in the
model**, which is the closest non-live proxy to rows on screen — the real paint is Claude's check.

| build | run | first items | loaded | peak RSS (VmHWM) |
|---|---|---|---|---|
| **A. P1b list** (`ListView`, baseline) | 1 | 0.131 s | 451 ms | 211 980 kB |
| | 2 | 0.130 s | 458 ms | 212 268 kB |
| | 3 | 0.133 s | 457 ms | 212 228 kB |
| **B. grid, before fixes** | 1 | 0.132 s | 769 ms | 252 208 kB |
| | 2 | 0.132 s | 670 ms | 252 460 kB |
| | 3 | 0.133 s | 709 ms | 251 884 kB |
| **C. grid, after fixes (delivered)** | 1 | 0.129 s | 685 ms | 234 344 kB |
| | 2 | 0.130 s | 885 ms | 253 348 kB |
| | 3 | 0.131 s | 783 ms | 242 652 kB |
| C repeated (noise check) | 4-6 | 0.131/0.133/0.137 s | 717/688/755 ms | 239 300/233 980/234 296 kB |

Row A is P1b's code **plus one inert log line** (the `first items` line, an `eprintln!` behind
`GIMBAL_SPIKE_LOG`). To prove that line is inert, the untouched P1b binary was measured first:
loaded 475/475/450 ms, VmHWM 212 196/212 228/212 144 kB — the same numbers, and it had no
first-items line **[observed]**.

Row C has a real spread (685-885 ms). The machine was not idle: `uptime` showed load average 2.15
while P1c/P1f/P1j were compiling in the same tree, and the second set of three runs of the *same*
binary came in 688-755 ms. So read C as "about 700 ms, median 736 ms", not as 685 ms **[inferred]**.

What the grid costs over the list is not the model, it is the cells: the list realises ~30 rows, the
grid pools `32 * max_columns + 1` cells (2049 at 64). That is the +250 ms to loaded and +40 MB in
row B **[inferred from the citations below]**.

## G_MAXUINT is not "no cap" (the trap in step 2)

Setting `max_columns` to `u32::MAX` and measuring gave **896 356 / 889 992 / 858 144 kB peak RSS and
the load never finished inside 9 s** (first items still 0.13 s) **[observed]**. The reason is in the
source: `GtkGridView` passes the property to
`gtk_list_base_set_anchor_max_widgets(max_columns * GTK_GRID_VIEW_MAX_VISIBLE_ROWS, max_columns)`
(`gtkgridview.c:1236-1238` in the constructor, `:1401-1403` in the setter; the rows constant is
`:40`, the default `DEFAULT_MAX_COLUMNS (7)` is `:42`, used at `:1233`), and the list base keeps
`2 * above_below + center + 1` widgets alive (`gtklistbase.c:2327`) — so `u32::MAX` asks for about
4 billion cells. `gtk_grid_view_measure_across` also multiplies the natural width by `max_columns`
(`gtkgridview.c:616`). `gtkgridview.c:1136-1139` allows `G_MAXUINT` in the property spec, which is
why this looks safe and is not.

The delivered value is a bounded 64 (= 8192 px of window at 128 px cells, wider than any real
screen), so the column count is always the width-derived one in
`gtk_grid_view_compute_n_columns` (`gtkgridview.c:620-638`, which `CLAMP`s to min/max) and the cap
never binds. `min-columns` default is 1 (`gtkgridview.c:1145-1149`), set explicitly anyway.
**This is a deviation from "with no cap", on purpose**: literal no-cap is unusable in 4.22.4.

## What each fix was measured to do

Single-variable A/B runs, 3 each, on the delivered code except the one knob.

| knob | first items | loaded | sort settled | peak RSS | kept? |
|---|---|---|---|---|---|
| `incremental = FALSE` (attrs trimmed, P1b comparator) | 0.130-0.132 s | 718/693/696 ms | 0.787/0.764/0.765 s | 239.6/239.2/239.6 MB | — |
| `incremental = TRUE` (same otherwise) | 0.129-0.136 s | 647/779/694 ms | 0.899/1.054/0.888 s | 234.7/234.7/234.5 MB | **yes** |
| attribute list trimmed (OFF arm) | 0.130-0.132 s | 693/696/718 ms | = loaded | 239.5 MB avg vs **252.2 MB** untrimmed | **yes** |
| allocation-free comparator (`cmp_ci`) vs P1b's `to_lowercase()` | 0.128-0.132 s | 647/779/694 vs 691/746/705 ms | 0.888-1.054 vs 0.901-0.921 s | 234.6 vs 234.4 MB | **no, reverted** |

- **Attribute trim, kept.** `standard::size` and `standard::is-symlink` were dropped; nothing in the
  tree reads them (`grep -n 'is_symlink\|\.size(' src/` finds only `listing.rs:9` and `info.icon()`),
  and peak RSS fell ~13 MB with no change to the times. `standard::name` stays in the string even
  though GTK prepends it itself (`gtkdirectorylist.c:559`). If a later brief shows size (P1g/P1i),
  it must add `standard::size` back — noted in *Outside my files*.
- **Comparator rewrite, measured and reverted.** I wrote an ASCII byte-folding `cmp_ci` (with a test
  proving it matches `to_lowercase()` ordering) because the P1b review item 4 flagged two allocations
  per comparison. It made **no measurable difference** at 100k (settle 0.888-1.054 s vs 0.901-0.921 s,
  identical RSS), so per "fix what the measurements show, and only that" the file keeps P1b's
  `to_lowercase()` version and the extra test is gone. That is the answer to review item 4.
- **Incremental sorting, kept, with a cost worth naming.** It does **not** improve any of the three
  table metrics: loaded is the same (~700 ms either way) and the sort finishes ~125 ms *later*
  (settle 0.90 s vs 0.77 s), exactly as GTK warns ("the total sorting time is a lot slower", worth
  turning on "around 10,000 to 100,000 items", `gtksortlistmodel.c:1300-1310`). The reason to keep it
  is the acceptance test's stall criterion: default FALSE re-sorts the whole model **synchronously
  inside every batch** (`:492-496` returns FALSE for non-incremental, and
  `gtk_sort_list_model_items_changed_cb` `:763-795` then calls `finish_sorting` inline), while TRUE
  paces it as idle steps capped at 1 ms (`GTK_SORT_STEP_TIME_US :54`, merge size 1024 `:41`,
  `start_sorting :470-500`). 20 batches x full re-sort is what can block a paint.
  **Visible cost: items appear in enumeration order and shuffle into place for the first ~0.2 s.**
  Only the live check can say whether that shuffle is acceptable to Kyle — flagging it deliberately.
  Default is FALSE (`g_param_spec_boolean ("incremental", …, FALSE, …)` at `:1035-1037`).
- **Loading is batched, not lazy** `gtkdirectorylist.c:54` `FILES_PER_QUERY 100` and `:493-494` ask
  for `50 * FILES_PER_QUERY` = **5000 per batch for native files**, which is exactly the `+5000` the
  first-items line prints **[observed]**. `io-priority` defaults to `G_PRIORITY_DEFAULT`
  (`:321-322`, `:376`), and `:918-920` suggests `G_PRIORITY_DEFAULT_IDLE` if it competes with other
  I/O. I did **not** change io-priority: on tmpfs nothing competed, and lowering it would only slow
  the load I am trying to measure.
- **Row factory does nothing per item beyond binding** **[read]**: `setup` builds the cell once (a
  box, an image, a label — no I/O); `bind` reads `info.icon()` and `info.display_name()`, both
  attributes the model already holds, so no `query_info`, no thumbnail, no synchronous I/O. The
  batched loader means `bind` runs per *realised* cell, not per item.

## The closed-tab leak (P1b review item 2)

The cycle was: `DirectoryList` owns the `loading`-notify closure, which held `Rc<State>` (strong) and
`selection` (strong), while `State` held `dir_list` (strong) — and `selection -> SortListModel ->
DirectoryList` closes a second GObject-only loop. Both are broken now:

- `State.dir_list` is a `WeakRef<gtk::DirectoryList>` (was a strong one), so `State` keeps no model
  alive; `navigate` upgrades it (`listing.rs:187-188`).
- the `loading`-notify closure holds `Rc::downgrade(&state)` and `selection.downgrade()`
  (`listing.rs:125-127`);
- the `activate` closure holds `Rc::downgrade(&state)` (`listing.rs:106`);
- the first-items closure holds only an `Rc<Cell<bool>>`;
- `setup` holds `selection` strongly, which is not a cycle: the factory is owned by the view and the
  model chain never points back at a cell.

Nothing strong keeps `State` after the tab's widget is gone, by reading the code: the only strong
owner is the `Rc<State>` in the `ScrolledWindow`'s data (`attach` is called at `listing.rs:67`,
defined with `state` at `listing.rs:194-200`), and glib frees that at object disposal. `State`'s
other fields are `Rc<RefCell<gio::File>>` (a file,
not a widget), a `Cell<Instant>`, and the `on_navigate` box, which is *held by* `State` and captures
only `weak_page` (`window.rs:59-61`, P1b's own fix) — so `TabPage -> ScrolledWindow -> State ->
weak TabPage` is broken by that weak ref. `dnd::attach_drop_target` receives `&state.dir`
(`listing.rs:68`), and P1c's closure clones only that `Rc<RefCell<gio::File>>` (`dnd.rs:87-96`, the
clone at `:90`), never `State` **[read]**. Weak upgrades also simply no-op if a signal arrives during
teardown, which is safer than the old strong refs.

**Not verified by me: the actual freeing.** That needs two tabs open and one closed with a VmRSS
comparison, which is Claude's live check. What I can say is that the code no longer contains a strong
path from the model chain back to `State`.

## API citations

gtk4-rs `gtk4-0.11.5` (in `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`):
`auto/grid_view.rs:47` `new(model, factory)`, `:164` `set_max_columns`, `:172` `set_min_columns`,
`:211` `connect_activate` (same `Fn(&Self, u32)` shape as `auto/list_view.rs:225`, so the activate
code did not change); `auto/image.rs:243` `set_pixel_size`, `:188` `set_from_gicon`;
`auto/label.rs:462` `set_xalign`, `:312` `set_justify`, `:446` `set_wrap`, `:328` `set_lines`,
`:293` `set_ellipsize`, `:350` `set_max_width_chars`; `auto/sort_list_model.rs:104` `set_incremental`,
`:79` `pending`, `:199` `connect_pending_notify`; `auto/directory_list.rs:15` (implements
`gio::ListModel`), `:200` `connect_loading_notify`; `auto/widget.rs:1382` `set_size_request`;
`auto/enums.rs:6236` `Justification`, and `:7302+` shows `Ordering` has **no**
`From<std::cmp::Ordering>` — the `match` in `compare_entries` has to stay. `lib.rs:17` re-exports
`pango`; `prelude.rs` does **not** re-export gio's prelude, hence the explicit
`use gtk::gio::prelude::ListModelExt` for `connect_items_changed`
(`gio-0.22.10/src/auto/list_model.rs:68`). `glib-0.22.10/src/object.rs:2240` `downgrade`,
`:3510` `WeakRef`, `:3550` `upgrade`.

GTK 4.22.4 (`../gimbal-refs/gtk-4.22.4/gtk/`): the grid and sort/directory citations above;
`gtklabel.c:1123-1155` `get_default_widths` (natural width = `char_pixels * MAX(width_chars,
max_width_chars)`), `:1347-1369` `gtk_label_measure` dispatches to `get_width_for_height` when
`wrap` is on, `:1276-1343` that function — this is why `max_width_chars(14)` plus a 128 px cell
request keeps a 60-character name from deciding the column width. `gtkimage.c:1189-1206`
`gtk_image_measure` reports `min = natural = gtk_icon_helper_get_size()`, and `:1246-1255`
`gtk_image_set_pixel_size` feeds that helper, so the 64 px icon really requests 64 px.

## Gate commands (RULES.md "Done means")

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
exit=0
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Checking gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s
$ cargo test
running 4 tests
test dnd::tests::conflicts_get_copies ... ok
test dnd::tests::extension_edge_cases ... ok
test listing::tests::sorts_folders_first_then_names ... ok
test tests::parses_cli ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
(the look-header bin adds "3 passed; 0 failed" in the same run; P1b's notes recorded 2 tests for
the skeleton, and the extra two here are P1c's dnd::tests, not mine — my module still has the one
sorts_folders_first_then_names test, unchanged)
$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.02s
$ wc -l src/listing.rs tools/make-big-dir.sh
  228 src/listing.rs
   41 tools/make-big-dir.sh
$ grep -nE 'remove|delete|trash' src/listing.rs tools/make-big-dir.sh ; echo $?
1                                   # no matches
```

Functional spot-checks I could do without injection **[observed]**: `--select Item-000608.json` on
the selftest folder still logs `selected 1/1 name(s)`; two `DIR` arguments still open two tabs, each
loading its own folder (`100 items` and `106 items`).

## Not verified without a live session (Claude)

- Scrolling the 100k grid by scrollbar and wheel: smooth, no stall over ~100 ms. **This is the check
  that decides whether `incremental = true` was worth its 125 ms.**
- That rows actually appear sorted (they shuffle for ~0.2 s first — see the cost above).
- Selection staying correct after scrolling, and dragging a file out of the folder (the dnd hooks are
  in P1b's positions: `attach_drag_source` in factory `setup` at `listing.rs:88` with the cell instead
  of the row, `attach_drop_target` at `listing.rs:68` unchanged, right after the view is created).
- Column count following the window width down to one column; the 64 cap only binds past 8192 px.
- The closed-tab free (VmRSS before/after closing one of two tabs on the 100k folder).
- That the cell looks right at all: 64 px icon, centred 2-line ellipsized name, ~128 px cell.
  P1b review already flagged that no screenshot was taken here.

## Anything I'd change outside my files

- `src/theme.rs` (P1f) broke the crate build twice while I worked (`error[E0505]: cannot move out of
  'pending'` at `theme.rs:58`, and earlier `E0282`/`E0308`), which blocked my `cargo build`/`test`
  for a few minutes; it is green now. Not my file, so nothing was done about it — but it means
  "clippy/test pass" here reflects the whole tree at 19:5x, not my module alone.
- `Cargo.toml` is fine as pinned; no new dependency and no feature was needed.
- If P1g or P1i wants a size column, `standard::size` has to go back into `ATTRIBUTES`
  (`listing.rs:12`); it was measured as ~13 MB of the peak RSS at 100k, so that is its price.
- `listing.rs` is 228 of the 240 allowed lines. To fit, the temporary `SortListModel::pending`
  instrumentation that produced the "sort settled" column was **removed from the delivered file**
  (the numbers above came from a build that had it; the delivered build differs only by that log).
  The `first items` line stays, since the table's first column depends on it.
- The `first items` and `loaded` logs fire once per view (the folder it opened with), not after every
  `navigate()`; P1b's `folder load finished` line still logs on every navigation.

## Review (Claude, Nov 1, 2026)

**Verdict: accepted on the code. Live checks still to run** (scroll the 100k grid, drag out of
it, narrow to one column, the closed-tab VmRSS comparison).

Checked: only the three Output files changed; the dnd hook calls are unchanged apart from the cell
widget; four RULES commands pass; `listing.rs` 228 lines, `make-big-dir.sh` 41. I reran the
timing on the delivered build: loaded 687 / 677 ms and VmHWM 235 / 235 MB on warm runs (the first,
cold run took 2.2 s), matching row C. Citations checked: `gtkgridview.c:40,42,1236-1238,1401-1403`,
`gtklistbase.c:2327`, `gtksortlistmodel.c:54,492-496`, `gtkdirectorylist.c:54,493` all say what
the notes say.

1. **The `max-columns = 64` deviation is accepted.** It's the most useful finding of this round:
   in GTK 4.22.4, `max-columns` also sizes the cell pool (`32 × max + 1` widgets), so "no cap"
   literally means billions of cells. Phase 2 should set `max-columns` from the width (columns
   that fit at the minimum cell size, plus one) on resize instead of a fixed number. That keeps
   the pool near the 7-column default's size and should give back most of the +40 MB.
2. Incremental sorting stays for now. Whether the ~0.2 s shuffle is acceptable is a live check
   for Kyle.
3. P1j sets `max-columns(1000)` on its grids: covered in `P1j-r1-fixes.md`.

### Live check (Claude, Nov 1, 2026): needs fixes (`P1e-r1-fixes.md`)

Window floating at 1220×1326 on an empty workspace, `/tmp/gimbal-spike-100k` in two tabs,
`WAYLAND_DEBUG=1`. Pointer through the runner's virtual mouse.

What passed:

- **Scrolling.** Dragging the scrollbar from the top to the end over 3.3 s: 62 commits, the
  longest gap between frames 65 ms, none over 100 ms. That's about 19 fps while jumping through
  99k items; it never stalls. Sort order is right at both ends (`sub-00`…`sub-05`, then
  `alpha-000609.csv` …; `Zeta-099994.py` last).
- **The closed-tab leak is fixed.** Two tabs: VmRSS 363.6 MB. Closing one: 358.8 MB (glibc
  keeps freed memory, so this alone proves nothing). Opening the same folder again in a new tab
  grew RSS by only 52 MB, against 128 MB for the original second tab, so most of the closed
  tab's memory was freed and reused.
- **Columns follow the width:** 8–9 at 1220 px, 5 at 700 px, 2 at 360 px (the window's minimum
  width, set by the stock header bar, so one column can't be reached by narrowing).
- Drag out of the grid into Zen: passes (P1g's matrix, on the same grid code).

Corrections:

1. **The visible tab opens in the middle of the folder.** The focused tab showed
   `file-094866.jpg` at the top, not `sub-00`. The second tab, not on screen during the load,
   opened at the top. The same happened in `/tmp` (153 items). So the grid's scroll anchor stays
   on whatever item was on screen first, and incremental sorting then carries that item deep into
   the list. A folder must open at its top (or at the `--select`ed item). Fix it, and
   say whether `incremental` stays on.
2. **The widest name sets every column's width.** `/tmp` showed **one column** across the full
   1220 px. Its longest names are 78–85 characters with 32-character hex runs that can't wrap
   (`systemd-private-892d…-power-profiles-daemon.service-CXfF2z`). In the 100k folder the same
   window showed 8 columns at the top and 9 at the end. Every cell must be the same width
   whatever its name: a long name wraps anywhere and ellipsizes within the 2 lines.
3. **Open question: navigating into the 100k folder took 3.6 s,** against about 0.7 s when it
   is opened at startup. (This was the new tab: home → `/` → `/tmp` → `gimbal-spike-100k`, after a
   tab on the same folder had been closed.) Read what differs when `navigate()` reuses a
   `DirectoryList` that already holds another folder, and explain or fix it. Claude remeasures
   it live.

## P1e-r1 changes (Nov 1, 2026)

Brief: `phase1/P1e-r1-fixes.md`, corrections 1–3 of the live check. Files changed: `src/listing.rs`
(279 lines) and `tools/make-big-dir.sh` (44), plus this section. Nothing else was touched; the two
`dnd::` calls are at their P1e positions and with their P1e arguments (`attach_drag_source(cell,
item, &selection)` in factory `setup`, `attach_drop_target(scrolled, &state.dir)` after the view is
created). No dependency or feature changed.

### 1. The view opened in the middle of the folder

**Cause [read].** The grid's scroll position is not stored as a pixel offset; it is stored as an
anchor *item*. `GtkListBase` owns a `GtkListItemTracker` for the anchor (`gtklistbase.c:2067`) and
computes both adjustment values from the anchor item's allocation (`gtklistbase.c:2136-2173`). When
the model changes, the item manager keeps that tracker on its item: if the tracked item was removed
it does the expensive "find where it moved" rebuild, otherwise it copies the item's new position
(`gtklistitemmanager.c:1546-1591`, the assignment at `:1584`). With `incremental = TRUE` the sort
runs in idle steps after the load (`gtksortlistmodel.c:490-499`, step time `:54`) and emits broad
`items_changed` ranges (`:796-797`), so the item that happened to be at the top when the first
5000-file batch landed was carried to its sorted place (the live run showed `file-094866.jpg` at the
top). The grid then kept showing that item. `GTK_LIST_SCROLL_NONE`/`scroll_to` sets the anchor explicitly
(`gtklistbase.c:2398-2434`; it needs the item's allocated area, `:899`), and an adjustment value
change re-anchors on whatever item is at the top (`gtklistbase.c:182-262`). The pool around the
anchor is sized `2 * above_below + center + 1` (`gtklistbase.c:2329-2344`), which for the grid is
`max_columns * 30` centre cells (`gtkgridview.c:1236-1238`) — the number from the P1e notes.

**Fix [observed].** `incremental` is now left at its default `FALSE` (`gtksortlistmodel.c:1035-1037`),
so each 5000-file batch is sorted before the next one lands (`gtksortlistmodel.c:493-496`, the
inline `finish_sorting` in `:763-795`) and the order is final when the `loading` property goes
false. The `loading`-notify handler (which already ran per load, so it covers startup and every
`navigate()`) now calls `show_item` after the `--select` pass: position 0 sets the vertical
adjustment to 0 and, if the view is mapped, also calls `scroll_to(0, NONE, None)`; a `--select` match
calls `scroll_to(position, NONE, None)` instead. Positions are safe to use because the sort is no
longer deferred. **`incremental` stays off**: it was not making the load faster (table below) and it
removes the post-load shuffle that P1e deliberately flagged for Kyle.

**Verified [observed].** With a temporary 900 ms one-shot log (removed before delivery; the
measurement build differed from the delivered `show_item` only by that log for the 100k cases, and
the `/tmp` row used the same adjustment call before the mapped `scroll_to(0)` backstop was added)
the vadjustment was:

| case | value / upper at +900 ms |
|---|---|
| startup, `/tmp/gimbal-spike-100k` | `0 / 1 038 324` |
| `--select Zeta-099994.py` | `1 036 972 / 1 038 324` |
| `/tmp` | `0 / 2 052` |

So the default load is at the top, and the selected last item is at the bottom. Nothing in the
`loading`-notify closure holds the view strongly (weak `State`, `selection`, `view`), so the P1b
review item 2 fix is unchanged.

### 2. The widest name set every column's width

**Cause [read].** The cell label is a wrapping label. `gtk_label_measure` sends a horizontal request
with `for_size = -1` to `get_width_for_height` (`gtklabel.c:1347-1369`); with no height, the minimum
width is measured at `MAX(minimum_default, 0)` and the natural width at `natural_default`
(`gtklabel.c:1283-1297`). `minimum_default` is `char_pixels * width_chars` and `natural_default` is
`char_pixels * MAX(width_chars, max_width_chars)` (`get_default_widths`, `gtklabel.c:1124-1159`), so
`max_width_chars(14)` alone only capped the natural request. The default wrap mode is
`PANGO_WRAP_WORD` (`gtklabel.c:678`, property default `:2572-2576`), and a 32-character hex run has
no break point, so both the minimum and the natural width stayed at the run's width (or wider). The
grid then takes the maximum over every realised cell (`gtk_grid_view_measure_column_size`,
`gtkgridview.c:571-599`), multiplies it by the columns for the widget request (`:615-616`), and
derives the column count from it (`:664-667`, `:777-783`); one such name made `/tmp` a single
column.

**Fix [observed].** `make_cell` now sets `wrap_mode(Char)` as well as `width_chars(14)` (the label
already had `max_width_chars(14)`): a wrapping label can break inside the hex run, and when
`width_chars` is set it is used as the minimum. Combined with the 128 px cell request, every cell
measures exactly the same width. The label still wraps to two lines and ellipsizes at the end.

The new log line (printed at every load, only when `GIMBAL_SPIKE_LOG=1`) measures two fresh cells
built by the same `make_cell`/binding code with `WidgetExt::measure` (`gtk4-0.11.5:auto/widget.rs:1000`),
one bound to the folder's shortest display name and one to its longest:

```text
100k folder (shortest name 6 chars, `sub-00`; longest 103 chars):
[gimbal-spike 0.821] listing: folder load finished: /tmp/gimbal-spike-100k (99406 items, 736 ms)
[gimbal-spike 0.879] listing: cell width min/nat short 128/128, long 128/128, equal true

/tmp (shortest 5 chars, longest 85 chars, the systemd 32-char hex name):
[gimbal-spike 0.135] listing: folder load finished: /tmp (171 items, 50 ms)
[gimbal-spike 0.137] listing: cell width min/nat short 128/128, long 128/128, equal true
```

The two numbers are `minimum/natural` px; both pairs are equal, so no name can move the column
width now. [observed] The two-name scan is behind `log_enabled()` and runs after the `folder load
finished` timestamp, so it does not affect the timing table.

### 3. Navigating into the 100k folder took 3.6 s

**What the code says [read].** Reusing the `DirectoryList` does not make loading do more work:
`gtk_directory_list_set_file` (`gtkdirectorylist.c:823-843`) swaps the file/monitor and calls
`start_loading` (`:544-572`), which cancels the old load and clears the old items in **one**
`items_changed(0, n, 0)` (`:406-425`), then starts the same async enumerate with the same
5000-file batches for native files (`:493`). The old folder's items are gone in that single
emission; there is no per-file pass over them.

The costs on the widget side are per model change, not per reuse: each batch (and, with incremental
sorting, each sort step) reaches `gtk_list_item_manager_model_items_changed_cb`
(`gtklistitemmanager.c:1447-1617`), which rebuilds the tracker and calls
`gtk_widget_queue_resize` (`:1616`); a tracked item inside a removed range takes the expensive
rebuild branch (`:1463-1540`). A mapped grid re-measures every realised cell on the resulting resize
(`gtkgridview.c:777`, `:584-596`; row heights `:672-704`) over the `32 * max_columns + 1` pool
(`gtklistbase.c:2327`, `gtkgridview.c:1236-1238`) — 2049 cells at `max_columns = 64`.

**Measured [observed].** I reproduced `navigate()` without pointer input with a temporary
`GIMBAL_SPIKE_GOTO` timeout in `build` (removed; it opened `/tmp`, navigated to the 100k folder at
1.5 s, back to `/tmp` at 3.0 s, and to the 100k folder again at 4.5 s):

| build | first 100k load | second 100k load (after freeing the first) |
|---|---|---|
| delivered (`incremental` off) | 646 / 651 ms | 655 ms |
| temporary `incremental = true` | 639 ms | 704 ms |

The load itself is ~0.65 s on `navigate()`, not 3.6 s, even on the second load in the same tab. So
the 3.6 s does not come from `DirectoryList` reuse (its extra work is one clear of the old folder's
items); it did not reproduce in this sequence at all. Two differences from Claude's run that I could
not reproduce without input remain: the live navigation came after a *tab* on the same 100k folder
had been closed (a different free/allocate pattern for the 100k `GFileInfo`s), and it came from a
real double-click in the resized 1220×1326 window. The delivered fixes that bear on the per-change
cost are the fixed cell width (the column no longer re-measures/re-lays-out when a longer name
appears) and incremental off (one model change per batch instead of idle-step reorders). I did
**not** change `max_columns`; the review's Phase 2 item (derive it from the width, ~9 here instead
of 64 → a ~289-cell pool) is the next measured lever if the live 3.6 s persists. The exact live
timing is Claude's to retake; [inferred] for that cause, [observed] for the 0.65 s.

### Timing table rerun (startup load, delivered build)

Same method as P1e: `GIMBAL_SPIKE_LOG=1 ./target/release/gimbal-spike /tmp/gimbal-spike-100k`, 9 s,
`VmHWM` from `/proc/<pid>/status`, stopped with `kill -TERM`. Run alone, 05:45, load average 1.4.

| build | run | first items | loaded | peak RSS (VmHWM) |
|---|---|---|---|---|
| **P1e-r1 delivered** | 1 | 0.149 s | 731 ms | 238 376 kB |
| | 2 | 0.152 s | 713 ms | 238 880 kB |
| | 3 | 0.149 s | 732 ms | 238 616 kB |
| P1e row C (before this round) | 1 | 0.129 s | 685 ms | 234 344 kB |
| | 2 | 0.130 s | 885 ms | 253 348 kB |
| | 3 | 0.131 s | 783 ms | 242 652 kB |

Nothing got slower where it matters: median loaded **731 ms** vs 736 ms, median peak RSS
**238.6 MB** vs 242.7 MB. The first batch in the model is 0.149–0.152 s vs 0.129–0.131 s, about
20 ms later; P1e's own noise rerun of the same binary reached 0.137 s, and loaded/RSS are not worse,
so I read that as host noise [inferred], not the change (the first-items log is connected before the
sort model, so the sorter cannot delay it).

### Gate commands (RULES.md "Done means")

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
exit=0
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.03s
exit=0
$ cargo test
     Running unittests src/main.rs
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running unittests src/bin/look-body/main.rs
running 10 tests
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running unittests src/bin/look-header/main.rs
running 3 tests
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
exit=0
$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.02s
exit=0
$ wc -l src/listing.rs tools/make-big-dir.sh
  279 src/listing.rs
   44 tools/make-big-dir.sh
$ grep -nE 'remove|delete|trash' src/listing.rs tools/make-big-dir.sh ; echo $?
1                                   # no matches
```

(The 10 `look-body` tests and 3 `look-header` tests are other agents' bins, not this module; the
4 in `main.rs` include this module's `sorts_folders_first_then_names`.)

### New API citations (gtk4-rs 0.11.5)

`auto/grid_view.rs:129` `scroll_to(pos, ListScrollFlags, Option<ScrollInfo>)`; `auto/flags.rs:932-944`
`ListScrollFlags::NONE`; `auto/widget.rs:1000` `WidgetExt::measure` (returns min, natural, min
baseline, natural baseline); `auto/scrollable.rs:63` `ScrollableExt::vadjustment`;
`auto/widget.rs:578` `WidgetExt::is_mapped`; `auto/label.rs:438` `set_width_chars`, `:454`
`set_wrap_mode`; `pango-0.22.9/src/auto/enums.rs:3224-3263` `WrapMode::Char`. GTK source citations
are inline above.

### Outside my files / still live

- The review's Phase 2 suggestion — set `max-columns` from the window width on resize instead of a
  fixed 64 — remains the right next step for the 2049-cell pool; not changed here (it would alter
  P1j's look work and was not one of corrections 1–3).
- `/tmp/gimbal-spike-100k` was regenerated with the new script (100 000 files, 7 dirs) so the three
  80+ character names with 32-character runs are present: `Systemd-Private-…` (81),
  `Screenshot-…` (103), `Backup-…` (102). It is left in place for the live check.
- `/tmp` now holds 171 entries (the extra ones are these run logs); nothing needed cleaning.
- Live checks still to run: the 100k scroll, drag-out, column count down to one, the closed-tab
  VmRSS, that the visible tab really starts at `sub-00`, and the 3.6 s navigate remeasure.


## Review of P1e-r1 (Claude, Nov 1, 2026)

**Verdict: accepted on code; the live remeasure follows.** Only `listing.rs` (279 lines),
`make-big-dir.sh` (44) and this file changed; the dnd hook calls are unchanged; the four RULES
commands pass on the combined tree (21 tests).

1. The anchor explanation is right and well cited, and turning `incremental` off is the right
   fix: the order is final when `loading` goes false, so `show_item` can trust positions.
2. The width fix holds. Checked `gtklabel.c:1283-1297`: with a width-for-height request of -1
   the minimum is measured with the layout set to `width_chars` wide, so a run that can't break
   overflows it under `Word` and can't under `Char`. Phase 2 note: `WrapMode::WordChar` gives the
   same guarantee (it falls back to a character break only when a word doesn't fit) and wraps
   ordinary names at word boundaries instead of mid-word. One word to change; not worth a round.
3. The 3.6 s didn't reproduce at 0.65 s, and the reasoning about what reuse does and doesn't
   cost is sound. I remeasure it live with the real double-click.

### Live check of P1e-r1 (Claude, Nov 1, 2026): needs fixes (`P1e-r2-fixes.md`)

Same setup as before (1220×1326 floating window, DP-2 only), but **without** `WAYLAND_DEBUG`
this time: the first live check had it on, which is part of why it saw 3.6 s.

What passed:

- **Correction 1.** The visible 100k tab opens at `sub-00` (screenshot `r1-100k-top`), and `/tmp`
  opens at `.font-unix`.
- **Correction 2.** `/tmp` now shows 9 columns at 1220 px, the same as the 100k folder; the log
  shows `128/128` for the shortest and longest names in both. Phase 2 should still switch to
  `WordChar`: `Char` breaks ordinary names mid-word (`gimbal-spike-dro-pin`,
  `gimbal-spike-the-me`).

Corrections:

1. **Crash: the Up button aborts the app.** `navigate_up` (line 150) runs
   `if let Some(parent) = state.dir.borrow().parent() { navigate(&state, &parent); }`. The
   `borrow()` lives for the whole `if let` body, `navigate` then calls `state.dir.borrow_mut()`
   (line 167), and the panic ("RefCell already borrowed") inside a GTK callback can't unwind, so
   the process aborts. Every click on Up crashes. This came in with P1e (P1b read the folder from
   `dir_list`), not r1; the first live check navigated only by double-click. Take the parent
   into a local first. Also look for the same pattern anywhere else in `listing.rs`.
2. **Navigating is still about 3× slower than opening at startup, and it reproduces.** Launched
   on `/tmp` alone, double-clicking `gimbal-spike-100k`: **2,149, 2,036, 2,024, 2,016, 2,015 and
   2,017 ms** over six fresh runs, against **604 ms** for the same folder opened at startup in
   the same session (and 2,034 ms in a run with a second tab open). Moving the pointer off the
   window right after the double-click changed nothing (2,024/2,015 ms away, 2,016/2,017 ms
   over the grid). So your 0.65 s timer navigation misses something a real activation does.
   The obvious difference: a double-click gives the grid keyboard focus and a focused item
   (position 6 here), and selects it, while a timer gives neither. `GtkListBase` tracks the
   focused and selected items through model changes as it does the anchor. Reproduce without
   input by doing what the click does from your timer: `grid_view.grab_focus()`, select the
   item, then `grid_view.emit_by_name::<()>("activate", &[&position])`. Find which part costs
   the 1.4 s and fix it, or say what Phase 2 must do about it. `perf` is not usable on this
   machine (`perf_event_paranoid` is 2 and even software events are refused); use timestamps.

## P1e-r2 changes (Nov 1, 2026)

Brief: `phase1/P1e-r2-fixes.md`, corrections 1-2 of the live check. Files changed: `src/listing.rs`
(304 lines) and this section. Nothing else was touched. The two `dnd::` calls are exactly where
P1e/P1e-r1 left them (`attach_drag_source(cell, item, &selection)` in factory `setup`,
`attach_drop_target(scrolled, &state.dir)` after the view is created), and no dependency or
feature changed.

### 1. The Up crash

**Cause [read].** `navigate_up()` was `if let Some(parent) = state.dir.borrow().parent() {
navigate(&state, &parent); }`. The temporary `Ref` from `borrow()` lives for the whole `if let`
body, and `navigate()` does `*state.dir.borrow_mut() = target.clone()` on the same cell, so every
Up click panicked with `BorrowMutError`. The panic crosses a GTK signal callback, which cannot
unwind, so the process aborted.

**Fix [observed].** `navigate_up()` now takes the parent into a local first (`listing.rs:168-175`):
the `Ref` is dropped at the end of the `let` statement and `navigate()` borrows mutably afterwards.
I audited every other borrow in the file:

- `navigate()` used to hold `state.on_navigate.borrow()` across `callback(target)`
  (`listing.rs:195-200`). The callback is now an `Rc<dyn Fn(&gio::File)>` in `State`, cloned out
  before the call. The window's callback does not re-enter today, but the pattern is gone.
- the activate handler's `state.dir.borrow().child(info.name())` was already safe (the temporary
  `Ref` drops at the end of the `let`), and is now an explicit block so the boundary is visible
  (`listing.rs:95-99`).
- `pending.borrow_mut()` is a one-statement `std::mem::take`; `set_on_navigate()` and `dir_of()`
  only borrow and clone. No other borrow is held across a call.

**Verified [observed].** A temporary 2 s / 4 s timer in `build()` called `navigate_up()` twice
(removed before delivery, as required). Start folder `/tmp/gimbal-spike-100k/sub-00`: first Up to
the 100k folder, second Up to `/tmp`. Both `folder load finished` lines appear and the process
kept running to the kill:

```text
[gimbal-spike 0.067] listing: folder load started: /tmp/gimbal-spike-100k/sub-00
[gimbal-spike 0.107] listing: first items in model: +100 (100 so far)
[gimbal-spike 0.110] listing: folder load finished: /tmp/gimbal-spike-100k/sub-00 (100 items, 43 ms)
[gimbal-spike 2.733] temp: up #1 from Some("/tmp/gimbal-spike-100k/sub-00")
[gimbal-spike 2.733] listing: folder load started: /tmp/gimbal-spike-100k
[gimbal-spike 3.159] listing: folder load finished: /tmp/gimbal-spike-100k (99406 items, 426 ms)
[gimbal-spike 4.732] temp: up #2 from Some("/tmp/gimbal-spike-100k")
[gimbal-spike 4.732] listing: folder load started: /tmp
[gimbal-spike 4.814] listing: folder load finished: /tmp (183 items, 81 ms)
```

The same two-load sequence also passed on the pre-cap build (685 ms and 122 ms). The temporary
`temp:` lines are the only lines from the measurement build; the delivered file has no timer.

### 2. Navigating is still about 3x slower than opening at startup

#### Reproduction without input

Temporary env-gated code in `build()` (removed before delivery). Startup on `/tmp`; at 2 s:

1. `view.grab_focus()`,
2. find `gimbal-spike-100k` in the selection (position 6 in every run),
3. `selection.select_item(6, false)`,
4. `view.emit_by_name::<()>("activate", &[&6])`.

That reaches the real `connect_activate` handler, which builds the child `gio::File` and calls
`navigate()`. Variants measured: `plain` (timer calls `navigate()` directly, the r1 method),
`focus` (grab_focus only), `cellfocus` (grab_focus on the realised cell at position 6),
`select` only, `activate` only, `full` (steps 1-4), and `cellfull` (cell focus instead of grid
focus, then select + activate). Times are the `ms` in `listing: folder load finished`, i.e. from
`navigate()` to `loading == FALSE`.

#### The three rows, before and after the fix

All runs a11y on (the default), app tiled by niri at 1272x1398 (close to the live 1220x1326),
run alone, 07:00-07:10. `niri msg --json windows` reports `window_size [1272, 1398]` for the app.
"before" means the r1 code with `max-columns = 64`; "after" is the delivered file.

| row | before (64-column pool) | after (width-derived cap) |
|---|---|---|
| startup load, 100k | 719 / 745 / 757 ms | 438 / 442 / 444 ms |
| timer navigation `/tmp` -> 100k (r1 style) | 668 / 681 / 665 ms | 433 / 442 / 429 ms |
| reproduction: focus + select + activate | 2094 / 2083 / 2080 ms | 462 / 468 / 469 ms |
| reproduction with cell focus (closest to a click) | not run | 456 / 465 / 457 ms |
| focus only (no selection, no activate) | 2047 / 2085 / 2101 ms | 451 / 452 / 448 ms |

`cellfocus` before the fix was 2099 / 2144 / 2162 ms, so focusing the actual cell behaves like
focusing the grid. The isolation runs before the fix:

| variant (before, a11y on unless stated) | run 1 / 2 / 3 |
|---|---|
| focus only | 2047 / 2085 / 2101 ms |
| cell focus only | 2099 / 2144 / 2162 ms |
| select only | 670 / 693 / 656 ms |
| emit `activate` only | 643 / 663 / 684 ms |
| focus only, `GTK_A11Y=none` | 641 / 657 / 647 ms |

Selection and `activate` on their own are normal; **focus alone reproduces the whole 1.4 s**, and
turning a11y off removes it.

#### Cause

`GTK_DEBUG=accessibility` on the pre-cap build counted the AT-SPI contexts created for
`GtkListItemWidget`:

| run | load | cell AT-SPI contexts |
|---|---|---|
| `plain` timer navigation | 670 ms | 0 |
| `focus` timer navigation | 2005 ms | 2052 |
| `focus` after the cap (10 columns) | 459-462 ms | 324 |

The batch timestamps show where the time goes. In the focus run the DirectoryList batches are
~20-30 ms apart except for two blocking gaps, one after the first batch and one after the third;
the plain run has neither:

```text
focus, before the cap
[gimbal-spike 2.093] temp: batch +0 total 0
[gimbal-spike 2.120] temp: batch +5000 total 5000
[gimbal-spike 3.226] temp: batch +5000 total 10000      <-- 1.106 s gap
[gimbal-spike 3.244] temp: batch +5000 total 15000
[gimbal-spike 3.680] temp: batch +5000 total 20000      <-- 0.436 s gap
[gimbal-spike 3.698] temp: batch +5000 total 25000
```

`gtk_directory_list_set_file()` is not the cost: a temporary `Instant` around it logged 11-17 ms
(`set_file returned in 13.5/16.4/11.1 ms`), and the plain run's total is 0.6-0.7 s. The main loop
is blocked *after* the first batch, inside GTK's accessibility registration [observed].

Mechanism [read]:

- A widget's AT context is realized lazily, and only if its parent context is realized:
  `maybe_realize_context()` (`gtkatcontext.c:543-560`). At startup only the toplevel is realized
  (`gtkwindow.c:4025`); the `GtkGridView`'s context stays unrealized.
- While the parent context is unrealized, adding or removing a cell is a no-op:
  `gtk_at_context_child_changed()` returns early on `!self->realized`
  (`gtkatcontext.c:1699-1707`). That is why startup and the r1 timer (which never focused the
  grid) pay nothing.
- Focusing the grid makes the AT-SPI client ask the focused object for its children
  (`GetChildren`, `gtkatspicontext.c:712-738`), which realizes each child and the grid's context.
  From then on, parenting a pooled cell fires `gtk_accessible_update_children(parent, cell,
  ADDED)` (`gtkwidget.c:6363-6366`; `gtkaccessible.c:1445-1473`), the realised grid's
  `child_change` calls `gtk_at_context_realize(child_context)` (`gtkatspicontext.c:1320-1355`),
  and realizing an AT-SPI context mints a UUID, builds a D-Bus object path and queues a
  registration (`gtkatspicontext.c:1574-1633`): one AT-SPI object per pooled cell.
- The pool is what makes this large. `GtkGridView` passes
  `gtk_list_base_set_anchor_max_widgets(max_columns * 30, max_columns)` in its constructor and
  setter (`gtkgridview.c:1236-1238`, `:1401-1403`), and the list base keeps
  `2 * above_below + center + 1` widgets (`gtklistbase.c:2327-2345`): 2049 cells at the old
  `max_columns = 64`. Realizing 2052 contexts blocked the main loop for ~1.4 s [observed].

The live double-click reproduced it and the r1 timer did not because a click focuses the row:
the press handler calls `gtk_widget_grab_focus(widget)` (`gtklistfactorywidget.c:433-434`, via
`gtkwidget.c:5154-5162`) and the release selects the item (`:459-475`); the double-click press
then activates it (`:424-430`). The r1 timer went straight to `navigate()` and never focused
anything. The pointer itself was irrelevant, as the live check found.

#### Fix and rerun

`max-columns` now follows the viewport width instead of being fixed at 64: the h-adjustment's
`page-size` notify computes `page_size / CELL_WIDTH + 1`, clamped to `1..=64`
(`listing.rs:105-120`), and the grid starts at 1 until that first allocation (`listing.rs:83-84`)
so a background tab does not build a 2049-cell pool either. At the measured 1272 px viewport that
is 10 columns (the grid lays out 9); the live 1220 px window also gives 10 (the grid lays out 9),
so the cap never binds and the grid's own width-derived column count is still what is shown
(`gtkgridview.c:620-638`, `:777-783`). The pool drops from 2049 to `2*10 + 30*10 + 1 = 321` cells,
and the focus run creates 324 AT-SPI contexts instead of 2052. The handler holds the grid weakly
(`grid_view.downgrade()`): a grid keeps a strong ref to its adjustment (`gtklistbase.c:753-757`),
so a strong capture here would be the P1b review item 2 cycle again and would leak every closed
tab's grid.

The three rows rerun on the delivered fix are the "after" column above: startup 438-444 ms,
timer navigation 429-442 ms, focus + selection + `activate` 462-469 ms. The 2.0 s is now 0.46 s,
and navigation is no longer slower than startup. `incremental` stays off, as in r1.

**Remaining / what Phase 2 must do.** The width-derived cap is now part of the code and must be
kept if the grid is rewritten. Even at 10 columns every pooled cell still gets an AT-SPI object
when the grid is focused; at 324 cells the measured cost is about 10-20 ms (focus-only
448-452 ms vs plain 429-442 ms), inside the batch noise, so it no longer explains a user-visible
delay. If a larger pool ever comes back, the real fix is GTK-side lazy realisation of child
contexts (realize in `GetChildren` instead of on every `child_change`), or a custom accessible
that presents only the visible range. Disabling a11y (`GTK_A11Y=none`) or setting the grid's role
to `NONE`/`PRESENTATION` would remove the whole list from screen readers and is not an acceptable
app-side fix.

### Gate commands (RULES.md "Done means")

```text
$ PATH="$T:$PATH" cargo fmt --check
exit=0
$ PATH="$T:$PATH" cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s
exit=0
$ cargo test
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-b2015085d72c48fe)
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-29a8f160103fcf7b)
running 12 tests
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-f6eea3612da1734e)
running 5 tests
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
exit=0
$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.02s
exit=0
$ wc -l src/listing.rs
  304 src/listing.rs
$ grep -nE 'remove|delete|trash' src/listing.rs ; echo $?
1                                   # no matches
```

(The 12 `look-body` and 5 `look-header` tests belong to other agents' bins; the 4 in `main.rs`
include this module's `sorts_folders_first_then_names`.)

A rerun after the session's OS restart also prints rustc's `corrupt incremental compilation
artifact` warning from `target/debug/incremental/`; all four exits are still 0 and no lint is
reported. That is host cache damage from the restart, not this module.

### New API citations

Delivered code, gtk4-rs 0.11.5 / glib 0.22.10:
`auto/scrolled_window.rs:64` `hadjustment() -> Adjustment`; `auto/adjustment.rs:177`
`page_size()`; `auto/grid_view.rs:89` `max_columns()`, `:164` `set_max_columns(u32)`;
`glib-0.22.10:src/object.rs:2199` `connect_notify_local` (the `notify::page-size` handler).

Temporary measurement code only (removed): `auto/widget.rs:857` `grab_focus()`;
`auto/selection_model.rs:70` `select_item(position, unselect_rest)`;
`glib-0.22.10:src/object.rs:2909` `emit_by_name`;
`glib-0.22.10:src/source.rs:642` `timeout_add_seconds_local_once`.

GTK source citations are inline above; also checked `gtklistbase.c:2327-2345` (pool formula),
`gtkgridview.c:1236-1238`/`:1401-1403` (GridView passes `max_columns * 30`, `max_columns`),
`gtkaccessible.c:1445-1473`, `gtkwidget.c:6363-6366`, `gtkatcontext.c:1699-1707`,
`gtkatspicontext.c:712-738`, `:1320-1355`, `:1574-1633`.

### Not verified without a live session (Claude)

- The real pointer double-click from `/tmp` to the 100k folder, and the same remeasure at the
  1220x1326 live window. My runs are tiled at 1272x1398 rather than the live floating geometry;
  both give cap 10 and nine laid-out columns, but the live numbers are Claude's to retake.
- Whether the first frame of a newly shown tab can ever draw one column before the
  `page-size` notify raises `max_columns` (the initial value is 1). It was not visible in the
  logs, but only a screenshot can settle it.
- That the focus ring, keyboard navigation (arrows/Tab) and the `--select` scroll still behave
  after the pool is rebuilt when `max_columns` changes on a resize.
- The earlier live checks that were already outstanding: drag out of the grid, the closed-tab
  VmRSS comparison, and narrowing to the 2-column minimum.

**One environment note for the live check:** the host restarted while this round was running and
its `/tmp` was wiped, so `/tmp/gimbal-spike-100k` (and my run logs) no longer exist. Regenerate
the folder with `tools/make-big-dir.sh 100000` before the live navigation measurement. This does
not affect the source changes; the log excerpts above were captured before the restart.

### Anything I'd change outside my files

- `src/bin/look-body/grid.rs` already derives its cap from the viewport
  (`max_columns()` at `:24-45`, `update_cap()` at `:38-45`, wired at `:121`), so it has the same
  protection; P1j-r1 must keep it.
- The remaining per-cell AT-SPI registration is a GTK performance issue, not a listing one: in
  `gtk_at_spi_context_child_change()` a child context is realized on every add
  (`gtkatspicontext.c:1350`), while the AT-SPI protocol can fetch children lazily through
  `GetChildren` (`:712-738`). This is worth an upstream report if Phase 2 keeps large pools.
- No `Cargo.toml` change and no new feature was needed; all APIs are already behind the pinned
  `gtk4`/`libadwaita` features.

## Review of P1e-r2 (Claude, Nov 1, 2026)

**Verdict: accepted on code; the live remeasure follows.** Only `listing.rs` (304 lines) and this
file changed; the dnd hook calls are unchanged; the four RULES commands pass on the combined tree
(21 tests, clippy clean after clearing the incremental cache the 07:15 reboot left behind).

1. The crash fix is right, and the audit went further than asked: cloning the `on_navigate`
   callback out before calling it removes the same trap from `navigate()`.
2. Excellent diagnosis. Isolating focus as the whole 1.4 s, then counting AT-SPI contexts
   (0, 2,052, 324) is the evidence the brief asked for. Checked: `gtkatcontext.c:1699-1707`
   (no child change while the parent context is unrealized), `a11y/gtkatspicontext.c:1320-1355`
   (realize on every added child), `gtklistfactorywidget.c:433-434` (a click grabs focus),
   `gtkwidget.c:6363-6366` (parenting updates the accessible children). The width-derived cap
   with a weak grid ref is the right fix, and `page_size / CELL_WIDTH + 1` can't bind below the
   grid's own column count because a cell is never narrower than `CELL_WIDTH`.
3. Not dropping a11y to win the number was the right call. Phase 2 note: the plan should keep
   the width-derived cap as a rule for every list or grid view, and an upstream GTK report on
   realizing child contexts eagerly is worth filing.

### Live check of P1e-r2 (Claude, Nov 1, 2026): passes

1220×1326 floating window on DP-2, `/tmp/gimbal-spike-100k` regenerated with
`make-big-dir.sh 100000` after the 07:15 reboot, `WAYLAND_DEBUG` off for timings.

- **Navigation by a real double-click** from `/tmp`: **396, 389, 431 ms** (2,015-2,149 ms before
  r2). It opens at `sub-00` with 9 columns; no one-column first frame was visible in `/tmp` or the
  100k folder.
- **Up works:** 100k → `/tmp` → `/`, twice over, no panic; the process stayed up. Peak RSS with one
  tab: 233 MB.
- **Scrolling got better, not worse,** with the smaller pool: the same 3.3 s scrollbar drag through
  99k items gave **201 frames, longest gap 34 ms, median 15 ms** (`WAYLAND_DEBUG` on for this run
  only), against 62 frames and a 65 ms longest gap in the first live check.
- Note: the very first `/tmp` load after the reboot took 745 ms for 35 items, then 76-94 ms on
  every later load: a cold cache (icon theme, file types), not the listing.

P1e is done for Phase 1.
