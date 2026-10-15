use super::*;

use crate::models::Scope;

/// Alice's PR #30 and my own #31, as `gh` would list them for `daan`.
fn two_prs() -> Vec<crate::models::GitHubPullRequest> {
    let mut draft = open_pr(30, "Fix the flaky test", "alice", "fix/flaky-test");
    draft.is_draft = true;
    vec![
        draft,
        open_pr(
            31,
            "Cache tests get their own cache",
            "daan",
            "fix/cache-race",
        ),
    ]
}

/// `s`, then `pr-review`, with the PR list already back from `gh`.
fn open_pr_review(app: &mut App, path: &str) {
    choose_pr_review(app);
    app.handle_event(AppEvent::PullRequests {
        scope: Scope::Mine,
        path: path.to_string(),
        result: Ok(pr_answer(two_prs(), Some("daan".into()))),
    });
}

fn listed_prs(app: &App) -> Vec<i64> {
    let picker = start_work(app).picker.as_ref().expect("PRs loaded");
    picker
        .entries()
        .filter_map(|e| match e {
            crate::work::ItemEntry::PullRequest(entry) => Some(entry.pr.number),
            _ => None,
        })
        .collect()
}

/// `pr-review` is the third mode, and its list waits for the PRs rather
/// than showing the issues meanwhile.
#[tokio::test]
async fn pr_review_lists_the_pull_requests_once_they_load() {
    let (_dir, mut app, path) = app_with_issues(two_issues());
    app.handle_key(key(KeyCode::Char('s')));
    highlight_mode(&mut app, crate::work::WorkMode::PrReview);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Item);
    assert!(start_work(&app).picker.is_none(), "still loading");
    assert!(rendered(&mut app).contains("Loading pull requests"));

    app.handle_event(AppEvent::PullRequests {
        scope: Scope::Mine,
        path: path.clone(),
        result: Ok(pr_answer(two_prs(), Some("daan".into()))),
    });

    assert_eq!(listed_prs(&app), vec![30, 31]);
    let screen = rendered(&mut app);
    let row = screen
        .lines()
        .find(|l| l.contains("#30 Fix the flaky test"))
        .expect("the PR is listed");
    assert!(row.contains("draft"), "{row}");
    assert!(screen.contains("2 PRs"), "{screen}");
}

/// Someone else's PR opens the confirm screen straight away — there is no
/// base branch to load — as a review that changes no code, on the PR's
/// own branch.
#[tokio::test]
async fn someone_elses_pr_confirms_as_a_report_only_review() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    open_pr_review(&mut app, &path);
    type_text(&mut app, "flaky");
    app.handle_key(key(KeyCode::Enter));

    let confirm = confirm_screen(&app);
    assert!(matches!(
        confirm.item,
        crate::work::WorkItem::PullRequest {
            number: 30,
            review: crate::work::ReviewKind::Others,
            ..
        }
    ));
    assert_eq!(confirm.name.value(), "pr-30-fix-the-flaky-test");
    assert_eq!(confirm.branch.value(), "fix/flaky-test");
    assert!(confirm.can_launch(), "no base branch to wait for");
    let prompt = confirm.prompt();
    assert!(prompt.contains("gh pr view 30 --comments"), "{prompt}");
    assert!(prompt.contains("Do not change any code"), "{prompt}");

    let screen = rendered(&mut app);
    assert!(screen.contains("Start Work: PR #30"), "{screen}");
    assert!(screen.contains("Review someone else's PR"), "{screen}");
    assert!(!screen.contains("Base Branch"), "{screen}");
    assert!(!screen.contains("Pull repo"), "{screen}");
}

/// My own PR gets the self-review, and the screen says so — with nothing
/// to switch it.
#[tokio::test]
async fn my_own_pr_confirms_as_a_self_review() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    open_pr_review(&mut app, &path);
    type_text(&mut app, "cache");
    app.handle_key(key(KeyCode::Enter));

    let confirm = confirm_screen(&app);
    assert!(matches!(
        confirm.item,
        crate::work::WorkItem::PullRequest {
            number: 31,
            review: crate::work::ReviewKind::Own,
            ..
        }
    ));
    let prompt = confirm.prompt();
    assert!(prompt.contains("wait for my go-ahead"), "{prompt}");
    let screen = rendered(&mut app);
    assert!(screen.contains("Review my own PR"), "{screen}");
    assert!(!screen.contains("Review someone else's PR"), "{screen}");
}

/// The focus ring skips the controls a PR review does not have, so Tab
/// never lands on something that is not drawn.
#[tokio::test]
async fn the_pr_confirm_screen_only_focuses_what_it_shows() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    open_pr_review(&mut app, &path);
    app.handle_key(key(KeyCode::Enter));

    let mut seen = Vec::new();
    for _ in 0..6 {
        seen.push(confirm_screen(&app).focus);
        app.handle_key(key(KeyCode::Tab));
    }
    use crate::modal::StartWorkConfirmModal as C;
    assert_eq!(
        seen,
        vec![
            C::FOCUS_NAME,
            C::FOCUS_EDIT,
            C::FOCUS_YOLO,
            C::FOCUS_LAUNCH,
            C::FOCUS_CANCEL,
            C::FOCUS_NAME
        ]
    );
}

/// Launch records the PR as the worktree's work item, and asks for no
/// base branch and no pull: `gh pr checkout` decides what is checked out.
#[tokio::test]
async fn launching_a_pr_review_records_the_pull_request() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    open_pr_review(&mut app, &path);
    app.handle_key(key(KeyCode::Enter));

    let crate::modal::ModalResult::WorktreeForWorkItem {
        link,
        name,
        branch,
        pull_first,
        base_branch,
        ..
    } = submitted(&mut app)
    else {
        panic!("a launch expected");
    };
    assert_eq!(link, WorkItemLink::PullRequest { number: 30 });
    assert_eq!(name, "pr-30-fix-the-flaky-test");
    assert_eq!(branch, "fix/flaky-test");
    assert!(!pull_first);
    assert_eq!(base_branch, None);
}

/// A PR that already has a worktree is marked in the list, and picking it
/// jumps there instead of confirming a second one.
#[tokio::test]
async fn picking_a_started_pr_jumps_to_its_worktree() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    let (beta, beta_path) = app.state.repositories()[0]
        .worktrees
        .iter()
        .find(|w| w.name == "beta")
        .map(|w| (w.id, w.path.clone()))
        .expect("fixture worktree");
    app.state.update_worktree(beta, |w| {
        w.work_item = Some(WorkItemLink::PullRequest { number: 31 });
    });
    let worktrees = app.state.repositories()[0].worktrees.len();

    open_pr_review(&mut app, &path);
    let screen = rendered(&mut app);
    let row = screen
        .lines()
        .find(|l| l.contains("#31 Cache tests"))
        .expect("the PR is listed");
    assert!(row.contains("→ beta"), "{row}");

    type_text(&mut app, "cache");
    app.handle_key(key(KeyCode::Enter));

    assert!(app.modals.is_empty(), "no confirm screen: {:?}", app.modals);
    assert_eq!(app.state.selection.worktree_id, Some(beta));
    assert_eq!(app.meta.path, beta_path);
    assert_eq!(app.state.repositories()[0].worktrees.len(), worktrees);
    assert!(
        notified(&app).contains(&"PR #31 is already in worktree 'beta'"),
        "{:?}",
        notified(&app)
    );
}

/// A PR whose branch some worktree already has checked out cannot be
/// checked out a second time; picking it goes to that worktree instead.
#[tokio::test]
async fn a_pr_whose_branch_is_checked_out_jumps_there() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    let (beta, branch) = app.state.repositories()[0]
        .worktrees
        .iter()
        .find(|w| w.name == "beta")
        .map(|w| (w.id, w.branch.clone()))
        .expect("fixture worktree");
    app.handle_key(key(KeyCode::Char('s')));
    highlight_mode(&mut app, crate::work::WorkMode::PrReview);
    app.handle_key(key(KeyCode::Enter));
    app.handle_event(AppEvent::PullRequests {
        scope: Scope::Mine,
        path,
        result: Ok(pr_answer(
            vec![open_pr(31, "Mine", "daan", &branch)],
            Some("daan".into()),
        )),
    });
    app.handle_key(key(KeyCode::Enter));

    assert!(app.modals.is_empty(), "no confirm screen: {:?}", app.modals);
    assert_eq!(app.state.selection.worktree_id, Some(beta));
    assert!(
        notified(&app)
            .iter()
            .any(|n| n.contains(&format!("'{branch}' is already checked out in 'beta'"))),
        "{:?}",
        notified(&app)
    );
}

/// A fork's branch names are the fork's: its `main` is not this
/// repository's `main`, and `gh pr checkout` names its local branch apart.
/// A worktree on a branch of the same name is no reason to jump.
#[tokio::test]
async fn a_fork_pr_is_not_matched_by_branch_name() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    let branch = app.state.repositories()[0]
        .worktrees
        .iter()
        .find(|w| w.name == "beta")
        .map(|w| w.branch.clone())
        .expect("fixture worktree");
    let mut fork = open_pr(32, "From a fork", "alice", &branch);
    fork.is_cross_repository = true;
    app.handle_key(key(KeyCode::Char('s')));
    highlight_mode(&mut app, crate::work::WorkMode::PrReview);
    app.handle_key(key(KeyCode::Enter));
    app.handle_event(AppEvent::PullRequests {
        scope: Scope::Mine,
        path,
        result: Ok(pr_answer(vec![fork], Some("daan".into()))),
    });
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(confirm_screen(&app).name.value(), "pr-32-from-a-fork");
}

/// A PR list for another repository, or one that arrives after the flow
/// closed, changes nothing.
#[tokio::test]
async fn a_stray_pull_request_list_is_ignored() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_event(AppEvent::PullRequests {
        scope: Scope::Mine,
        path: "/somewhere/else".into(),
        result: Ok(pr_answer(two_prs(), None)),
    });
    highlight_mode(&mut app, crate::work::WorkMode::PrReview);
    app.handle_key(key(KeyCode::Enter));
    assert!(start_work(&app).picker.is_none(), "still loading");
}

/// `s`, then `pr-review`, before `gh` has answered.
fn choose_pr_review(app: &mut App) {
    app.handle_key(key(KeyCode::Char('s')));
    highlight_mode(app, crate::work::WorkMode::PrReview);
    app.handle_key(key(KeyCode::Enter));
}

/// A list that could not be fetched says why, and never reads as "you have
/// no PRs".
#[tokio::test]
async fn a_failed_pull_request_list_says_so_rather_than_empty() {
    use crate::services::github::ListError;
    for (error, said) in [
        (ListError::NotAuthenticated, "gh is not logged in"),
        (ListError::NotInstalled, "gh is not installed"),
        (ListError::Failed, "Could not load the PRs from GitHub"),
    ] {
        let (_dir, mut app, path) = app_with_issues(Vec::new());
        choose_pr_review(&mut app);
        app.handle_event(AppEvent::PullRequests {
            scope: Scope::Mine,
            path,
            result: Err(error),
        });

        assert!(listed_prs(&app).is_empty());
        let screen = rendered(&mut app);
        assert!(screen.contains(said), "{error:?}: {screen}");
        assert!(!screen.contains("No open PRs"), "{error:?}: {screen}");
    }
}

/// An empty answer is still the empty state.
#[tokio::test]
async fn an_empty_pull_request_list_reads_as_empty() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    choose_pr_review(&mut app);
    app.handle_event(AppEvent::PullRequests {
        scope: Scope::Mine,
        path,
        result: Ok(pr_answer(Vec::new(), Some("daan".into()))),
    });

    let screen = rendered(&mut app);
    assert!(
        screen.contains("No open PRs awaiting your review or opened by you"),
        "{screen}"
    );
}

/// When one of the two queries failed, the other's PRs are listed and a
/// notification says there may be more.
#[tokio::test]
async fn a_partial_pull_request_list_is_listed_and_flagged() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    choose_pr_review(&mut app);
    let mut answer = pr_answer(two_prs(), Some("daan".into()));
    answer.incomplete = true;
    app.handle_event(AppEvent::PullRequests {
        scope: Scope::Mine,
        path,
        result: Ok(answer),
    });

    assert_eq!(listed_prs(&app), vec![30, 31]);
    assert!(
        notified(&app)
            .iter()
            .any(|n| n.contains("may be incomplete")),
        "{:?}",
        notified(&app)
    );
}

/// A partial list nobody is waiting for is not announced.
#[tokio::test]
async fn a_stray_partial_list_is_not_announced() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    let mut answer = pr_answer(two_prs(), None);
    answer.incomplete = true;
    app.handle_event(AppEvent::PullRequests {
        scope: Scope::Mine,
        path,
        result: Ok(answer),
    });
    assert!(notified(&app).is_empty(), "{:?}", notified(&app));
}
