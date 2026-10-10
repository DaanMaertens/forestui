//! Startup self-update.
//!
//! The Python build shelled out to `uv tool upgrade forestui` on every launch.
//! There is no sane Cargo equivalent — `cargo install` would recompile the crate
//! at startup — so the binary path replaces it: download the release asset for
//! this platform and swap it over the running executable.
//!
//! Two things make that safe to do automatically rather than behind a
//! subcommand, which is where this differs from `lineark` and `terminal-use`:
//!
//! - It never blocks the UI. The check runs on a background task after the
//!   terminal is up, on every launch — nothing is cached, so a release is
//!   picked up as soon as it exists. A newer version marks the title bar for
//!   the rest of the run rather than raising a toast; anything the network is
//!   responsible for stays silent (offline is the common case, not the user's
//!   problem); only a *persistent local* failure — an
//!   unwritable install dir — surfaces as an error, and is remembered for an
//!   hour so the download is not re-spent on every launch meanwhile.
//! - It only ever replaces a binary that came from a release. A `cargo install`
//!   build defers to cargo, and a source build (version `0.0.0`) does nothing at
//!   all, so `cargo run` in a checkout is never overwritten.
//! - It never installs what it cannot verify: the downloaded bytes must match
//!   the `.sha256` published beside the asset, so a release must ship its
//!   checksums for the updater to accept it.

use anyhow::{Context, Result};
#[cfg(any(feature = "binary-release", test))]
use serde::{Deserialize, Serialize};
#[cfg(feature = "binary-release")]
use std::path::PathBuf;
#[cfg(feature = "binary-release")]
use std::time::{SystemTime, UNIX_EPOCH};

const GITHUB_REPO: &str = "flipbit03/forestui";
/// How long a failed install is taken at its word before the download is spent
/// again. Short enough that an install dir the user has since made writable
/// heals on its own, long enough that a persistent failure is not re-downloaded
/// every time forestui is opened.
#[cfg(any(feature = "binary-release", test))]
const INSTALL_RETRY_SECS: u64 = 60 * 60;
const USER_AGENT: &str = "forestui-self-update";

/// The compiled-in version, stamped from the git tag by the release workflow.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A build from the repository rather than a release. The checked-in version is
/// `0.0.0`, which is also what auto-enables dev mode, so this is the same signal
/// that already keeps a source build out of the release window naming.
pub fn is_dev_build() -> bool {
    current_version() == "0.0.0"
}

/// Is `latest` newer than `current`?
///
/// Anything that will not parse is treated as "not newer". The failure mode
/// worth avoiding is a malformed tag upstream convincing a working build to
/// download something over itself.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (
        semver::Version::parse(current),
        semver::Version::parse(latest),
    ) {
        (Ok(c), Ok(l)) => l > c,
        _ => false,
    }
}

/// The only thing worth carrying between launches.
///
/// The version lookup is not: it runs on every launch, so a release reaches the
/// user the next time they open forestui rather than up to a day later. A
/// failed install is, because it repeats — an unwritable install dir fails
/// identically every time, and rediscovering that costs a multi-MB download.
///
/// The filename is kept from when this cached the lookup itself: an older build
/// reading it simply fails to parse and looks the version up, which is now the
/// behaviour anyway, and nothing is left littering the config dir.
#[cfg(any(feature = "binary-release", test))]
#[derive(Debug, Default, Serialize, Deserialize)]
struct UpdateMemo {
    /// Set when downloading or installing a known-newer version failed for a
    /// local reason; cleared by the retry cooldown, a newer release, or a
    /// successful install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    install_failed: Option<InstallFailure>,
}

#[cfg(any(feature = "binary-release", test))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct InstallFailure {
    version: String,
    reason: String,
    /// When it happened. The memo saves bandwidth, it is not a verdict: past
    /// the cooldown the install is tried again.
    #[serde(default)]
    failed_at: u64,
}

#[cfg(feature = "binary-release")]
fn memo_path() -> PathBuf {
    crate::util::home_dir()
        .join(".config")
        .join("forestui")
        .join("latest_version_check.json")
}

#[cfg(feature = "binary-release")]
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(feature = "binary-release")]
fn read_memo() -> Option<UpdateMemo> {
    serde_json::from_str(&std::fs::read_to_string(memo_path()).ok()?).ok()
}

#[cfg(feature = "binary-release")]
fn write_memo(memo: &UpdateMemo) {
    let path = memo_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(memo) {
        // Atomic like the config files: the memo is shared by concurrent
        // instances, so a torn write must not be able to silently drop it.
        let _ = crate::util::write_atomically(&path, &json);
    }
}

async fn fetch_latest_version() -> Result<String> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    // Without a timeout, a network that accepts the connection but never
    // answers parks this task forever — and with it the whole check, which now
    // runs on every launch with nothing cached to fall back on.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("could not build the HTTP client")?;
    let body: serde_json::Value = client
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .context("could not reach the GitHub API")?
        .error_for_status()
        .context("the GitHub API returned an error")?
        .json()
        .await
        .context("could not parse the GitHub API response")?;

    let tag = body["tag_name"]
        .as_str()
        .context("no tag_name in the latest release")?;
    Ok(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

/// Release asset for the platform this binary is running on.
///
/// Only the in-place updater needs this, so without `binary-release` it would be
/// dead code — but the tests assert it against the layout `install.sh` and the
/// release workflow use, and that agreement is worth checking on every build.
#[cfg(any(feature = "binary-release", test))]
pub fn release_asset_url(version: &str) -> Result<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        other => anyhow::bail!("unsupported OS: {other}"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => anyhow::bail!("unsupported architecture: {other}"),
    };
    if os == "macos" && arch == "x86_64" {
        anyhow::bail!("macOS x86_64 binaries are not published — use `cargo install forestui`");
    }
    Ok(format!(
        "https://github.com/{GITHUB_REPO}/releases/download/v{version}/forestui_{os}_{arch}"
    ))
}

/// Download `url`, verify it against its published `.sha256`, and put it at
/// `target`, atomically.
///
/// The release workflow publishes a checksum beside every asset and
/// `install.sh` verifies it; the updater holds itself to the same standard —
/// stricter, in fact: a missing or malformed checksum aborts the update rather
/// than trusting whatever arrived over the wire, because the updater lives
/// inside the binary it would break and the user cannot self-recover from a
/// corrupt install.
///
/// Staged beside the target under a per-process name and renamed into place, so
/// the swap is atomic, a half-downloaded file can never end up executable, and
/// two instances updating at once cannot rename each other's half-written
/// staging file over the real binary. Replacing the file under a running
/// process is safe on Unix — the running image is already mapped — which is why
/// this can happen while the UI is up, with the new build taking effect on the
/// next launch.
///
/// Takes its URL and destination rather than deriving them, so the download and
/// the swap can be tested against a local server and a temporary file. Deciding
/// *whether* to run it is the caller's job, and that is what carries the
/// `binary-release` gate.
#[cfg(any(feature = "binary-release", test))]
pub(crate) async fn install_from(url: &str, target: &std::path::Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        // Idle-based, not a total deadline: the asset is several MB, and a
        // slow-but-moving link must not be killed mid-transfer. What the
        // timeout exists for is a connection that stops answering.
        .read_timeout(std::time::Duration::from_secs(30))
        .build()
        .context("could not build the HTTP client")?;

    // The checksum is a hundred bytes and gates the multi-MB download; fetch
    // it first so its absence or unreachability costs nothing.
    let checksum_body = client
        .get(format!("{url}.sha256"))
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .context("could not fetch the release checksum")?
        .error_for_status()
        .context("the release checksum request failed")?
        .text()
        .await
        .context("could not read the release checksum")?;
    // Coreutils format: `<64 hex>  <filename>`. Only the hash is trusted.
    let expected = checksum_body
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ChecksumError("the release checksum file is malformed".into()).into());
    }

    let bytes = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .context("could not download the release")?
        .error_for_status()
        .context("the release download failed")?
        .bytes()
        .await
        .context("could not read the downloaded release")?;

    use sha2::Digest;
    let actual = format!("{:x}", sha2::Sha256::digest(&bytes));
    if actual != expected {
        return Err(ChecksumError(
            "the downloaded release does not match its published checksum".into(),
        )
        .into());
    }

    // The swap reuses the config files' atomic writer: sibling temp file with a
    // per-pid name (concurrent instances stage privately), fsync before the
    // rename and the directory after it (a power loss must not land the rename
    // without the bytes), write-through for a symlinked install path, and the
    // target's own mode carried over — with 0o755 as the floor that keeps the
    // binary runnable.
    let target = target.to_path_buf();
    tokio::task::spawn_blocking(move || {
        sweep_stale_staging(&target);
        crate::util::write_bytes_atomically(&target, &bytes, Some(0o755))
    })
    .await
    .context("the install task was cancelled")?
    .context("could not replace the running binary")?;
    Ok(())
}

/// A download that could not be verified. Its own type so the caller can tell
/// it from a local install failure: an unverifiable download is retried on the
/// next launch, never memoized as persistent.
#[cfg(any(feature = "binary-release", test))]
#[derive(Debug)]
pub(crate) struct ChecksumError(String);

#[cfg(any(feature = "binary-release", test))]
impl std::fmt::Display for ChecksumError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(any(feature = "binary-release", test))]
impl std::error::Error for ChecksumError {}

/// Remove staging leftovers from crashed installs. A SIGKILL or power loss
/// between write and rename orphans `<name>.tmp-<pid>` — a multi-MB, possibly
/// executable file on `$PATH` that nothing else ever looks at again. Only
/// files older than an hour go: a younger sibling may be a live concurrent
/// instance mid-stage.
#[cfg(any(feature = "binary-release", test))]
fn sweep_stale_staging(target: &std::path::Path) {
    let Some(parent) = target.parent() else {
        return;
    };
    let Some(name) = target.file_name().map(|n| n.to_string_lossy().to_string()) else {
        return;
    };
    let prefix = format!("{name}.tmp-");
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|modified| {
                modified.elapsed().unwrap_or_default() > std::time::Duration::from_secs(3600)
            })
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// What the startup update check concluded.
///
/// The enum is the cfg-independent contract: which variants a build actually
/// constructs depends on the `binary-release` gate (`Installed`/`InstallFailed`
/// with it, `Available` without), so each side sees the others as dead.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Nothing to say: already current, a source build, or no answer from
    /// GitHub. Being offline is the common case, not the user's problem.
    Silent,
    /// The new build is already in place; takes effect on the next launch.
    Installed(String),
    /// A newer version exists. Installed by cargo, so replacing the binary in
    /// place would be undone by the next `cargo install`, and recompiling the
    /// crate underneath a running TUI is not something to do unasked.
    Available(String),
    /// A newer version exists but could not be installed. Unlike being
    /// offline, this is actionable (an unwritable install dir, a corrupted
    /// mirror) and would otherwise repeat invisibly on every launch.
    InstallFailed { version: String, reason: String },
}

impl UpdateStatus {
    /// The toast this outcome earns, if any.
    ///
    /// Only a failure is a toast. A newer version is a standing fact about
    /// this process until it restarts, so it lives in the title bar instead
    /// (`title_suffix`), where it stays put rather than timing out while the
    /// user is looking at something else.
    pub fn notification(&self) -> Option<(String, crate::event::Severity)> {
        use crate::event::Severity;
        match self {
            UpdateStatus::InstallFailed { version, reason } => Some((
                format!("forestui v{version} is available but could not be installed: {reason}"),
                Severity::Error,
            )),
            UpdateStatus::Silent | UpdateStatus::Installed(_) | UpdateStatus::Available(_) => None,
        }
    }

    /// What the title bar appends to `forestui v<current>` for the rest of
    /// this run, if anything. `compact` is the form for a bar too narrow to
    /// hold the full one: it keeps the version and the one thing to do.
    pub fn title_suffix(&self, compact: bool) -> Option<String> {
        match (self, compact) {
            (UpdateStatus::Installed(version), false) => {
                Some(format!("(v{version} ready — restart to update)"))
            }
            (UpdateStatus::Installed(version), true) => Some(format!("(v{version} — restart)")),
            (UpdateStatus::Available(version), false) => {
                Some(format!("(v{version} available — cargo install forestui)"))
            }
            (UpdateStatus::Available(version), true) => Some(format!("(v{version} available)")),
            (UpdateStatus::Silent | UpdateStatus::InstallFailed { .. }, _) => None,
        }
    }
}

/// Bring the installed binary up to date.
///
/// The lookup happens on every launch. It runs on a background task with the
/// UI already up, so nobody waits on the round trip, and the reward is that a
/// release lands the next time forestui is opened instead of whenever a cached
/// answer happened to expire.
///
/// A lookup that fails is not an answer, and there is no remembered version to
/// act on in its place: offline, a rate-limited API, DNS that never came back
/// — every one of them is silence, not a notification.
pub async fn update_if_stale() -> UpdateStatus {
    if is_dev_build() {
        return UpdateStatus::Silent;
    }
    let Ok(latest) = fetch_latest_version().await else {
        return UpdateStatus::Silent;
    };
    if !is_newer(current_version(), &latest) {
        return UpdateStatus::Silent;
    }

    #[cfg(feature = "binary-release")]
    {
        install_update(&latest).await
    }
    #[cfg(not(feature = "binary-release"))]
    {
        UpdateStatus::Available(latest)
    }
}

/// A failed install of `latest` still inside its cooldown: the same attempt
/// would fail the same way, so it is reported from memory rather than
/// re-downloaded. Past the cooldown — or for any other version — there is
/// nothing standing and the install is tried again.
#[cfg(any(feature = "binary-release", test))]
fn standing_failure(memo: Option<&UpdateMemo>, latest: &str, now: u64) -> Option<InstallFailure> {
    memo.and_then(|m| m.install_failed.clone())
        .filter(|f| f.version == latest && now.saturating_sub(f.failed_at) < INSTALL_RETRY_SECS)
}

/// Whether a running executable that no longer exists at its own path was
/// replaced rather than removed: Linux reports a replaced binary as
/// `<path> (deleted)`, and the replacement sits at `<path>`.
///
/// What it was replaced *with* is not checked. The window this can happen in
/// is the seconds between launch and the check finishing, and what lands in
/// it is another instance's install of the same `latest` this one just
/// looked up — so reporting that version is a guess, but a safe one.
/// Compared as bytes so an install path that is not UTF-8 is still seen.
#[cfg(any(feature = "binary-release", test))]
fn replaced_in_place(current: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        current
            .as_os_str()
            .as_bytes()
            .strip_suffix(b" (deleted)")
            .is_some_and(|original| {
                std::path::Path::new(std::ffi::OsStr::from_bytes(original)).is_file()
            })
    }
    // Only Linux reports a replaced executable this way.
    #[cfg(not(unix))]
    {
        let _ = current;
        false
    }
}

#[cfg(feature = "binary-release")]
async fn install_update(latest: &str) -> UpdateStatus {
    // One read, threaded through the guard and the writes below: the file is
    // read-modify-write state shared with concurrent instances, and every
    // extra read widens the race.
    let memo = read_memo();

    if let Some(failure) = standing_failure(memo.as_ref(), latest, now_secs()) {
        return UpdateStatus::InstallFailed {
            version: failure.version,
            reason: failure.reason,
        };
    }

    let result = async {
        let current = std::env::current_exe().context("could not locate the running executable")?;
        // On Linux `/proc/self/exe` reads "<path> (deleted)" once another
        // instance's update has replaced the binary under this process. The
        // new build is already in place; installing to that literal name would
        // only strand a copy beside it. This process is still the old build,
        // though, so it is just as much "restart to update" as if it had done
        // the install itself — provided something really does stand at the
        // original path, rather than the binary having been uninstalled.
        if !current.exists() {
            return Ok(replaced_in_place(&current));
        }
        install_from(&release_asset_url(latest)?, &current)
            .await
            .map(|()| true)
    }
    .await;

    match result {
        Ok(false) => UpdateStatus::Silent,
        Ok(true) => {
            // Clear a memo left by an older release's failed install.
            if memo.is_some_and(|m| m.install_failed.is_some()) {
                write_memo(&UpdateMemo::default());
            }
            UpdateStatus::Installed(latest.to_string())
        }
        Err(error) => {
            // The network, a mid-transfer drop, a release published before its
            // assets finished uploading, a download that failed verification:
            // all transient. Retried silently on the next launch — being
            // offline is not the user's problem.
            if failure_is_transient(&error) {
                return UpdateStatus::Silent;
            }
            // What remains is local and will repeat identically (an unwritable
            // install dir): remember it and say so.
            let reason = format!("{error:#}");
            write_memo(&UpdateMemo {
                install_failed: Some(InstallFailure {
                    version: latest.to_string(),
                    reason: reason.clone(),
                    failed_at: now_secs(),
                }),
            });
            UpdateStatus::InstallFailed {
                version: latest.to_string(),
                reason,
            }
        }
    }
}

/// Whether a failed install should be retried silently rather than remembered
/// and surfaced. Network-level failures and unverifiable downloads are
/// transient; local filesystem failures repeat identically until the user acts.
#[cfg(any(feature = "binary-release", test))]
fn failure_is_transient(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<reqwest::Error>().is_some()
            || cause.downcast_ref::<ChecksumError>().is_some()
    })
}

#[cfg(test)]
mod tests;
