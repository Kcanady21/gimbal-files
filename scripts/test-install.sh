#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Manual test for scripts/install.sh. It needs a release build, so
# scripts/check.sh does not run it. Nothing outside a /tmp scratch folder is
# touched.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd -- "$script_dir/.." && pwd -P)"

for binary in gimbal-files gimbal-files-cli; do
    if [[ ! -x "$repo_root/target/release/$binary" ]]; then
        echo "test-install.sh: target/release/$binary is missing; run 'cargo build --workspace --release' first" >&2
        exit 1
    fi
done

scratch="$(mktemp -d /tmp/gimbal-p2u-test.XXXXXX)"
cleanup() {
    rm -rf -- "$scratch"
}
trap cleanup EXIT

home="$scratch/home"
# Spaces, & and parentheses: the launcher's Exec line must still work.
prefix="$scratch/pre fix & (x)"
mkdir -p \
    "$home/.config/gimbal-files" \
    "$home/.local/state/gimbal-files" \
    "$prefix/bin" \
    "$prefix/share/applications" \
    "$prefix/share/dbus-1/services" \
    "$prefix/share/icons/hicolor/scalable/apps" \
    "$prefix/share/gimbal-files"

printf 'keep\n' >"$home/.config/gimbal-files/config.toml"
printf 'keep\n' >"$home/.local/state/gimbal-files/session.toml"
printf 'keep\n' >"$prefix/bin/keep-me"
printf 'keep\n' >"$prefix/share/applications/keep-me.desktop"
printf 'keep\n' >"$prefix/share/dbus-1/services/keep-me.service"
printf 'keep\n' >"$prefix/share/icons/hicolor/scalable/apps/keep-me.svg"
printf 'keep\n' >"$prefix/share/gimbal-files/keep-me"

fail() {
    echo "test-install.sh: FAIL: $*" >&2
    exit 1
}

check_mode() {
    local path="$1"
    local expected="$2"
    [[ -f "$path" ]] || fail "missing $path"
    local actual
    actual="$(stat -c '%a' "$path")"
    [[ "$actual" == "$expected" ]] || fail "$path has mode $actual, expected $expected"
}

run_install() {
    HOME="$home" XDG_DATA_HOME="$scratch/ignored-data" \
        "$repo_root/scripts/install.sh" --prefix "$prefix" --no-prompt
}

run_install

version="$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' "$repo_root/Cargo.toml" | head -n 1)"
[[ -n "$version" ]] || fail "could not read the workspace version"

check_mode "$prefix/bin/gimbal-files" 755
check_mode "$prefix/bin/gimbal-files-cli" 755
check_mode "$prefix/share/applications/io.github.kcanady21.GimbalFiles.desktop" 644
check_mode "$prefix/share/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service" 644
check_mode "$prefix/share/icons/hicolor/scalable/apps/io.github.kcanady21.GimbalFiles.svg" 644
check_mode "$prefix/share/gimbal-files/install.toml" 644

cmp -s "$repo_root/target/release/gimbal-files" "$prefix/bin/gimbal-files" \
    || fail "installed gimbal-files differs from target/release"
cmp -s "$repo_root/target/release/gimbal-files-cli" "$prefix/bin/gimbal-files-cli" \
    || fail "installed gimbal-files-cli differs from target/release"

grep -qx "bin_dir = \"$prefix/bin\"" "$prefix/share/gimbal-files/install.toml" \
    || fail "marker bin_dir is wrong"
grep -qx "prefix = \"$prefix\"" "$prefix/share/gimbal-files/install.toml" \
    || fail "marker prefix is wrong"
grep -qx "version = \"$version\"" "$prefix/share/gimbal-files/install.toml" \
    || fail "marker version is wrong"
grep -qx "data_dir = \"$prefix/share\"" "$prefix/share/gimbal-files/install.toml" \
    || fail "marker data_dir is wrong"
grep -qxF "Exec=\"$prefix/bin/gimbal-files\" %U" \
    "$prefix/share/applications/io.github.kcanady21.GimbalFiles.desktop" \
    || fail "the desktop entry does not run the installed binary by absolute path"
if command -v desktop-file-validate >/dev/null 2>&1; then
    desktop-file-validate "$prefix/share/applications/io.github.kcanady21.GimbalFiles.desktop" \
        || fail "the installed desktop entry does not validate"
fi
grep -qxF "Exec='$prefix/bin/gimbal-files' --gapplication-service" \
    "$prefix/share/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service" \
    || fail "the FileManager1 Exec is not the absolute, quoted binary"
grep -qxF "Name=org.freedesktop.FileManager1" \
    "$prefix/share/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service" \
    || fail "the FileManager1 service name is wrong"

[[ -z "$(find "$prefix" \( -name '.*.tmp.*' -o -name '.*.src.*' \) -print -quit)" ]] \
    || fail "a temporary install file was left behind"

# Reinstall over a damaged copy: the copy-then-mv path must replace it and
# restore the mode.
printf 'stale\n' >"$prefix/bin/gimbal-files"
chmod 0644 "$prefix/bin/gimbal-files"
run_install
check_mode "$prefix/bin/gimbal-files" 755
cmp -s "$repo_root/target/release/gimbal-files" "$prefix/bin/gimbal-files" \
    || fail "reinstall did not replace gimbal-files"

[[ -z "$(find "$prefix" \( -name '.*.tmp.*' -o -name '.*.src.*' \) -print -quit)" ]] \
    || fail "a temporary install file was left behind after reinstall"

HOME="$home" XDG_DATA_HOME="$scratch/ignored-data" \
    "$repo_root/scripts/install.sh" --prefix "$prefix" --uninstall

for gone in \
    "$prefix/bin/gimbal-files" \
    "$prefix/bin/gimbal-files-cli" \
    "$prefix/share/applications/io.github.kcanady21.GimbalFiles.desktop" \
    "$prefix/share/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service" \
    "$prefix/share/icons/hicolor/scalable/apps/io.github.kcanady21.GimbalFiles.svg" \
    "$prefix/share/gimbal-files/install.toml"; do
    [[ ! -e "$gone" ]] || fail "uninstall left $gone"
done

for kept in \
    "$prefix/bin/keep-me" \
    "$prefix/share/applications/keep-me.desktop" \
    "$prefix/share/dbus-1/services/keep-me.service" \
    "$prefix/share/icons/hicolor/scalable/apps/keep-me.svg" \
    "$prefix/share/gimbal-files/keep-me" \
    "$home/.config/gimbal-files/config.toml" \
    "$home/.local/state/gimbal-files/session.toml"; do
    [[ -f "$kept" ]] || fail "uninstall removed $kept"
done

# A missing Baobab is reported with the install command and never runs sudo
# when nobody can answer: neither with --no-prompt nor without a terminal. A
# fake sudo records any call.
fakebin="$scratch/fakebin"
mkdir -p "$fakebin"
printf '#!/bin/sh\ntouch "%s/sudo-called"\nexit 1\n' "$scratch" >"$fakebin/sudo"
chmod 0755 "$fakebin/sudo"
for flags in "--no-prompt" ""; do
    # shellcheck disable=SC2086 # flags is empty or one word on purpose
    out="$(PATH="$fakebin:$PATH" HOME="$home" GIMBAL_INSTALL_PRETEND_MISSING=baobab \
        "$repo_root/scripts/install.sh" --prefix "$prefix" $flags </dev/null 2>&1)"
    grep -q "needs Baobab" <<<"$out" || fail "missing Baobab was not reported (${flags:-no tty})"
    grep -q "baobab" <<<"$(grep -E 'run: |package' <<<"$out")" \
        || fail "no install hint for Baobab (${flags:-no tty})"
    [[ ! -e "$scratch/sudo-called" ]] || fail "sudo ran without a person to answer (${flags:-no tty})"
done
out="$(HOME="$home" "$repo_root/scripts/install.sh" --prefix "$prefix" --no-prompt 2>&1)"
if command -v baobab >/dev/null 2>&1; then
    ! grep -q "needs Baobab" <<<"$out" || fail "Baobab is installed but was reported missing"
fi
HOME="$home" "$repo_root/scripts/install.sh" --prefix "$prefix" --uninstall >/dev/null

# --data-dir keeps the data where an earlier install put it (an updater passes
# the marker's data_dir back), separate from the prefix.
data="$scratch/custom data"
HOME="$home" "$repo_root/scripts/install.sh" --prefix "$prefix" --data-dir "$data"
grep -qx "data_dir = \"$data\"" "$data/gimbal-files/install.toml" \
    || fail "--data-dir marker data_dir is wrong"
[[ -f "$data/applications/io.github.kcanady21.GimbalFiles.desktop" ]] \
    || fail "--data-dir did not place the desktop entry"
[[ -f "$data/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service" ]] \
    || fail "--data-dir did not place the FileManager1 service"
grep -qxF "Exec='$prefix/bin/gimbal-files' --gapplication-service" \
    "$data/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service" \
    || fail "--data-dir FileManager1 Exec is not the absolute, quoted binary"
[[ ! -e "$prefix/share/gimbal-files/install.toml" ]] \
    || fail "--data-dir also wrote a marker under the prefix"
HOME="$home" "$repo_root/scripts/install.sh" --prefix "$prefix" --data-dir "$data" --uninstall
[[ ! -e "$data/gimbal-files/install.toml" && ! -e "$prefix/bin/gimbal-files" ]] \
    || fail "--data-dir uninstall left files"
[[ ! -e "$data/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service" ]] \
    || fail "--data-dir uninstall left the FileManager1 service"

# A single quote or a backslash in the prefix must still give a service file
# dbus-daemon accepts: '\'' is rejected by its desktop-file parser, so the
# quote is written '"'"' and the backslash is doubled.
odd="$scratch/it's a \\ dir"
HOME="$home" "$repo_root/scripts/install.sh" --prefix "$odd" --no-prompt >/dev/null
odd_exec="${odd//\\/\\\\}"
odd_exec="'${odd_exec//\'/\'\"\'\"\'}/bin/gimbal-files' --gapplication-service"
grep -qxF "Exec=$odd_exec" \
    "$odd/share/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service" \
    || fail "FileManager1 Exec is not quoted for a prefix with ' and \\"
if grep -qF "'\\''" "$odd/share/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service"; then
    fail "FileManager1 Exec uses the '\\'' escape dbus-daemon rejects"
fi
HOME="$home" "$repo_root/scripts/install.sh" --prefix "$odd" --uninstall >/dev/null

echo "PASS: install, reinstall and uninstall in $scratch"
