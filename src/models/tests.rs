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
        WorkItemLink::FreeText {
            description: "dark mode".into(),
        },
    ] {
        let mut worktree = Worktree::new("wt".into(), "b".into(), "/tmp/wt".into());
        worktree.work_item = Some(link.clone());
        let json = serde_json::to_string(&worktree).unwrap();
        let back: Worktree = serde_json::from_str(&json).unwrap();
        assert_eq!(back.work_item, Some(link), "{json}");
    }
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
