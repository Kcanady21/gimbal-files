#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build the system-wide gimbal-files .deb from an existing release build.
# It never runs cargo. The apt repository helpers below are shared with
# scripts/release.sh, which sources this file.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd -- "$script_dir/.." && pwd -P)"

deb_error() {
    echo "build-deb.sh: error: $*" >&2
    exit 1
}

deb_usage() {
    cat <<'EOF'
Usage: build-deb.sh <version>

Build dist/gimbal-files_<version>_amd64.deb and its optional
dist/gimbal-files-dbgsym_<version>_amd64.deb from target/release. This script
never runs cargo; run `cargo build --workspace --release` first. It reads the
workspace repository for Homepage, DEBFULLNAME/DEBEMAIL (falling back to git
config) for Maintainer, and SOURCE_DATE_EPOCH (falling back to the release
commit's date) for reproducible timestamps.
EOF
}

workspace_version() {
    sed -n '/^\[workspace\.package\]/,/^\[/s/^version[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' \
        "$repo_root/Cargo.toml" | head -n 1
}

workspace_repository() {
    sed -n '/^\[workspace\.package\]/,/^\[/s/^repository[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' \
        "$repo_root/Cargo.toml" | head -n 1
}

sed_replacement() {
    local value="$1"
    value="${value//\\/\\\\}"
    value="${value//&/\\&}"
    value="${value//|/\\|}"
    printf '%s' "$value"
}

deb_maintainer() {
    local name email
    name="${DEBFULLNAME:-$(git -C "$repo_root" config user.name 2>/dev/null || true)}"
    email="${DEBEMAIL:-$(git -C "$repo_root" config user.email 2>/dev/null || true)}"
    if [[ -z "$name" || -z "$email" ]]; then
        deb_error "Maintainer is empty; set DEBFULLNAME and DEBEMAIL (or git config user.name and user.email)"
    fi
    case "$name$email" in
        *$'\n'*|*$'\r'*) deb_error "Maintainer name or email contains a newline" ;;
    esac
    printf '%s <%s>\n' "$name" "$email"
}

deb_source_date_epoch() {
    local epoch="${SOURCE_DATE_EPOCH:-}"
    if [[ -z "$epoch" ]]; then
        epoch="$(git -C "$repo_root" log -1 --format=%ct 2>/dev/null || true)"
        [[ -n "$epoch" ]] \
            || deb_error "SOURCE_DATE_EPOCH is unset and no release commit date is available"
    fi
    [[ "$epoch" =~ ^[0-9]+$ ]] || deb_error "SOURCE_DATE_EPOCH must be seconds since the epoch"
    printf '%s\n' "$epoch"
}

# Build one .deb and print its path on stdout. Everything else goes to stderr.
deb_build() {
    local version="$1"
    (
        set -euo pipefail
        umask 022

        [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.+~][A-Za-z0-9.+~:-]+)?$ ]] \
            || deb_error "version must look like 0.1.0 (got: $version)"

        for tool in dpkg-deb dpkg-shlibdeps dpkg-gencontrol gzip strip objcopy readelf; do
            command -v "$tool" >/dev/null 2>&1 || deb_error "$tool is not installed"
        done
        for binary in gimbal-files gimbal-files-cli; do
            [[ -x "$repo_root/target/release/$binary" ]] \
                || deb_error "target/release/$binary is missing; run 'cargo build --workspace --release' first (this script never runs cargo)"
        done
        [[ -f "$repo_root/packaging/debian/control" ]] \
            || deb_error "packaging/debian/control is missing"
        [[ -f "$repo_root/packaging/debian/changelog" ]] \
            || deb_error "packaging/debian/changelog is missing"
        [[ -f "$repo_root/packaging/debian/copyright" ]] \
            || deb_error "packaging/debian/copyright is missing"
        [[ -f "$repo_root/packaging/io.github.kcanady21.GimbalFiles.desktop" ]] \
            || deb_error "the desktop entry is missing"
        [[ -f "$repo_root/packaging/icons/io.github.kcanady21.GimbalFiles.svg" ]] \
            || deb_error "the app icon is missing"
        [[ -f "$repo_root/packaging/dbus/io.github.kcanady21.GimbalFiles.FileManager1.service" ]] \
            || deb_error "the FileManager1 activation file is missing"

        local homepage maintainer epoch
        homepage="$(workspace_repository)"
        [[ -n "$homepage" ]] || deb_error "could not read the workspace repository from Cargo.toml"
        maintainer="$(deb_maintainer)"
        epoch="$(deb_source_date_epoch)"
        export SOURCE_DATE_EPOCH="$epoch"

        local stage pkg meta
        stage="$(mktemp -d "${TMPDIR:-/tmp}/gimbal-deb.XXXXXX")"
        trap 'rm -rf -- "$stage"' EXIT
        pkg="$stage/pkg"
        meta="$stage/meta"
        mkdir -p \
            "$pkg/usr/bin" \
            "$pkg/usr/share/applications" \
            "$pkg/usr/share/dbus-1/services" \
            "$pkg/usr/share/icons/hicolor/scalable/apps" \
            "$pkg/usr/share/doc/gimbal-files" \
            "$pkg/DEBIAN" \
            "$meta/debian" \
            "$stage/debug"

        install -m 0755 "$repo_root/target/release/gimbal-files" "$pkg/usr/bin/gimbal-files"
        install -m 0755 "$repo_root/target/release/gimbal-files-cli" "$pkg/usr/bin/gimbal-files-cli"
        # Keep the file/line tables in a separate debug package. The installed
        # binary carries a build ID and a debuglink; Debian's dbgsym layout
        # finds the symbols through the build ID.
        for binary in gimbal-files gimbal-files-cli; do
            objcopy --only-keep-debug "$pkg/usr/bin/$binary" "$stage/debug/$binary.debug"
            strip --strip-unneeded "$pkg/usr/bin/$binary"
            objcopy --add-gnu-debuglink="$stage/debug/$binary.debug" "$pkg/usr/bin/$binary"
        done
        chmod 0755 "$pkg/usr/bin/gimbal-files" "$pkg/usr/bin/gimbal-files-cli"

        # Exec=gimbal-files %U works unchanged because /usr/bin is on PATH.
        install -m 0644 "$repo_root/packaging/io.github.kcanady21.GimbalFiles.desktop" \
            "$pkg/usr/share/applications/io.github.kcanady21.GimbalFiles.desktop"
        # The bus reads this system-wide file; its Exec stays /usr/bin because
        # that is where the package puts the binary.
        install -m 0644 \
            "$repo_root/packaging/dbus/io.github.kcanady21.GimbalFiles.FileManager1.service" \
            "$pkg/usr/share/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service"
        install -m 0644 "$repo_root/packaging/icons/io.github.kcanady21.GimbalFiles.svg" \
            "$pkg/usr/share/icons/hicolor/scalable/apps/io.github.kcanady21.GimbalFiles.svg"
        install -m 0644 "$repo_root/packaging/debian/copyright" \
            "$pkg/usr/share/doc/gimbal-files/copyright"

        local date changelog
        date="$(LC_ALL=C date -u -d "@$epoch" '+%a, %d %b %Y %H:%M:%S +0000')"
        changelog="$stage/changelog.Debian"
        sed \
            -e '/^#/d' \
            -e "s|@VERSION@|$(sed_replacement "$version")|g" \
            -e "s|@DATE@|$(sed_replacement "$date")|g" \
            -e "s|@MAINTAINER@|$(sed_replacement "$maintainer")|g" \
            "$repo_root/packaging/debian/changelog" >"$changelog"
        gzip -9n -c "$changelog" >"$pkg/usr/share/doc/gimbal-files/changelog.Debian.gz"
        chmod 0644 "$pkg/usr/share/doc/gimbal-files/changelog.Debian.gz"

        local control
        control="$meta/debian/control"
        sed \
            -e '/^#/d' \
            -e "s|@MAINTAINER@|$(sed_replacement "$maintainer")|g" \
            -e "s|@HOMEPAGE@|$(sed_replacement "$homepage")|g" \
            "$repo_root/packaging/debian/control" >"$control"
        cp -- "$changelog" "$meta/debian/changelog"
        : >"$meta/debian/substvars"
        (cd -- "$meta" && dpkg-shlibdeps \
            -T"$meta/debian/substvars" \
            -e"$pkg/usr/bin/gimbal-files" \
            -e"$pkg/usr/bin/gimbal-files-cli" >&2)
        (cd -- "$meta" && dpkg-gencontrol \
            -c"$control" \
            -P"$pkg" \
            -v"$version" \
            -T"$meta/debian/substvars" \
            -O"$pkg/DEBIAN/control" >&2)

        # Fixed timestamps and sorted tar entries make the same inputs produce
        # the same bytes. dpkg-deb also uses SOURCE_DATE_EPOCH for its ar header.
        find "$pkg" -exec touch -h -d "@$epoch" -- {} +

        local deb="$repo_root/dist/gimbal-files_${version}_amd64.deb"
        mkdir -p "$repo_root/dist"
        rm -f -- "$deb"
        dpkg-deb --root-owner-group --threads-max=1 --build "$pkg" "$deb" >&2

        # The optional dbgsym package: Debian's /usr/lib/debug/.build-id
        # layout, keyed by the build ID gdb and apport already know.
        local dbgstage="$stage/dbgsym"
        mkdir -p "$dbgstage/usr/lib/debug/.build-id" "$dbgstage/DEBIAN"
        for binary in gimbal-files gimbal-files-cli; do
            local build_id
            build_id="$(readelf -n "$pkg/usr/bin/$binary" \
                | sed -n 's/.*Build ID: //p' | head -n 1)"
            [[ -n "$build_id" ]] \
                || deb_error "$binary has no build ID for the dbgsym package"
            local debug_file="$dbgstage/usr/lib/debug/.build-id/${build_id:0:2}/${build_id:2}.debug"
            mkdir -p "$(dirname -- "$debug_file")"
            install -m 0644 "$stage/debug/$binary.debug" "$debug_file"
        done
        local installed_size
        installed_size="$(du -sk "$dbgstage" | cut -f1)"
        cat >"$dbgstage/DEBIAN/control" <<EOF
Package: gimbal-files-dbgsym
Version: $version
Architecture: amd64
Maintainer: $maintainer
Installed-Size: $installed_size
Depends: gimbal-files (= $version)
Section: debug
Priority: optional
Auto-Built-Package: debug-symbols
Description: debug symbols for gimbal-files
 Debug symbols for Gimbal Files. Install this package to let gdb and apport
 show function names and line numbers in crash reports.
EOF
        find "$dbgstage" -exec touch -h -d "@$epoch" -- {} +
        local dbgsym="$repo_root/dist/gimbal-files-dbgsym_${version}_amd64.deb"
        rm -f -- "$dbgsym"
        dpkg-deb --root-owner-group --threads-max=1 --build "$dbgstage" "$dbgsym" >&2
        printf '%s\n' "$deb"
    )
}

# The dbgsym path for a version; `deb_build` writes it next to the main deb.
deb_dbgsym_path() {
    printf '%s\n' "$repo_root/dist/gimbal-files-dbgsym_${1}_amd64.deb"
}


# Build Packages, Packages.gz and Release in one apt root. Release deliberately
# has no Valid-Until: this repository is rebuilt only when Kyle releases, and a
# gap between releases must not make apt reject it. See docs/notes/P2w.md.
apt_build_indexes() {
    local apt_dir="$1"
    local pool_rel="pool/main/g/gimbal-files"
    local binary_dir="$apt_dir/dists/stable/main/binary-amd64"
    local epoch date
    epoch="${SOURCE_DATE_EPOCH:-$(date +%s)}"
    [[ "$epoch" =~ ^[0-9]+$ ]] || deb_error "SOURCE_DATE_EPOCH must be seconds since the epoch"
    date="$(LC_ALL=C date -u -d "@$epoch" '+%a, %d %b %Y %H:%M:%S +0000')"

    [[ -d "$apt_dir/$pool_rel" ]] || deb_error "apt pool not found at $apt_dir/$pool_rel"
    mkdir -p "$binary_dir"
    (
        cd -- "$apt_dir"
        dpkg-scanpackages --multiversion "$pool_rel" >"$binary_dir/Packages"
    )
    gzip -9n -c "$binary_dir/Packages" >"$binary_dir/Packages.gz"

    {
        printf 'Origin: Gimbal Files\n'
        printf 'Label: Gimbal Files\n'
        printf 'Suite: stable\n'
        printf 'Codename: stable\n'
        printf 'Architectures: amd64\n'
        printf 'Components: main\n'
        printf 'Description: Gimbal Files apt repository\n'
        printf 'Date: %s\n' "$date"
        printf 'SHA256:\n'
        (
            cd -- "$apt_dir/dists/stable"
            sha256sum main/binary-amd64/Packages main/binary-amd64/Packages.gz
        ) | while read -r hash path; do
            printf ' %s %s %s\n' "$hash" "$(stat -c '%s' "$apt_dir/dists/stable/$path")" "$path"
        done
    } >"$apt_dir/dists/stable/Release"
}

# Sign Release with a dedicated archive key. GIMBAL_APT_KEY is a fingerprint,
# and GIMBAL_APT_KEYRING may point at a different public keyring (the manual
# test uses a scratch one). The public key is copied into the site root.
apt_sign() {
    local apt_dir="$1"
    local key="${2:-${GIMBAL_APT_KEY:-}}"
    local keyring="${3:-${GIMBAL_APT_KEYRING:-$repo_root/packaging/gimbal-files-archive-keyring.gpg}}"
    local dist_dir="$apt_dir/dists/stable"

    [[ -n "$key" ]] \
        || deb_error "GIMBAL_APT_KEY is not set; see the one-time setup in docs/notes/P2w.md"
    gpg --batch --list-secret-keys -- "$key" >/dev/null 2>&1 \
        || deb_error "the secret archive key $key is not in the keyring; see docs/notes/P2w.md"
    [[ -f "$keyring" ]] \
        || deb_error "the public archive keyring is not at $keyring; see docs/notes/P2w.md"
    if head -c 5 "$keyring" | grep -q -- '-----'; then
        deb_error "the public archive keyring is armored; export it with gpg --export, without --armor (see docs/notes/P2w.md)"
    fi
    [[ -f "$dist_dir/Release" ]] || deb_error "no Release file at $dist_dir/Release"

    install -m 0644 "$keyring" "$apt_dir/gimbal-files-archive-keyring.gpg"
    gpg --batch --yes --no-tty --local-user "$key" \
        --clearsign --digest-algo SHA256 \
        --output "$dist_dir/InRelease" "$dist_dir/Release"
    gpg --batch --yes --no-tty --local-user "$key" \
        --detach-sign --armor --digest-algo SHA256 \
        --output "$dist_dir/Release.gpg" "$dist_dir/Release"
}

# Remove all but the newest <keep> package versions from the apt pool.
apt_prune_pool() {
    apt_prune_matching "$1" "$2" "gimbal-files_" "_amd64.deb"
}

apt_prune_dbgsym_pool() {
    apt_prune_matching "$1" "$2" "gimbal-files-dbgsym_" "_amd64.deb"
}

apt_prune_matching() {
    local pool="$1"
    local keep="$2"
    local prefix="$3"
    local suffix="$4"
    [[ "$keep" =~ ^[0-9]+$ ]] || deb_error "apt_prune_matching needs a non-negative count"
    local files=() versions=() v o i placed
    while IFS= read -r v; do
        files+=("$v")
    done < <(find "$pool" -maxdepth 1 -type f -name "${prefix}*${suffix}" -printf '%f\n')
    (( ${#files[@]} > keep )) || return 0

    local ordered=()
    for v in "${files[@]}"; do
        v="${v#"$prefix"}"
        v="${v%"$suffix"}"
        versions+=("$v")
    done
    for v in "${versions[@]}"; do
        placed=false
        local next=()
        for o in "${ordered[@]}"; do
            if ! $placed && dpkg --compare-versions "$v" gt "$o"; then
                next+=("$v")
                placed=true
            fi
            next+=("$o")
        done
        $placed || next+=("$v")
        ordered=("${next[@]}")
    done
    for ((i = keep; i < ${#ordered[@]}; i++)); do
        rm -f -- "$pool/${prefix}${ordered[i]}${suffix}"
    done
}

deb_main() {
    if (( $# != 1 )); then
        deb_usage >&2
        exit 2
    fi
    case "$1" in
        -h|--help)
            deb_usage
            exit 0
            ;;
    esac
    deb_build "$1"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    deb_main "$@"
fi
