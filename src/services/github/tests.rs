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

// The cache tests below each hold their own `IssueCache`. The process-wide one
// is shared by every test thread, so a test that read its generation could see
// another test's invalidation bump it before its own store landed — which made
// `empty_answers_are_cached_as_already_stale` fail now and then.

/// The synchronous peek is what lets the render thread paint cached issues
/// on selection change instead of flashing "Loading…" (#29).
#[test]
fn peek_reports_freshness_without_io() {
    let mut cache = IssueCache::default();
    assert!(cache.peek("/nowhere/never-seen").is_none());

    let issue: GitHubIssue = serde_json::from_str(
        r#"{"number":1,"title":"T","state":"OPEN","url":"https://x/1",
                "createdAt":"2026-08-01T10:00:00Z","updatedAt":"2026-08-01T10:00:00Z",
                "author":{"login":"me"},"assignees":[],"labels":[]}"#,
    )
    .unwrap();

    cache.store_if_current(
        cache.generation,
        "/repo/fresh",
        std::slice::from_ref(&issue),
        true,
    );
    let (issues, fresh) = cache.peek("/repo/fresh").expect("cached");
    assert_eq!(issues.len(), 1);
    assert!(fresh);

    let old = Utc::now() - chrono::Duration::seconds(CACHE_TTL_SECONDS + 10);
    cache.entries.insert(
        "/repo/stale".into(),
        Cached {
            issues: vec![issue],
            fetched_at: old,
        },
    );
    let (_, fresh) = cache.peek("/repo/stale").expect("cached");
    assert!(!fresh, "an expired entry must read as stale");

    cache.invalidate("/repo/fresh");
    assert!(cache.peek("/repo/fresh").is_none());
    assert!(cache.peek("/repo/stale").is_some(), "targeted invalidation");
}

/// A fetch that started before an invalidation must not store afterwards:
/// it would re-stamp pre-invalidation data as fresh and silently undo the
/// refresh the user just asked for.
#[test]
fn an_invalidation_fences_out_older_fetches() {
    let mut cache = IssueCache::default();
    let before = cache.generation;
    cache.invalidate("/repo/fenced");
    cache.store_if_current(before, "/repo/fenced", &[], true);
    assert!(
        cache.peek("/repo/fenced").is_none(),
        "a pre-invalidation fetch must not repopulate the cache"
    );

    // A fetch that started after the invalidation stores normally.
    cache.store_if_current(cache.generation, "/repo/fenced", &[], true);
    assert!(cache.peek("/repo/fenced").is_some());
}

/// Negative answers render instantly but re-check on the next visit — a
/// transient failure must not read as a confident fresh "No issues found".
#[test]
fn empty_answers_are_cached_as_already_stale() {
    let mut cache = IssueCache::default();
    cache.store_if_current(cache.generation, "/repo/negative", &[], false);
    let (issues, fresh) = cache.peek("/repo/negative").expect("cached");
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

/// Captured from `gh pr list --json number,title,author,isDraft,headRefName,url,createdAt,updatedAt`,
/// with a second author and a draft added. The review-requested and the
/// authored query can both return one PR; it is listed once, newest first.
#[test]
fn pull_request_lists_parse_and_merge() {
    let review_requested = r#"[
      {"author":{"id":"MDQ6VXNlcjE=","is_bot":false,"login":"alice","name":"Alice"},
       "createdAt":"2026-10-09T08:00:00Z","headRefName":"fix/flaky-test","isDraft":true,
       "number":30,"title":"Fix the flaky test","updatedAt":"2026-10-09T09:00:00Z",
       "url":"https://github.com/DaanMaertens/forestui/pull/30"},
      {"author":{"id":"MDQ6VXNlcjQ0NjY5NTMz","is_bot":false,"login":"DaanMaertens","name":"Daan Maertens"},
       "createdAt":"2026-10-10T20:30:21Z","headRefName":"worktree-piped-floating-seahorse","isDraft":false,
       "number":32,"title":"Start work: free-text features","updatedAt":"2026-10-10T20:48:11Z",
       "url":"https://github.com/DaanMaertens/forestui/pull/32"}
    ]"#;
    let authored = r#"[
      {"author":{"id":"MDQ6VXNlcjQ0NjY5NTMz","is_bot":false,"login":"DaanMaertens","name":"Daan Maertens"},
       "createdAt":"2026-10-10T20:30:21Z","headRefName":"worktree-piped-floating-seahorse","isDraft":false,
       "number":32,"title":"Start work: free-text features","updatedAt":"2026-10-10T20:48:11Z",
       "url":"https://github.com/DaanMaertens/forestui/pull/32"},
      {"author":{"id":"MDQ6VXNlcjQ0NjY5NTMz","is_bot":false,"login":"DaanMaertens","name":"Daan Maertens"},
       "createdAt":"2026-10-10T19:59:07Z","headRefName":"fix/github-cache-test-race","isDraft":false,
       "number":31,"title":"Give the GitHub issue cache tests their own cache","updatedAt":"2026-10-10T20:01:20Z",
       "url":"https://github.com/DaanMaertens/forestui/pull/31"}
    ]"#;

    let prs = merge_pull_requests(&[review_requested, authored]);

    let numbers: Vec<i64> = prs.iter().map(|pr| pr.number).collect();
    assert_eq!(numbers, vec![32, 31, 30]);
    let draft = &prs[2];
    assert!(draft.is_draft);
    assert_eq!(draft.author.login, "alice");
    assert_eq!(draft.head_ref_name, "fix/flaky-test");
    assert_eq!(draft.title, "Fix the flaky test");
    assert!(!prs[0].is_draft);
}

/// A query that failed or printed nothing contributes nothing, rather than
/// emptying the list the other one returned.
#[test]
fn an_unreadable_pull_request_list_is_skipped() {
    let ok = r#"[{"author":{"login":"alice"},"createdAt":"2026-10-09T08:00:00Z",
      "headRefName":"b","isDraft":false,"number":3,"title":"T"}]"#;
    let prs = merge_pull_requests(&["", "not json", ok]);
    assert_eq!(prs.len(), 1);
}
