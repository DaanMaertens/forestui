use super::*;

#[test]
fn auth_status_display_text() {
    assert_eq!(AuthStatus::Authenticated.display(Some("kir")), "ok (kir)");
    assert_eq!(AuthStatus::Authenticated.display(None), "ok");
    assert_eq!(AuthStatus::NotAuthenticated.display(None), "unauth'd");
    assert_eq!(AuthStatus::NotInstalled.display(None), "missing");
}

/// A killed `gh` must fail, but must not masquerade as a missing binary
/// (-1 flips the sidebar to "missing"), nor as a real "not logged in"
/// (1, which `get_auth_status` caches for the lifetime of the process).
#[test]
fn signal_killed_process_is_a_plain_failure() {
    let status = std::process::Command::new("sh")
        .args(["-c", "kill -9 $$"])
        .status()
        .unwrap();
    assert!(status.code().is_none(), "expected a signal-killed process");
    let code = exit_code(status);
    assert_ne!(code, 0);
    assert_ne!(code, -1);
    assert_ne!(
        code, 1,
        "a killed probe is indistinguishable from `gh auth status` saying \
             'not logged in', which is cached and never retried"
    );
    assert_eq!(code, KILLED);
}

/// The synchronous peek is what lets the render thread paint cached issues
/// on selection change instead of flashing "Loading…" (#29).
#[test]
fn peek_reports_freshness_without_io() {
    assert!(peek_issues("/nowhere/never-seen").is_none());

    let issue: GitHubIssue = serde_json::from_str(
        r#"{"number":1,"title":"T","state":"OPEN","url":"https://x/1",
                "createdAt":"2026-08-01T10:00:00Z","updatedAt":"2026-08-01T10:00:00Z",
                "author":{"login":"me"},"assignees":[],"labels":[]}"#,
    )
    .unwrap();

    seed_cache_for_test("/repo/fresh", vec![issue.clone()], Utc::now());
    let (issues, fresh) = peek_issues("/repo/fresh").expect("cached");
    assert_eq!(issues.len(), 1);
    assert!(fresh);

    let old = Utc::now() - chrono::Duration::seconds(CACHE_TTL_SECONDS + 10);
    seed_cache_for_test("/repo/stale", vec![issue], old);
    let (_, fresh) = peek_issues("/repo/stale").expect("cached");
    assert!(!fresh, "an expired entry must read as stale");

    invalidate_cache("/repo/fresh");
    assert!(peek_issues("/repo/fresh").is_none());
    assert!(
        peek_issues("/repo/stale").is_some(),
        "targeted invalidation"
    );
}

/// A fetch that started before an invalidation must not store afterwards:
/// it would re-stamp pre-invalidation data as fresh and silently undo the
/// refresh the user just asked for.
#[test]
fn an_invalidation_fences_out_older_fetches() {
    let before = state().lock().unwrap().generation;
    invalidate_cache("/repo/fenced");
    store_if_current(before, "/repo/fenced", Vec::new(), true);
    assert!(
        peek_issues("/repo/fenced").is_none(),
        "a pre-invalidation fetch must not repopulate the cache"
    );

    // Re-read and retry: another test may invalidate concurrently, and
    // only a store with the *current* generation may land.
    for _ in 0..10 {
        let current = state().lock().unwrap().generation;
        store_if_current(current, "/repo/fenced", Vec::new(), true);
        if peek_issues("/repo/fenced").is_some() {
            break;
        }
    }
    assert!(peek_issues("/repo/fenced").is_some());
}

/// Negative answers render instantly but re-check on the next visit — a
/// transient failure must not read as a confident fresh "No issues found".
#[test]
fn empty_answers_are_cached_as_already_stale() {
    let generation = state().lock().unwrap().generation;
    store_if_current(generation, "/repo/negative", Vec::new(), false);
    let (issues, fresh) = peek_issues("/repo/negative").expect("cached");
    assert!(issues.is_empty());
    assert!(!fresh, "a negative answer must not count as fresh");
}

#[test]
fn issue_list_parses_gh_json() {
    let raw = r#"[
          {"number":7,"title":"Bug","state":"OPEN","url":"https://x/7",
           "createdAt":"2026-08-01T10:00:00Z","updatedAt":"2026-08-02T10:00:00Z",
           "author":{"login":"me"},"assignees":[{"login":"me"}],
           "labels":[{"name":"bug","color":"ff0000"}]}
        ]"#;
    let parsed: Vec<GitHubIssue> = serde_json::from_str(raw).unwrap();
    assert_eq!(parsed[0].number, 7);
    assert_eq!(parsed[0].labels[0].name, "bug");
    assert_eq!(parsed[0].title, "Bug");
}
