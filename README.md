# Gimbal Files

Gimbal Files is a Linux file manager for Wayland, written in Rust with GTK 4
and libadwaita. It has tabs and a split view, a keyboard-driven command
registry, file operations with undo, previews, drag and drop, drive mounting,
and a deliberately quiet, transparent look.

## Install with apt

This is the main install for Debian-family systems on x86_64 (amd64). Updates
then come through apt.

### One command

Paste this into a terminal and type your password when `sudo` asks:

```sh
curl -fsSL https://kcanady21.github.io/gimbal-files/apt/setup.sh | bash
```

It adds the signed repository and installs `gimbal-files`, doing exactly the
manual steps below. To read the script before running it, open
<https://kcanady21.github.io/gimbal-files/apt/setup.sh>.

### Or set it up by hand

1. Download the archive keyring:

   ```sh
   sudo install -d -m 0755 /etc/apt/keyrings
   curl -fsSL https://kcanady21.github.io/gimbal-files/apt/gimbal-files-archive-keyring.gpg \
     | sudo tee /etc/apt/keyrings/gimbal-files.gpg >/dev/null
   sudo chmod 0644 /etc/apt/keyrings/gimbal-files.gpg
   ```

2. Add the repository. Write this deb822-style file to
   `/etc/apt/sources.list.d/gimbal-files.sources`:

   ```sh
   sudo tee /etc/apt/sources.list.d/gimbal-files.sources >/dev/null <<'EOF'
   Types: deb
   URIs: https://kcanady21.github.io/gimbal-files/apt
   Suites: stable
   Components: main
   Architectures: amd64
   Signed-By: /etc/apt/keyrings/gimbal-files.gpg
   EOF
   ```

3. Install it:

   ```sh
   sudo apt update
   sudo apt install gimbal-files
   ```

`Recommends: baobab` pulls in GNOME Disk Usage Analyzer, which **Disk Usage**
in the folder menu needs.

### Updates and removal

Update everything, including Gimbal Files, with:

```sh
sudo apt upgrade
```

On GimbalOS, `gimbal` runs that for you.

Remove the package with:

```sh
sudo apt remove gimbal-files
```

To stop updates as well, remove the two setup files:

```sh
sudo rm /etc/apt/sources.list.d/gimbal-files.sources \
  /etc/apt/keyrings/gimbal-files.gpg
sudo apt update
```

Or do both in one command:

```sh
curl -fsSL https://kcanady21.github.io/gimbal-files/apt/setup.sh | bash -s -- --uninstall
```

Your configuration and session are left alone.

## Other distributions

Releases also include a signed prebuilt tarball for x86_64 Linux, for Fedora,
Arch and other non-Debian distributions. It installs for the current user
under `~/.local`. Download one, check both signatures with the committed
public key, then run its installer. The check needs `minisign`
(`sudo apt install minisign` on Ubuntu):

```sh
VERSION=0.1.1
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

A per-user install in `~/.local/bin` usually takes precedence over `/usr/bin`
on `PATH`, so don't use both the apt package and the tarball at the same time.
Remove one before installing the other.

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

## What it needs

- Linux with a Wayland compositor.
- **GTK 4.22 or newer** and **libadwaita 1.9 or newer**. The workspace pins
  the `v4_22` and `v1_9` features in `Cargo.toml`, which set the minimums.
- **Rust 1.92 or newer** to build it (`rust-version` in `Cargo.toml`).

Ubuntu 26.04 ships this stack. Older distributions may not. The apt package
pulls in the runtime libraries it needs; the source build needs the
development packages listed above.

Optional: **Baobab** (GNOME Disk Usage Analyzer) for **Disk Usage** in the
folder menu. The apt package recommends it. With the tarball, the row is
greyed out without it. When it is missing, `install.sh` offers to install it
with your distribution's package manager (that step asks for your password;
Gimbal's own install never does). `--no-prompt` skips the question and only
prints the command.

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

"Show in folder" from browsers, the desktop portal and many other apps does
not use that MIME default. It calls the separate
`org.freedesktop.FileManager1` desktop service. Gimbal ships an activation
file for that service, but it only answers while its Settings switch,
**Open "Show in folder" in Gimbal**, is on. The switch also applies while
Gimbal is already running, without a restart. With other file managers
installed, the one the desktop starts while none is running can vary, but a
running Gimbal with the switch on always takes over.

## Uninstall a tarball or source install

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
- Update check (last check time and the last version announced):
  `$XDG_STATE_HOME/gimbal-files/update.toml`, or
  `~/.local/state/gimbal-files/update.toml`.
- Install marker (tarball and source installs):
  `$XDG_DATA_HOME/gimbal-files/install.toml`, or
  `~/.local/share/gimbal-files/install.toml`; under `--prefix DIR` it is
  `DIR/share/gimbal-files/install.toml`.

The apt package does not write an install marker.

## Updates

Gimbal Files checks GitHub once a day for a newer release. It only tells you;
it never downloads or installs anything. The apt package updates with
`sudo apt upgrade` (or `gimbal` on GimbalOS), and tarball users download the new
tarball. Turn the daily check off in Settings with **Check for updates**.

Only a release notice signed with Gimbal's minisign key is believed, so a
spoofed or replayed notice cannot make Gimbal report a false update. A
development build run from `target/` never checks for updates; Settings says
so.

## Contributing

Read `CLAUDE.md` for the project rules, the toolchain, the gates and the
notes-file format. `scripts/check.sh` runs them all.

## License

Gimbal Files is free software under the GNU General Public License, version 3
or later. See `LICENSE`.
