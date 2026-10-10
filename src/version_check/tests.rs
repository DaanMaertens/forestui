use super::*;

#[test]
fn semver_ordering_decides_staleness() {
    assert!(is_newer("2.0.0", "2.0.1"));
    assert!(is_newer("2.0.0", "2.1.0"));
    assert!(is_newer("1.9.9", "2.0.0"));
    assert!(!is_newer("2.0.1", "2.0.0"));
    assert!(!is_newer("2.0.0", "2.0.0"));
    // A pre-release precedes the release it names.
    assert!(!is_newer("2.0.0", "2.0.0-rc1"));
}

/// A tag that will not parse must never look newer: the failure mode is
/// downloading something unknown over a working binary.
#[test]
fn unparseable_versions_never_look_newer() {
    assert!(!is_newer("2.0.0", "not-a-version"));
    assert!(!is_newer("not-a-version", "2.0.0"));
    assert!(!is_newer("", ""));
}

/// The updater, `install.sh` and the release workflow all have to agree on
/// this name. Drift is a silent 404 on every update.
#[test]
fn asset_url_matches_the_release_layout() {
    let url = release_asset_url("2.0.0").expect("supported platform");
    assert!(url.starts_with("https://github.com/flipbit03/forestui/releases/download/v2.0.0/"));
    let expected = format!(
        "forestui_{}_{}",
        if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        },
        if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        }
    );
    assert!(url.ends_with(&expected), "{url} should end with {expected}");
}

#[test]
fn a_source_build_never_updates() {
    // The checked-in version is 0.0.0 and only the release workflow stamps
    // it, so this is what keeps `cargo run` from replacing itself.
    assert_eq!(is_dev_build(), current_version() == "0.0.0");
}

/// Serve the asset and its sibling `.sha256` from a throwaway local
/// server. `checksum` is the full text of the checksum file; `None` serves
/// a 404 for it, the shape of a release published without one. `requests`
/// is how many connections the caller expects `install_from` to make — a
/// rejected checksum never fetches the asset at all.
fn serve_release(
    payload: &'static [u8],
    checksum: Option<String>,
    requests: usize,
) -> (u16, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        for _ in 0..requests {
            let Ok((mut socket, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 1024];
            let n = socket.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let wants_checksum = request
                .split_whitespace()
                .nth(1)
                .is_some_and(|path| path.ends_with(".sha256"));
            let response = if wants_checksum {
                match &checksum {
                    Some(text) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        text.len(),
                        text
                    )
                    .into_bytes(),
                    None => {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .to_vec()
                    }
                }
            } else {
                let mut head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                )
                .into_bytes();
                head.extend_from_slice(payload);
                head
            };
            socket.write_all(&response).expect("respond");
        }
    });
    (port, handle)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// The directory must hold exactly the target — no staging leftovers under
/// any name.
fn assert_no_leftovers(dir: &std::path::Path) {
    let entries: Vec<_> = std::fs::read_dir(dir)
        .expect("read dir")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(
        entries,
        vec![std::ffi::OsString::from("forestui")],
        "leftover staging files"
    );
}

/// The download-and-swap is the one step that can destroy a working install,
/// so it is exercised for real: a local server, a temporary file standing in
/// for the running binary, and assertions on the bytes, the permissions and
/// the absence of leftovers.
#[tokio::test]
async fn install_from_replaces_the_target_atomically() {
    let payload = b"#!/bin/sh\necho new version\n";
    let checksum = format!("{}  forestui_linux_x86_64\n", sha256_hex(payload));
    let (port, server) = serve_release(payload, Some(checksum), 2);

    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("forestui");
    std::fs::write(&target, b"old version").expect("seed the target");

    install_from(&format!("http://127.0.0.1:{port}/asset"), &target)
        .await
        .expect("install");
    server.join().expect("server");

    assert_eq!(std::fs::read(&target).expect("read back"), payload);
    assert_no_leftovers(dir.path());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&target)
            .expect("stat")
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "the replacement is not executable");
    }
}

/// A download that does not match its published checksum must leave the
/// running install byte-identical and no staging file behind.
#[tokio::test]
async fn a_corrupted_download_leaves_the_target_untouched() {
    let payload = b"corrupted-in-transit";
    let checksum = format!(
        "{}  forestui_linux_x86_64\n",
        sha256_hex(b"what was published")
    );
    let (port, server) = serve_release(payload, Some(checksum), 2);

    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("forestui");
    std::fs::write(&target, b"old version").expect("seed the target");

    let error = install_from(&format!("http://127.0.0.1:{port}/asset"), &target)
        .await
        .expect_err("a checksum mismatch must fail");
    server.join().expect("server");

    assert!(error.to_string().contains("checksum"), "got: {error:#}");
    assert_eq!(std::fs::read(&target).expect("read back"), b"old version");
    assert_no_leftovers(dir.path());
}

/// No checksum, no update: trusting whatever arrived over the wire is the
/// failure mode the verification exists to prevent.
#[tokio::test]
async fn a_missing_checksum_aborts_the_update() {
    let payload = b"plausible payload";
    // The checksum gates the download, so the asset is never requested.
    let (port, server) = serve_release(payload, None, 1);

    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("forestui");
    std::fs::write(&target, b"old version").expect("seed the target");

    assert!(
        install_from(&format!("http://127.0.0.1:{port}/asset"), &target)
            .await
            .is_err()
    );
    server.join().expect("server");
    assert_eq!(std::fs::read(&target).expect("read back"), b"old version");
    assert_no_leftovers(dir.path());
}

/// A crashed install orphans its staged file — a multi-MB executable on
/// `$PATH`. The next install sweeps old leftovers, but must leave young
/// siblings alone: one may be a live concurrent instance mid-stage.
#[tokio::test]
async fn stale_staging_leftovers_are_swept() {
    let payload = b"#!/bin/sh\necho new version\n";
    let checksum = format!("{}  forestui_linux_x86_64\n", sha256_hex(payload));
    let (port, server) = serve_release(payload, Some(checksum), 2);

    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("forestui");
    std::fs::write(&target, b"old version").expect("seed the target");

    let stale = dir.path().join("forestui.tmp-99999");
    std::fs::write(&stale, b"crashed install").expect("seed stale leftover");
    let two_hours_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600);
    std::fs::File::options()
        .write(true)
        .open(&stale)
        .expect("open stale")
        .set_times(std::fs::FileTimes::new().set_modified(two_hours_ago))
        .expect("age the leftover");

    let fresh = dir.path().join("forestui.tmp-88888");
    std::fs::write(&fresh, b"concurrent instance").expect("seed fresh sibling");

    install_from(&format!("http://127.0.0.1:{port}/asset"), &target)
        .await
        .expect("install");
    server.join().expect("server");

    assert!(!stale.exists(), "the stale leftover was not swept");
    assert!(fresh.exists(), "a young sibling may be a live stage");
    assert_eq!(std::fs::read(&target).expect("read back"), payload);
}

/// The toast and the title suffix are the module's contract with the app:
/// a newer version marks the title for the rest of the run and raises no
/// toast; only a failed install, which carries a reason to read, is one.
#[test]
fn every_outcome_maps_to_its_toast_and_title() {
    use crate::event::Severity;

    let silent = UpdateStatus::Silent;
    assert!(silent.notification().is_none());
    assert!(silent.title_suffix(false).is_none());
    assert!(silent.title_suffix(true).is_none());

    let installed = UpdateStatus::Installed("2.0.0".into());
    assert!(
        installed.notification().is_none(),
        "a finished install is the title's to announce, not a toast's"
    );
    assert_eq!(
        installed.title_suffix(false).as_deref(),
        Some("(v2.0.0 ready — restart to update)")
    );
    assert_eq!(
        installed.title_suffix(true).as_deref(),
        Some("(v2.0.0 — restart)")
    );

    let available = UpdateStatus::Available("2.0.0".into());
    assert!(available.notification().is_none());
    assert_eq!(
        available.title_suffix(false).as_deref(),
        Some("(v2.0.0 available — cargo install forestui)")
    );
    assert_eq!(
        available.title_suffix(true).as_deref(),
        Some("(v2.0.0 available)")
    );

    let failed = UpdateStatus::InstallFailed {
        version: "2.0.0".into(),
        reason: "permission denied".into(),
    };
    let (message, severity) = failed.notification().expect("failure notifies");
    assert!(
        message.contains("could not be installed") && message.contains("permission denied"),
        "{message}"
    );
    assert!(matches!(severity, Severity::Error));
    assert!(
        failed.title_suffix(false).is_none(),
        "nothing was installed, so there is nothing to restart into"
    );
}

/// Another instance's update replaces the binary under this one; that is
/// still a build waiting for a restart. An uninstalled binary is not.
#[test]
fn a_binary_replaced_under_the_process_counts_as_installed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = dir.path().join("forestui");
    let deleted = std::path::PathBuf::from(format!("{} (deleted)", exe.display()));

    assert!(
        !replaced_in_place(&deleted),
        "nothing stands at the path yet"
    );
    std::fs::write(&exe, b"new build").expect("write replacement");
    assert!(replaced_in_place(&deleted));
    assert!(
        !replaced_in_place(&exe),
        "only the kernel's deleted marker means replaced"
    );

    // An install path that is not UTF-8 is still recognised.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let odd = dir
            .path()
            .join(std::ffi::OsStr::from_bytes(b"forest\xffui"));
        std::fs::write(&odd, b"new build").expect("write replacement");
        let mut deleted = odd.into_os_string();
        deleted.push(" (deleted)");
        assert!(replaced_in_place(std::path::Path::new(&deleted)));
    }
}

/// Network failures and unverifiable downloads retry silently; only local
/// failures — which repeat identically — are remembered and surfaced.
#[test]
fn only_local_failures_count_as_persistent() {
    let checksum = anyhow::Error::new(ChecksumError("mismatch".into()));
    assert!(failure_is_transient(&checksum));
    assert!(failure_is_transient(&checksum.context("wrapped")));

    let io = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "read-only file system",
    ));
    assert!(!failure_is_transient(&io));
    assert!(!failure_is_transient(
        &io.context("could not replace the running binary")
    ));
}

/// A failed download must leave the existing binary alone — a missed update
/// is a non-event, a broken install is not.
#[tokio::test]
async fn a_failed_download_leaves_the_target_untouched() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    // Nothing is ever served: dropping the listener refuses the connection,
    // which is the same shape as an unreachable GitHub.
    drop(listener);

    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("forestui");
    std::fs::write(&target, b"old version").expect("seed the target");

    assert!(
        install_from(&format!("http://127.0.0.1:{port}/asset"), &target)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&target).expect("read back"), b"old version");
    assert_no_leftovers(dir.path());
}

/// The memo exists to save a repeat download, not to give up on the
/// version: it applies to that version alone and expires, so an install
/// dir the user has since fixed heals without waiting for a new release.
#[test]
fn a_failed_install_is_remembered_until_the_cooldown_passes() {
    let now = 10 * INSTALL_RETRY_SECS;
    let memo = UpdateMemo {
        install_failed: Some(InstallFailure {
            version: "2.0.0".into(),
            reason: "read-only file system".into(),
            failed_at: now,
        }),
    };

    let standing = standing_failure(Some(&memo), "2.0.0", now).expect("still standing");
    assert_eq!(standing.reason, "read-only file system");

    assert!(
        standing_failure(Some(&memo), "2.0.0", now + INSTALL_RETRY_SECS - 1).is_some(),
        "inside the cooldown the download must not be re-spent"
    );
    assert!(
        standing_failure(Some(&memo), "2.0.0", now + INSTALL_RETRY_SECS).is_none(),
        "past the cooldown the install is tried again"
    );
    assert!(
        standing_failure(Some(&memo), "2.0.1", now).is_none(),
        "a newer release is a different install"
    );
    assert!(standing_failure(None, "2.0.0", now).is_none());
    assert!(standing_failure(Some(&UpdateMemo::default()), "2.0.0", now).is_none());
}

/// A memo written before `failed_at` existed defaults to the epoch, which
/// must read as expired rather than as a failure that never retries.
#[test]
fn a_memo_without_a_timestamp_has_already_expired() {
    let memo: UpdateMemo =
        serde_json::from_str(r#"{"install_failed":{"version":"2.0.0","reason":"denied"}}"#)
            .expect("an older memo still parses");
    assert!(standing_failure(Some(&memo), "2.0.0", 10 * INSTALL_RETRY_SECS).is_none());
}

#[tokio::test]
async fn a_dev_build_does_no_work() {
    if is_dev_build() {
        assert_eq!(update_if_stale().await, UpdateStatus::Silent);
    }
}
