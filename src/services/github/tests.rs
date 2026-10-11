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
    assert!(cache.peek("/nowhere/never-seen", Scope::Mine).is_none());

    let issue: GitHubIssue = serde_json::from_str(
        r#"{"number":1,"title":"T","state":"OPEN","url":"https://x/1",
                "createdAt":"2026-08-01T10:00:00Z","updatedAt":"2026-08-01T10:00:00Z",
                "author":{"login":"me"},"assignees":[],"labels":[]}"#,
    )
    .unwrap();

    cache.store_if_current(
        cache.generation,
        "/repo/fresh",
        Scope::Mine,
        std::slice::from_ref(&issue),
        true,
    );
    let (issues, fresh) = cache.peek("/repo/fresh", Scope::Mine).expect("cached");
    assert_eq!(issues.len(), 1);
    assert!(fresh);

    let old = Utc::now() - chrono::Duration::seconds(CACHE_TTL_SECONDS + 10);
    cache.entries.insert(
        ("/repo/stale".into(), Scope::Mine),
        Cached {
            issues: vec![issue],
            fetched_at: old,
        },
    );
    let (_, fresh) = cache.peek("/repo/stale", Scope::Mine).expect("cached");
    assert!(!fresh, "an expired entry must read as stale");

    cache.invalidate("/repo/fresh");
    assert!(cache.peek("/repo/fresh", Scope::Mine).is_none());
    assert!(
        cache.peek("/repo/stale", Scope::Mine).is_some(),
        "targeted invalidation"
    );
}

/// A fetch that started before an invalidation must not store afterwards:
/// it would re-stamp pre-invalidation data as fresh and silently undo the
/// refresh the user just asked for.
#[test]
fn an_invalidation_fences_out_older_fetches() {
    let mut cache = IssueCache::default();
    let before = cache.generation;
    cache.invalidate("/repo/fenced");
    cache.store_if_current(before, "/repo/fenced", Scope::Mine, &[], true);
    assert!(
        cache.peek("/repo/fenced", Scope::Mine).is_none(),
        "a pre-invalidation fetch must not repopulate the cache"
    );

    // A fetch that started after the invalidation stores normally.
    cache.store_if_current(cache.generation, "/repo/fenced", Scope::Mine, &[], true);
    assert!(cache.peek("/repo/fenced", Scope::Mine).is_some());
}

/// Negative answers render instantly but re-check on the next visit — a
/// transient failure must not read as a confident fresh "No issues found".
#[test]
fn empty_answers_are_cached_as_already_stale() {
    let mut cache = IssueCache::default();
    cache.store_if_current(cache.generation, "/repo/negative", Scope::Mine, &[], false);
    let (issues, fresh) = cache.peek("/repo/negative", Scope::Mine).expect("cached");
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

/// A list fetched for one scope is never served for the other: switching
/// to `all` must not show the `mine` list as though it were everything.
#[test]
fn the_cache_answers_only_for_the_scope_it_was_fetched_for() {
    let mut cache = IssueCache::default();
    cache.store_if_current(cache.generation, "/repo/scoped", Scope::Mine, &[], true);
    assert!(cache.peek("/repo/scoped", Scope::Mine).is_some());
    assert!(cache.peek("/repo/scoped", Scope::All).is_none());

    cache.store_if_current(cache.generation, "/repo/scoped", Scope::All, &[], true);
    assert!(cache.peek("/repo/scoped", Scope::All).is_some());
}

/// Both scopes are held side by side: Start work listing `all` must not
/// evict the `mine` list the repository pane shows. Invalidating a path
/// drops both.
#[test]
fn the_cache_holds_both_scopes_of_a_path() {
    let mut cache = IssueCache::default();
    cache.store_if_current(cache.generation, "/repo/both", Scope::Mine, &[], true);
    cache.store_if_current(cache.generation, "/repo/both", Scope::All, &[], true);
    assert!(cache.peek("/repo/both", Scope::Mine).is_some());
    assert!(cache.peek("/repo/both", Scope::All).is_some());

    cache.invalidate("/repo/both");
    assert!(cache.peek("/repo/both", Scope::Mine).is_none());
    assert!(cache.peek("/repo/both", Scope::All).is_none());
}

/// `mine` keeps the two lists it always asked for; `all` asks once, with no
/// filter, for every open item.
#[test]
fn the_scope_decides_the_gh_filters() {
    assert_eq!(
        issue_filters(Scope::Mine),
        &[&["--assignee", "@me"][..], &["--author", "@me"][..]]
    );
    assert_eq!(
        pull_request_filters(Scope::Mine),
        &[
            &["--search", "review-requested:@me"][..],
            &["--author", "@me"][..]
        ]
    );
    let unfiltered: &[&[&str]] = &[&[]];
    assert_eq!(issue_filters(Scope::All), unfiltered);
    assert_eq!(pull_request_filters(Scope::All), unfiltered);
}

/// A `gh` from before issue dependencies refuses the whole call over the
/// one field it does not know; only that refusal is worth a retry without it.
#[test]
fn only_an_unknown_blocked_by_field_asks_for_the_older_fields() {
    let refusal = "Unknown JSON field: \"blockedBy\"\nAvailable fields:\n  assignees\n  author";
    assert!(rejects_blocked_by(refusal));
    assert!(!rejects_blocked_by("HTTP 502: Bad Gateway"));
    assert!(!rejects_blocked_by(""));
    assert!(ISSUE_FIELDS.ends_with(",blockedBy"));
    assert_eq!(
        format!("{LEGACY_ISSUE_FIELDS},blockedBy"),
        ISSUE_FIELDS,
        "the fallback differs from the full list by blockedBy alone"
    );
}

/// Captured from `gh issue view <n> --json state` and
/// `gh pr view <n> --json state`.
#[test]
fn item_state_parses_what_gh_prints() {
    assert_eq!(
        parse_item_state(r#"{"state":"OPEN"}"#),
        Some(ItemState::Open)
    );
    assert_eq!(
        parse_item_state(r#"{"state":"CLOSED"}"#),
        Some(ItemState::Closed)
    );
    assert_eq!(
        parse_item_state(r#"{"state":"MERGED"}"#),
        Some(ItemState::Merged)
    );
}

/// An answer that is not one of the three states says nothing — never
/// "open", which would unmark a finished worktree over a parse failure.
#[test]
fn an_unreadable_item_state_is_unknown() {
    assert_eq!(parse_item_state(""), None);
    assert_eq!(parse_item_state("not json"), None);
    assert_eq!(parse_item_state(r#"{"state":"DRAFT"}"#), None);
    assert_eq!(parse_item_state(r#"{"number":4}"#), None);
}
