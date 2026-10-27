use super::*;

use crate::models::Scope;

/// Into Start work's `issue` list, with `typed` typed and the highlight
/// moved `down` rows.
fn issue_list_with(app: &mut App, typed: &str, down: usize) {
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    type_text(app, typed);
    for _ in 0..down {
        app.handle_key(key(KeyCode::Down));
    }
}

fn highlighted(app: &App) -> Option<i64> {
    let picker = start_work(app).picker.as_ref()?;
    issue_of(picker.selected()?).map(|entry| entry.issue.number)
}

fn issues_arrive(app: &mut App, path: &str, scope: Scope, issues: Vec<crate::models::GitHubIssue>) {
    app.handle_event(AppEvent::Issues {
        path: path.to_string(),
        scope,
        issues: Ok(issues),
    });
}

/// A fresh cache still opens at once, but `gh` is asked again behind it: a
/// list up to five minutes old misses an issue assigned since (#119).
#[tokio::test]
async fn opening_start_work_revalidates_a_fresh_cache() {
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(1, "Cached")]);
    app.issues_in_flight.clear();

    app.handle_key(key(KeyCode::Char('s')));

    assert!(start_work(&app).picker.is_some(), "the cached list waited");
    assert!(
        app.issues_in_flight.contains(&(path, Scope::Mine)),
        "a fresh cache was not asked again"
    );
}

/// Re-listing for a scope revalidates too, even when that scope's cache
/// is fresh.
#[tokio::test]
async fn toggling_the_scope_revalidates_a_fresh_cache() {
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(1, "Mine")]);
    github::seed_cache_for_test(
        &path,
        Scope::All,
        vec![open_issue(1, "Mine")],
        chrono::Utc::now(),
    );
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    app.issues_in_flight.clear();

    // Past the filter and "Show blocked" onto "Not just mine".
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Tab));
    }
    assert_eq!(
        start_work(&app).focus,
        crate::modal::StartWorkModal::FOCUS_SCOPE
    );
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(listed(&app), vec![1], "the cached list waited");
    assert!(app.issues_in_flight.contains(&(path, Scope::All)));
}

/// The refetch's answer replaces the list on screen, without a scope
/// toggle or a reopen, and keeps what was typed and what was highlighted.
#[tokio::test]
async fn a_late_list_replaces_the_one_on_screen_keeping_the_place() {
    let (_dir, mut app, path) = app_with_issues(vec![
        open_issue(1, "Fix login"),
        open_issue(2, "Fix logout"),
    ]);
    issue_list_with(&mut app, "fix", 1);
    assert_eq!(highlighted(&app), Some(2));

    issues_arrive(
        &mut app,
        &path,
        Scope::Mine,
        vec![
            open_issue(3, "Fix assigned since"),
            open_issue(1, "Fix login"),
            open_issue(2, "Fix logout"),
        ],
    );

    assert_eq!(listed(&app), vec![3, 1, 2]);
    let picker = start_work(&app).picker.as_ref().expect("listed");
    assert_eq!(picker.query().value(), "fix");
    assert_eq!(highlighted(&app), Some(2));
}

/// A highlighted issue the new list no longer holds leaves the highlight
/// on the top row.
#[tokio::test]
async fn a_late_list_without_the_highlighted_issue_selects_the_top() {
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(1, "One"), open_issue(2, "Two")]);
    issue_list_with(&mut app, "", 1);
    assert_eq!(highlighted(&app), Some(2));

    issues_arrive(&mut app, &path, Scope::Mine, vec![open_issue(1, "One")]);

    assert_eq!(listed(&app), vec![1]);
    assert_eq!(
        start_work(&app).picker.as_ref().map(|p| p.selected_index()),
        Some(0)
    );
    assert_eq!(highlighted(&app), Some(1));
}

/// A failed refetch says less than the list it would replace.
#[tokio::test]
async fn a_failed_refetch_keeps_the_loaded_list() {
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(1, "One")]);
    issue_list_with(&mut app, "", 0);

    app.handle_event(AppEvent::Issues {
        path,
        scope: Scope::Mine,
        issues: Err(github::ListError::Failed),
    });

    assert_eq!(listed(&app), vec![1]);
    assert!(!rendered(&mut app).contains("Could not load"));
}

/// A refetch for the scope the flow does not list is not its list.
#[tokio::test]
async fn a_late_list_for_the_other_scope_is_ignored() {
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(1, "Mine")]);
    issue_list_with(&mut app, "", 0);

    issues_arrive(
        &mut app,
        &path,
        Scope::All,
        vec![open_issue(1, "Mine"), open_issue(2, "Theirs")],
    );

    assert_eq!(listed(&app), vec![1]);
}

/// A prefetch already in flight may answer from the fresh cache without
/// asking `gh`, so the revalidation it held off still runs once it lands.
#[tokio::test]
async fn a_revalidation_behind_a_prefetch_in_flight_still_runs() {
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(1, "Cached")]);
    app.issues_in_flight.clear();
    app.issues_in_flight.insert((path.clone(), Scope::Mine));

    app.handle_key(key(KeyCode::Char('s')));
    issues_arrive(&mut app, &path, Scope::Mine, vec![open_issue(1, "Cached")]);

    assert!(
        app.issues_in_flight.contains(&(path.clone(), Scope::Mine)),
        "the revalidation was dropped"
    );
    issues_arrive(&mut app, &path, Scope::Mine, vec![open_issue(1, "Cached")]);
    assert!(
        app.issues_in_flight.is_empty(),
        "revalidated more than once"
    );
}
