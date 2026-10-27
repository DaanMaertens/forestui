use super::*;

#[tokio::test]
async fn sidebar_lists_repository_then_active_worktrees() {
    let (_dir, mut app) = app_with_fixture();
    app.rebuild_rows();

    assert_eq!(app.rows.len(), 3, "one repo + two active worktrees");
    assert!(matches!(app.rows[0], SidebarRow::Repository { .. }));
    assert!(matches!(app.rows[1], SidebarRow::Worktree { .. }));

    // The archived section only appears once it is toggled on.
    app.handle_key(key(KeyCode::Char('A')));
    assert_eq!(app.rows.len(), 5, "plus the archived header and entry");
    assert!(matches!(app.rows[3], SidebarRow::ArchivedHeader));
    assert!(matches!(app.rows[4], SidebarRow::ArchivedWorktree { .. }));
}

#[tokio::test]
async fn moving_the_sidebar_cursor_changes_the_selection() {
    let (_dir, mut app) = app_with_fixture();
    assert!(app.state.selection.is_repository());

    app.handle_key(key(KeyCode::Down));
    assert!(app.state.selection.is_worktree());
    assert_eq!(app.sidebar_index, 1);

    app.handle_key(key(KeyCode::Up));
    assert!(app.state.selection.is_repository());
}

#[tokio::test]
async fn detail_items_differ_between_repository_and_worktree() {
    let (_dir, mut app) = app_with_fixture();
    app.sessions = Some(Vec::new());

    let repo_items = app.detail_items();
    assert!(repo_items.contains(&DetailItem::Action(Action::StartWork)));
    assert!(repo_items.contains(&DetailItem::Action(Action::RemoveRepository)));
    assert!(!repo_items.contains(&DetailItem::Field(Field::WorktreeName)));

    app.handle_key(key(KeyCode::Down));
    app.sessions = Some(Vec::new());
    let worktree_items = app.detail_items();
    assert!(worktree_items.contains(&DetailItem::Field(Field::WorktreeName)));
    assert!(worktree_items.contains(&DetailItem::Field(Field::BranchName)));
    assert!(worktree_items.contains(&DetailItem::Action(Action::Archive)));
    assert!(!worktree_items.contains(&DetailItem::Action(Action::StartWork)));
}

#[tokio::test]
async fn custom_buttons_add_one_control_per_session_and_one_at_the_top() {
    let (_dir, mut app) = app_with_fixture();
    app.sessions = Some(Vec::new());
    let baseline = app.detail_items().len();

    app.settings.custom_buttons.push(CustomClaudeButton {
        label: "Opus".into(),
        prefix: "opus".into(),
        command: "claude --model opus".into(),
    });
    assert_eq!(app.detail_items().len(), baseline + 1);

    app.sessions = Some(vec![ClaudeSession {
        custom_title: None,
        id: "s1".into(),
        title: "t".into(),
        recent_turns: Vec::new(),
        last_timestamp: chrono::Utc::now(),
        message_count: 1,
        git_branch: None,
        tokens: Default::default(),
        model: None,
        continued_in: None,
    }]);
    // Resume + YOLO + one per custom button + Rename + Pin + Del.
    assert_eq!(app.detail_items().len(), baseline + 1 + 6);
}

#[tokio::test]
async fn typing_in_a_rename_field_does_not_trigger_hotkeys() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    app.sessions = Some(Vec::new());
    snapshot_drawn(&mut app);

    app.focus = Focus::Detail;
    let index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Field(Field::WorktreeName))
        .expect("rename field present");
    app.detail_index = index;
    let original = app.name_input.value().to_string();

    // 'q' would otherwise quit.
    app.handle_key(key(KeyCode::Char('q')));
    assert!(!app.should_quit);
    assert!(app.name_input.value().ends_with('q'));

    // Escape undoes the edit and hands focus back, so hotkeys work again.
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(app.name_input.value(), original);
    assert_eq!(app.focus, Focus::Sidebar);
    app.handle_key(key(KeyCode::Char('q')));
    assert!(app.should_quit);
}

#[tokio::test]
async fn a_completed_rename_resyncs_the_field_it_renamed() {
    let (_dir, mut app) = app_with_fixture();
    // Select the first worktree, which seeds the rename fields from it.
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("a worktree");
    let old_name = app.name_input.value().to_string();

    // The rename runs off the event loop now, and the user is free to move
    // around while it does. Coming back re-seeds the field from the state
    // as it still is — the old name.
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(app.name_input.value(), old_name);

    // Now the task lands.
    let change = begun(&mut app, worktree_id, WorktreeSync::begin_rename);
    app.handle_event(AppEvent::WorktreeRenamed {
        change,
        name: "renamed".into(),
        path: "/tmp/renamed".into(),
    });

    assert_eq!(
        app.name_input.value(),
        "renamed",
        "the field still holds the pre-rename name; pressing Enter would rename it back"
    );
}

#[tokio::test]
async fn quit_hotkey_quits_and_unknown_keys_do_nothing() {
    let (_dir, mut app) = app_with_fixture();
    // `?` is deliberately unbound: the footer is the complete key surface.
    app.handle_key(key(KeyCode::Char('?')));
    assert!(app.notifications.is_empty());
    assert!(app.modals.is_empty());

    app.handle_key(key(KeyCode::Char('q')));
    assert!(app.should_quit);
}

/// Defect B of issue #51: every binding fired on `Ctrl`+letter, so a
/// `Ctrl+A` that reached the pane — which is exactly what tmux's own
/// `bind C-a send-prefix` does — opened Add Repository, and `Ctrl+D`
/// started deleting a worktree. `Ctrl+C` stays quit.
#[tokio::test]
async fn control_modified_keys_never_fire_footer_bindings() {
    let (_dir, mut app) = app_with_fixture();
    let archived_before = app.state.show_archived;

    for binding in ['a', 's', 'w', 'A', 'q'] {
        app.handle_key(ctrl(KeyCode::Char(binding)));
    }

    assert!(app.modals.is_empty(), "a Ctrl combination opened a modal");
    assert!(!app.should_quit, "Ctrl+Q quit the app");
    assert_eq!(
        app.state.show_archived, archived_before,
        "Ctrl+A toggled the archived section"
    );

    app.handle_key(ctrl(KeyCode::Char('c')));
    assert!(app.should_quit, "Ctrl+C no longer quits");
}

#[tokio::test]
async fn opening_a_modal_captures_keys() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Char('a')));
    assert_eq!(app.modals.len(), 1);

    // 'q' now types into the modal instead of quitting.
    app.handle_key(key(KeyCode::Char('q')));
    assert!(!app.should_quit);

    app.handle_key(key(KeyCode::Esc));
    assert!(app.modals.is_empty());
}

/// The regression that made themes non-sticky: startup activated the
/// legacy `theme` field (always "system") instead of `theme_name`, so a
/// saved theme applied in-session and silently reset on every launch.
#[tokio::test]
async fn startup_activates_the_saved_theme() {
    let _guard = crate::theme::test_lock();
    let (_dir, _app) = test_support::app_with_settings(Settings {
        theme_name: "nord".into(),
        ..Settings::default()
    });
    assert_eq!(
        crate::theme::active().slug,
        "nord",
        "the saved theme must be active before the first frame"
    );
    // The guard restores the pre-test theme on drop.
}

#[tokio::test]
async fn selecting_a_repository_prefetches_only_a_cold_cache() {
    let (_dir, mut app) = app_with_fixture();
    let path = app
        .state
        .selected_repository()
        .expect("repo selected")
        .source_path
        .clone();
    let issue: crate::models::GitHubIssue = serde_json::from_str(
        r#"{"number":9,"title":"Cached","state":"OPEN","url":"https://x/9",
                "createdAt":"2026-08-01T10:00:00Z","updatedAt":"2026-08-01T10:00:00Z",
                "author":{"login":"me"},"assignees":[],"labels":[]}"#,
    )
    .expect("issue json");
    crate::services::github::seed_cache_for_test(
        &path,
        crate::models::Scope::Mine,
        vec![issue],
        chrono::Utc::now(),
    );

    // Leave the repository and come back: a fresh cache is what Start
    // work will list, so there is nothing to fetch.
    app.issues_in_flight.clear();
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Up));
    assert!(
        app.issues_in_flight.is_empty(),
        "a warm cache was refetched"
    );

    // Once it goes cold, coming back warms it again in the background.
    crate::services::github::invalidate_cache(&path);
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Up));
    assert!(
        app.issues_in_flight
            .contains(&(path, crate::models::Scope::Mine)),
        "a cold cache was not prefetched"
    );
}

#[tokio::test]
async fn archive_toggle_moves_the_worktree_out_of_the_active_list() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");

    app.handle_key(key(KeyCode::Char('h')));
    let (_repo, worktree) = app.state.find_worktree(worktree_id).expect("still tracked");
    assert!(worktree.is_archived);
    assert_eq!(
        app.rows.len(),
        2,
        "one repo + one remaining active worktree"
    );
}

/// The sidebar's branch column earns its space or does not appear.
#[test]
fn only_a_branch_that_differs_from_the_name_is_shown() {
    // What a plain worktree on a new branch gets: prefix + name, and so pure repetition.
    assert_eq!(informative_branch("feat/wt-a", "wt-a", "feat/"), None);
    // No prefix configured.
    assert_eq!(informative_branch("wt-a", "wt-a", ""), None);
    // An existing branch checked out under a different worktree name.
    assert_eq!(
        informative_branch("release-2", "hotfix", "feat/"),
        Some("release-2".to_string())
    );
    // Same name, different prefix — still worth showing.
    assert_eq!(
        informative_branch("bugfix/wt-a", "wt-a", "feat/"),
        Some("bugfix/wt-a".to_string())
    );
}

/// Folding a repository hides its worktrees without moving the selection.
#[tokio::test]
async fn collapsing_a_repository_hides_its_worktrees() {
    let (_dir, mut app) = app_with_fixture();
    let before = app.rows.len();
    assert!(
        app.rows
            .iter()
            .any(|row| matches!(row, SidebarRow::Worktree { .. })),
        "fixture has no worktree to fold away"
    );
    let selected = app.state.selection;

    app.toggle_collapsed(0);
    assert!(
        !app.rows
            .iter()
            .any(|row| matches!(row, SidebarRow::Worktree { .. }))
    );
    assert_eq!(
        app.state.selection, selected,
        "folding changed the selection"
    );

    app.toggle_collapsed(0);
    assert_eq!(app.rows.len(), before);
}

#[tokio::test]
async fn actions_refuse_to_run_against_a_missing_directory() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));

    // The fixture never creates the worktree directories on disk.
    app.run_action(Action::Terminal);
    assert_eq!(app.notifications.len(), 1);
    assert!(app.notifications[0].text.contains("no longer exists"));
    assert_eq!(app.notifications[0].severity, Severity::Error);
}
