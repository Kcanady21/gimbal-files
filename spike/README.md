# Gimbal Spike

A throwaway GTK4 + libadwaita prototype for Phase 1 of Gimbal Files. It is not the app: it exists
to test drag and drop, tabs, large folders and Noctalia theming. See
`../Rust File Manager Plan for Claude Code.md` (Phase 1).

## Build and run

```bash
cargo build --release
./target/release/gimbal-spike
./target/release/gimbal-spike /tmp/gimbal-spike-p1b
```

Rust 1.92 or newer, GTK 4.22 and libadwaita 1.9 development packages are required.

## Command line

```
gimbal-spike [--select NAME[,NAME...]] [DIR...]
```

- Each `DIR` opens in its own tab. With no `DIR`, the home folder opens.
- `--select` selects those file names in the first tab once its folder has loaded. It is the drag
  runner's test hook. `--select=b.txt` is also accepted.
- An unknown flag prints the usage and exits with status 2.

## Environment

- `GIMBAL_SPIKE_LOG=1`: print one event per line to stderr as
  `[gimbal-spike <seconds.millis>] <module>: <message>`. Off by default.
- `GIMBAL_SPIKE_THEME_CSS`: reserved for the P1f theme test; not read by this skeleton.

## Modules

| File | Owns |
|---|---|
| `src/main.rs` | application, CLI parsing, `log!` macro |
| `src/window.rs` | window, header bar, tab bar/view, tab actions |
| `src/listing.rs` | one folder view: DirectoryList, sort, selection, ListView |
| `src/dnd.rs` | drag-out and drop-in hooks (stubs until P1c) |
| `src/tabs.rs` | tab transfer and context menu (stub until P1d) |
| `src/theme.rs` | Noctalia theme reload (stub until P1f) |

## Phase 1 hook contract

Later briefs fill these in without changing their signatures:

- `dnd::attach_drag_source(row, list_item, selection)` — row factory `setup`.
- `dnd::attach_drop_target(view, dir)` — once per folder view, just after creation.
- `tabs::setup_tab_view(window, tab_view)` — once per window, after its TabView exists.
- `theme::init(app)` — from the application's `startup` signal.

`window::new_window` and `window::open_dir_in_tab` are also part of that contract for P1d.

## Tests

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Unit tests cover CLI parsing and the folder-first, case-insensitive name sort. There are no GUI
tests; the live checks are in the brief and `notes/P1b.md`.
