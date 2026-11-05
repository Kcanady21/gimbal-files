// SPDX-License-Identifier: GPL-3.0-or-later
//! The notice-only update check.
//!
//! Each release carries `latest.toml` and `latest.toml.minisig`. This module
//! fetches both, verifies the signature against the built-in release key,
//! parses the notice with unknown keys refused, and compares its version with
//! this build. It never downloads or installs a release: apt or the tarball
//! installer does that.
//!
//! The file and network access sit behind [`Fetcher`], so the policy above is
//! tested without a network. Install-kind detection and the 24-hour schedule
//! are pure functions over paths and Unix seconds.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use minisign_verify::{PublicKey, Signature};
use toml_edit::{Document, Item};

use crate::fsutil;

/// How long between automatic checks, in seconds.
pub const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// The most bytes either `latest.toml` or its signature may contain.
pub const MAX_NOTICE_BYTES: usize = 4 * 1024;

/// The total time allowed for one HTTP request, including its body.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// The folder below the state directory that holds `update.toml`.
pub const STATE_DIR: &str = "gimbal-files";
pub const STATE_FILE: &str = "update.toml";

/// The install marker written by `scripts/install.sh`.
pub const MARKER_DIR: &str = "gimbal-files";
pub const MARKER_FILE: &str = "install.toml";

/// The release download base, derived from the crate repository. The trailing
/// slash is part of the shape: the two file names are appended directly.
pub const DEFAULT_RELEASE_BASE: &str =
    concat!(env!("CARGO_PKG_REPOSITORY"), "/releases/latest/download/");

/// The public releases page, opened by the tarball install's button.
pub const RELEASES_PAGE: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases");

/// Every user-facing string this feature uses, in one place.
pub mod text {
    use super::{InstallKind, Version};

    pub const UPDATES: &str = "Updates";
    pub const CHECK_FOR_UPDATES: &str = "Check for updates";
    pub const CHECK_NOW: &str = "Check now";
    pub const OPEN_RELEASES: &str = "Open releases page";
    pub const CHECKING: &str = "Checking…";
    pub const UP_TO_DATE: &str = "Up to date";
    pub const DEVELOPMENT: &str = "Development build: update checks are off";
    pub const NO_KEY: &str = "This build has no release key";
    pub const OFFLINE: &str = "Could not reach GitHub. Check your connection.";
    pub const TOO_LARGE: &str = "The update notice was too large.";
    pub const BAD_SIGNATURE: &str = "The update notice is not signed correctly.";
    pub const BAD_TRUSTED_COMMENT: &str = "The update notice's signed comment does not match.";
    pub const PARSE: &str = "The update notice could not be read.";
    pub const INVALID_URL: &str = "The update address is not allowed.";
    pub const ALREADY_CHECKING: &str = "An update check is already running.";
    pub const CHECKS_OFF: &str = "Update checks are off.";
    pub const SET_ENABLED_NEEDS_BOOL: &str = "updates.set-enabled needs a boolean.";
    pub const NO_CONFIG_CHANGE: &str = "The update setting could not be saved.";
    pub const UI_ONLY: &str = "This update control is only available in Settings.";

    pub fn version_label() -> String {
        format!("Gimbal Files {}", env!("CARGO_PKG_VERSION"))
    }

    pub fn http_status(code: u16) -> String {
        format!("The update server returned HTTP {code}.")
    }

    pub fn how_to(kind: InstallKind) -> &'static str {
        match kind {
            InstallKind::System => "Update with your system updater (sudo apt upgrade).",
            InstallKind::Tarball => "Download it from the releases page.",
            InstallKind::Development => DEVELOPMENT,
        }
    }

    pub fn available(kind: InstallKind, version: Version) -> String {
        format!("{version} is available. {}", how_to(kind))
    }

    pub fn summary(kind: InstallKind, version: Version) -> String {
        format!("Gimbal Files {version} is available. {}", how_to(kind))
    }
}

/// One `MAJOR.MINOR.PATCH` version, compared as numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.');
        let major = parse_number(parts.next()?)?;
        let minor = parse_number(parts.next()?)?;
        let patch = parse_number(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }

    /// This build's version, from Cargo at compile time.
    pub fn running() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("the workspace version is MAJOR.MINOR.PATCH")
    }
}

fn parse_number(text: &str) -> Option<u64> {
    if text.is_empty()
        || !text.bytes().all(|byte| byte.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return None;
    }
    text.parse().ok()
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A parsed `latest.toml`. Unknown keys are refused, not ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Latest {
    pub version: Version,
    pub tarball: String,
}

/// Why a check failed, with one short user-facing sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailedReason {
    Offline,
    Http(u16),
    TooLarge,
    BadSignature,
    BadTrustedComment,
    Parse,
    NoKey,
    InvalidUrl,
}

impl FailedReason {
    pub fn message(self) -> String {
        match self {
            Self::Offline => text::OFFLINE.to_string(),
            Self::Http(code) => text::http_status(code),
            Self::TooLarge => text::TOO_LARGE.to_string(),
            Self::BadSignature => text::BAD_SIGNATURE.to_string(),
            Self::BadTrustedComment => text::BAD_TRUSTED_COMMENT.to_string(),
            Self::Parse => text::PARSE.to_string(),
            Self::NoKey => text::NO_KEY.to_string(),
            Self::InvalidUrl => text::INVALID_URL.to_string(),
        }
    }
}

/// The result of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateResult {
    UpToDate,
    Available(Version),
    Failed(FailedReason),
}

impl UpdateResult {
    /// A compact line for `GIMBAL_LOG=1`.
    pub fn log_text(self) -> String {
        match self {
            Self::UpToDate => "up to date".to_string(),
            Self::Available(version) => format!("{version} available"),
            Self::Failed(reason) => format!("failed: {}", reason.message()),
        }
    }
}

impl fmt::Display for UpdateResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.log_text())
    }
}

/// A fetch failure the check can explain without knowing the HTTP client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchFailure {
    Offline,
    Status(u16),
    TooLarge,
}

impl FetchFailure {
    fn reason(self) -> FailedReason {
        match self {
            Self::Offline => FailedReason::Offline,
            Self::Status(code) => FailedReason::Http(code),
            Self::TooLarge => FailedReason::TooLarge,
        }
    }
}

/// The network seam. Production uses [`HttpFetcher`]; tests use a small map.
pub trait Fetcher {
    fn get(&self, url: &str) -> Result<Vec<u8>, FetchFailure>;
}

/// `ureq` with rustls, asking the platform verifier for the system's CA
/// certificates rather than using a bundled root list. No cookies, compression
/// or JSON features are on.
pub struct HttpFetcher {
    agent: ureq::Agent,
}

impl HttpFetcher {
    pub fn new() -> Self {
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(FETCH_TIMEOUT))
            .user_agent(format!("gimbal-files/{}", env!("CARGO_PKG_VERSION")))
            .tls_config(tls)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Fetcher for HttpFetcher {
    fn get(&self, url: &str) -> Result<Vec<u8>, FetchFailure> {
        let mut response = self.agent.get(url).call().map_err(|error| match error {
            ureq::Error::StatusCode(code) => FetchFailure::Status(code),
            _ => FetchFailure::Offline,
        })?;

        // ureq stops reading at the limit, so a larger body never reaches a
        // Vec. The extra byte also catches a body exactly at the cap.
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_NOTICE_BYTES as u64 + 1)
            .read_to_vec()
            .map_err(|error| match error {
                ureq::Error::BodyExceedsLimit(_) => FetchFailure::TooLarge,
                _ => FetchFailure::Offline,
            })?;
        if body.len() > MAX_NOTICE_BYTES {
            return Err(FetchFailure::TooLarge);
        }
        Ok(body)
    }
}

/// Run one check. The key is optional so a build without
/// `packaging/minisign.pub` fails before touching the network.
pub fn check(
    fetcher: &dyn Fetcher,
    base_url: &str,
    key: Option<&PublicKey>,
    running: Version,
) -> UpdateResult {
    let Some(key) = key else {
        return UpdateResult::Failed(FailedReason::NoKey);
    };
    let base = match validated_base(base_url) {
        Ok(base) => base,
        Err(reason) => return UpdateResult::Failed(reason),
    };

    let latest_bytes = match fetcher.get(&format!("{base}latest.toml")) {
        Ok(body) => body,
        Err(failure) => return UpdateResult::Failed(failure.reason()),
    };
    if latest_bytes.len() > MAX_NOTICE_BYTES {
        return UpdateResult::Failed(FailedReason::TooLarge);
    }
    let latest = match parse_latest(&latest_bytes) {
        Ok(latest) => latest,
        Err(reason) => return UpdateResult::Failed(reason),
    };

    let signature_bytes = match fetcher.get(&format!("{base}latest.toml.minisig")) {
        Ok(body) => body,
        Err(failure) => return UpdateResult::Failed(failure.reason()),
    };
    if signature_bytes.len() > MAX_NOTICE_BYTES {
        return UpdateResult::Failed(FailedReason::TooLarge);
    }
    let signature_text = match std::str::from_utf8(&signature_bytes) {
        Ok(text) => text,
        Err(_) => return UpdateResult::Failed(FailedReason::BadSignature),
    };
    let signature = match Signature::decode(signature_text) {
        Ok(signature) => signature,
        Err(_) => return UpdateResult::Failed(FailedReason::BadSignature),
    };

    if key.verify(&latest_bytes, &signature, true).is_err() {
        return UpdateResult::Failed(FailedReason::BadSignature);
    }
    if !trusted_comment_matches(signature.trusted_comment(), latest.version) {
        return UpdateResult::Failed(FailedReason::BadTrustedComment);
    }

    if latest.version > running {
        UpdateResult::Available(latest.version)
    } else {
        UpdateResult::UpToDate
    }
}

/// Parse `latest.toml`, refusing unknown keys, wrong types and malformed
/// versions.
pub fn parse_latest(bytes: &[u8]) -> Result<Latest, FailedReason> {
    let text = std::str::from_utf8(bytes).map_err(|_| FailedReason::Parse)?;
    let document = Document::parse(text).map_err(|_| FailedReason::Parse)?;
    let root = document.as_table();

    for (key, _) in root.iter() {
        if !matches!(key, "version" | "tarball") {
            return Err(FailedReason::Parse);
        }
    }

    let version = root
        .get("version")
        .and_then(Item::as_str)
        .and_then(Version::parse)
        .ok_or(FailedReason::Parse)?;
    let tarball = root
        .get("tarball")
        .and_then(Item::as_str)
        .filter(|tarball| !tarball.is_empty())
        .ok_or(FailedReason::Parse)?;

    Ok(Latest {
        version,
        tarball: tarball.to_string(),
    })
}

/// The trusted comment minisign must have signed over `latest.toml`.
pub fn trusted_comment_matches(comment: &str, version: Version) -> bool {
    comment == format!("gimbal-files {version} latest")
}

/// Normalize and validate a base URL. HTTPS is open; plain HTTP is allowed
/// only for the loopback names the test server uses.
pub fn validated_base(input: &str) -> Result<String, FailedReason> {
    let input = input.trim();
    if input.is_empty() {
        return Err(FailedReason::InvalidUrl);
    }
    let base = if input.ends_with('/') {
        input.to_string()
    } else {
        format!("{input}/")
    };
    if allowed_base(&base) {
        Ok(base)
    } else {
        Err(FailedReason::InvalidUrl)
    }
}

fn allowed_base(base: &str) -> bool {
    if let Some(rest) = base.strip_prefix("https://") {
        return !rest.is_empty() && !rest.starts_with('/');
    }
    let Some(rest) = base.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let host = authority.split(':').next().unwrap_or_default();
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1"
}

/// The base URL for this build: `GIMBAL_UPDATE_URL` when set, otherwise the
/// repository's latest-release download folder.
pub fn default_base_url() -> String {
    std::env::var("GIMBAL_UPDATE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_RELEASE_BASE.to_string())
}

/// The release key for this build. In debug builds `GIMBAL_UPDATE_PUBKEY` names
/// a file that replaces the embedded key; release builds ignore it.
pub fn release_key() -> Option<PublicKey> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("GIMBAL_UPDATE_PUBKEY") {
        // The override replaces the embedded key. A bad path or file is a
        // no-key build, never a fallback to the release key.
        return fs::read_to_string(PathBuf::from(path))
            .ok()
            .and_then(|text| PublicKey::decode(&text).ok());
    }

    option_env!("GIMBAL_RELEASE_PUBKEY").and_then(|base64| PublicKey::from_base64(base64).ok())
}

/// A parsed P2u install marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallMarker {
    pub bin_dir: PathBuf,
}

/// Parse the install marker. `bin_dir` must be absolute; the other marker
/// keys (`prefix`, `data_dir`, `version`) are accepted and ignored.
pub fn parse_install_marker(text: &str) -> Option<InstallMarker> {
    let document = Document::parse(text).ok()?;
    let bin_dir = document
        .as_table()
        .get("bin_dir")
        .and_then(Item::as_str)
        .filter(|path| Path::new(path).is_absolute())?;
    Some(InstallMarker {
        bin_dir: PathBuf::from(bin_dir),
    })
}

/// Where the tarball installer writes its marker.
pub fn install_marker_path(xdg_data_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    match xdg_data_home {
        Some(directory) if directory.is_absolute() => {
            Some(directory.join(MARKER_DIR).join(MARKER_FILE))
        }
        _ => home.map(|home| {
            home.join(".local")
                .join("share")
                .join(MARKER_DIR)
                .join(MARKER_FILE)
        }),
    }
}

/// The marker path from the process environment.
pub fn default_install_marker_path() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    install_marker_path(xdg.as_deref(), home.as_deref())
}

/// Read the marker when it exists. Its `bin_dir` is canonicalized so a
/// symlinked install still matches `current_exe()`.
pub fn load_install_marker(path: Option<&Path>) -> Option<InstallMarker> {
    let text = fs::read_to_string(path?).ok()?;
    let mut marker = parse_install_marker(&text)?;
    marker.bin_dir = fs::canonicalize(&marker.bin_dir).ok()?;
    Some(marker)
}

/// How this build was installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    /// The system `.deb`, at `/usr/bin/gimbal-files`.
    System,
    /// The P2u tarball installed under `~/.local` or another prefix.
    Tarball,
    /// A source or `target/` build. These never check.
    Development,
}

impl InstallKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Tarball => "tarball",
            Self::Development => "development",
        }
    }
}

/// Classify an already-canonical executable path against the install marker.
pub fn install_kind(canonical_exe: &Path, marker: Option<&InstallMarker>) -> InstallKind {
    if canonical_exe == Path::new("/usr/bin/gimbal-files") {
        return InstallKind::System;
    }
    if let Some(marker) = marker
        && canonical_exe == marker.bin_dir.join("gimbal-files")
    {
        return InstallKind::Tarball;
    }
    InstallKind::Development
}

/// Classify this running binary, canonicalizing `current_exe()` first.
pub fn current_install_kind() -> InstallKind {
    let executable = std::env::current_exe()
        .ok()
        .and_then(|path| fs::canonicalize(path).ok());
    let marker = load_install_marker(default_install_marker_path().as_deref());
    match executable {
        Some(executable) => install_kind(&executable, marker.as_ref()),
        None => InstallKind::Development,
    }
}

/// Where the update state lives, next to the session file.
pub fn state_path(xdg_state_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    match xdg_state_home {
        Some(directory) if directory.is_absolute() => {
            Some(directory.join(STATE_DIR).join(STATE_FILE))
        }
        _ => home.map(|home| {
            home.join(".local")
                .join("state")
                .join(STATE_DIR)
                .join(STATE_FILE)
        }),
    }
}

/// The state path from the process environment.
pub fn default_state_path() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    state_path(xdg.as_deref(), home.as_deref())
}

/// The small state file: when the last check ran and the last version shown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateState {
    pub last_check: Option<u64>,
    pub notified: Option<Version>,
}

/// Read the state. A missing or malformed file means "never checked"; the
/// next automatic check repairs it.
pub fn load_state(path: &Path) -> UpdateState {
    let Ok(text) = fs::read_to_string(path) else {
        return UpdateState::default();
    };
    let Ok(document) = Document::parse(&text) else {
        return UpdateState::default();
    };
    let root = document.as_table();
    let last_check = root
        .get("last_check")
        .and_then(Item::as_integer)
        .and_then(|value| u64::try_from(value).ok());
    let notified = root
        .get("notified")
        .and_then(Item::as_str)
        .and_then(Version::parse);
    UpdateState {
        last_check,
        notified,
    }
}

/// Write the state atomically. It is a separate file from the session.
pub fn save_state(path: &Path, state: &UpdateState) -> std::io::Result<()> {
    let mut output = String::new();
    output.push_str("# SPDX-License-Identifier: GPL-3.0-or-later\n");
    if let Some(last_check) = state.last_check {
        output.push_str(&format!("last_check = {last_check}\n"));
    }
    if let Some(notified) = state.notified {
        output.push_str(&format!("notified = \"{notified}\"\n"));
    }
    fsutil::save_atomic(path, &output)
}

/// Whether a found version has not already been announced. Equal and older
/// versions stay quiet.
pub fn should_announce(notified: Option<Version>, version: Version) -> bool {
    notified.is_none_or(|notified| version > notified)
}

/// Whether the daily check is due. A future timestamp is stale, so a clock
/// correction cannot suppress checks indefinitely.
pub fn should_check(now: u64, last_check: Option<u64>) -> bool {
    match last_check {
        None => true,
        Some(last) if last > now => true,
        Some(last) => now.saturating_sub(last) >= CHECK_INTERVAL_SECS,
    }
}

/// Seconds until the next daily check. Due, missing and future timestamps
/// return zero.
pub fn seconds_until_check(now: u64, last_check: Option<u64>) -> u64 {
    match last_check {
        Some(last) if last <= now => CHECK_INTERVAL_SECS.saturating_sub(now - last),
        _ => 0,
    }
}

/// The current Unix time in seconds, saturating at the epoch.
pub fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    const LATEST_011: &str = include_str!("../tests/data/test-only-latest-0.1.1.toml");
    const LATEST_010: &str = include_str!("../tests/data/test-only-latest-0.1.0.toml");
    const SIG_011: &str = include_str!("../tests/data/test-only-latest-0.1.1.toml.minisig");
    const SIG_010: &str = include_str!("../tests/data/test-only-latest-0.1.0.toml.minisig");
    const SIG_WRONG_COMMENT: &str = include_str!("../tests/data/test-only-wrong-comment.minisig");
    const SIG_BAD: &str = include_str!("../tests/data/test-only-bad.minisig");
    const PUB: &str = include_str!("../tests/data/test-only.pub");

    fn key() -> PublicKey {
        PublicKey::decode(PUB).expect("the committed test key parses")
    }

    struct FakeFetcher {
        bodies: HashMap<String, Result<Vec<u8>, FetchFailure>>,
        calls: std::cell::Cell<usize>,
    }

    impl FakeFetcher {
        fn good(latest: &str, signature: &str) -> Self {
            let mut bodies = HashMap::new();
            bodies.insert(
                "https://example.test/latest.toml".to_string(),
                Ok(latest.as_bytes().to_vec()),
            );
            bodies.insert(
                "https://example.test/latest.toml.minisig".to_string(),
                Ok(signature.as_bytes().to_vec()),
            );
            Self {
                bodies,
                calls: std::cell::Cell::new(0),
            }
        }

        fn with_latest(result: Result<Vec<u8>, FetchFailure>) -> Self {
            let mut bodies = HashMap::new();
            bodies.insert("https://example.test/latest.toml".to_string(), result);
            bodies.insert(
                "https://example.test/latest.toml.minisig".to_string(),
                Ok(SIG_011.as_bytes().to_vec()),
            );
            Self {
                bodies,
                calls: std::cell::Cell::new(0),
            }
        }
    }

    impl Fetcher for FakeFetcher {
        fn get(&self, url: &str) -> Result<Vec<u8>, FetchFailure> {
            self.calls.set(self.calls.get() + 1);
            self.bodies
                .get(url)
                .cloned()
                .unwrap_or(Err(FetchFailure::Status(404)))
        }
    }

    fn run(latest: &str, signature: &str, running: &str) -> UpdateResult {
        let fetcher = FakeFetcher::good(latest, signature);
        check(
            &fetcher,
            "https://example.test/",
            Some(&key()),
            Version::parse(running).expect("running version"),
        )
    }

    fn temp_dir(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "gimbal-p2v-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("scratch folder");
        path
    }

    #[test]
    fn versions_compare_numerically() {
        let parse = |text| Version::parse(text).expect(text);
        assert_eq!(parse("1.2.3"), parse("1.2.3"));
        assert!(parse("1.2.4") > parse("1.2.3"));
        assert!(parse("1.3.0") > parse("1.2.99"));
        assert!(parse("2.0.0") > parse("1.99.99"));
        assert!(parse("0.10.0") > parse("0.9.0"));
        assert!(parse("0.9.1") < parse("0.10.0"));
        assert_eq!(parse("0.10.0").to_string(), "0.10.0");

        for bad in [
            "", "1", "1.2", "1.2.3.4", "1..3", "1.2.-1", "1.2.x", "v1.2.3", "01.2.3", "1.02.3",
            "1.2.03",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn latest_toml_parses_and_refuses_unknown_or_wrong_keys() {
        let latest = parse_latest(LATEST_011.as_bytes()).expect("valid notice");
        assert_eq!(latest.version, Version::parse("0.1.1").unwrap());
        assert_eq!(latest.tarball, "gimbal-files-0.1.1-x86_64-linux.tar.gz");

        for bad in [
            "version = \"0.1.1\"\n",
            "tarball = \"x\"\n",
            "version = \"0.1.1\"\ntarball = \"x\"\nextra = 1\n",
            "version = \"0.1\"\ntarball = \"x\"\n",
            "version = 1\ntarball = \"x\"\n",
            "version = \"0.1.1\"\ntarball = 1\n",
            "version = \"0.1.1\"\ntarball = \"\"\n",
            "[version]\nvalue = \"0.1.1\"\n",
            "not toml",
        ] {
            assert_eq!(
                parse_latest(bad.as_bytes()),
                Err(FailedReason::Parse),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn trusted_comments_must_name_the_file_version() {
        let version = Version::parse("0.1.1").unwrap();
        assert!(trusted_comment_matches(
            "gimbal-files 0.1.1 latest",
            version
        ));
        for bad in [
            "gimbal-files 0.1.0 latest",
            "gimbal-files 0.1.1",
            "gimbal-files 0.1.1 latest ",
            "Gimbal-files 0.1.1 latest",
        ] {
            assert!(!trusted_comment_matches(bad, version), "{bad:?}");
        }
    }

    #[test]
    fn the_committed_test_pair_verifies_and_detects_tampering() {
        let signature = Signature::decode(SIG_011).expect("signature parses");
        key()
            .verify(LATEST_011.as_bytes(), &signature, true)
            .expect("valid");
        let bad = Signature::decode(SIG_BAD).expect("signature parses");
        assert!(key().verify(LATEST_011.as_bytes(), &bad, true).is_err());

        let wrong = Signature::decode(SIG_WRONG_COMMENT).expect("signature parses");
        key()
            .verify(LATEST_011.as_bytes(), &wrong, true)
            .expect("the file is authentic, only its comment is wrong");
        assert!(!trusted_comment_matches(
            wrong.trusted_comment(),
            Version::parse("0.1.1").unwrap()
        ));
    }

    #[test]
    fn a_newer_signed_notice_is_available() {
        assert_eq!(
            run(LATEST_011, SIG_011, "0.1.0"),
            UpdateResult::Available(Version::parse("0.1.1").unwrap())
        );
    }

    #[test]
    fn an_equal_or_replayed_older_notice_is_up_to_date() {
        assert_eq!(run(LATEST_011, SIG_011, "0.1.1"), UpdateResult::UpToDate);
        assert_eq!(run(LATEST_010, SIG_010, "0.1.1"), UpdateResult::UpToDate);
        assert_eq!(run(LATEST_010, SIG_010, "0.1.2"), UpdateResult::UpToDate);
    }

    #[test]
    fn a_wrong_signed_comment_is_refused() {
        assert_eq!(
            run(LATEST_011, SIG_WRONG_COMMENT, "0.1.0"),
            UpdateResult::Failed(FailedReason::BadTrustedComment)
        );
    }

    #[test]
    fn a_missing_key_fails_without_fetching() {
        let fetcher = FakeFetcher::good(LATEST_011, SIG_011);
        let result = check(
            &fetcher,
            "https://example.test/",
            None,
            Version::parse("0.1.0").unwrap(),
        );
        assert_eq!(result, UpdateResult::Failed(FailedReason::NoKey));
        assert_eq!(fetcher.calls.get(), 0);
    }

    #[test]
    fn the_fake_fetch_layer_reports_status_size_and_signature_failures() {
        let running = Version::parse("0.1.0").unwrap();

        let fetcher = FakeFetcher::with_latest(Err(FetchFailure::Status(404)));
        assert_eq!(
            check(&fetcher, "https://example.test/", Some(&key()), running),
            UpdateResult::Failed(FailedReason::Http(404))
        );

        let fetcher = FakeFetcher::with_latest(Ok(vec![b'x'; MAX_NOTICE_BYTES + 1]));
        assert_eq!(
            check(&fetcher, "https://example.test/", Some(&key()), running),
            UpdateResult::Failed(FailedReason::TooLarge)
        );

        let fetcher = FakeFetcher::good(LATEST_011, SIG_BAD);
        assert_eq!(
            check(&fetcher, "https://example.test/", Some(&key()), running),
            UpdateResult::Failed(FailedReason::BadSignature)
        );
    }

    #[test]
    fn plain_http_is_only_allowed_on_loopback() {
        assert!(validated_base("https://github.com/x/releases/latest/download/").is_ok());
        assert!(validated_base("https://github.com/x").is_ok());
        assert!(validated_base("http://127.0.0.1:8000").is_ok());
        assert!(validated_base("http://localhost:8000/").is_ok());
        assert!(validated_base("http://LOCALHOST:8000/").is_ok());
        for bad in [
            "http://example.com/",
            "http://127.0.0.1.example.com/",
            "http://localhost.example.com/",
            "http://user@localhost/",
            "ftp://localhost/",
            "file:///tmp/",
            "",
        ] {
            assert!(validated_base(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_base_url_gains_exactly_one_trailing_slash() {
        assert_eq!(
            validated_base("https://example.test").unwrap(),
            "https://example.test/"
        );
        assert_eq!(
            validated_base("https://example.test/").unwrap(),
            "https://example.test/"
        );
    }

    #[test]
    fn a_version_is_announced_once_and_only_when_it_is_newer() {
        let one = Version::parse("0.1.1").unwrap();
        assert!(should_announce(None, one));
        assert!(should_announce(Some(Version::parse("0.1.0").unwrap()), one));
        assert!(!should_announce(Some(one), one));
        assert!(!should_announce(
            Some(Version::parse("0.1.2").unwrap()),
            one
        ));
    }

    #[test]
    fn the_daily_schedule_handles_missing_recent_and_future_times() {
        let now = 1_000_000;
        assert!(should_check(now, None));
        assert!(!should_check(now, Some(now)));
        assert!(!should_check(now, Some(now - CHECK_INTERVAL_SECS + 1)));
        assert!(should_check(now, Some(now - CHECK_INTERVAL_SECS)));
        assert!(should_check(now, Some(now - CHECK_INTERVAL_SECS - 1)));
        assert!(should_check(now, Some(now + 1)));
        assert!(should_check(now, Some(u64::MAX)));

        assert_eq!(seconds_until_check(now, None), 0);
        assert_eq!(seconds_until_check(now, Some(now)), CHECK_INTERVAL_SECS);
        assert_eq!(
            seconds_until_check(now, Some(now - CHECK_INTERVAL_SECS / 2)),
            CHECK_INTERVAL_SECS / 2
        );
        assert_eq!(seconds_until_check(now, Some(now - CHECK_INTERVAL_SECS)), 0);
        assert_eq!(
            seconds_until_check(now, Some(now - CHECK_INTERVAL_SECS - 1)),
            0
        );
        assert_eq!(seconds_until_check(now, Some(now + 1)), 0);
    }

    #[test]
    fn install_kinds_cover_system_tarball_development_and_a_symlink() {
        let system = Path::new("/usr/bin/gimbal-files");
        assert_eq!(install_kind(system, None), InstallKind::System);

        let marker = InstallMarker {
            bin_dir: PathBuf::from("/home/kyle/.local/bin"),
        };
        assert_eq!(
            install_kind(
                Path::new("/home/kyle/.local/bin/gimbal-files"),
                Some(&marker)
            ),
            InstallKind::Tarball
        );
        assert_eq!(
            install_kind(Path::new("/home/kyle/.local/bin/other"), Some(&marker)),
            InstallKind::Development
        );
        assert_eq!(
            install_kind(
                Path::new("/home/kyle/gimbal-files/target/debug/gimbal-files"),
                Some(&marker)
            ),
            InstallKind::Development
        );

        let scratch = temp_dir("symlink");
        let real = scratch.join("real");
        let link = scratch.join("link");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("gimbal-files"), b"binary").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let canonical = fs::canonicalize(link.join("gimbal-files")).unwrap();
        let marker = InstallMarker {
            bin_dir: fs::canonicalize(&real).unwrap(),
        };
        assert_eq!(
            install_kind(&canonical, Some(&marker)),
            InstallKind::Tarball
        );
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn the_marker_and_state_paths_follow_the_xdg_rules() {
        assert_eq!(
            install_marker_path(Some(Path::new("/tmp/data")), Some(Path::new("/home/kyle"))),
            Some(PathBuf::from("/tmp/data/gimbal-files/install.toml"))
        );
        assert_eq!(
            install_marker_path(Some(Path::new("relative")), Some(Path::new("/home/kyle"))),
            Some(PathBuf::from(
                "/home/kyle/.local/share/gimbal-files/install.toml"
            ))
        );
        assert_eq!(
            state_path(Some(Path::new("/tmp/state")), Some(Path::new("/home/kyle"))),
            Some(PathBuf::from("/tmp/state/gimbal-files/update.toml"))
        );
        assert_eq!(
            state_path(None, Some(Path::new("/home/kyle"))),
            Some(PathBuf::from(
                "/home/kyle/.local/state/gimbal-files/update.toml"
            ))
        );
        assert_eq!(state_path(None, None), None);
    }

    #[test]
    fn the_marker_requires_an_absolute_bin_dir_and_the_state_round_trips() {
        let marker = parse_install_marker(
            "bin_dir = \"/home/kyle/.local/bin\"\nprefix = \"/home/kyle/.local\"\nversion = \"0.1.0\"\n",
        )
        .expect("valid marker");
        assert_eq!(marker.bin_dir, PathBuf::from("/home/kyle/.local/bin"));
        assert!(parse_install_marker("bin_dir = \"relative\"\n").is_none());
        assert!(parse_install_marker("prefix = \"/home/kyle/.local\"\n").is_none());

        let scratch = temp_dir("state");
        let path = scratch.join("update.toml");
        assert_eq!(load_state(&path), UpdateState::default());
        let state = UpdateState {
            last_check: Some(1_700_000_000),
            notified: Some(Version::parse("0.1.1").unwrap()),
        };
        save_state(&path, &state).expect("save");
        assert_eq!(load_state(&path), state);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# SPDX-License-Identifier: GPL-3.0-or-later\n"));
        assert!(text.contains("last_check = 1700000000\n"));
        assert!(text.contains("notified = \"0.1.1\"\n"));
        assert!(fs::read_dir(&scratch).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp-")
        }));
        fs::remove_dir_all(&scratch).unwrap();
    }
}
