# P2al: an Internal Drives section for fstab-mounted disks

Tags: **[observed]** for a command or its output, **[read]** for source, and
**[inferred]** for anything that needs a real key, pointer or live feel.

Starts from `c3bc8aa` (P2ak) on `main`.

## What I built

- **A pure parser and filter (`fm-core/src/volumes.rs`).** The brief allowed
  a `volumes/internal.rs` module "if `volumes.rs` becomes a module"; it did
  not need to. `volumes.rs` is one concern per section already (raw types,
  the inventory, then the internal-drives block), and the new block is ~250
  lines with its tests, keeping the crate's file count unchanged.
  `internal_drives(mountinfo, devices_roots, by_label)` returns
  `Vec<InternalDrive>`:

  - mountinfo fields are split on whitespace (the kernel octal-escapes every
    space, so this is exact) and fields 4/5/source decode `\040`-style
    escapes (`proc_pid_mountinfo(5)`); mount paths become `PathBuf` from
    bytes, so a non-UTF-8 folder name survives;
  - every rule from the brief: `/dev/…` source; a disk filesystem
    (btrfs, ext2/3/4, xfs, f2fs, ntfs, ntfs3, exfat, vfat, jfs, reiserfs,
    zfs, bcachefs); no system mount point (`/`, `/home`, `/boot`, `/efi`
    exactly, everything under `/boot /efi /usr /var /opt /srv /nix /snap
    /proc /sys /dev /run`, with `/run/media` exempt); not the root
    filesystem's device; no `x-gvfs-hide` in the mount **or** super
    options; not a path Devices already shows; one device at several paths
    keeps the shortest. Rows are sorted by mount path.
  - **One addition of mine:** the mount path must be absolute. Real
    mountinfo always satisfies this; the guard keeps a junk line from
    producing a row whose `nav.go` argument could only be refused anyway.
  - **The root-device test uses the source string *and* the major:minor
    pair.** Kyle's `/` and `/@home` lines carry `/dev/nvme1n1p2` twice —
    string equality alone covers it — but btrfs mounts report one anonymous
    `(0, n)` pair per filesystem, so two entries of the same filesystem
    mounted by *different* names (say `/dev/disk/by-uuid/…` in one fstab
    line and `/dev/nvme1n1p2` in another) still compare equal on `0:37`.

- **Labels.** `/dev/disk/by-label/` is read in `fm-ui`
  (`drive_labels_from`, one default method on the `Backend` trait): each
  symlink's canonical target maps to its label, and when two labels name one
  device the alphabetically first wins, so the map never depends on the
  directory's iteration order. The naming decision — label, else the mount
  folder's name — is `InternalDrive::name()` in `fm-core` and is pure. The
  tooltip is `InternalDrive::tooltip()`: `/mnt/wd-blue — /dev/sda1`. **[read]** for the layout, **[observed]** for Kyle's own map: `WD-Blue-SSD -> ../../sda1`, `Nvme-Storage-1 -> ../../nvme0n1p1`.

- **The section.** `Section::Internal` (`"internal"`) in `config.rs`, saved
  and toggled like the others (default `expanded`; `[sections]` gains
  `internal` when written), listed in `SECTIONS` for `view.toggle-section`,
  and accepted by the sections-table reader. The pill sits **right after
  Devices and before Remote**. The whole section view — pill included — is
  hidden while the list is empty, so a machine without such mounts sees no
  change.

- **Rows.** One `DriveRow` per mount, built like a mounted Devices row:
  click to open, drop target, usage bar with percent and capacity. The
  right-click menu is the mounted menu **without Unmount or Eject** —
  `build_internal_drive_menu` offers exactly Open and Open in new tab. The
  two open actions are now `append_open_actions`, shared with
  `build_mounted_device_menu`, so the two menus cannot drift apart. **[read]** for the wiring; whether the menu *feels* right is Kyle's.

- **Updates.** `Volumes::poll_internal` re-reads the backend's `mountinfo`
  text, re-filters against the **freshly reconciled** inventory (the poll
  runs after `reconcile`, so a mount Devices has just started to show
  retires its Internal Drives row in the same pass — the injected test
  caught my first ordering), and logs one line when the list changes:
  `internal-drives: 2 shown (/mnt/nvme-storage, /mnt/wd-blue)`. It runs on
  every service refresh (the monitor already drives those) and on a
  `glib::timeout_add_local` every `MOUNTINFO_POLL_SECS = 5`, defined beside
  the parser in `fm-core` — a polling period, not an animation, so it stays
  outside `look::motion`, the same as `CHECK_INTERVAL_SECS` in `update.rs`.
  `shutdown` removes the source. The sidebar rebuilds rows only when the
  list signature changes and updates bars in place otherwise, exactly as
  `sync_devices` does.

- **Usage.** No second queue: `UsageKey`'s `owner` became an explicit
  `UsageOwner { Main, Entry(..), Internal }` and internal rows key the
  shared deduplicated queue by mount path (an fstab mount has no backend
  object serial). `usage_key_live` checks the internal list;
  `reconcile_usage` keeps its records, so a mount that leaves the table
  cancels its query. **[read]**

- **Why they were missing** is now documented where the next reader looks:
  the module header of `gio_backend.rs` names GLib's rule and points at the
  new path.

## Binding check

`gio-0.22.10` and `glib-0.22.10` in the vendored registry carry no
`GUnixMountEntry`, no `GUnixMountMonitor` and no `unix_fd_add_local` — a
grep of both `src/` trees returns nothing **[read]**, confirming the brief:
a `mountinfo` text parse is the only route without a new dependency. No
dependency was added; `cargo deny` is unchanged. **[observed]**

## GLib's own rule, for comparison

From `gio/gunixmounts.c`, GLib `gnome-2-86` branch (raw.gnome.org mirror,
fetched Nov 16, 2026): `g_unix_is_mount_path_system_internal` hides a fixed
list of FHS mount points — that list includes `/mnt` and `/media` as exact
names — and prefixes `/dev/ /proc/ /sys/`; `guess_system_internal` also
hides entries whose *root* inside the filesystem is not `/` (bind mounts and
subvolume mounts); and `g_unix_mount_entry_guess_should_display` then shows
only paths under `/media/`, `/run/media/$USER/` or the user's home, hiding
anything containing a `/.` component. **[read]** Kyle's `/mnt/wd-blue` is
not "system internal" by that first rule — it fails the *display* whitelist,
which is why udisks2's monitor reports the drive with no volume or mount.

The two lists differ from ours on purpose: GLib hides exact `/mnt` and `/tmp`
mount points we would show (a data disk mounted *at* `/mnt` is the user's
disk), keeps a subvolume-mount rule we express as the root-device rule
(covering btrfs `subvol=` lines that report `root=/@…`), and knows nothing
of bcachefs or f2fs. The `x-gvfs-hide` respect and the Devices-duplication
rule have no counterpart here; they come from the brief.

## Tests

- `fm-core/src/volumes.rs`, seven new: Kyle's real `/`, `/home`, `/tmp`,
  `/boot/efi` and both `/mnt` lines give **exactly** the two data drives
  (names, devices, fs types, tooltips); each skip rule loses its own mount
  (tmpfs, overlay, `fuse.gvfsd-fuse`, nfs, cifs, the root device reached via
  a `/home/kyle/snap`-style path outside every prefix, `x-gvfs-hide` in
  mount *and* super options, `/boot/efi`, a relative junk line); Devices'
  path and `/run/media` share the dedup rules; one device twice keeps the
  shortest path; `/mnt/My\040Disk` and `\134` decode; labels and the blank
  label; a table without `/` keeps both disks.
- `fm-core/src/config.rs`: `the_internal_section_parses_writes_and_rejects_unknown_keys`
  (parse, in-place write, `[sections]` creation when absent, unknown-key
  problem at the right key and line); the inline-formatting and
  default-sections tests now carry `internal`; `change_for` maps
  `section=internal` to `SetSection { Internal, Collapsed }`.
- `fm-core/src/command/tests.rs`: `section=internal` is accepted and the
  refusal lists `recent, devices, internal, remote`.
- `fm-ui/src/volumes/tests.rs`, three new: injected mountinfo yields rows
  whose usage runs through the shared bounded queue and is shared across
  refreshes; a Devices root retires the row and the poll re-reads the text
  (and an unchanged table is *not* a change); a service with no injected
  text never lists rows and never opens the real `/proc`.
- `fm-ui/src/app.rs`: the subprocess sidebar regression
  `real_sidebar_internal_drives_section_opens_hides_and_orders_inner` checks
  the hidden-when-empty rule, the order (hidden, it still sits third, right
  after Devices), the two rows' names `nvme-storage` + `WD-Blue-SSD` with
  tooltips, bars from the queue, the menu being exactly
  `Open`/`Open in new tab`, Open actually navigating (`recent_dirs` gains the
  mount), and the section hiding again when the table empties.
- UI tests inject the `mountinfo` text exactly like the volume tests inject
  a backend (`Backend::mountinfo`/`drive_labels`, overridden in
  `FakeBackend`/`HeadlessBackend`); no test reads this machine's disks.

One GTK detail the regression caught: a `gtk::ScrolledWindow` wraps a
non-scrollable child in a `GtkViewport` internally, so the test helper walks
descendants rather than `scroll.child()`'s direct children.

## Five gates

**[observed]** `./scripts/check.sh` from the repo root: exit 0, all five
steps pass — `cargo fmt --all --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace`,
`cargo build --workspace --release`, `cargo deny check`.

Totals: **1027 passed, 0 failed, 26 ignored** — P2ak's accepted 1014/25 plus
the thirteen new tests and the one new ignored subprocess inner.

## Live run on Kyle's machine

**[observed]** debug build, scratch XDG dirs under `/tmp/gimbal-p2al`,
`WAYLAND_DISPLAY=wayland-0`, 10 s then SIGTERM:

```
volumes: GIO volume monitor acquired
volumes: internal-drives: 2 shown (/mnt/nvme-storage, /mnt/wd-blue)
volumes: GIO volume monitor GUnionVolumeMonitor; 0 volume(s), 0 mount(s)
```

The line appears once across two 5 s poll cycles — the silent timer and the
changed-only rule both hold on the real table — and the log has no warning
or error lines. Nothing was ever written to the two drives; they were never
even navigated into by me.

## Live checks (Claude and Kyle, debug build, scratch XDG dirs)

**[inferred]** until someone looks at the window. The drives are Kyle's real
ones, so only open and browse, never write.

1. Internal Drives shows **WD-Blue-SSD** and **Nvme-Storage-1** — Kyle's two
   disks both *have* labels (`/dev/disk/by-label` above), so the rows use
   them, not `wd-blue`/`nvme-storage` — right after Devices, with usage bars
   matching `df -h /mnt/wd-blue /mnt/nvme-storage`.
2. Clicking a row opens the drive; right-click offers only Open and Open in
   new tab; dropping a file onto a row moves it like a Devices row.
3. The section collapses and expands from its pill, and stays that way after
   a restart (`[sections] internal` lands in the config on the first toggle).
4. A USB stick still shows only in Devices and never also in Internal
   Drives (plug one in if handy).
5. `/`, `/home`, `/boot/efi` and `/tmp` never appear.
6. Light and dark.
7. Worth a look: the two rows are one name longer than the folder names
   Kyle knows them by. If he prefers the folder names, the label map is one
   line to ignore — but the same rule that names his data disks would name
   any future labelled drive, and labels are the Devices convention.

**For Kyle:** once the section is right, the two hand-pins
(`/mnt/wd-blue`, `/mnt/nvme-storage`) can go — unpin them from the pin's
right-click menu yourself, nothing in this change touches the pin list.

## Files changed

- `crates/fm-core/src/volumes.rs`
- `crates/fm-core/src/config.rs`
- `crates/fm-core/src/command/catalogue.rs`
- `crates/fm-core/src/command/tests.rs`
- `crates/fm-ui/src/volumes/mod.rs`
- `crates/fm-ui/src/volumes/gio_backend.rs` (module doc only)
- `crates/fm-ui/src/volumes/tests.rs`
- `crates/fm-ui/src/sidebar.rs`
- `crates/fm-ui/src/app.rs` (tests)
- `README.md` (one entry under "Where your settings live")
- `docs/notes/P2al.md`

No new dependencies, no new `unsafe`, no new GTK calls beyond what Devices
already uses.

## Claude's review (Nov 16, 2026)

The return matches the brief. The parser and filter are pure and cover every
skip rule, octal escapes and labels; the Devices and Internal Drives menus
share `append_open_actions`, so the open actions cannot drift apart; usage
goes through the shared queue with its own `UsageOwner::Internal`; the
popover lifecycle matches the Devices menu. The only file outside the Output
list is `crates/fm-ui/src/app.rs`, for the subprocess widget test, which is
where the other real-widget tests live. **[read]**

One small limit, not fixed: a label is matched by the mount source, so a disk
mounted through `/dev/mapper/…` (LUKS, LVM) resolves its label to `/dev/dm-N`
and falls back to the folder name. Harmless; worth a line if Kyle ever
encrypts a data disk. **[inferred]**

The agent committed the change itself (89fbddf, on a new local branch
`p2al-internal-drives`, under Kyle's name). Nothing was pushed.

### Gates

`scripts/check.sh`: exit 0; 1027 passed, 26 ignored. **[observed]**

### Live checks (Kyle, Nov 16, scratch XDG dirs, `/tmp/gimbal-p2al-check`)

All passed: Internal Drives sits after Devices with WD-Blue-SSD and
Nvme-Storage-1, usage bars match `df` (73 % and 13 %); clicking opens the
drive and the menu offers only Open and Open in new tab; the section
collapses, expands and is remembered; no system mount appears; light and dark
are fine. **[observed]**
