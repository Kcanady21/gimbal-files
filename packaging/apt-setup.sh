#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Add the Gimbal Files apt repository and install the package, in one step:
#
#   curl -fsSL https://kcanady21.github.io/gimbal-files/apt/setup.sh | bash
#
# It does exactly what the README's manual setup does, and asks for your
# password through sudo. `| bash -s -- --uninstall` removes the package and the
# repository again. release.sh publishes this file to the Pages site.
#
# Everything runs from main() on the last line, so a download that is cut short
# runs nothing.
set -euo pipefail

base="https://kcanady21.github.io/gimbal-files/apt"
# Test-only: a scratch folder that stands in for / (scripts/test-apt-setup.sh).
root="${GIMBAL_SETUP_ROOT:-}"
keyring="$root/etc/apt/keyrings/gimbal-files.gpg"
sources="$root/etc/apt/sources.list.d/gimbal-files.sources"

say() {
    echo "==> $*"
}

error() {
    echo "gimbal-files setup: error: $*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
Usage: setup.sh [--uninstall]

Add the signed Gimbal Files apt repository, then install gimbal-files.
Updates then come with `sudo apt upgrade`.

  --uninstall  remove gimbal-files and the repository's two setup files
  -h, --help   show this help
EOF
}

as_root() {
    if (( EUID == 0 )); then
        "$@"
    else
        sudo "$@"
    fi
}

install_repo() {
    tmp_dir="$(mktemp -d)"
    trap 'rm -rf -- "$tmp_dir"' EXIT
    local tmp="$tmp_dir"

    say "downloading the archive key"
    curl -fsSL "$base/gimbal-files-archive-keyring.gpg" -o "$tmp/keyring.gpg" \
        || error "could not download $base/gimbal-files-archive-keyring.gpg"
    # A binary OpenPGP public key starts with a public-key packet (0x98, 0x99
    # or 0xc6). Anything else, such as an HTML error page, is refused.
    case "$(od -An -tx1 -N1 "$tmp/keyring.gpg" | tr -d ' \n')" in
        98|99|c6) ;;
        *) error "the downloaded archive key is not a binary OpenPGP key" ;;
    esac

    cat >"$tmp/gimbal-files.sources" <<EOF
Types: deb
URIs: $base
Suites: stable
Components: main
Architectures: amd64
Signed-By: /etc/apt/keyrings/gimbal-files.gpg
EOF

    say "adding the repository (sudo may ask for your password)"
    as_root install -d -m 0755 "$root/etc/apt/keyrings" "$root/etc/apt/sources.list.d"
    as_root install -m 0644 "$tmp/keyring.gpg" "$keyring"
    as_root install -m 0644 "$tmp/gimbal-files.sources" "$sources"

    say "updating the package lists"
    as_root apt-get update
    say "installing gimbal-files"
    as_root apt-get install -y gimbal-files
    say "Gimbal Files is installed. Updates come with: sudo apt upgrade"
}

uninstall_repo() {
    if dpkg-query -W -f='${Status}' gimbal-files 2>/dev/null | grep -q 'install ok installed'; then
        say "removing gimbal-files (sudo may ask for your password)"
        as_root apt-get remove -y gimbal-files
    fi
    say "removing the repository"
    as_root rm -f -- "$sources" "$keyring"
    as_root apt-get update
    say "Gimbal Files and its repository are removed. Your settings are left alone."
}

main() {
    local uninstall=false
    while (( $# > 0 )); do
        case "$1" in
            --uninstall) uninstall=true ;;
            -h|--help) usage; exit 0 ;;
            *) usage >&2; exit 2 ;;
        esac
        shift
    done

    command -v apt-get >/dev/null 2>&1 \
        || error "this needs apt (Debian, Ubuntu and their relatives); other distributions can use the release tarball, see the README"
    local arch
    arch="$(dpkg --print-architecture)"
    [[ "$arch" == amd64 ]] \
        || error "Gimbal Files is built for amd64 only (this system is $arch)"
    if (( EUID != 0 )); then
        command -v sudo >/dev/null 2>&1 || error "sudo is not installed; run this as root instead"
    fi

    if $uninstall; then
        uninstall_repo
    else
        command -v curl >/dev/null 2>&1 || error "curl is not installed (sudo apt install curl)"
        install_repo
    fi
}

main "$@"
