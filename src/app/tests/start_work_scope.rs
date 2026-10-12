use super::*;

use crate::models::Scope;

/// Into Start work's `issue` list, then onto its "Not just mine" checkbox.
fn open_issue_list_on_the_scope_toggle(app: &mut App) {
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(
        start_work(app).focus,
        crate::modal::StartWorkModal::FOCUS_SCOPE
    );
}

/// The toggle widens the list to every open issue for this flow alone: the
/// setting, and the pane's cards that follow it, stay on "mine".
#[tokio::test]
async fn the_scope_toggle_lists_every_open_issue() {
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(1, "Mine")]);
    github::seed_cache_for_test(
        &path,
        Scope::All,
        vec![open_issue(1, "Mine"), open_issue(2, "Someone else's")],
        chrono::Utc::now(),
    );
    open_issue_list_on_the_scope_toggle(&mut app);
    assert_eq!(listed(&app), vec![1]);
    assert!(rendered(&mut app).contains("[ ] Not just mine"));

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).scope, Scope::All);
    assert_eq!(listed(&app), vec![1, 2]);
    assert!(rendered(&mut app).contains("[x] Not just mine"));
    assert_eq!(app.start_work_for(&path).scope, Scope::Mine, "not saved");

    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(listed(&app), vec![1]);
}

/// Nothing cached for the other scope: the list loads, and only a result
/// for the scope the flow now shows fills it.
#[tokio::test]
async fn the_scope_toggle_waits_for_its_own_list() {
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(1, "Mine")]);
    open_issue_list_on_the_scope_toggle(&mut app);
    app.handle_key(key(KeyCode::Enter));
    assert!(start_work(&app).picker.is_none(), "loading the wider list");
    assert!(rendered(&mut app).contains("Loading issues"));

    // The pane's own refresh, for the setting's scope, is not the flow's.
    app.handle_event(AppEvent::Issues {
        path: path.clone(),
        scope: Scope::Mine,
        issues: vec![open_issue(1, "Mine")],
    });
    assert!(start_work(&app).picker.is_none());

    app.handle_event(AppEvent::Issues {
        path: path.clone(),
        scope: Scope::All,
        issues: vec![open_issue(1, "Mine"), open_issue(3, "Unassigned")],
    });
    assert_eq!(listed(&app), vec![1, 3]);
}

/// `pr-review` offers the toggle too, and a PR list for the scope the flow
/// has left is not taken.
#[tokio::test]
async fn the_scope_toggle_relists_the_pull_requests() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('s')));
    highlight_mode(&mut app, crate::work::WorkMode::PrReview);
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(
        start_work(&app).focus,
        crate::modal::StartWorkModal::FOCUS_SCOPE
    );
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).scope, Scope::All);

    let pr = |number| open_pr(number, "A PR", "someone", "some-branch");
    app.handle_event(AppEvent::PullRequests {
        path: path.clone(),
        scope: Scope::Mine,
        result: Ok(pr_answer(vec![pr(7)], Some("daan".into()))),
    });
    assert!(start_work(&app).picker.is_none(), "the old scope's list");

    app.handle_event(AppEvent::PullRequests {
        path,
        scope: Scope::All,
        result: Ok(pr_answer(vec![pr(7), pr(8)], Some("daan".into()))),
    });
    let listed: Vec<i64> = start_work(&app)
        .picker
        .as_ref()
        .expect("PRs loaded")
        .entries()
        .filter_map(|e| match e {
            crate::work::ItemEntry::PullRequest(entry) => Some(entry.pr.number),
            _ => None,
        })
        .collect();
    assert_eq!(listed, vec![7, 8]);
}

/// A flow opened under an "all" setting starts ticked, and its empty list
/// does not claim to be only the user's.
#[tokio::test]
async fn an_all_setting_starts_the_toggle_ticked() {
    let mut settings = crate::models::Settings::default();
    settings.start_work.scope = Scope::All;
    let (_dir, mut app, _) = app_with_settings_and_issues(settings, Scope::All, Vec::new());
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    let screen = rendered(&mut app);
    assert!(screen.contains("[x] Not just mine"), "{screen}");
    assert!(screen.contains("No open non-feature issues"), "{screen}");
    assert!(!screen.contains("assigned to or opened by you"), "{screen}");
}

/// Repository settings that answer after the user ticked the box change the
/// labels and names, but leave their choice of scope alone.
#[tokio::test]
async fn a_late_settings_answer_keeps_a_ticked_scope() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(1, "Mine")]);
    open_issue_list_on_the_scope_toggle(&mut app);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).scope, Scope::All);

    let mut settings = start_work(&app).start_work.clone();
    settings.scope = Scope::All;
    settings.feature_labels = vec!["feature".into()];
    match app.modals.last_mut() {
        Some(Modal::StartWork(flow)) => flow.set_start_work(settings.clone()),
        other => panic!("Start work expected, found {other:?}"),
    }
    settings.scope = Scope::Mine;
    match app.modals.last_mut() {
        Some(Modal::StartWork(flow)) => flow.set_start_work(settings),
        other => panic!("Start work expected, found {other:?}"),
    }
    assert_eq!(start_work(&app).scope, Scope::All, "the tick stands");
    assert_eq!(start_work(&app).start_work.feature_labels, vec!["feature"]);
}
