# Gimbal Files

Gimbal Files is a Linux file manager for Wayland, written in Rust with GTK 4
and libadwaita. It has tabs and a split view, a keyboard-driven command
registry, file operations with undo, previews, drag and drop, drive mounting,
and a deliberately quiet, transparent look.

## What it needs

- Linux with a Wayland compositor.
- **GTK 4.22 or newer** and **libadwaita 1.9 or newer**. The workspace pins
  the `v4_22` and `v1_9` features in `Cargo.toml`, which set the minimums.
- **Rust 1.92 or newer** to build it (`rust-version` in `Cargo.toml`).

Ubuntu 26.04 ships this stack. Older distributions may not.

## Install from a release

Releases are signed prebuilt tarballs for x86_64 Linux. Download one, check
both signatures with the committed public key, then run its installer. The
check needs `minisign` (`sudo apt install minisign` on Ubuntu):

```sh
VERSION=0.1.0
BASE="https://github.com/Kcanady21/gimbal-files/releases/download/v${VERSION}"

curl -LO "${BASE}/gimbal-files-${VERSION}-x86_64-linux.tar.gz"
curl -LO "${BASE}/gimbal-files-${VERSION}-x86_64-linux.tar.gz.minisig"
curl -LO "${BASE}/latest.toml"
curl -LO "${BASE}/latest.toml.minisig"
curl -LO "https://raw.githubusercontent.com/Kcanady21/gimbal-files/main/packaging/minisign.pub"

minisign -V -p minisign.pub -m latest.toml
minisign -V -p minisign.pub -m "gimbal-files-${VERSION}-x86_64-linux.tar.gz"

tar xzf "gimbal-files-${VERSION}-x86_64-linux.tar.gz"
cd "gimbal-files-${VERSION}-x86_64-linux"
./install.sh
```

Checking `latest.toml` as well as the tarball is what stops an older, validly
signed release from being replayed as an update. While the repository is
private, the download URLs need authentication; use `gh release download` from
an authenticated `gh` session instead of `curl`.

The installer needs no password. It writes the two binaries to
`~/.local/bin`, the desktop entry to `~/.local/share/applications`, the icon
to `~/.local/share/icons/hicolor/scalable/apps`, and an install marker to
`$XDG_DATA_HOME/gimbal-files/install.toml` (by default
`~/.local/share/gimbal-files/install.toml`). If `~/.local/bin` is not on your
`PATH`, it tells you; add it to your shell's `PATH` to run `gimbal-files` from
a terminal.

## Install from source

With the GTK, libadwaita and Wayland development files installed (on Ubuntu:
`sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev
libwayland-dev`), build and install:

```sh
cargo build --release
scripts/install.sh
```

`scripts/install.sh` never runs cargo. It fails with a clear message if
`target/release/` has not been built. It also runs from an unpacked release
tarball, where it uses the binaries beside it.

## Transparent window on niri

Gimbal asks the compositor to blur what is behind its window. niri draws a
solid focus-ring block behind every window that draws its own decorations,
and GTK 4 windows always do. Without this rule the block shows through the
transparent background as an opaque rectangle around the window. Add it to
`~/.config/niri/config.kdl`:

```kdl
window-rule {
    match app-id="^io\\.github\\.kcanady21\\.GimbalFiles$"
    draw-border-with-background false
}
```

niri reloads its configuration automatically.

## Make it the default file manager

Gimbal's installer never changes your default applications. If you want it:

```sh
xdg-mime default io.github.kcanady21.GimbalFiles.desktop inode/directory
```

This covers applications that ask the XDG MIME database for the handler of
`inode/directory`.

## Uninstall

From a checkout:

```sh
scripts/install.sh --uninstall
```

From an unpacked release tarball, use `./install.sh --uninstall`. If you
installed with `--prefix DIR`, pass the same `--prefix DIR` when uninstalling.
The uninstaller removes exactly the files the installer wrote. It never
touches your configuration or session.

## Where your settings live

- Configuration: `$XDG_CONFIG_HOME/gimbal-files/config.toml`, or
  `~/.config/gimbal-files/config.toml`.
- Session (windows, tabs and split layout):
  `$XDG_STATE_HOME/gimbal-files/session.toml`, or
  `~/.local/state/gimbal-files/session.toml`.
- Install marker:
  `$XDG_DATA_HOME/gimbal-files/install.toml`, or
  `~/.local/share/gimbal-files/install.toml`; under `--prefix DIR` it is
  `DIR/share/gimbal-files/install.toml`.

## Updates

Gimbal Files will check GitHub once a day for a newer signed release and
offer **Install** in Settings; only correctly signed releases are installed,
and a development build run from `target/` never updates itself. This check
is not part of the 0.1.0 build yet.

## Contributing

Read `CLAUDE.md` for the project rules, the toolchain, the gates and the
notes-file format. `scripts/check.sh` runs them all.

## License

Gimbal Files is free software under the GNU General Public License, version 3
or later. See `LICENSE`.
