# P1f notes: colors that follow Noctalia

## What I built

`src/theme.rs` (99 lines), `init(app)` unchanged signature, called from `startup`:

- Resolves the CSS file: `GIMBAL_SPIKE_THEME_CSS` if set (**test-only**, points at a copy under
  `/tmp`), else `~/.config/gtk-4.0/noctalia.css`.
- Loads it once into one `gtk::CssProvider` added for the default display at
  `gtk::STYLE_PROVIDER_PRIORITY_USER`, and parses its `@import url("…")` lines (one level) to
  find the files to also watch — for Noctalia that is `noctalia-extensions.css`.
- Watches the file and its imports with `gio::File::monitor_file`; on any change event it
  (re)schedules a 200 ms debounced reload of the provider, so Noctalia's multi-file writes cause
  one reload. Every watch, change event, reload and parse error is logged via `crate::log!`.

Two behaviours matter and are intentional:

- In the default case our provider duplicates what GTK already loaded from
  `~/.config/gtk-4.0/gtk.css` (which is just `@import url("noctalia.css");` on Kyle's machine).
  Identical definitions at the same priority, so the duplicate is inert; it exists so there is
  something to reload on change. We load `noctalia.css` itself, not `gtk.css`, so only Noctalia's
  colors are in our provider.
- A dropped `gio::FileMonitor` cancels itself (observed in the first test run: no events at all),
  so the monitors are kept alive in a leaked `Vec` for the process lifetime. `init` never returns
  anything to hold them from, and this is a throwaway spike.

## The three questions

### 1. Does GTK 4.22.4 load `~/.config/gtk-4.0/gtk.css`, and does it reload it?

Yes, once per process, at startup only. `settings_init_style` builds
`$XDG_CONFIG_HOME/gtk-4.0/gtk.css` and, if it is a regular file, calls
`gtk_css_provider_load_from_path` — `gtk-4.22.4/gtk/gtksettings.c:1147-1154`. The provider is a
`static` shared across displays and is added to the display's style cascade at
`GTK_STYLE_PROVIDER_PRIORITY_USER` — `gtksettings.c:1159-1161`. There is no file monitor anywhere
in `gtksettings.c` or `gtkcssprovider.c` (grepped for `monitor` — no hits), so a later rewrite of
`gtk.css`/`noctalia.css` is invisible until restart. The docs say the same: the user file is read
"when GTK is initialized" — `gtk-4.22.4/gtk/gtkcssprovider.c:71-73`. [read]

### 2. Does libadwaita 1.9.1 honor `@define-color` named colors?

Yes — and it is a hybrid. The stylesheet's `:root` block publishes CSS custom properties
(`--accent-bg-color` …) that libadwaita's own rules consume, but every one of them is seeded from
a named color: `--accent-bg-color: @accent_bg_color;` —
`libadwaita-1.9.1/src/stylesheet/_colors.scss:172`, same pattern for window/view/headerbar/
sidebar/secondary-sidebar/card/dialog/popover/thumbnail/shade and the destructive/success/
warning/error sets (`_colors.scss:167-247`). Named-color references resolve through the style
cascade taking the highest-priority provider that defines the name —
`gtk-4.22.4/gtk/gtkcsscolorvalue.c:747`, `gtk-4.22.4/gtk/gtkstylecascade.c:129-148` — and
libadwaita's stylesheet provider sits at `GTK_STYLE_PROVIDER_PRIORITY_THEME`
(`libadwaita-1.9.1/src/adw-style-manager.c:505-511`), below our USER provider. So Noctalia's
`@define-color` overrides do flow into libadwaita's custom properties. [read]

Caveats:

- GTK 4.22 deprecates named colors: parsing `@define-color` or an `@name` reference emits a
  "deprecated" warning — `gtk-4.22.4/gtk/gtkcsscolorvalue.c:1996`. Custom properties are the
  future direction; Noctalia already writes both forms. [read]
- Which of Noctalia's names **do not** take effect (grep of every `@…` reference in the 1.9.1
  stylesheet, `src/stylesheet/*.scss` + `widgets/*.scss`): [read]
  - `overview_bg_color`, `overview_fg_color` — never referenced; libadwaita hardcodes
    `--overview-bg-color`/`--overview-fg-color` instead (`_colors.scss:247-248` light,
    `256-257` dark). Noctalia's overview colors are inert.
  - `accent_color` — libadwaita defines it itself: `--accent-color` is derived from
    `--accent-bg-color` via oklab at `_colors.scss:25` and published in `:root`, so Noctalia's
    `@define-color accent_color` overrides a real colour rather than an unused name. Our look spec
    uses `@accent_color`, which therefore resolves to Noctalia's value.
  - Everything else Noctalia defines (`accent_bg_color`, `accent_fg_color`, `destructive_*`,
    `error_*`, `window_*`, `view_*`, `headerbar_*`, `popover_*`, `card_*`, `dialog_*`,
    `sidebar_*`, `secondary_sidebar_*`, `theme_selected_*`, `theme_unfocused_*`) is either
    referenced by the stylesheet or by GTK's own fallback theme and takes effect.
  - Per the brief, the spike does not translate any of them; the two inert names are input for
    the custom theme later.

### 3. Does libadwaita follow `color-scheme` changes live?

Yes. `AdwSettings` registers a settings-portal impl (`org.freedesktop.appearance`) whose
`changed_cb` fires on scheme changes and emits `color-scheme-changed` —
`libadwaita-1.9.1/src/adw-settings-impl-portal.c:144,359`, wired in `adw-settings.c:266-273` —
and falls back to watching GSettings `org.gnome.desktop.interface color-scheme` when the portal
has no answer (`adw-settings.c:277-287`, `adw-settings-impl-gsettings.c:133-141`). StyleManager
then flips `prefers-color-scheme` on its stylesheet provider and `gtk-interface-color-scheme` on
GtkSettings (`adw-style-manager.c:271-294`), and GtkSettings binds that setting to the user
css provider's `prefers-color-scheme`, so `@media (prefers-color-scheme: dark)` blocks toggle
live (`gtk-4.22.4/gtk/gtksettings.c:1120,1137`). [read]

Consequence for Noctalia: libadwaita's own dark/light switch is live without a restart, but
Noctalia encodes the chosen palette as a **static** file (`noctalia.css` has no `@media` blocks —
[observed] from Kyle's copy), rewritten by Noctalia when the scheme or wallpaper changes. Since
GTK never reloads that file (Q1), the user-provided colors only update because our `init`
watches and reloads it.

## API citations (pinned crates)

- `gio-0.22.10/src/auto/file.rs:1153` — `FileExt::monitor_file(flags, cancellable)`.
- `gio-0.22.10/src/auto/file_monitor.rs:74` — `FileMonitor::connect_changed` gives
  `(monitor, file, other_file, event)`.
- `gtk4-0.11.5/src/auto/css_provider.rs:69` — `CssProvider::load_from_file` (returns `()`;
  errors go to the `parsing-error` signal, which we log).
- `gtk4-0.11.5/src/functions.rs:268` — `gtk::style_context_add_provider_for_display`.
- `gtk4-0.11.5/src/lib.rs:43` — `STYLE_PROVIDER_PRIORITY_USER`.
- `gtk4-0.11.5/src/auto/css_section.rs:101` — `CssSection: Display` for the parse-error log.

No new dependencies, no features beyond the existing `v4_22`/`v1_9`. [observed]

## What I verified

Reload test, entirely on a copy under `/tmp/gimbal-spike-theme/` (created by me; left in place so
the live check can reuse it). Noctalia's real files were never touched.

```
$ cp ~/.config/gtk-4.0/noctalia.css ~/.config/gtk-4.0/noctalia-extensions.css /tmp/gimbal-spike-theme/
$ cd /tmp/gimbal-spike-theme
$ GIMBAL_SPIKE_LOG=1 GIMBAL_SPIKE_THEME_CSS=/tmp/gimbal-spike-theme/noctalia.css \
    $SPIKE/target/release/gimbal-spike > run.log 2>&1 &
$ sed -i 's/window_bg_color #180637/window_bg_color #003b1e/' noctalia.css   # rename+replace
$ printf '/* ext change */\n' >> noctalia-extensions.css
$ for n in 1 2 3; do printf '@define-color card_bg_color #290f5%s;\n' $n >> noctalia.css; sleep 0.05; done
```

Log (abridged, timestamps are seconds since process start):

```
[gimbal-spike 0.066] theme: reloaded file:///tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 0.066] theme: watching /tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 0.066] theme: watching /tmp/gimbal-spike-theme/noctalia-extensions.css
[gimbal-spike 3.092] theme: change Deleted on file:///tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 3.092] theme: change Created on file:///tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 3.092] theme: change ChangesDoneHint on file:///tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 3.292] theme: reloaded file:///tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 11.384] theme: change Changed on file:///tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 11.385] theme: change Changed on file:///tmp/gimbal-spike-theme/noctalia-extensions.css
[gimbal-spike 11.485] theme: change Changed on file:///tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 11.585] theme: change Changed on file:///tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 11.785] theme: reloaded file:///tmp/gimbal-spike-theme/noctalia.css
```

One reload per change burst, including when both files change together; the three 50 ms-apart
writes collapse into the single 11.785 reload (200 ms after the last event). [observed]

## RULES.md gate (all from `$SPIKE`)

```
$ PATH="$T:$PATH" cargo fmt --check
(no output — pass)

$ PATH="$T:$PATH" cargo clippy --all-targets -- -D warnings
    Checking gimbal-spike v0.0.0 (…/spike)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s

$ cargo test
test result: ok. 4 passed; 0 failed; … (bin)
test result: ok. 3 passed; 0 failed; … (lib)

$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.61s
```

(Note: `cargo test` shows 4 bin tests because other agents' modules add tests; P1b's CLI tests
all pass.)

## Not verifiable without a live session (Claude + Kyle check)

- Whether colors visibly update when the wallpaper/scheme changes in Noctalia, and whether it is
  all colors or only some. Prediction [inferred]: accent/window/view/sidebar/etc. update within
  ~200 ms of Noctalia rewriting `noctalia.css`; the overview colors will not (Q2); switching
  light↔dark changes libadwaita's chrome instantly (Q3) and the Noctalia palette when the file
  rewrite lands.
- Whether Noctalia rewrites the files atomically enough that our 200 ms debounce always reads a
  complete file. If a torn read ever produces a parse error, it is logged and the next event
  reloads again.
- GTK theme-name/scheme switching (adw-gtk3) was not exercised; `GTK_THEME` is unset here.

## Things I'd change outside my files (not done)

- `spike/README.md` documents `GIMBAL_SPIKE_THEME_CSS` as "reserved for the P1f theme test; not
  read by this skeleton" — now it is read; the README line could drop "not read by this
  skeleton". Per rules I did not edit it.

## Review (Claude, Nov 1, 2026)

**Verdict: needs one fix (`P1f-r1-fixes.md`).** The three answers are right and well cited; one
bug in the reload.

Checked: only `theme.rs` and this file changed; `init`'s signature matches `P1b-contract.rs`;
the four RULES commands pass; `~/.config/gtk-4.0/` untouched (newest mtime Oct 29). Citations
checked: `gtksettings.c:1147-1161`, `gtkcsscolorvalue.c:1996` (the deprecation call),
`_colors.scss:172`, `adw-style-manager.c:505-511`.

1. **Bug: a change to an imported file replaces the colours with that file.** The monitor loop
   gives each watched path its own `target`, and the debounced callback calls
   `reload(&provider, &target)`. If Noctalia rewrites only `noctalia-extensions.css`, the provider
   is loaded with the extensions file alone, and every Noctalia colour in our provider is gone
   until `noctalia.css` changes again. Your test passed only because the last event in each burst
   was on `noctalia.css`. Always reload the main file (`path`), whichever file changed. Add a test
   run that changes only the extensions copy and shows a reload of `noctalia.css` in the log.
2. Note, no change needed: `accent_color` is defined by libadwaita itself
   (`_colors.scss:25`, derived from `accent_bg_color` in oklab), so Noctalia's definition
   overrides a real colour rather than an unused name. Our look spec uses `@accent_color`, which
   therefore resolves to Noctalia's value. Say this in the notes instead of "never referenced".
3. Finding for Phase 2, no change now: GTK 4.22 marks `@define-color` and `@name` references as
   deprecated (`gtkcsscolorvalue.c:1996`). The warning is silent by default (the P1i/P1j runs
   printed nothing), but the look spec relies on named colours. Phase 2 should be able to switch
   to libadwaita's `var(--accent-bg-color)` properties, which Noctalia also writes.

## P1f-r1 changes

Correction 1 (the reload bug): every monitor used to carry its own watched path as the reload
target, so a change to `noctalia-extensions.css` alone loaded that file alone into the provider
and dropped every colour defined only in `noctalia.css`. In `src/theme.rs` the debounced callback
now always reloads the main file (`file` for `path` / `GIMBAL_SPIKE_THEME_CSS`); the changed file's
URI is still what the change log line reports. `init`'s signature is unchanged and the file is 102
lines. New API use: `FileExt::uri` — `gio-0.22.10/src/auto/file.rs:771`. [read]

Item 2 (note only): the `accent_color` bullet in question 2 now says that libadwaita defines
`accent_color` itself (`_colors.scss:25`, from `accent_bg_color` in oklab), so Noctalia's
definition overrides a real colour and `@accent_color` resolves to Noctalia's value. No code
change.

Both runs used fresh copies under `/tmp/gimbal-spike-theme/`, each binary stopped by PID with
SIGTERM within 3 s; no real Noctalia file was touched.

Run A — only the extensions copy changes; the reload is of the main copy:

```
$ cp ~/.config/gtk-4.0/noctalia.css ~/.config/gtk-4.0/noctalia-extensions.css /tmp/gimbal-spike-theme/
$ GIMBAL_SPIKE_LOG=1 GIMBAL_SPIKE_THEME_CSS=/tmp/gimbal-spike-theme/noctalia.css \
    $SPIKE/target/release/gimbal-spike > a.log 2>&1 &
$ sleep 1.5; printf '/* ext only change A */\n' >> /tmp/gimbal-spike-theme/noctalia-extensions.css
$ sleep 1.2; kill -TERM <pid>
```

```
[gimbal-spike 0.163] theme: reloaded file:///tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 0.164] theme: watching /tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 0.164] theme: watching /tmp/gimbal-spike-theme/noctalia-extensions.css
[gimbal-spike 0.171] window: window created
[gimbal-spike 0.172] listing: folder load started: /home/kyle
[gimbal-spike 0.173] window: tab opened: /home/kyle
[gimbal-spike 0.275] listing: first items in model: +55 (55 so far)
[gimbal-spike 0.279] listing: folder load finished: /home/kyle (55 items, 108 ms)
[gimbal-spike 0.286] listing: cell width min/nat short 128/128, long 128/128, equal true
[gimbal-spike 1.474] theme: change Changed on file:///tmp/gimbal-spike-theme/noctalia-extensions.css
[gimbal-spike 1.474] theme: change ChangesDoneHint on file:///tmp/gimbal-spike-theme/noctalia-extensions.css
[gimbal-spike 1.675] theme: reloaded file:///tmp/gimbal-spike-theme/noctalia.css
```

One change on the extensions copy, one reload — of `noctalia.css`. [observed]

Run B — a burst touching both files (extensions, main, extensions, 50 ms apart):

```
$ sleep 1.5; printf '/* ext change B1 */\n' >> /tmp/gimbal-spike-theme/noctalia-extensions.css
$ sleep 0.05; printf '/* main change B1 */\n' >> /tmp/gimbal-spike-theme/noctalia.css
$ sleep 0.05; printf '/* ext change B2 */\n' >> /tmp/gimbal-spike-theme/noctalia-extensions.css
$ sleep 1.2; kill -TERM <pid>
```

```
[gimbal-spike 0.161] theme: reloaded file:///tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 0.162] theme: watching /tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 0.162] theme: watching /tmp/gimbal-spike-theme/noctalia-extensions.css
[gimbal-spike 0.169] window: window created
[gimbal-spike 0.169] listing: folder load started: /home/kyle
[gimbal-spike 0.170] window: tab opened: /home/kyle
[gimbal-spike 0.272] listing: first items in model: +55 (55 so far)
[gimbal-spike 0.275] listing: folder load finished: /home/kyle (55 items, 106 ms)
[gimbal-spike 0.278] listing: cell width min/nat short 128/128, long 128/128, equal true
[gimbal-spike 1.478] theme: change Changed on file:///tmp/gimbal-spike-theme/noctalia-extensions.css
[gimbal-spike 1.478] theme: change ChangesDoneHint on file:///tmp/gimbal-spike-theme/noctalia-extensions.css
[gimbal-spike 1.578] theme: change Changed on file:///tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 1.578] theme: change ChangesDoneHint on file:///tmp/gimbal-spike-theme/noctalia.css
[gimbal-spike 1.678] theme: change Changed on file:///tmp/gimbal-spike-theme/noctalia-extensions.css
[gimbal-spike 1.679] theme: change ChangesDoneHint on file:///tmp/gimbal-spike-theme/noctalia-extensions.css
[gimbal-spike 1.880] theme: reloaded file:///tmp/gimbal-spike-theme/noctalia.css
```

Six change events across both files collapse into one reload, 200 ms after the last event, and it
is again a reload of the main copy. [observed]

## RULES.md gate after P1f-r1 (all from `$SPIKE`)

```
$ PATH="$T:$PATH" cargo fmt --check
(no output — pass)

$ PATH="$T:$PATH" cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.10s

$ cargo test
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-b2015085d72c48fe)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-29a8f160103fcf7b)
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-f6eea3612da1734e)
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.06s
```

## Review of P1f-r1 (Claude, Nov 1, 2026)

**Verdict: accepted.** The reload always targets the main file; run A (extensions only) and run B
(a burst on both files) each end in one reload of `noctalia.css`. The `accent_color` bullet now
says what libadwaita does. `init`'s signature is unchanged, `theme.rs` is 102 lines, and the four
RULES commands pass on the combined tree.
