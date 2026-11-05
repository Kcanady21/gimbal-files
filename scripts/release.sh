#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build and sign one Gimbal Files release. Kyle runs this on his machine; it
# refuses to publish from anywhere except a clean, pushed main.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd -- "$script_dir/.." && pwd -P)"
toolchain_dir="${HOME}/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin"
export PATH="${toolchain_dir}:${PATH}"

usage() {
    cat <<'EOF'
Usage: release.sh [--dry-run | --sign-only] [--notes FILE]

Build, verify and sign a Gimbal Files release. Publishing asks first.

  --dry-run    build, stage and make the tarball, then stop before signing
  --sign-only  build, stage, sign, and stop before publishing
  --notes FILE release notes; defaults to a one-line note
  -h, --help   show this help
EOF
}

error() {
    echo "release.sh: error: $*" >&2
    exit 1
}

dry_run=false
sign_only=false
notes=""
while (( $# > 0 )); do
    case "$1" in
        --dry-run)
            dry_run=true
            shift
            ;;
        --sign-only)
            sign_only=true
            shift
            ;;
        --notes)
            (( $# >= 2 )) || error "--notes needs a file"
            notes="$2"
            shift 2
            ;;
        --notes=*)
            notes="${1#*=}"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            error "unknown argument: $1"
            ;;
    esac
done

if $dry_run && $sign_only; then
    error "--dry-run and --sign-only cannot be used together"
fi

# A relative --notes path means the caller's folder, not the repo root.
if [[ -n "$notes" && "$notes" != /* ]]; then
    notes="$(pwd -P)/$notes"
fi

cd -- "$repo_root"

version="$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' Cargo.toml | head -n 1)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || error "could not read a version from Cargo.toml"

tag="v$version"
stage_name="gimbal-files-$version-x86_64-linux"
dist="$repo_root/dist"
stage="$dist/$stage_name"
tarball="$dist/$stage_name.tar.gz"
latest="$dist/latest.toml"

echo "==> checking the tree"
branch="$(git symbolic-ref --quiet --short HEAD || true)"
[[ "$branch" == "main" ]] || error "not on main (on ${branch:-a detached HEAD})"
[[ -z "$(git status --porcelain --untracked-files=normal)" ]] \
    || error "the working tree is not clean; commit or stash the changes first"
git fetch --quiet origin main || error "could not fetch origin/main"
local_main="$(git rev-parse --verify HEAD)"
remote_main="$(git rev-parse --verify refs/remotes/origin/main)"
[[ "$local_main" == "$remote_main" ]] \
    || error "main is not equal to origin/main; push or pull first"

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null 2>&1; then
    error "local tag $tag already exists"
fi
remote_tag_status=0
git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1 || remote_tag_status=$?
case "$remote_tag_status" in
    0) error "remote tag $tag already exists" ;;
    2) ;;
    *) error "could not check origin for tag $tag" ;;
esac

if ! $dry_run; then
    command -v minisign >/dev/null 2>&1 \
        || error "minisign is not installed; install it with: sudo apt install minisign"
    minisign_key="${MINISIGN_KEY:-$HOME/.minisign/gimbal-files.key}"
    [[ -f "$minisign_key" ]] \
        || error "minisign secret key not found at $minisign_key (set MINISIGN_KEY to use another path)"
fi

step() {
    local label="$1"
    shift
    echo "==> $label"
    "$@"
}

step "scripts/check.sh" "$repo_root/scripts/check.sh"
step "cargo build --workspace --release" cargo build --workspace --release

echo "==> staging $stage_name"
rm -rf -- "$stage"
rm -f -- "$tarball" "$tarball.minisig" "$latest" "$latest.minisig"
mkdir -p -- "$stage/bin" "$stage/share/applications" "$stage/share/icons/hicolor/scalable/apps"
install -m 0755 target/release/gimbal-files "$stage/bin/gimbal-files"
install -m 0755 target/release/gimbal-files-cli "$stage/bin/gimbal-files-cli"
install -m 0644 packaging/io.github.kcanady21.GimbalFiles.desktop \
    "$stage/share/applications/io.github.kcanady21.GimbalFiles.desktop"
install -m 0644 packaging/icons/io.github.kcanady21.GimbalFiles.svg \
    "$stage/share/icons/hicolor/scalable/apps/io.github.kcanady21.GimbalFiles.svg"
install -m 0755 scripts/install.sh "$stage/install.sh"
install -m 0644 README.md LICENSE "$stage/"

echo "==> writing dist/latest.toml"
printf 'version = "%s"\ntarball = "%s"\n' "$version" "$stage_name.tar.gz" >"$latest"
install -m 0644 "$latest" "$stage/latest.toml"

echo "==> making $stage_name.tar.gz"
tar -C "$dist" --owner=0 --group=0 --numeric-owner -czf "$tarball" "$stage_name"
while IFS= read -r entry; do
    case "$entry" in
        "$stage_name"|"$stage_name/"*) ;;
        *) error "tarball entry is outside $stage_name/: $entry" ;;
    esac
done < <(tar -tzf "$tarball")

if $dry_run; then
    echo "dry run: left the build and tarball in $dist; skipped signing and publishing"
    exit 0
fi

echo "==> signing"
minisign -S -s "$minisign_key" -t "gimbal-files $version x86_64-linux" -m "$tarball"
minisign -S -s "$minisign_key" -t "gimbal-files $version latest" -m "$latest"
[[ -f "$tarball.minisig" && -f "$latest.minisig" ]] || error "minisign did not write both signatures"

if $sign_only; then
    echo "sign-only: signed $tarball and $latest; not publishing"
    exit 0
fi

cat <<EOF
Ready to publish $tag:
  $tarball
  $latest
  $tarball.minisig
  $latest.minisig
EOF

printf 'Publish %s? [y/N] ' "$tag"
read -r answer
case "$answer" in
    y|Y) ;;
    *)
        echo "Not publishing."
        exit 0
        ;;
esac

notes_tmp=""
cleanup_notes() {
    if [[ -n "$notes_tmp" ]]; then
        rm -f -- "$notes_tmp"
    fi
}
trap cleanup_notes EXIT

if [[ -n "$notes" ]]; then
    [[ -f "$notes" ]] || error "notes file not found: $notes"
    notes_file="$notes"
else
    notes_tmp="$(mktemp "${TMPDIR:-/tmp}/gimbal-files-release-notes.XXXXXX")"
    notes_file="$notes_tmp"
    printf 'Gimbal Files %s\n' "$version" >"$notes_file"
fi

step "gh release create $tag" gh release create "$tag" \
    --target "$local_main" \
    --title "$tag" \
    --notes-file "$notes_file" \
    "$tarball" "$latest" "$tarball.minisig" "$latest.minisig"
