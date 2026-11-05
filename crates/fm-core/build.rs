// SPDX-License-Identifier: GPL-3.0-or-later
//! Bake in the release minisign public key when Kyle has created
//! `packaging/minisign.pub`.
//!
//! The key file's second line is the base64 key, and `cargo:rustc-env` values
//! are one line each. When the file is absent this emits nothing, so
//! `option_env!("GIMBAL_RELEASE_PUBKEY")` is `None` and a build still succeeds.
//! The check then reports that the build has no release key without touching
//! the network.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets this"));
    let key_path = manifest.join("../../packaging/minisign.pub");
    println!("cargo:rerun-if-changed={}", key_path.display());

    let Ok(text) = fs::read_to_string(&key_path) else {
        return;
    };
    let Some(base64) = text.lines().nth(1).map(str::trim) else {
        return;
    };
    if !base64.is_empty() {
        println!("cargo:rustc-env=GIMBAL_RELEASE_PUBKEY={base64}");
    }
}
