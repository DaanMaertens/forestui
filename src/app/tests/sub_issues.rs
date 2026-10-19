use super::*;

/// An issue as `gh issue list` prints it, in `octo/demo`, with `parent` as
/// given.
fn issue_in_demo(
    number: i64,
    title: &str,
    parent: serde_json::Value,
) -> crate::models::GitHubIssue {
    serde_json::from_value(serde_json::json!({
        "number": number,
        "title": title,
        "url": format!("https://github.com/octo/demo/issues/{number}"),
        "createdAt": "2026-01-01T00:00:00Z",
        "parent": parent,
    }))
    .expect("issue fixture")
}

fn parent_12(state: &str, repo: &str) -> serde_json::Value {
    serde_json::json!({
        "number": 12,
        "title": "Epic work",
        "state": state,
        "url": format!("https://github.com/{repo}/issues/12"),
    })
}

/// Sub-issue #20 of an open #12, with #12 itself listed as a feature.
fn sub_issue_and_parent() -> Vec<crate::models::GitHubIssue> {
    let mut parent = issue_in_demo(12, "Epic work", serde_json::Value::Null);
    parent.labels = vec![crate::models::GitHubLabel {
        name: "enhancement".into(),
    }];
    vec![
        issue_in_demo(20, "Slice one", parent_12("OPEN", "octo/demo")),
        parent,
    ]
}

/// Record on the fixture's worktree `name` that it was started for issue
/// `number`.
fn link_worktree(app: &mut App, name: &str, number: i64) {
    let id = app.state.repositories()[0]
        .worktrees
        .iter()
        .find(|w| w.name == name)
        .map(|w| w.id)
        .expect("fixture worktree");
    app.state.update_worktree(id, |w| {
        w.work_item = Some(WorkItemLink::Issue { number });
    });
}

#[tokio::test]
async fn a_sub_issue_row_names_its_parent() {
    let (_dir, mut app, _) = app_with_issues(sub_issue_and_parent());
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    let screen = rendered(&mut app);
    let row = screen
        .lines()
        .find(|line| line.contains("#20 Slice one"))
        .expect("the sub-issue is listed");
    assert!(row.contains("↳ #12"), "{screen}");
    let parent_row = screen
        .lines()
        .find(|line| line.contains("#12 Epic work"))
        .expect("the parent is listed");
    assert!(!parent_row.contains('↳'), "{screen}");
}

/// The parent's one worktree is branched from without asking, and the
/// screen says so.
#[tokio::test]
async fn a_sub_issue_branches_from_its_parents_worktree() {
    let (_dir, mut app, _) = app_with_issues(sub_issue_and_parent());
    link_worktree(&mut app, "alpha", 12);
    walk_to_confirm(&mut app, "slice");

    let confirm = confirm_screen(&app);
    let parent = confirm.parent.as_ref().expect("the toggle is offered");
    assert!(parent.on, "on by default");
    assert_eq!(confirm.base_branch.value(), "feat/alpha");
    assert!(confirm.prompt().contains("gh pr create --base feat/alpha"));
    let screen = rendered(&mut app);
    assert!(
        screen.contains("[x] Branch from #12 (feat/alpha)"),
        "{screen}"
    );
    assert!(
        screen.contains("#12's branch, in worktree 'alpha'"),
        "{screen}"
    );
}

/// With no worktree for the parent, the launch makes one under the names
/// Start work gives the parent — a feature here, by its label — and the
/// screen says it will.
#[tokio::test]
async fn a_parent_without_a_worktree_is_created_by_the_launch() {
    let (_dir, mut app, _) = app_with_issues(sub_issue_and_parent());
    walk_to_confirm(&mut app, "slice");

    let confirm = confirm_screen(&app);
    let parent = confirm.parent.as_ref().expect("the toggle is offered");
    assert_eq!(
        parent.worktree,
        crate::work::ParentWorktree::Create(crate::work::WorktreeNames {
            worktree: "feat-12-epic-work".into(),
            branch: "feat/12-epic-work".into(),
        })
    );
    let screen = rendered(&mut app);
    assert!(
        screen.contains("also creates feat/12-epic-work for #12"),
        "{screen}"
    );
}

#[tokio::test]
async fn several_parent_worktrees_ask_which_one() {
    let (_dir, mut app, _) = app_with_issues(sub_issue_and_parent());
    link_worktree(&mut app, "alpha", 12);
    link_worktree(&mut app, "beta", 12);
    walk_to_confirm(&mut app, "slice");

    assert!(!confirm_screen(&app).can_launch());
    let screen = rendered(&mut app);
    assert!(
        screen.contains("#12 has 2 worktrees: pick one to branch from"),
        "{screen}"
    );
    assert!(screen.contains("pick a worktree"), "{screen}");
}

/// A closed parent, or one in another repository, offers nothing: the
/// screen is an issue's as it always was.
#[tokio::test]
async fn only_an_open_parent_in_this_repository_offers_the_toggle() {
    for parent in [
        parent_12("CLOSED", "octo/demo"),
        parent_12("OPEN", "octo/other"),
    ] {
        let (_dir, mut app, _) = app_with_issues(vec![issue_in_demo(20, "Slice one", parent)]);
        walk_to_confirm(&mut app, "slice");

        let confirm = confirm_screen(&app);
        assert!(confirm.parent.is_none());
        assert_eq!(confirm.base_branch.value(), "origin/main");
        let screen = rendered(&mut app);
        assert!(!screen.contains("Branch from"), "{screen}");
    }
}
