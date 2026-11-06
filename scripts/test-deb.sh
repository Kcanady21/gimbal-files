#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Manual test for scripts/build-deb.sh and the apt repository helpers in it.
# It needs a release build and never uses sudo. Everything happens inside a
# /tmp scratch folder with a throwaway GnuPG home.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd -- "$script_dir/.." && pwd -P)"
# shellcheck source=scripts/build-deb.sh
source "$script_dir/build-deb.sh"

for binary in gimbal-files gimbal-files-cli; do
    if [[ ! -x "$repo_root/target/release/$binary" ]]; then
        echo "test-deb.sh: target/release/$binary is missing; run 'cargo build --workspace --release' first" >&2
        exit 1
    fi
done

scratch="$(mktemp -d /tmp/gimbal-p2w-test.XXXXXX)"
cleanup() {
    rm -rf -- "$scratch"
}
trap cleanup EXIT

fail() {
    echo "test-deb.sh: FAIL: $*" >&2
    exit 1
}

export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(date -u -d '2026-11-05 00:00:00 UTC' +%s)}"
export DEBFULLNAME="${DEBFULLNAME:-Gimbal Files Test}"
export DEBEMAIL="${DEBEMAIL:-gimbal-files-test@example.invalid}"

version="$(workspace_version)"
[[ -n "$version" ]] || fail "could not read the workspace version"
homepage="$(workspace_repository)"
[[ -n "$homepage" ]] || fail "could not read the workspace repository"

echo "==> building the $version .deb"
deb="$(deb_build "$version")"
[[ -f "$deb" ]] || fail "build-deb.sh did not produce $deb"
cp -- "$deb" "$scratch/first.deb"

echo "==> checking the control fields"
field() {
    dpkg-deb --field "$deb" "$1"
}
[[ "$(field Package)" == "gimbal-files" ]] || fail "Package is wrong"
[[ "$(field Version)" == "$version" ]] || fail "Version is wrong"
[[ "$(field Architecture)" == "amd64" ]] || fail "Architecture is wrong"
[[ "$(field Section)" == "utils" ]] || fail "Section is wrong"
[[ "$(field Priority)" == "optional" ]] || fail "Priority is wrong"
[[ "$(field Maintainer)" == "$DEBFULLNAME <$DEBEMAIL>" ]] || fail "Maintainer is wrong"
[[ "$(field Homepage)" == "$homepage" ]] || fail "Homepage is wrong"
[[ -n "$(field Description)" ]] || fail "Description is empty"
[[ "$(field Installed-Size)" =~ ^[0-9]+$ ]] || fail "Installed-Size is not a number"
[[ -n "$(field Depends)" ]] || fail "Depends is empty"
grep -q 'libgtk-4-1' <<<"$(field Depends)" || fail "Depends does not name libgtk-4-1"
grep -q 'libadwaita-1-0' <<<"$(field Depends)" || fail "Depends does not name libadwaita-1-0"
[[ "$(field Recommends)" == "baobab" ]] || fail "Recommends is not baobab"
dpkg-deb --info "$deb" | grep -q '^ Package: gimbal-files$' || fail "dpkg-deb --info does not show the package"

echo "==> checking the exact contents and modes"
expected="$scratch/expected"
actual="$scratch/actual"
cat >"$expected" <<'EOF'
drwxr-xr-x root/root ./
drwxr-xr-x root/root ./usr/
drwxr-xr-x root/root ./usr/bin/
-rwxr-xr-x root/root ./usr/bin/gimbal-files
-rwxr-xr-x root/root ./usr/bin/gimbal-files-cli
drwxr-xr-x root/root ./usr/share/
drwxr-xr-x root/root ./usr/share/applications/
-rw-r--r-- root/root ./usr/share/applications/io.github.kcanady21.GimbalFiles.desktop
drwxr-xr-x root/root ./usr/share/dbus-1/
drwxr-xr-x root/root ./usr/share/dbus-1/services/
-rw-r--r-- root/root ./usr/share/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service
drwxr-xr-x root/root ./usr/share/doc/
drwxr-xr-x root/root ./usr/share/doc/gimbal-files/
-rw-r--r-- root/root ./usr/share/doc/gimbal-files/changelog.Debian.gz
-rw-r--r-- root/root ./usr/share/doc/gimbal-files/copyright
drwxr-xr-x root/root ./usr/share/icons/
drwxr-xr-x root/root ./usr/share/icons/hicolor/
drwxr-xr-x root/root ./usr/share/icons/hicolor/scalable/
drwxr-xr-x root/root ./usr/share/icons/hicolor/scalable/apps/
-rw-r--r-- root/root ./usr/share/icons/hicolor/scalable/apps/io.github.kcanady21.GimbalFiles.svg
EOF
dpkg-deb --contents "$deb" | awk '{print $1, $2, $NF}' | sort >"$actual"
sort -o "$expected" "$expected"
diff -u "$expected" "$actual" || fail "the package contents, modes or owners differ"
grep -q './DEBIAN' <<<"$actual" && fail "the package contains DEBIAN metadata"

echo "==> checking the control area has no maintainer scripts"
ctrl="$(dpkg-deb --ctrl-tarfile "$deb" | tar -tf - | sort)"
[[ "$ctrl" == $'./\n./control' ]] || fail "unexpected control files: $ctrl"

echo "==> comparing the installed binaries with stripped target/release"
mkdir -p "$scratch/extract"
dpkg-deb --extract "$deb" "$scratch/extract"
for binary in gimbal-files gimbal-files-cli; do
    cp -- "$repo_root/target/release/$binary" "$scratch/$binary.stripped"
    strip --strip-unneeded "$scratch/$binary.stripped"
    cmp -s "$scratch/$binary.stripped" "$scratch/extract/usr/bin/$binary" \
        || fail "installed $binary differs from target/release after stripping"
done
"$scratch/extract/usr/bin/gimbal-files-cli" --version >/dev/null \
    || fail "the extracted CLI does not run"

echo "==> checking the FileManager1 activation file"
service="$scratch/extract/usr/share/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service"
[[ -f "$service" ]] || fail "the .deb does not contain the FileManager1 service file"
grep -qxF 'Name=org.freedesktop.FileManager1' "$service" \
    || fail "the FileManager1 service name is wrong"
grep -qxF 'Exec=/usr/bin/gimbal-files --gapplication-service' "$service" \
    || fail "the FileManager1 Exec is not the system-wide binary"

echo "==> checking copyright and changelog"
changelog="$(dpkg-deb --fsys-tarfile "$deb" | tar -xOf - ./usr/share/doc/gimbal-files/changelog.Debian.gz | gzip -dc)"
grep -q "^gimbal-files ($version) stable; urgency=medium$" <<<"$changelog" \
    || fail "changelog.Debian does not start with the release"
copyright="$(dpkg-deb --fsys-tarfile "$deb" | tar -xOf - ./usr/share/doc/gimbal-files/copyright)"
grep -q '^Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/$' <<<"$copyright" \
    || fail "copyright is not DEP-5"
grep -q '^License: GPL-3.0-or-later$' <<<"$copyright" \
    || fail "copyright does not cover Gimbal's license"
grep -q '^ gtk4 0.11.5 - MIT$' <<<"$copyright" \
    || fail "copyright does not list the bundled crates"

echo "==> checking reproducibility"
deb_again="$(deb_build "$version")"
cmp -s "$scratch/first.deb" "$deb_again" || fail "two builds of $version differ"
first_sum="$(sha256sum "$scratch/first.deb" | awk '{print $1}')"
echo "    two builds are byte-identical: $first_sum"

echo "==> building a second version for the repository"
deb_new="$(deb_build "${version}.1")"
[[ -f "$deb_new" ]] || fail "the second build did not produce a .deb"

repo="$scratch/pages/apt"
pool="$repo/pool/main/g/gimbal-files"
mkdir -p "$pool"
cp -- "$scratch/first.deb" "$pool/gimbal-files_${version}_amd64.deb"
cp -- "$deb_new" "$pool/gimbal-files_${version}.1_amd64.deb"

echo "==> making a scratch archive key and signing the repo"
export GNUPGHOME="$scratch/gnupg"
mkdir -m 0700 -p "$GNUPGHOME"
gpg --batch --pinentry-mode loopback --passphrase '' \
    --quick-generate-key "Gimbal Files Test <$DEBEMAIL>" ed25519 sign 0 >/dev/null 2>&1
key="$(gpg --batch --with-colons --list-secret-keys | awk -F: '$1 == "fpr" { print $10; exit }')"
[[ -n "$key" ]] || fail "the scratch key was not created"
keyring="$scratch/gimbal-files-archive-keyring.gpg"
gpg --batch --export --output "$keyring" "$key"
[[ -s "$keyring" ]] || fail "the scratch public keyring is empty"

apt_build_indexes "$repo"
apt_sign "$repo" "$key" "$keyring"

grep -qx 'Origin: Gimbal Files' "$repo/dists/stable/Release" || fail "Release Origin is wrong"
grep -qx 'Label: Gimbal Files' "$repo/dists/stable/Release" || fail "Release Label is wrong"
grep -qx 'Suite: stable' "$repo/dists/stable/Release" || fail "Release Suite is wrong"
grep -qx 'Codename: stable' "$repo/dists/stable/Release" || fail "Release Codename is wrong"
grep -qx 'Architectures: amd64' "$repo/dists/stable/Release" || fail "Release Architectures is wrong"
grep -qx 'Components: main' "$repo/dists/stable/Release" || fail "Release Components is wrong"
grep -q '^Date: ' "$repo/dists/stable/Release" || fail "Release has no Date"
grep -q '^SHA256:' "$repo/dists/stable/Release" || fail "Release has no SHA256 list"
! grep -q '^Valid-Until:' "$repo/dists/stable/Release" || fail "Release must not have Valid-Until"
gpg --batch --verify "$repo/dists/stable/InRelease" >/dev/null 2>&1 \
    || fail "InRelease does not verify"
gpg --batch --verify "$repo/dists/stable/Release.gpg" "$repo/dists/stable/Release" >/dev/null 2>&1 \
    || fail "Release.gpg does not verify"
head -c 5 "$repo/gimbal-files-archive-keyring.gpg" | grep -q -- '-----' \
    && fail "the keyring is armored"
gpg --batch --no-default-keyring --keyring "$repo/gimbal-files-archive-keyring.gpg" \
    --list-keys "$key" >/dev/null 2>&1 || fail "the copied keyring does not contain the key"

echo "==> checking the index contains both versions"
grep -q "^Version: $version$" "$repo/dists/stable/main/binary-amd64/Packages" \
    || fail "Packages lacks $version"
grep -q "^Version: ${version}.1$" "$repo/dists/stable/main/binary-amd64/Packages" \
    || fail "Packages lacks ${version}.1"

echo "==> pointing a scratch apt at the repository (no root)"
aptroot="$scratch/apt-client"
mkdir -p \
    "$aptroot/state/lists/partial" \
    "$aptroot/cache/archives/partial" \
    "$aptroot/etc/apt/sources.list.d" \
    "$aptroot/etc/apt/trusted.gpg.d" \
    "$aptroot/etc/apt/apt.conf.d"
: >"$aptroot/status"
: >"$aptroot/etc/apt/apt.conf"
: >"$aptroot/etc/apt/sources.list"
: >"$aptroot/trusted.gpg"
cat >"$aptroot/etc/apt/sources.list.d/gimbal-files.sources" <<EOF
Types: deb
URIs: file://$repo
Suites: stable
Components: main
Architectures: amd64
Signed-By: $keyring
EOF

apt_opts=(
    -o "Dir::State=$aptroot/state"
    -o "Dir::Cache=$aptroot/cache"
    -o "Dir::Etc::sourcelist=$aptroot/etc/apt/sources.list"
    -o "Dir::Etc::sourceparts=$aptroot/etc/apt/sources.list.d"
    -o "Dir::Etc::trusted=$aptroot/trusted.gpg"
    -o "Dir::Etc::trustedparts=$aptroot/etc/apt/trusted.gpg.d"
    -o "Dir::Etc::main=$aptroot/etc/apt/apt.conf"
    -o "Dir::Etc::parts=$aptroot/etc/apt/apt.conf.d"
    -o "Dir::State::status=$aptroot/status"
    -o "APT::Architecture=amd64"
    -o "APT::Architectures::=amd64"
    -o "Acquire::Languages=none"
    -o "APT::Sandbox::User=${USER:-$(id -un)}"
    -o "Debug::NoLocking=1"
    -o "APT::Get::List-Cleanup=0"
)

apt-get "${apt_opts[@]}" update
policy="$(apt-cache "${apt_opts[@]}" policy gimbal-files)"
grep -q "Candidate: ${version}.1$" <<<"$policy" \
    || fail "apt does not pick ${version}.1 as the candidate"
grep -q " ${version} " <<<"$policy" || fail "apt does not show $version"
grep -q " ${version}.1 " <<<"$policy" || fail "apt does not show ${version}.1"
grep -q "file:$repo" <<<"$policy" || fail "apt-cache does not show the file repository"

echo "==> tampering with Packages must fail the update"
packages="$repo/dists/stable/main/binary-amd64/Packages"
cp -- "$packages" "$scratch/Packages.good"
printf '\nX-Tampered: yes\n' >>"$packages"
rm -rf -- "$aptroot/state/lists"
mkdir -p "$aptroot/state/lists/partial"
tamper_status=0
tamper_output="$(apt-get "${apt_opts[@]}" update 2>&1)" || tamper_status=$?
[[ "$tamper_status" -ne 0 ]] || fail "a tampered Packages file did not fail apt-get update"
grep -qi 'Hash Sum mismatch' <<<"$tamper_output" || fail "the tamper failure was not a hash mismatch"
mv -- "$scratch/Packages.good" "$packages"

echo "PASS: .deb, two-version signed repo and scratch apt all work in $scratch"
