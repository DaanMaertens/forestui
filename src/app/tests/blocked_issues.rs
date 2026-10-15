use super::*;

/// `open_issue`, blocked by the open issues `by` — and by a closed #99,
/// which blocks nothing.
fn blocked_issue(number: i64, title: &str, by: &[i64]) -> crate::models::GitHubIssue {
    let mut issue = open_issue(number, title);
    let mut nodes: Vec<_> = by
        .iter()
        .map(|n| serde_json::json!({"number": n, "state": "OPEN"}))
        .collect();
    nodes.push(serde_json::json!({"number": 99, "state": "CLOSED"}));
    issue.blocked_by = serde_json::from_value(serde_json::json!({ "nodes": nodes })).unwrap();
    issue
}

/// The settings a restart would load: what the last toggle saved.
fn saved_settings() -> crate::models::Settings {
    settings_service::load_settings_from(&settings_service::settings_path())
}

fn forget_saved_settings() {
    let _ = std::fs::remove_file(settings_service::settings_path());
}

/// Into Start work's `issue` list, then past the filter onto its "Show
/// blocked" checkbox.
fn open_issue_list_on_the_toggle(app: &mut App) {
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    assert_eq!(
        start_work(app).focus,
        crate::modal::StartWorkModal::FOCUS_BLOCKED
    );
}

/// Hidden by default, with the count said; the checkbox shows them, marked,
/// and hides them again — remembering each choice for a restart.
#[tokio::test]
async fn the_picker_toggle_shows_and_hides_blocked_issues() {
    forget_saved_settings();
    let (_dir, mut app, _) = app_with_issues(vec![
        open_issue(1, "Free to start"),
        blocked_issue(2, "Waits on nine", &[9, 10]),
        blocked_issue(3, "Blockers all closed", &[]),
    ]);
    open_issue_list_on_the_toggle(&mut app);
    assert_eq!(listed(&app), vec![1, 3]);
    let screen = rendered(&mut app);
    assert!(screen.contains("1 blocked hidden"), "{screen}");
    assert!(screen.contains("[ ] Show blocked"), "{screen}");

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(listed(&app), vec![1, 2, 3]);
    assert!(app.settings.show_blocked_issues);
    assert!(saved_settings().show_blocked_issues, "survives a restart");
    let screen = rendered(&mut app);
    assert!(screen.contains("blocked by #9, #10"), "{screen}");
    assert!(screen.contains("[x] Show blocked"), "{screen}");
    assert!(!screen.contains("blocked hidden"), "{screen}");
    // Only the blocked issue is marked.
    assert_eq!(screen.matches("blocked by").count(), 1, "{screen}");

    app.handle_key(key(KeyCode::Char(' ')));
    assert_eq!(listed(&app), vec![1, 3]);
    assert!(!saved_settings().show_blocked_issues);
    forget_saved_settings();
}

/// Toggling keeps a half-typed query: only the rows under it change.
#[tokio::test]
async fn the_picker_toggle_keeps_the_query() {
    forget_saved_settings();
    let (_dir, mut app, _) = app_with_issues(vec![
        open_issue(1, "Wait for it"),
        blocked_issue(2, "Waits on nine", &[9]),
    ]);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    type_text(&mut app, "wait");
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Enter));
    let picker = start_work(&app).picker.as_ref().expect("issues loaded");
    assert_eq!(picker.query().value(), "wait");
    assert_eq!(picker.matches().filter_map(issue_of).count(), 2);
    forget_saved_settings();
}

/// The choice is one preference: what the picker was left showing is what
/// it shows when Start work reopens.
#[tokio::test]
async fn the_toggle_outlives_the_flow() {
    forget_saved_settings();
    let (_dir, mut app, _) =
        app_with_issues(vec![open_issue(1, "Free"), blocked_issue(2, "Waits", &[9])]);
    open_issue_list_on_the_toggle(&mut app);
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Esc));
    assert!(app.modals.is_empty());

    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    assert!(start_work(&app).show_blocked);
    assert_eq!(listed(&app), vec![1, 2]);
    forget_saved_settings();
}

/// A shown blocked issue is picked and confirmed like any other.
#[tokio::test]
async fn a_shown_blocked_issue_can_be_started() {
    let settings = crate::models::Settings {
        show_blocked_issues: true,
        ..Default::default()
    };
    let (_dir, mut app, _) = app_with_settings_and_issues(
        settings,
        crate::models::Scope::Mine,
        vec![blocked_issue(2, "Waits on nine", &[9])],
    );
    walk_to_confirm(&mut app, "nine");
    assert_eq!(confirm_number(confirm_screen(&app)), Some(2));
}

/// The one list counts every blocked issue it hides, features and bugs
/// alike.
#[tokio::test]
async fn the_issue_list_counts_every_hidden_issue() {
    let mut feature = blocked_issue(5, "Blocked feature", &[9]);
    feature.labels = vec![crate::models::GitHubLabel {
        name: "enhancement".into(),
    }];
    let (_dir, mut app, _) = app_with_issues(vec![
        feature,
        blocked_issue(6, "Blocked bug", &[9]),
        blocked_issue(7, "Another blocked bug", &[9]),
    ]);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).hidden_blocked(), 3);
    let screen = rendered(&mut app);
    assert!(screen.contains("3 blocked hidden"), "{screen}");
}

/// More newer blocked issues than the old page of ten leave an older
/// unblocked one listed, and the count says how many were hidden (#76).
#[tokio::test]
async fn older_unblocked_issues_survive_many_newer_blocked_ones() {
    forget_saved_settings();
    let mut issues: Vec<_> = (10..22).map(|n| blocked_issue(n, "Waits", &[9])).collect();
    issues.push(open_issue(3, "Old but free"));
    let (_dir, mut app, _) = app_with_issues(issues);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(listed(&app), vec![3]);
    let screen = rendered(&mut app);
    assert!(screen.contains("12 blocked hidden"), "{screen}");
}
