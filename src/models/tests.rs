use super::*;

#[test]
fn derive_prefix_slugifies() {
    assert_eq!(derive_prefix("YoloDisc"), "yolodisc");
    assert_eq!(derive_prefix("New Session: Opus"), "new-session-opus");
    assert_eq!(derive_prefix("--weird--"), "weird");
    assert_eq!(derive_prefix(""), "");
    assert_eq!(derive_prefix("a".repeat(40).as_str()).len(), 20);
}

#[test]
fn button_validation_matches_python_rules() {
    assert!(validate_button_label("").is_some());
    assert!(validate_button_label(&"x".repeat(21)).is_some());
    assert!(validate_button_label("ok\nno").is_some());
    assert!(validate_button_label("fine").is_none());

    assert!(validate_button_prefix("Bad").is_some());
    assert!(validate_button_prefix("good-1_2").is_none());

    assert!(validate_claude_command("").is_none());
    assert!(validate_claude_command(&"x".repeat(201)).is_some());
    assert!(validate_claude_command("claude --model opus").is_none());
}

#[test]
fn yolo_style_detection() {
    let b = CustomClaudeButton {
        label: "x".into(),
        prefix: "x".into(),
        command: "claude --dangerously-skip-permissions".into(),
    };
    assert!(b.is_yolo_style());
}

#[test]
fn active_worktrees_sorted_by_order_then_recency() {
    let mut repo = Repository::new("r".into(), "/tmp/r".into());
    let mut a = Worktree::new("a".into(), "b".into(), "/tmp/a".into());
    a.sort_order = Some(1);
    let mut b = Worktree::new("b".into(), "b".into(), "/tmp/b".into());
    b.sort_order = Some(0);
    let mut c = Worktree::new("c".into(), "b".into(), "/tmp/c".into());
    c.is_archived = true;
    repo.worktrees = vec![a, b, c];
    let names: Vec<&str> = repo
        .active_worktrees()
        .iter()
        .map(|w| w.name.as_str())
        .collect();
    assert_eq!(names, vec!["b", "a"]);
    assert_eq!(repo.archived_worktrees().len(), 1);
}

/// A file written before work item links existed loads unchanged, with
/// no link on its worktrees.
#[test]
fn a_worktree_without_a_link_loads_as_unlinked() {
    let worktree: Worktree =
        serde_json::from_str(r#"{"name":"wt","branch":"b","path":"/tmp/wt"}"#).unwrap();
    assert_eq!(worktree.work_item, None);
}

#[test]
fn every_kind_of_link_roundtrips() {
    for link in [
        WorkItemLink::Issue { number: 3 },
        WorkItemLink::PullRequest { number: 19 },
        WorkItemLink::Session {
            prompt: "dark mode".into(),
        },
    ] {
        let mut worktree = Worktree::new("wt".into(), "b".into(), "/tmp/wt".into());
        worktree.work_item = Some(link.clone());
        let json = serde_json::to_string(&worktree).unwrap();
        let back: Worktree = serde_json::from_str(&json).unwrap();
        assert_eq!(back.work_item, Some(link), "{json}");
    }
}

/// A session keeps the free-text feature's wire format, so a worktree
/// started for one before the rename keeps its link.
#[test]
fn a_session_link_is_recorded_as_free_text() {
    let link = WorkItemLink::Session {
        prompt: "dark mode".into(),
    };
    let json = serde_json::to_value(&link).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"kind": "free_text", "description": "dark mode"})
    );
    let worktree: Worktree = serde_json::from_str(
        r#"{"name":"wt","branch":"b","path":"/tmp/wt",
                "work_item":{"kind":"free_text","description":"dark mode"}}"#,
    )
    .unwrap();
    assert_eq!(worktree.work_item, Some(link));
}

/// A newer release may record a kind of link this one does not know. That
/// worktree must still load — unlinked — rather than failing the whole
/// file, which loads as *empty* state when it does not parse.
#[test]
fn a_link_of_an_unknown_kind_loads_as_unlinked() {
    let worktree: Worktree = serde_json::from_str(
        r#"{"name":"wt","branch":"b","path":"/tmp/wt",
                "work_item":{"kind":"gitlab_issue","number":1}}"#,
    )
    .unwrap();
    assert_eq!(worktree.work_item, None);
}

#[test]
fn state_roundtrips_its_json_shape() {
    let json = r#"{
          "repositories": [
            {"id": "0f2f2b7e-4d9c-4a1f-9f1a-1f2a3b4c5d6e", "name": "demo",
             "source_path": "/tmp/demo", "worktrees": [
               {"id": "1f2f2b7e-4d9c-4a1f-9f1a-1f2a3b4c5d6e", "name": "wt",
                "branch": "feat/x", "path": "/tmp/forest/demo/wt",
                "is_archived": false, "sort_order": null,
                "last_modified": "2026-08-14T18:37:28.123456Z",
                "base_branch": "origin/main", "created_from_ref": "abc1234"}]}]}"#;
    let state: AppStateData = serde_json::from_str(json).unwrap();
    assert_eq!(state.repositories.len(), 1);
    assert_eq!(state.repositories[0].worktrees[0].branch, "feat/x");
    let out = serde_json::to_string(&state).unwrap();
    assert!(out.contains("\"base_branch\":\"origin/main\""));
}

/// Two issues as `gh issue list --json ...,blockedBy` prints them: #12 waits
/// on an open #9 and a closed #10, #13 only on the closed #10.
const BLOCKED_BY_JSON: &str = r#"[
  {"number":12,"title":"Twelve","state":"OPEN","url":"https://github.com/o/r/issues/12",
   "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z",
   "author":{"login":"me"},"assignees":[],"labels":[],
   "blockedBy":{"nodes":[
     {"number":9,"state":"OPEN","title":"Nine","url":"https://github.com/o/r/issues/9"},
     {"number":10,"state":"CLOSED","title":"Ten","url":"https://github.com/o/r/issues/10"}
   ],"totalCount":2}},
  {"number":13,"title":"Thirteen","state":"OPEN","url":"https://github.com/o/r/issues/13",
   "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z",
   "author":{"login":"me"},"assignees":[],"labels":[],
   "blockedBy":{"nodes":[
     {"number":10,"state":"CLOSED","title":"Ten","url":"https://github.com/o/r/issues/10"}
   ],"totalCount":1}}
]"#;

#[test]
fn blocked_by_parses_from_gh_issue_list() {
    let issues: Vec<GitHubIssue> = serde_json::from_str(BLOCKED_BY_JSON).unwrap();
    assert_eq!(issues[0].open_blockers(), vec![9]);
    assert!(issues[0].is_blocked());
}

#[test]
fn an_issue_whose_blockers_are_all_closed_is_not_blocked() {
    let issues: Vec<GitHubIssue> = serde_json::from_str(BLOCKED_BY_JSON).unwrap();
    assert!(issues[1].open_blockers().is_empty());
    assert!(!issues[1].is_blocked());
}

#[test]
fn gh_output_without_blocked_by_counts_as_unblocked() {
    let older = r#"[{"number":5,"title":"Five","createdAt":"2026-01-01T00:00:00Z",
        "updatedAt":"2026-01-01T00:00:00Z","labels":[]},
        {"number":6,"title":"Six","createdAt":"2026-01-01T00:00:00Z",
        "updatedAt":"2026-01-01T00:00:00Z","blockedBy":null}]"#;
    let issues: Vec<GitHubIssue> = serde_json::from_str(older).unwrap();
    assert!(issues.iter().all(|issue| !issue.is_blocked()));
}

#[test]
fn blocker_state_is_compared_without_case() {
    let json = r#"{"number":1,"createdAt":"2026-01-01T00:00:00Z",
        "updatedAt":"2026-01-01T00:00:00Z",
        "blockedBy":{"nodes":[{"number":2,"state":"open"}]}}"#;
    let issue: GitHubIssue = serde_json::from_str(json).unwrap();
    assert_eq!(issue.open_blockers(), vec![2]);
}

#[test]
fn settings_without_the_blocked_key_hide_blocked_issues() {
    let settings: Settings = serde_json::from_str("{}").unwrap();
    assert!(!settings.show_blocked_issues);
    assert!(!Settings::default().show_blocked_issues);
}

#[test]
fn settings_keep_show_blocked_issues_across_a_round_trip() {
    let settings: Settings = serde_json::from_str(r#"{"show_blocked_issues":true}"#).unwrap();
    assert!(settings.show_blocked_issues);
    let back: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
    assert!(back.show_blocked_issues);
}

#[test]
fn settings_without_the_filter_key_list_every_issue() {
    let settings: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(settings.issue_filter, IssueFilter::All);
    assert_eq!(settings.start_work.bug_labels, vec!["bug".to_string()]);
}

#[test]
fn settings_keep_the_issue_filter_across_a_round_trip() {
    let settings: Settings = serde_json::from_str(r#"{"issue_filter":"bugs"}"#).unwrap();
    assert_eq!(settings.issue_filter, IssueFilter::Bugs);
    let back: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
    assert_eq!(back.issue_filter, IssueFilter::Bugs);
}

/// A filter value a newer build wrote loads as All, and the rest of the file
/// with it — not the whole file as defaults.
#[test]
fn an_unknown_issue_filter_loads_as_all_and_keeps_the_rest() {
    let settings: Settings =
        serde_json::from_str(r#"{"issue_filter":"chores","show_blocked_issues":true}"#).unwrap();
    assert_eq!(settings.issue_filter, IssueFilter::All);
    assert!(settings.show_blocked_issues);
}

/// A file from before the back key loads with it on, as prefix + ←.
#[test]
fn settings_without_the_back_key_load_it_on_left() {
    let settings: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(settings.back_key.as_deref(), Some("Left"));
    assert_eq!(settings.back_key_accepted, None);
    assert_eq!(Settings::default().back_key.as_deref(), Some("Left"));
}

/// Off is saved as `null`, which must not load back as the default.
#[test]
fn a_back_key_turned_off_stays_off_across_a_round_trip() {
    let settings = Settings {
        back_key: None,
        ..Settings::default()
    };
    let json = serde_json::to_string(&settings).unwrap();
    assert!(json.contains(r#""back_key":null"#), "{json}");
    let back: Settings = serde_json::from_str(&json).unwrap();
    assert_eq!(back.back_key, None);
}

#[test]
fn a_chosen_back_key_and_its_accepted_binding_survive_a_round_trip() {
    let settings: Settings =
        serde_json::from_str(r#"{"back_key":"b","back_key_accepted":"previous-window"}"#).unwrap();
    let back: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
    assert_eq!(back.back_key.as_deref(), Some("b"));
    assert_eq!(back.back_key_accepted.as_deref(), Some("previous-window"));
}
