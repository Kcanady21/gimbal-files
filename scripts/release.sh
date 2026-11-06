#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build and sign one Gimbal Files release. Kyle runs this on his machine; it
# refuses to publish from anywhere except a clean, pushed main.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd -- "$script_dir/.." && pwd -P)"
toolchain_dir="${HOME}/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin"
export PATH="${toolchain_dir}:${PATH}"
# shellcheck source=scripts/build-deb.sh
source "$script_dir/build-deb.sh"

usage() {
    cat <<'EOF'
Usage: release.sh [--dry-run | --sign-only] [--notes FILE]

Build, verify and sign a Gimbal Files release. Publishing asks first.

  --dry-run    build, stage, make the tarball, the .deb and an unsigned apt
               repo in a scratch copy; stop before signing
  --sign-only  build, stage, sign the tarball and apt repo, and stop before
               publishing or committing the gh-pages branch
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
deb="$dist/gimbal-files_${version}_amd64.deb"
pages_root="$dist/pages"
apt_dir="$pages_root/apt"

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

git check-ignore -q -- "dist/pages" \
    || error "dist/ is not ignored by git; the gh-pages worktree would dirty main"

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

    apt_key="${GIMBAL_APT_KEY:-}"
    [[ -n "$apt_key" ]] \
        || error "GIMBAL_APT_KEY is not set; make the archive key and export its fingerprint (one-time setup: docs/notes/P2w.md)"
    gpg --batch --list-secret-keys -- "$apt_key" >/dev/null 2>&1 \
        || error "the secret archive key $apt_key is not in the keyring (one-time setup: docs/notes/P2w.md)"
    apt_keyring="${GIMBAL_APT_KEYRING:-$repo_root/packaging/gimbal-files-archive-keyring.gpg}"
    [[ -f "$apt_keyring" ]] \
        || error "public archive keyring not found at $apt_keyring (one-time setup: docs/notes/P2w.md)"
fi
apt_key="${GIMBAL_APT_KEY:-}"
apt_keyring="${GIMBAL_APT_KEYRING:-$repo_root/packaging/gimbal-files-archive-keyring.gpg}"

# The .deb changelog, the ar header and the apt Release Date all use the
# checked commit's date, so the same inputs give the same bytes.
SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct "$local_main")}"
[[ "$SOURCE_DATE_EPOCH" =~ ^[0-9]+$ ]] || error "SOURCE_DATE_EPOCH must be seconds since the epoch"
export SOURCE_DATE_EPOCH

step() {
    local label="$1"
    shift
    echo "==> $label"
    "$@"
}

write_pages_site() {
    local pages="$1"
    mkdir -p "$pages/apt"
    : >"$pages/.nojekyll"
    # The one-line setup the README offers: curl -fsSL …/apt/setup.sh | bash
    install -m 0644 "$repo_root/packaging/apt-setup.sh" "$pages/apt/setup.sh"
    cat >"$pages/index.html" <<'EOF'
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<!doctype html>
<html lang="en">
<meta charset="utf-8">
<title>Gimbal Files apt repository</title>
<h1>Gimbal Files apt repository</h1>
<p>Setup and install instructions are in the
<a href="https://github.com/Kcanady21/gimbal-files#install-with-apt">Gimbal Files README</a>.</p>
<p>The signed apt repository is served from <code>/apt</code>.</p>
</html>
EOF
}

prepare_pages_worktree() {
    local remote_status=0
    git ls-remote --exit-code --heads origin "refs/heads/gh-pages" >/dev/null 2>&1 \
        || remote_status=$?
    case "$remote_status" in
        0)
            git fetch --quiet origin "+refs/heads/gh-pages:refs/remotes/origin/gh-pages" \
                || error "could not fetch origin/gh-pages"
            ;;
        2) ;;
        *) error "could not check origin for the gh-pages branch" ;;
    esac

    if [[ -e "$pages_root" ]]; then
        [[ -e "$pages_root/.git" ]] \
            || error "$pages_root exists but is not a git worktree; move it aside first"
        local current
        current="$(git -C "$pages_root" symbolic-ref --quiet --short HEAD || true)"
        [[ "$current" == "gh-pages" ]] \
            || error "$pages_root is a worktree on ${current:-a detached HEAD}, not gh-pages"
    elif git show-ref --verify --quiet refs/heads/gh-pages; then
        git worktree add --quiet "$pages_root" gh-pages \
            || error "could not add the gh-pages worktree at $pages_root"
    elif (( remote_status == 0 )); then
        git worktree add --quiet --track -b gh-pages "$pages_root" refs/remotes/origin/gh-pages \
            || error "could not add the gh-pages worktree at $pages_root"
    else
        echo "==> creating the local gh-pages orphan branch (not pushed yet)"
        git worktree add --quiet --orphan -b gh-pages "$pages_root" \
            || error "could not create the gh-pages orphan worktree at $pages_root"
    fi

    # Everything under dist/pages is generated here. Start each release from the
    # branch tip, so files left by an earlier --sign-only run (a deb that never
    # got a GitHub release, say) are never published.
    if git -C "$pages_root" rev-parse --quiet --verify HEAD >/dev/null; then
        git -C "$pages_root" reset --quiet --hard
    fi
    git -C "$pages_root" clean -fdxq
    if (( remote_status == 0 )); then
        local here there
        here="$(git -C "$pages_root" rev-parse --quiet --verify HEAD || true)"
        there="$(git rev-parse refs/remotes/origin/gh-pages)"
        if [[ "$here" != "$there" ]]; then
            [[ -n "$here" ]] && git merge-base --is-ancestor "$here" "$there" \
                || error "local gh-pages differs from origin/gh-pages; if an earlier apt push failed, push it first: git -C \"$pages_root\" push origin gh-pages"
            git -C "$pages_root" merge --quiet --ff-only "$there" \
                || error "could not fast-forward gh-pages to origin/gh-pages"
        fi
    fi
    write_pages_site "$pages_root"
}

update_apt_repo() {
    local sign="$1"
    local pages="$2"
    local pool="$pages/apt/pool/main/g/gimbal-files"
    mkdir -p "$pool"
    install -m 0644 "$deb" "$pool/$(basename -- "$deb")"
    apt_prune_pool "$pool" 3
    apt_build_indexes "$pages/apt"
    if $sign; then
        apt_sign "$pages/apt" "$apt_key" "$apt_keyring"
    else
        rm -f -- "$pages/apt/dists/stable/InRelease" "$pages/apt/dists/stable/Release.gpg"
        if [[ -f "$apt_keyring" ]]; then
            install -m 0644 "$apt_keyring" "$pages/apt/gimbal-files-archive-keyring.gpg"
        fi
    fi
}

step "scripts/check.sh" "$repo_root/scripts/check.sh"
step "cargo build --workspace --release" cargo build --workspace --release

# The .deb's copyright credits every crate in Cargo.lock. Refuse a stale list.
echo "==> checking the crate list in packaging/debian/copyright"
cargo metadata --format-version 1 --offline --locked \
    | python3 -I "$script_dir/copyright-crates.py" \
    || error "packaging/debian/copyright does not match Cargo.lock; regenerate it with: cargo metadata --format-version 1 --offline | python3 -I scripts/copyright-crates.py --write"

echo "==> staging $stage_name"
rm -rf -- "$stage"
rm -f -- "$tarball" "$tarball.minisig" "$latest" "$latest.minisig" "$deb"
mkdir -p -- "$stage/bin" "$stage/share/applications" "$stage/share/dbus-1/services" "$stage/share/icons/hicolor/scalable/apps"
install -m 0755 target/release/gimbal-files "$stage/bin/gimbal-files"
install -m 0755 target/release/gimbal-files-cli "$stage/bin/gimbal-files-cli"
install -m 0644 packaging/io.github.kcanady21.GimbalFiles.desktop \
    "$stage/share/applications/io.github.kcanady21.GimbalFiles.desktop"
install -m 0644 packaging/dbus/io.github.kcanady21.GimbalFiles.FileManager1.service \
    "$stage/share/dbus-1/services/io.github.kcanady21.GimbalFiles.FileManager1.service"
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

echo "==> scripts/build-deb.sh $version"
deb="$(deb_build "$version")"
[[ -f "$deb" ]] || error "build-deb.sh did not produce $deb"

if $dry_run; then
    scratch_pages="$(mktemp -d "${TMPDIR:-/tmp}/gimbal-pages-dry-run.XXXXXX")"
    if [[ -d "$pages_root/apt" ]]; then
        mkdir -p "$scratch_pages/apt"
        cp -a -- "$pages_root/apt/." "$scratch_pages/apt/"
    fi
    write_pages_site "$scratch_pages"
    update_apt_repo false "$scratch_pages"
    echo "dry run: left the build, tarball and unsigned apt repo in $dist and $scratch_pages; skipped signing and publishing"
    exit 0
fi

echo "==> signing"
minisign -S -s "$minisign_key" -t "gimbal-files $version x86_64-linux" -m "$tarball"
minisign -S -s "$minisign_key" -t "gimbal-files $version latest" -m "$latest"
[[ -f "$tarball.minisig" && -f "$latest.minisig" ]] || error "minisign did not write both signatures"

prepare_pages_worktree
update_apt_repo true "$pages_root"

if $sign_only; then
    echo "sign-only: signed $tarball, $latest and the apt repo at $apt_dir; not publishing or committing"
    exit 0
fi

cat <<EOF
Ready to publish $tag:
  $tarball
  $deb
  $latest
  $tarball.minisig
  $latest.minisig
  $apt_dir/dists/stable/InRelease
  $apt_dir/dists/stable/Release.gpg
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
    "$tarball" "$deb" "$latest" "$tarball.minisig" "$latest.minisig"

# The apt index is pushed only after the GitHub release exists, so a failed
# release never leaves apt pointing at a version that cannot be downloaded.
echo "==> committing and pushing gh-pages"
git -C "$pages_root" add --all
if git -C "$pages_root" diff --cached --quiet; then
    error "the gh-pages worktree has no changes to commit"
fi
git -C "$pages_root" commit -m "apt: gimbal-files $version" \
    || error "$tag is on GitHub, but the apt update was not committed; commit and push $pages_root by hand"
git -C "$pages_root" push --set-upstream origin gh-pages \
    || error "$tag is on GitHub, but the apt update was not pushed; push it with: git -C \"$pages_root\" push --set-upstream origin gh-pages"
