#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"
toolchain_dir="${HOME}/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin"

if [[ ! -x "${toolchain_dir}/cargo" ]]; then
    echo "error: rustup 1.95.0 toolchain not found at ${toolchain_dir}" >&2
    exit 1
fi

export PATH="${toolchain_dir}:${PATH}"
cd -- "${repo_root}"

step() {
    local label="$1"
    shift
    echo "==> ${label}"
    "$@"
}

step "cargo fmt --all --check" cargo fmt --all --check
step "cargo clippy --workspace --all-targets -- -D warnings" cargo clippy --workspace --all-targets -- -D warnings
step "cargo test --workspace" cargo test --workspace
step "cargo build --workspace --release" cargo build --workspace --release
step "cargo deny check" cargo deny check

echo "all checks passed"
