use super::*;

use crate::modal::StartWorkModal;
use crate::models::{IssueFilter, Scope, Settings};

/// The settings a restart would load: what the last change saved.
fn saved_settings() -> Settings {
    settings_service::load_settings_from(&settings_service::settings_path())
}

fn forget_saved_settings() {
    let _ = std::fs::remove_file(settings_service::settings_path());
}

/// A feature, a bug, one that is both, and two that are neither — one of
/// them labelled, one not.
fn mixed_issues() -> Vec<crate::models::GitHubIssue> {
    vec![
        labelled_issue(1, "Dark mode", &["enhancement"]),
        labelled_issue(2, "Crash on save", &["bug"]),
        labelled_issue(3, "Slow export", &["Bug", "Enhancement"]),
        labelled_issue(4, "Typo in docs", &["documentation"]),
        open_issue(5, "Untitled thought"),
    ]
}

/// Into Start work's `issue` list, then onto its filter.
fn open_issue_list_on_the_filter(app: &mut App) {
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(start_work(app).focus, StartWorkModal::FOCUS_FILTER);
}

/// Enter or Space moves the filter on, and the list follows: features, bugs
/// — the issue with both labels in each — then the issues with neither,
/// unlabelled or not, and back to every issue.
#[tokio::test]
async fn the_filter_cycles_through_features_bugs_and_other() {
    forget_saved_settings();
    let (_dir, mut app, _) = app_with_issues(mixed_issues());
    open_issue_list_on_the_filter(&mut app);
    assert_eq!(listed(&app), vec![1, 2, 3, 4, 5]);
    assert!(rendered(&mut app).contains("Filter: All"));

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).filter, IssueFilter::Features);
    assert_eq!(listed(&app), vec![1, 3]);
    assert!(rendered(&mut app).contains("Filter: Features"));

    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(listed(&app), vec![2, 3]);
    assert!(rendered(&mut app).contains("Filter: Bugs"));

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(listed(&app), vec![4, 5]);
    assert!(rendered(&mut app).contains("Filter: Other"));

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(listed(&app), vec![1, 2, 3, 4, 5]);
    forget_saved_settings();
}

/// An issue with both labels is listed under Bugs, and still starts as the
/// feature it is.
#[tokio::test]
async fn an_issue_listed_under_bugs_still_starts_as_a_feature() {
    let settings = Settings {
        issue_filter: IssueFilter::Bugs,
        ..Default::default()
    };
    let (_dir, mut app, _) = app_with_settings_and_issues(settings, Scope::Mine, mixed_issues());
    walk_to_confirm(&mut app, "export");
    assert_eq!(confirm_screen(&app).branch.value(), "feat/3-slow-export");
}

/// The filter is a preference like "Show blocked": saved as it changes, and
/// what Start work opens on next time.
#[tokio::test]
async fn the_filter_outlives_the_flow() {
    forget_saved_settings();
    let (_dir, mut app, _) = app_with_issues(mixed_issues());
    open_issue_list_on_the_filter(&mut app);
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.settings.issue_filter, IssueFilter::Bugs);
    assert_eq!(
        saved_settings().issue_filter,
        IssueFilter::Bugs,
        "survives a restart"
    );
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Esc));
    assert!(app.modals.is_empty());

    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).filter, IssueFilter::Bugs);
    assert_eq!(listed(&app), vec![2, 3]);
    forget_saved_settings();
}

/// The filter applies first: "Show blocked" hides only the filter's blocked
/// issues, and counts only those.
#[tokio::test]
async fn the_filter_combines_with_show_blocked() {
    let mut blocked_bug = labelled_issue(6, "Blocked bug", &["bug"]);
    blocked_bug.blocked_by = serde_json::from_value(serde_json::json!({
        "nodes": [{"number": 9, "state": "OPEN"}]
    }))
    .unwrap();
    let mut blocked_feature = labelled_issue(7, "Blocked feature", &["enhancement"]);
    blocked_feature.blocked_by = blocked_bug.blocked_by.clone();
    let mut issues = mixed_issues();
    issues.extend([blocked_bug, blocked_feature]);
    let settings = Settings {
        issue_filter: IssueFilter::Bugs,
        ..Default::default()
    };
    let (_dir, mut app, _) = app_with_settings_and_issues(settings, Scope::Mine, issues);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(listed(&app), vec![2, 3]);
    assert_eq!(start_work(&app).hidden_blocked(), 1);

    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Enter));
    assert!(start_work(&app).show_blocked);
    assert_eq!(listed(&app), vec![2, 3, 6]);
}

/// Widening the scope keeps the filter: the new list is filtered too.
#[tokio::test]
async fn the_filter_combines_with_the_scope() {
    let settings = Settings {
        issue_filter: IssueFilter::Bugs,
        ..Default::default()
    };
    let (_dir, mut app, path) = app_with_settings_and_issues(
        settings,
        Scope::Mine,
        vec![labelled_issue(2, "Mine", &["bug"])],
    );
    github::seed_cache_for_test(
        &path,
        Scope::All,
        vec![
            labelled_issue(2, "Mine", &["bug"]),
            labelled_issue(8, "Someone else's bug", &["bug"]),
            labelled_issue(9, "Someone else's feature", &["enhancement"]),
        ],
        chrono::Utc::now(),
    );
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(listed(&app), vec![2]);
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Tab));
    }
    assert_eq!(start_work(&app).focus, StartWorkModal::FOCUS_SCOPE);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(listed(&app), vec![2, 8]);
}

/// Changing the filter keeps a half-typed query, which searches within it.
#[tokio::test]
async fn the_filter_keeps_the_query() {
    forget_saved_settings();
    let (_dir, mut app, _) = app_with_issues(mixed_issues());
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    type_text(&mut app, "s");
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Enter));
    let picker = start_work(&app).picker.as_ref().expect("issues loaded");
    assert_eq!(picker.query().value(), "s");
    let found: Vec<_> = picker
        .matches()
        .filter_map(issue_of)
        .map(|e| e.issue.number)
        .collect();
    assert!(!found.is_empty());
    assert!(found.iter().all(|n| [2, 3].contains(n)), "{found:?}");
    forget_saved_settings();
}

/// The filter button answers a click like the keyboard.
#[tokio::test]
async fn clicking_the_filter_moves_it_on() {
    forget_saved_settings();
    let (_dir, mut app, _) = app_with_issues(mixed_issues());
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    rendered(&mut app);
    let hit = app
        .hits
        .iter()
        .find(|hit| {
            matches!(
                hit.target,
                HitTarget::ModalControl { index, .. } if index == StartWorkModal::FOCUS_FILTER
            )
        })
        .expect("the filter is clickable");
    let (x, y) = (hit.rect.x + 1, hit.rect.y + 1);
    app.handle_mouse(click(x, y));
    assert_eq!(start_work(&app).filter, IssueFilter::Features);
    assert_eq!(listed(&app), vec![1, 3]);
    forget_saved_settings();
}

/// The count names the filter's issues, and an empty list says which
/// issues it has none of.
#[tokio::test]
async fn the_count_and_empty_text_name_the_filter() {
    let settings = Settings {
        issue_filter: IssueFilter::Bugs,
        ..Default::default()
    };
    let (_dir, mut app, _) =
        app_with_settings_and_issues(settings.clone(), Scope::Mine, mixed_issues());
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    let screen = rendered(&mut app);
    assert!(screen.contains("2 bug issues"), "{screen}");

    let (_dir, mut app, _) = app_with_settings_and_issues(
        settings,
        Scope::Mine,
        vec![labelled_issue(1, "Dark mode", &["enhancement"])],
    );
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    let screen = rendered(&mut app);
    assert!(
        screen.contains("No open bug issues assigned to or opened by you"),
        "{screen}"
    );

    let other = Settings {
        issue_filter: IssueFilter::Other,
        ..Default::default()
    };
    let (_dir, mut app, _) = app_with_settings_and_issues(
        other,
        Scope::Mine,
        vec![labelled_issue(1, "Dark mode", &["enhancement"])],
    );
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    let screen = rendered(&mut app);
    assert!(screen.contains("No other open issues"), "{screen}");
}

/// Under All the count reads as it always did.
#[tokio::test]
async fn the_count_is_plain_under_all() {
    let (_dir, mut app, _) = app_with_issues(mixed_issues());
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    let screen = rendered(&mut app);
    assert!(screen.contains("5 issues"), "{screen}");
}

/// The bug labels are a setting: the defaults' `bug` is only a default.
#[tokio::test]
async fn the_bug_labels_setting_decides_what_a_bug_is() {
    let mut settings = Settings {
        issue_filter: IssueFilter::Bugs,
        ..Default::default()
    };
    settings.start_work.bug_labels = vec!["kind/defect".into()];
    let (_dir, mut app, _) = app_with_settings_and_issues(
        settings,
        Scope::Mine,
        vec![
            labelled_issue(1, "Old style", &["bug"]),
            labelled_issue(2, "New style", &["Kind/Defect"]),
        ],
    );
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(listed(&app), vec![2]);
}

/// Neither `pr-review` nor a repository without GitHub has issues to
/// filter, so neither offers the filter.
#[tokio::test]
async fn only_the_issue_list_offers_the_filter() {
    let (_dir, mut app, _) = app_with_issues(mixed_issues());
    app.handle_key(key(KeyCode::Char('s')));
    highlight_mode(&mut app, crate::work::WorkMode::PrReview);
    app.handle_key(key(KeyCode::Enter));
    assert!(
        !start_work(&app)
            .ring()
            .contains(&StartWorkModal::FOCUS_FILTER)
    );
    assert!(!rendered(&mut app).contains("Filter:"));
}

/// Repository settings that answer after the list is up, with other bug
/// labels, re-list a Bugs list by them — keeping what was typed.
#[tokio::test]
async fn a_late_settings_answer_relists_by_its_labels() {
    let settings = Settings {
        issue_filter: IssueFilter::Bugs,
        ..Default::default()
    };
    let (_dir, mut app, _) = app_with_settings_and_issues(
        settings,
        Scope::Mine,
        vec![
            labelled_issue(1, "Global crash", &["bug"]),
            labelled_issue(2, "Repository crash", &["kind/defect"]),
        ],
    );
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    type_text(&mut app, "crash");
    assert_eq!(listed(&app), vec![1]);

    let mut start_work_settings = start_work(&app).start_work.clone();
    start_work_settings.bug_labels = vec!["kind/defect".into()];
    match app.modals.last_mut() {
        Some(Modal::StartWork(flow)) => flow.set_start_work(start_work_settings),
        other => panic!("Start work expected, found {other:?}"),
    }
    assert_eq!(listed(&app), vec![2]);
    let picker = start_work(&app).picker.as_ref().expect("issues loaded");
    assert_eq!(picker.query().value(), "crash");
}

/// Features and Other count and come up empty in their own words too.
#[tokio::test]
async fn features_and_other_name_themselves() {
    for (filter, count) in [
        (IssueFilter::Features, "2 feature issues"),
        (IssueFilter::Other, "2 other issues"),
    ] {
        let settings = Settings {
            issue_filter: filter,
            ..Default::default()
        };
        let (_dir, mut app, _) =
            app_with_settings_and_issues(settings, Scope::Mine, mixed_issues());
        app.handle_key(key(KeyCode::Char('s')));
        app.handle_key(key(KeyCode::Enter));
        let screen = rendered(&mut app);
        assert!(screen.contains(count), "{screen}");
    }

    let settings = Settings {
        issue_filter: IssueFilter::Features,
        ..Default::default()
    };
    let (_dir, mut app, _) = app_with_settings_and_issues(
        settings,
        Scope::Mine,
        vec![labelled_issue(2, "Crash on save", &["bug"])],
    );
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    let screen = rendered(&mut app);
    assert!(screen.contains("No open feature issues"), "{screen}");
}
