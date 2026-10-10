use super::test_support::{app_with_fixture, ctrl, key};
use super::worktree_sync::{ChangeTicket, WorktreeSync};
use super::*;
use crate::models::{CustomClaudeButton, Repository, WorkItemLink, Worktree};
use ratatui::crossterm::event::KeyCode;
use std::path::PathBuf;

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
    app.issues = Some(Vec::new());

    let repo_items = app.detail_items();
    assert!(repo_items.contains(&DetailItem::Action(Action::AddWorktree)));
    assert!(repo_items.contains(&DetailItem::Action(Action::RemoveRepository)));
    assert!(!repo_items.contains(&DetailItem::Field(Field::WorktreeName)));

    app.handle_key(key(KeyCode::Down));
    app.sessions = Some(Vec::new());
    let worktree_items = app.detail_items();
    assert!(worktree_items.contains(&DetailItem::Field(Field::WorktreeName)));
    assert!(worktree_items.contains(&DetailItem::Field(Field::BranchName)));
    assert!(worktree_items.contains(&DetailItem::Action(Action::Archive)));
    assert!(!worktree_items.contains(&DetailItem::Action(Action::AddWorktree)));
}

#[tokio::test]
async fn custom_buttons_add_one_control_per_session_and_one_at_the_top() {
    let (_dir, mut app) = app_with_fixture();
    app.issues = Some(Vec::new());
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

#[tokio::test]
async fn delete_asks_for_confirmation_first() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");

    app.handle_key(key(KeyCode::Char('d')));
    assert!(matches!(app.modals.last(), Some(Modal::Confirm(_))));
    // Still there: nothing is destroyed before the user says yes.
    assert!(app.state.find_worktree(worktree_id).is_some());

    app.handle_key(key(KeyCode::Char('y')));
    assert!(app.modals.is_empty());
    // Still there: the entry leaves state only when git reports success.
    assert!(app.state.find_worktree(worktree_id).is_some());

    let change = app.worktree_sync.change_ticket_for_test(worktree_id);
    app.handle_event(AppEvent::WorktreeRemoveResult {
        change,
        outcome: Ok(git::RemoveOutcome::Removed),
    });
    assert!(app.state.find_worktree(worktree_id).is_none());
}

#[tokio::test]
async fn a_dirty_worktree_gets_a_second_confirmation() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");

    let change = begun(&mut app, worktree_id, WorktreeSync::begin_removal);
    app.handle_event(AppEvent::WorktreeRemoveResult {
        change,
        outcome: Ok(git::RemoveOutcome::Dirty("2 modified, 1 untracked".into())),
    });

    let Some(Modal::Confirm(confirm)) = app.modals.last() else {
        panic!("expected the second confirmation");
    };
    assert!(confirm.message.contains("2 modified, 1 untracked"));
    assert_eq!(confirm.action.confirm_label(), "Delete anyway");
    assert!(matches!(
        confirm.action,
        crate::modal::ConfirmAction::ForceDeleteWorktree(id) if id == worktree_id
    ));
    // Nothing destroyed, nothing dropped from state.
    assert!(app.state.find_worktree(worktree_id).is_some());

    // Pushed by a background event: a keystroke already in flight when the
    // modal appeared must not confirm it.
    app.handle_key(key(KeyCode::Char('y')));
    assert!(
        matches!(app.modals.last(), Some(Modal::Confirm(_))),
        "an unarmed confirm must swallow the stale 'y'"
    );

    if let Some(Modal::Confirm(confirm)) = app.modals.last_mut() {
        confirm.disarm();
    }
    app.handle_key(key(KeyCode::Char('y')));
    assert!(app.modals.is_empty());
    // The forced removal is now in flight; the entry waits for the fold.
    assert!(app.worktree_sync.is_removing(worktree_id));
    assert!(app.state.find_worktree(worktree_id).is_some());
}

#[tokio::test]
async fn a_failed_removal_keeps_the_entry_and_reports() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");

    let change = begun(&mut app, worktree_id, WorktreeSync::begin_removal);
    app.handle_event(AppEvent::WorktreeRemoveResult {
        change,
        outcome: Err("worktree is locked".into()),
    });

    assert!(app.state.find_worktree(worktree_id).is_some());
    assert_eq!(app.notifications.len(), 1);
    assert!(app.notifications[0].text.contains("worktree is locked"));
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
async fn a_warm_cache_renders_issues_without_a_loading_flash() {
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
    crate::services::github::seed_cache_for_test(&path, vec![issue], chrono::Utc::now());

    // Leave the repository and come back: the second visit must render
    // from cache synchronously, not drop to the Loading state (#29).
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Up));
    let issues = app
        .issues
        .as_ref()
        .expect("no Loading flash on a warm cache");
    assert_eq!(issues[0].number, 9);
}

#[tokio::test]
async fn delete_is_ignored_while_a_removal_is_in_flight() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");

    let _removal = begun(&mut app, worktree_id, WorktreeSync::begin_removal);
    app.handle_key(key(KeyCode::Char('d')));
    assert!(
        app.modals.is_empty(),
        "no duplicate confirm while the removal runs"
    );
}

/// A worktree takes one change at a time, and a rename running is not on
/// screen the way a removal is — so Delete says why instead of offering a
/// confirm whose answer would be refused.
#[tokio::test]
async fn delete_waits_for_a_rename_in_flight() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
    let rename = begun(&mut app, worktree_id, WorktreeSync::begin_rename);

    app.handle_key(key(KeyCode::Char('d')));
    assert!(app.modals.is_empty(), "a confirm opened mid-rename");
    assert!(
        app.notifications
            .last()
            .is_some_and(|n| n.text.contains("still being renamed")),
        "the refusal was silent"
    );

    // Once the rename lands, Delete works again.
    app.handle_event(AppEvent::WorktreeRenameAborted { change: rename });
    app.handle_key(key(KeyCode::Char('d')));
    assert!(matches!(app.modals.last(), Some(Modal::Confirm(_))));
}

#[tokio::test]
async fn quit_waits_for_an_in_flight_removal() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
    let change = begun(&mut app, worktree_id, WorktreeSync::begin_removal);

    app.handle_key(key(KeyCode::Char('q')));
    assert!(!app.should_quit, "quit must wait for the fold");
    assert!(app.quit_pending);

    app.handle_event(AppEvent::WorktreeRemoveResult {
        change,
        outcome: Ok(git::RemoveOutcome::Removed),
    });
    assert!(app.should_quit, "the fold releases the pending quit");
}

#[tokio::test]
async fn a_second_quit_forces_out_past_an_in_flight_removal() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
    let _removal = begun(&mut app, worktree_id, WorktreeSync::begin_removal);

    app.handle_key(key(KeyCode::Char('q')));
    assert!(!app.should_quit);
    app.handle_key(key(KeyCode::Char('q')));
    assert!(app.should_quit);
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

/// A finished background creation lands in state on the main loop — the
/// single writer — and moves the selection onto the new worktree.
#[tokio::test]
async fn a_created_worktree_is_folded_into_state_and_selected() {
    let (_dir, mut app) = app_with_fixture();
    let repo_id = app.state.repositories()[0].id;
    // Collapsed on purpose: selecting a hidden row would leave the cursor
    // pointing at nothing, so the fold has to open to reveal the result.
    app.toggle_collapsed(0);
    assert!(app.collapsed.contains(&repo_id));
    let worktree = Worktree::new("fresh".into(), "feat/fresh".into(), "/tmp/fresh".into());
    let worktree_id = worktree.id;

    let create = app.worktree_sync.begin_create(repo_id);
    app.handle_event(AppEvent::WorktreeAdded {
        create,
        worktree: Box::new(worktree),
        first_prompt: None,
        yolo: false,
    });

    assert!(app.state.find_worktree(worktree_id).is_some());
    assert_eq!(app.state.selection.worktree_id, Some(worktree_id));
    assert!(!app.collapsed.contains(&repo_id), "the fold stayed shut");
    assert!(
        app.rows
            .iter()
            .any(|row| matches!(row, SidebarRow::Worktree { id, .. } if *id == worktree_id)),
        "the sidebar was not rebuilt"
    );
    assert_eq!(
        app.notifications.last().map(|n| n.text.as_str()),
        Some("Created worktree 'fresh'"),
        "the toast is announced by the fold, not the task"
    );
}

/// The work item link arrives on the created worktree and is the config's
/// own annotation from then on: a scan that corrects the branch and a
/// rename of the worktree or its branch all leave it in place, on disk too.
#[tokio::test]
async fn a_created_worktrees_work_item_link_survives_scans_and_renames() {
    let (dir, mut app) = app_with_fixture();
    let repo_id = app.state.repositories()[0].id;
    let path = dir.path().join("issue-3");
    std::fs::create_dir_all(&path).unwrap();
    let mut worktree = Worktree::new(
        "issue-3".into(),
        "fix/issue-3".into(),
        path.to_string_lossy().to_string(),
    );
    worktree.work_item = Some(WorkItemLink::Issue { number: 3 });
    let worktree_id = worktree.id;

    let create = app.worktree_sync.begin_create(repo_id);
    app.handle_event(AppEvent::WorktreeAdded {
        create,
        worktree: Box::new(worktree),
        first_prompt: None,
        yolo: false,
    });
    let link = |app: &App| {
        app.state
            .find_worktree(worktree_id)
            .and_then(|(_, w)| w.work_item.clone())
    };
    assert_eq!(link(&app), Some(WorkItemLink::Issue { number: 3 }));

    // git reports the branch under another name: reconciling corrects the
    // branch and keeps the annotation.
    let mut listed = scan_listing(&app, &[]);
    if let Some(entry) = listed.iter_mut().find(|i| i.path == path.to_string_lossy()) {
        entry.branch = Some("fix/moved".into());
    }
    let event = scanned(&mut app, repo_id, listed);
    app.handle_event(event);
    let (_, after_scan) = app.state.find_worktree(worktree_id).expect("still tracked");
    assert_eq!(after_scan.branch, "fix/moved", "the scan did not reconcile");
    assert_eq!(link(&app), Some(WorkItemLink::Issue { number: 3 }));

    let change = begun(&mut app, worktree_id, WorktreeSync::begin_rename);
    app.handle_event(AppEvent::WorktreeRenamed {
        change,
        name: "renamed".into(),
        path: "/tmp/renamed".into(),
    });
    let change = begun(&mut app, worktree_id, WorktreeSync::begin_rename);
    app.handle_event(AppEvent::WorktreeBranchRenamed {
        change,
        branch: "fix/renamed".into(),
    });
    assert_eq!(link(&app), Some(WorkItemLink::Issue { number: 3 }));

    let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
    let (_, saved) = reloaded.find_worktree(worktree_id).expect("persisted");
    assert_eq!(saved.work_item, Some(WorkItemLink::Issue { number: 3 }));
}

#[tokio::test]
async fn a_worktree_that_could_not_be_saved_is_not_announced_as_created() {
    let (dir, mut app) = app_with_fixture();
    let repo_id = app.state.repositories()[0].id;

    // Make the config unwritable: the directory holding it denies writes, so
    // the atomic write cannot even stage its temp file.
    let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(dir.path(), perms).unwrap();

    let worktree = Worktree::new("doomed".into(), "feat/doomed".into(), "/tmp/doomed".into());
    let create = app.worktree_sync.begin_create(repo_id);
    app.handle_event(AppEvent::WorktreeAdded {
        create,
        worktree: Box::new(worktree),
        first_prompt: None,
        yolo: false,
    });

    let last = app.notifications.last().expect("a toast");
    assert_eq!(
        last.severity,
        Severity::Error,
        "a failed write was announced as a success: {}",
        last.text
    );
    assert!(
        last.text.starts_with("Could not save the config"),
        "unexpected toast: {}",
        last.text
    );

    // Let the tempdir clean itself up.
    let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(dir.path(), perms).unwrap();
}

/// If the repository is removed while git runs, the late result is dropped
/// rather than resurrected as an entry nothing owns.
#[tokio::test]
async fn a_worktree_for_a_removed_repository_is_dropped() {
    let (_dir, mut app) = app_with_fixture();
    let repo_id = app.state.repositories()[0].id;
    app.state.remove_repository(repo_id);
    let worktree = Worktree::new("late".into(), "feat/late".into(), "/tmp/late".into());
    let worktree_id = worktree.id;

    let create = app.worktree_sync.begin_create(repo_id);
    app.handle_event(AppEvent::WorktreeAdded {
        create,
        worktree: Box::new(worktree),
        first_prompt: None,
        yolo: false,
    });

    assert!(app.state.find_worktree(worktree_id).is_none());
}

/// A scan result stamped with the repository's *current* epoch, the way a
/// listing that raced nothing would arrive.
fn scanned(app: &mut App, repo_id: Uuid, listed: Vec<git::WorktreeInfo>) -> AppEvent {
    AppEvent::WorktreesScanned {
        scan: app.worktree_sync.scan_ticket_for_test(repo_id),
        listed: Some(listed),
    }
}

/// Starts a change the way its flow does — `begin` is
/// [`WorktreeSync::begin_rename`] or [`WorktreeSync::begin_removal`] — for a
/// test that then delivers the task's result itself.
fn begun(
    app: &mut App,
    worktree_id: Uuid,
    begin: fn(&mut WorktreeSync, Uuid, Uuid) -> Option<ChangeTicket>,
) -> ChangeTicket {
    let (repo, _) = app.state.find_worktree(worktree_id).expect("tracked");
    let repo_id = repo.id;
    begin(&mut app.worktree_sync, repo_id, worktree_id).expect("no change running")
}

/// What `git worktree list` would print for the fixture repository: the
/// main checkout first, then every tracked worktree, plus `extra` paths.
fn scan_listing(app: &App, extra: &[(&std::path::Path, &str)]) -> Vec<git::WorktreeInfo> {
    let repo = &app.state.repositories()[0];
    let mut listed = vec![git::WorktreeInfo {
        path: repo.source_path.clone(),
        head: "abc123".into(),
        branch: Some("main".into()),
    }];
    listed.extend(repo.worktrees.iter().map(|w| git::WorktreeInfo {
        path: w.path.clone(),
        head: "abc123".into(),
        branch: Some(w.branch.clone()),
    }));
    listed.extend(extra.iter().map(|(path, branch)| git::WorktreeInfo {
        path: path.to_string_lossy().to_string(),
        head: "abc123".into(),
        branch: Some((*branch).to_string()),
    }));
    listed
}

/// A scan delivers git's listing as one event; the fold adopts the
/// worktrees the config has never seen, keeps every tracked one
/// (archived included), and persists the result once.
#[tokio::test]
async fn scanned_worktrees_reconcile_into_state() {
    let (dir, mut app) = app_with_fixture();
    let repo_id = app.state.repositories()[0].id;
    let before = app.state.repositories()[0].worktrees.len();

    // A worktree created outside forestui — the directory must exist, or
    // the reconcile treats it as a prunable stub.
    let adhoc = dir.path().join("adhoc");
    std::fs::create_dir_all(&adhoc).unwrap();
    let listed = scan_listing(&app, &[(&adhoc, "feat/adhoc")]);

    let event = scanned(&mut app, repo_id, listed.clone());
    app.handle_event(event);

    let worktrees = &app.state.repositories()[0].worktrees;
    assert_eq!(worktrees.len(), before + 1);
    let adopted = worktrees.last().expect("the adopted worktree");
    assert_eq!(adopted.name, "adhoc");
    assert_eq!(adopted.branch, "feat/adhoc");
    assert!(
        worktrees.iter().any(|w| w.is_archived),
        "reconciling dropped the archived flag"
    );
    assert_eq!(app.state.selection.repository_id, Some(repo_id));
    assert_eq!(app.state.selection.worktree_id, None);

    // In memory is not enough: an adoption that never reaches the config
    // is gone on the next launch, and the fold is the only writer.
    let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
    assert_eq!(
        reloaded.repositories()[0].worktrees.len(),
        before + 1,
        "the adoption was never persisted"
    );

    // The same listing again is a no-op, not a duplicate row.
    let event = scanned(&mut app, repo_id, listed.clone());
    app.handle_event(event);
    assert_eq!(app.state.repositories()[0].worktrees.len(), before + 1);

    // A result for a repository that has since been removed is dropped.
    let event = scanned(&mut app, Uuid::new_v4(), listed);
    app.handle_event(event);
    assert_eq!(app.state.repositories()[0].worktrees.len(), before + 1);
}

/// A scan that no longer lists a tracked worktree prunes it — someone ran
/// `git worktree remove` outside forestui — and when it was the worktree
/// on screen, the selection falls back to its repository.
#[tokio::test]
async fn a_scan_prunes_worktrees_git_no_longer_lists() {
    let (dir, mut app) = app_with_fixture();
    let repo_id = app.state.repositories()[0].id;

    app.handle_key(key(KeyCode::Down));
    let selected = app.state.selection.worktree_id.expect("a worktree");

    let mut listed = scan_listing(&app, &[]);
    let removed_path = app
        .state
        .find_worktree(selected)
        .map(|(_, w)| w.path.clone())
        .expect("the selected worktree");
    listed.retain(|info| info.path != removed_path);

    let event = scanned(&mut app, repo_id, listed);
    app.handle_event(event);

    assert!(app.state.find_worktree(selected).is_none());
    assert_eq!(app.state.selection.repository_id, Some(repo_id));
    assert_eq!(app.state.selection.worktree_id, None);
    let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
    assert!(
        reloaded
            .repositories()
            .iter()
            .flat_map(|r| &r.worktrees)
            .all(|w| w.path != removed_path),
        "the prune was never persisted"
    );
}

/// The wiring half of the in-flight rule: WorktreeSync names the worktrees
/// a listing must leave alone, and the fold has to pass them on to the
/// reconcile. git may already have deleted the directory, but the removal's
/// own fold owns both the state change and the toast.
#[tokio::test]
async fn a_scan_leaves_a_worktree_with_a_change_in_flight_alone() {
    let (_dir, mut app) = app_with_fixture();
    let repo_id = app.state.repositories()[0].id;
    let worktree_id = app.state.repositories()[0].worktrees[0].id;
    let _removal = begun(&mut app, worktree_id, WorktreeSync::begin_removal);

    let mut listed = scan_listing(&app, &[]);
    let removed_path = app.state.repositories()[0].worktrees[0].path.clone();
    listed.retain(|info| info.path != removed_path);

    let event = scanned(&mut app, repo_id, listed);
    app.handle_event(event);
    assert!(
        app.state.find_worktree(worktree_id).is_some(),
        "the scan pruned an entry whose removal fold still owns it"
    );
}

/// Focus return and the periodic tick are the standing triggers — the
/// promise that an agent's worktree appears without being asked for.
/// A minimal session card for the refresh tests.
fn a_session(id: &str) -> ClaudeSession {
    ClaudeSession {
        id: id.to_string(),
        title: format!("session {id}"),
        custom_title: None,
        recent_turns: Vec::new(),
        last_timestamp: chrono::Utc::now(),
        message_count: 1,
        git_branch: None,
        tokens: Default::default(),
        model: None,
    }
}

/// Coming back to the window is when the conversations on screen have
/// moved on, so each card on screen is re-read and says so while it is.
#[tokio::test]
async fn focus_return_refreshes_every_session_on_screen() {
    let (_dir, mut app) = app_with_fixture();
    app.sessions = Some(vec![a_session("one"), a_session("two")]);

    app.handle_event(AppEvent::Term(
        ratatui::crossterm::event::Event::FocusGained,
    ));

    assert_eq!(
        app.sessions_refreshing.len(),
        2,
        "focus return did not re-read the cards on screen"
    );

    // The tick keeps the list current too, but with the directory walk
    // alone: a per-card pass raises a spinner on every card, and one that
    // clears before the next frame is a flicker every ten seconds.
    app.sessions_refreshing.clear();
    app.sessions_in_flight.clear();
    app.last_session_refresh = Instant::now() - SESSION_REFRESH_INTERVAL;
    app.handle_event(AppEvent::Tick);
    assert!(
        app.sessions_refreshing.is_empty(),
        "the tick must not raise per-card spinners"
    );
    assert!(
        !app.sessions_in_flight.is_empty(),
        "the tick did not scan for new or removed sessions"
    );
}

/// The scan runs on a timer and almost always finds what is already on
/// screen. Repainting for that is what the tick's whole design is against.
#[tokio::test]
async fn a_scan_that_changes_nothing_does_not_repaint() {
    let (_dir, mut app) = app_with_fixture();
    // The fold keys on the pane's own path, which is what a late result for
    // a selection the user has left is checked against.
    let path = app.state.selected_path().expect("a selection");
    app.meta.path = path.clone();
    let sessions = vec![a_session("one")];
    app.sessions = Some(sessions.clone());

    app.redraw = false;
    app.handle_event(AppEvent::Sessions {
        path: path.clone(),
        sessions: Some(sessions),
    });
    assert!(!app.redraw, "an unchanged scan repainted");

    app.handle_event(AppEvent::Sessions {
        path,
        sessions: Some(vec![a_session("one"), a_session("two")]),
    });
    assert!(
        app.redraw,
        "a scan that found a new session did not repaint"
    );
}

/// Each card lands on its own, so the fold replaces one entry and leaves
/// the rest alone — and a session that took a turn while we were away
/// becomes the newest, which is where the list puts it.
#[tokio::test]
async fn a_refreshed_session_replaces_its_own_card_and_reorders() {
    let (_dir, mut app) = app_with_fixture();
    let path = app.state.selected_path().expect("a selection");
    app.meta.path = path.clone();
    let mut older = a_session("one");
    older.last_timestamp = chrono::Utc::now() - chrono::Duration::hours(2);
    app.sessions = Some(vec![a_session("two"), older]);
    app.sessions_refreshing.insert("one".to_string());

    let mut moved_on = a_session("one");
    moved_on.title = "renamed while we were away".to_string();
    moved_on.message_count = 99;
    app.handle_event(AppEvent::SessionRefreshed {
        path: path.clone(),
        id: "one".to_string(),
        session: Box::new(Some(moved_on)),
    });

    assert!(app.sessions_refreshing.is_empty(), "spinner never cleared");
    let list = app.sessions.as_ref().expect("sessions");
    assert_eq!(list.len(), 2, "a refresh must not add or drop cards");
    assert_eq!(list[0].id, "one", "the session that just moved is newest");
    assert_eq!(list[0].title, "renamed while we were away");
    assert_eq!(list[1].id, "two", "the other card is untouched");
}

/// A transcript deleted while forestui was in the background leaves a card
/// whose Resume button points at nothing.
#[tokio::test]
async fn a_session_that_vanished_loses_its_card() {
    let (_dir, mut app) = app_with_fixture();
    let path = app.state.selected_path().expect("a selection");
    app.meta.path = path.clone();
    app.sessions = Some(vec![a_session("one"), a_session("two")]);
    app.sessions_refreshing.insert("one".to_string());

    app.handle_event(AppEvent::SessionRefreshed {
        path,
        id: "one".to_string(),
        session: Box::new(None),
    });

    let list = app.sessions.as_ref().expect("sessions");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "two");
}

fn a_live_window(session_id: &str) -> crate::services::live::LiveSession {
    crate::services::live::LiveSession {
        session_id: session_id.into(),
        place: crate::services::live::LivePlace::Window {
            window_id: "@7".into(),
            window_name: "claude:demo:wt".into(),
        },
    }
}

fn a_live_elsewhere(session_id: &str, pid: u32) -> crate::services::live::LiveSession {
    crate::services::live::LiveSession {
        session_id: session_id.into(),
        place: crate::services::live::LivePlace::Elsewhere { pid },
    }
}

/// Resuming a session that is *running* must guard: an offer to switch
/// when it sits in one of our windows, an eyes-open "resume anyway" when
/// its heartbeat says it runs somewhere unreachable. A session with no
/// live entry resumes freely.
///
/// Exercised through `guard_open_session` directly, never through
/// `run_action(Resume…)`: the resume path really launches windows, and a
/// test binary inherits the developer's own `TMUX_PANE` — an earlier
/// version of this test opened a real window in a real tmux session.
/// The `cfg!(test)` gate in `services::tmux` now makes that impossible,
/// and this test stays off the launch path anyway.
#[tokio::test]
async fn resuming_an_open_session_offers_its_window_instead() {
    let (_dir, mut app) = app_with_fixture();
    let path = app.state.selected_path().expect("a selection");
    app.sessions = Some(vec![a_session("one")]);
    app.handle_event(AppEvent::LiveSessions {
        sessions: vec![a_live_window("one")],
    });

    assert!(app.guard_open_session(0, &path, false, None));

    let Some(crate::modal::Modal::Confirm(confirm)) = app.modals.last() else {
        panic!("the guard did not open a dialog: {:?}", app.modals.last());
    };
    assert!(matches!(
        confirm.action,
        crate::modal::ConfirmAction::SwitchToWindow { .. }
    ));
    assert!(confirm.message.contains("claude:demo:wt"));

    // Running somewhere unreachable: the guard warns about the fork and
    // the confirm carries everything the resume needs, since there is no
    // window to offer.
    app.modals.clear();
    app.handle_event(AppEvent::LiveSessions {
        sessions: vec![a_live_elsewhere("one", 4242)],
    });
    assert!(app.guard_open_session(0, &path, true, None));
    let Some(crate::modal::Modal::Confirm(confirm)) = app.modals.last() else {
        panic!("the elsewhere guard did not open a dialog");
    };
    assert!(confirm.message.contains("4242"), "{}", confirm.message);
    match &confirm.action {
        crate::modal::ConfirmAction::ForceResume {
            session_id, yolo, ..
        } => {
            assert_eq!(session_id, "one");
            assert!(*yolo, "the resume flavour must survive the dialog");
        }
        other => panic!("expected ForceResume, got {other:?}"),
    }
    assert_eq!(confirm.action.confirm_label(), "Resume anyway");
    assert!(confirm.action.is_destructive(), "forking earns the red");

    // No live entry at all: the guard declines and the resume may run.
    app.modals.clear();
    app.handle_event(AppEvent::LiveSessions {
        sessions: Vec::new(),
    });
    assert!(!app.guard_open_session(0, &path, false, None));
    assert!(app.modals.is_empty(), "a free session must not guard");
}

/// Deleting a session asks first and really deletes after — and refuses
/// outright while a window holds the transcript open.
#[tokio::test]
async fn deleting_a_session_confirms_then_folds_the_result() {
    let (_dir, mut app) = app_with_fixture();
    let path = app.state.selected_path().expect("a selection");
    app.meta.path = path.clone();
    app.sessions = Some(vec![a_session("one")]);
    app.state.toggle_pin(&path, "one");

    // Open in a window: refused with a warning, no dialog.
    app.live_sessions.insert("one".into(), a_live_window("one"));
    app.run_action(Action::DeleteSession(0));
    assert!(app.modals.is_empty(), "a live session must not confirm");
    assert!(
        app.notifications
            .iter()
            .any(|n| n.text.contains("close it first"))
    );

    // Not open: the confirm carries the session, and the fold removes the
    // card, drops the pin and announces.
    app.live_sessions.clear();
    app.run_action(Action::DeleteSession(0));
    let Some(crate::modal::Modal::Confirm(confirm)) = app.modals.last() else {
        panic!("no confirmation dialog");
    };
    assert!(matches!(
        confirm.action,
        crate::modal::ConfirmAction::DeleteClaudeSession { .. }
    ));

    app.handle_event(AppEvent::SessionDeleted {
        path: path.clone(),
        session_id: "one".into(),
        title: "session one".into(),
        result: Ok(()),
    });
    assert!(app.sessions.as_ref().is_some_and(Vec::is_empty));
    assert!(!app.state.is_pinned(&path, "one"), "the pin outlived it");
    assert!(
        app.notifications
            .iter()
            .any(|n| n.text.contains("Deleted session"))
    );

    // A failed delete keeps the card and says why.
    app.sessions = Some(vec![a_session("two")]);
    app.handle_event(AppEvent::SessionDeleted {
        path,
        session_id: "two".into(),
        title: "session two".into(),
        result: Err("permission denied".into()),
    });
    assert_eq!(app.sessions.as_ref().map(Vec::len), Some(1));
    assert!(
        app.notifications
            .iter()
            .any(|n| n.text.contains("permission denied"))
    );
}

/// Pinning floats the card to the top; K/J rearrange within the pinned
/// block, from whichever of the card's controls holds the cursor.
#[tokio::test]
async fn pins_reorder_the_list_and_follow_the_keys() {
    let (_dir, mut app) = app_with_fixture();
    let path = app.state.selected_path().expect("a selection");
    app.meta.path = path.clone();
    let mut old = a_session("old");
    old.last_timestamp = chrono::Utc::now() - chrono::Duration::hours(10);
    app.sessions = Some(vec![a_session("new"), old]);

    // Pin the older session: it leads the list.
    app.run_action(Action::TogglePinSession(1));
    assert!(app.state.is_pinned(&path, "old"));
    assert_eq!(app.sessions.as_ref().unwrap()[0].id, "old");

    // Pin the other one too (now at index 1) and move it up with K.
    app.run_action(Action::TogglePinSession(1));
    assert_eq!(app.state.pinned_for(&path), vec!["old", "new"]);

    // The cursor sits on a control of the second pinned card.
    app.drawn_items = detail::drawn(&detail::content(&app));
    let second_card_control = app
        .drawn_items
        .iter()
        .position(|(item, _)| *item == DetailItem::Action(Action::ResumeSession(1)))
        .expect("the second card renders");
    app.focus = Focus::Detail;
    app.detail_index = second_card_control;

    app.handle_key(key(KeyCode::Char('K')));
    assert_eq!(app.state.pinned_for(&path), vec!["new", "old"]);
    assert_eq!(app.sessions.as_ref().unwrap()[0].id, "new");

    // The cursor rode along: it now points at the same control on the
    // card's new position, one card-width up.
    let per_card = 5 + app.settings.custom_buttons.len();
    assert_eq!(app.detail_index, second_card_control - per_card);

    // On an unpinned card the key falls through to the (absent) binding.
    app.sessions.as_mut().unwrap().push(a_session("loose"));
    app.drawn_items = detail::drawn(&detail::content(&app));
    let loose_control = app
        .drawn_items
        .iter()
        .position(|(item, _)| *item == DetailItem::Action(Action::ResumeSession(2)))
        .expect("the loose card renders");
    app.detail_index = loose_control;
    app.handle_key(key(KeyCode::Char('J')));
    assert_eq!(app.state.pinned_for(&path), vec!["new", "old"]);
}

/// The rename dialog continues the name that already exists, the way
/// tmux's own rename prompt does: the window's name for a live session,
/// the card's shown title otherwise — never an empty field on a session
/// that visibly has a name.
#[tokio::test]
async fn the_rename_dialog_seeds_with_the_current_name() {
    let (_dir, mut app) = app_with_fixture();
    let path = app.state.selected_path().expect("a selection");
    app.meta.path = path.clone();

    // Unnamed session: the shown title is its first prompt.
    app.sessions = Some(vec![a_session("one")]);
    app.run_action(Action::RenameSession(0));
    let Some(crate::modal::Modal::RenameSession(modal)) = app.modals.last() else {
        panic!("no rename dialog");
    };
    assert_eq!(modal.name.value(), "session one");
    app.modals.clear();

    // A chosen name outranks the fallback title.
    app.sessions.as_mut().unwrap()[0].custom_title = Some("chosen name".into());
    app.run_action(Action::RenameSession(0));
    let Some(crate::modal::Modal::RenameSession(modal)) = app.modals.last() else {
        panic!("no rename dialog");
    };
    assert_eq!(modal.name.value(), "chosen name");
    app.modals.clear();

    // Live: the window's name is the truest form of the current name.
    app.live_sessions.insert("one".into(), a_live_window("one"));
    app.run_action(Action::RenameSession(0));
    let Some(crate::modal::Modal::RenameSession(modal)) = app.modals.last() else {
        panic!("no rename dialog");
    };
    assert_eq!(modal.name.value(), "claude:demo:wt");
    app.modals.clear();

    // An over-long fallback title is clipped to what typing could produce.
    app.live_sessions.clear();
    app.sessions.as_mut().unwrap()[0].custom_title = None;
    app.sessions.as_mut().unwrap()[0].title = "x".repeat(100);
    app.run_action(Action::RenameSession(0));
    let Some(crate::modal::Modal::RenameSession(modal)) = app.modals.last() else {
        panic!("no rename dialog");
    };
    assert_eq!(
        modal.name.value().chars().count(),
        crate::modal::MAX_SESSION_NAME_LENGTH
    );
}

/// The live scan's fold replaces the map and repaints only on change.
#[tokio::test]
async fn the_live_scan_folds_quietly_when_nothing_changed() {
    let (_dir, mut app) = app_with_fixture();
    let sessions = vec![a_live_window("one"), a_live_elsewhere("two", 99)];

    app.redraw = false;
    app.handle_event(AppEvent::LiveSessions {
        sessions: sessions.clone(),
    });
    assert!(app.redraw, "the first result changes the frame");
    assert_eq!(app.live_sessions.len(), 2);

    app.redraw = false;
    app.handle_event(AppEvent::LiveSessions { sessions });
    assert!(!app.redraw, "an identical sweep must not repaint");

    // A session that ended disappears from the map wholesale.
    app.handle_event(AppEvent::LiveSessions {
        sessions: Vec::new(),
    });
    assert!(app.live_sessions.is_empty());
}

/// A late answer for a selection the user has already left must not put
/// another path's conversation into this pane.
#[tokio::test]
async fn a_refresh_for_another_path_is_dropped() {
    let (_dir, mut app) = app_with_fixture();
    app.sessions = Some(vec![a_session("one")]);
    app.sessions_refreshing.insert("one".to_string());

    app.handle_event(AppEvent::SessionRefreshed {
        path: "/somewhere/else".to_string(),
        id: "one".to_string(),
        session: Box::new(Some(a_session("intruder"))),
    });

    let list = app.sessions.as_ref().expect("sessions");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "one", "another path's result was folded in");
    assert!(
        app.sessions_refreshing.is_empty(),
        "the spinner must clear even for a result that is dropped"
    );
}

#[tokio::test]
async fn focus_return_and_the_tick_kick_a_sweep() {
    let (_dir, mut app) = app_with_fixture();
    let repo_id = app.state.repositories()[0].id;
    // A scan is running exactly when another may not start.
    let scan_running = |app: &mut App| app.worktree_sync.start_scan(repo_id).is_none();

    app.handle_event(AppEvent::Term(
        ratatui::crossterm::event::Event::FocusGained,
    ));
    assert!(scan_running(&mut app), "focus return did not start a scan");

    // Land that scan, so the sweep below has to start its own.
    let scan = app.worktree_sync.scan_ticket_for_test(repo_id);
    app.handle_event(AppEvent::WorktreesScanned { scan, listed: None });
    app.last_worktree_scan = Instant::now() - WORKTREE_SCAN_INTERVAL;
    app.handle_event(AppEvent::Tick);
    assert!(
        scan_running(&mut app),
        "the periodic sweep did not start a scan"
    );
}

/// When a scan corrects the branch of the worktree on screen, the rename
/// input must be reseeded — otherwise Enter renames the branch straight
/// back to the stale value it still shows.
#[tokio::test]
async fn branch_drift_on_the_selected_worktree_reseeds_the_rename_input() {
    let (_dir, mut app) = app_with_fixture();
    let repo_id = app.state.repositories()[0].id;
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("a worktree");

    let mut listed = scan_listing(&app, &[]);
    let path = app
        .state
        .find_worktree(worktree_id)
        .map(|(_, w)| w.path.clone())
        .expect("the selected worktree");
    for info in &mut listed {
        if info.path == path {
            info.branch = Some("feat/drifted".into());
        }
    }

    // The pane's async content must survive: the path did not change, so
    // a full reload here would only reset the reader's scroll and drop
    // the sessions list back to Loading on every external checkout.
    app.sessions = Some(Vec::new());

    let event = scanned(&mut app, repo_id, listed);
    app.handle_event(event);
    assert_eq!(
        app.branch_input.value(),
        "feat/drifted",
        "the rename input still shows the branch git no longer has"
    );
    assert!(
        app.sessions.is_some(),
        "a branch drift reloaded the whole pane"
    );
}

/// A branch rename that git refuses must leave no trace. The name used to
/// be written and persisted before `git branch -m` ran, so a collision left
/// the config and the sidebar naming a branch that does not exist.
#[tokio::test]
async fn a_branch_rename_lands_only_when_git_agrees() {
    let (dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("a worktree");
    let original = app
        .state
        .find_worktree(worktree_id)
        .map(|(_, w)| w.branch.clone())
        .expect("a branch");

    // What the failing path does: report, and fold nothing.
    app.handle_event(AppEvent::Notify(
        "Branch rename failed: already exists".into(),
        Severity::Error,
    ));
    assert_eq!(
        app.state.find_worktree(worktree_id).map(|(_, w)| &w.branch),
        Some(&original),
        "the branch was renamed in state before git ran"
    );

    // And the success path folds through the event.
    let change = begun(&mut app, worktree_id, WorktreeSync::begin_rename);
    app.handle_event(AppEvent::WorktreeBranchRenamed {
        change,
        branch: "feat/accepted".into(),
    });
    assert_eq!(
        app.state
            .find_worktree(worktree_id)
            .map(|(_, w)| w.branch.as_str()),
        Some("feat/accepted")
    );
    let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
    assert!(
        reloaded
            .repositories()
            .iter()
            .flat_map(|r| &r.worktrees)
            .any(|w| w.branch == "feat/accepted"),
        "the accepted rename was never persisted"
    );
}

#[tokio::test]
async fn a_scan_that_lands_late_does_not_steal_the_selection() {
    let (dir, mut app) = app_with_fixture();
    let repo_id = app.state.repositories()[0].id;

    // The user moved on to a worktree while the scan was still running.
    app.handle_key(key(KeyCode::Down));
    let selected = app.state.selection.worktree_id.expect("a worktree");
    app.sessions = Some(Vec::new());

    let adhoc = dir.path().join("late-adhoc");
    std::fs::create_dir_all(&adhoc).unwrap();
    let listed = scan_listing(&app, &[(&adhoc, "feat/late")]);
    let event = scanned(&mut app, repo_id, listed);
    app.handle_event(event);

    assert_eq!(
        app.state.selection.worktree_id,
        Some(selected),
        "the scan yanked the cursor back to the repository"
    );
    assert!(
        app.sessions.is_some(),
        "the pane the user was reading was reloaded out from under them"
    );
}

#[tokio::test]
async fn stale_results_for_a_previous_selection_are_ignored() {
    let (_dir, mut app) = app_with_fixture();
    app.meta.path = "/current".into();

    app.handle_event(AppEvent::Sessions {
        path: "/stale".into(),
        sessions: Some(vec![]),
    });
    assert!(app.sessions.is_none(), "a late result must not land");

    app.handle_event(AppEvent::Sessions {
        path: "/current".into(),
        sessions: Some(vec![]),
    });
    assert!(app.sessions.is_some());
}

#[tokio::test]
async fn a_failed_session_scan_keeps_what_the_cache_painted() {
    let (_dir, mut app) = app_with_fixture();
    app.meta.path = "/current".into();
    app.sessions = Some(vec![]);

    app.handle_event(AppEvent::Sessions {
        path: "/current".into(),
        sessions: None,
    });
    assert!(
        app.sessions.is_some(),
        "a panicked scan must not overwrite painted content"
    );
}

/// What the renderer does each frame: snapshot the drawn items that
/// activation resolves against.
fn snapshot_drawn(app: &mut App) {
    app.drawn_items = detail::drawn(&detail::content(app));
}

fn click(column: u16, row: u16) -> ratatui::crossterm::event::MouseEvent {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn rect(x: u16, y: u16, width: u16, height: u16) -> ratatui::layout::Rect {
    ratatui::layout::Rect {
        x,
        y,
        width,
        height,
    }
}

#[tokio::test]
async fn hit_at_prefers_the_last_recorded_region() {
    let (_dir, mut app) = app_with_fixture();
    app.push_hit(rect(0, 0, 10, 1), HitTarget::SidebarRow(3));
    // A modal drawn afterwards covers the same cell and must win.
    app.push_hit(
        rect(0, 0, 10, 1),
        HitTarget::ModalControl {
            index: 1,
            click: ModalClick::Activate,
        },
    );

    assert_eq!(
        app.hit_at(5, 0),
        Some(HitTarget::ModalControl {
            index: 1,
            click: ModalClick::Activate
        })
    );
    assert_eq!(app.hit_at(50, 0), None, "outside every region");
    assert_eq!(app.hit_at(5, 9), None, "wrong row");
}

fn wheel(column: u16, row: u16) -> ratatui::crossterm::event::MouseEvent {
    use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
    MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// The footer is a row of buttons, and a click on one says which binding
/// was meant. Replaying it as a keystroke let a focused rename field eat it
/// as text — every footer button was dead while renaming.
#[tokio::test]
async fn a_footer_click_works_while_a_rename_field_has_focus() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    app.sessions = Some(Vec::new());
    snapshot_drawn(&mut app);

    app.focus = Focus::Detail;
    app.detail_index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Field(Field::WorktreeName))
        .expect("rename field present");
    let name = app.name_input.value().to_string();

    app.push_hit(rect(0, 40, 10, 1), HitTarget::FooterKey('s'));
    app.handle_mouse(click(2, 40));

    assert_eq!(app.modals.len(), 1, "the settings dialog never opened");
    assert_eq!(
        app.name_input.value(),
        name,
        "the click was typed into the rename field"
    );
}

#[tokio::test]
async fn a_toast_swallows_the_clicks_it_covers() {
    let (_dir, mut app) = app_with_fixture();
    // A control is recorded first, then a toast is drawn over the same cell.
    app.push_hit(rect(10, 5, 20, 3), HitTarget::DetailItem(0));
    app.push_hit(rect(10, 5, 20, 3), HitTarget::Notification);

    assert_eq!(
        app.hit_at(12, 6),
        Some(HitTarget::Notification),
        "the toast must win the cells it covers, or the control under it runs"
    );
}

#[tokio::test]
async fn an_open_modal_owns_the_wheel_as_well_as_the_click() {
    let (_dir, mut app) = app_with_fixture();
    app.sidebar_rect = rect(0, 0, 30, 20);
    let before = app.state.selection;

    // The panes are still drawn behind a modal, so they are still
    // hit-testable; only the guard stops the wheel reaching them.
    app.handle_key(key(KeyCode::Char('S')));
    assert_eq!(app.modals.len(), 1);
    app.handle_mouse(wheel(4, 5));

    assert_eq!(
        app.state.selection, before,
        "the wheel reselected the sidebar behind an open modal"
    );
}

#[tokio::test]
async fn clicking_a_sidebar_row_selects_it() {
    let (_dir, mut app) = app_with_fixture();
    assert!(app.state.selection.is_repository());

    app.push_hit(rect(0, 1, 30, 1), HitTarget::SidebarRow(1));
    app.handle_mouse(click(4, 1));

    assert_eq!(app.sidebar_index, 1);
    assert!(app.state.selection.is_worktree());
    assert_eq!(app.focus, Focus::Sidebar);
}

#[tokio::test]
async fn clicking_a_detail_control_focuses_and_runs_it() {
    let (_dir, mut app) = app_with_fixture();
    app.sessions = Some(Vec::new());
    app.issues = Some(Vec::new());
    snapshot_drawn(&mut app);

    let index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Action(Action::Terminal))
        .expect("terminal control present");
    app.push_hit(rect(40, 5, 12, 1), HitTarget::DetailItem(index));
    app.handle_mouse(click(45, 5));

    assert_eq!(app.focus, Focus::Detail);
    assert_eq!(app.detail_index, index);
    // The fixture's directories do not exist, so the action refuses loudly
    // rather than silently opening a window somewhere else.
    assert_eq!(app.notifications.len(), 1);
    assert!(app.notifications[0].text.contains("no longer exists"));
}

/// A control the frame drew as disabled must not fire — from the mouse or
/// from Enter. The greyed-out `↓ Git Pull (No remote)` used to run `git
/// pull` anyway and answer with an error toast. A click must not take
/// focus either: a focused control renders with the full accent style, so
/// moving the cursor there made a disabled `[Del]` light up as if it ran.
#[tokio::test]
async fn a_disabled_control_does_not_activate() {
    let (_dir, mut app) = app_with_fixture();
    app.sessions = Some(Vec::new());
    app.issues = Some(Vec::new());
    // The fixture has no remote, so the sync control renders disabled.
    assert!(!app.meta.has_remote);
    snapshot_drawn(&mut app);

    let index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Action(Action::Sync))
        .expect("sync control present");
    assert!(!app.drawn_items[index].1, "sync should be drawn disabled");

    app.push_hit(rect(40, 5, 12, 1), HitTarget::DetailItem(index));
    app.handle_mouse(click(45, 5));
    assert!(app.notifications.is_empty(), "the click ran the action");
    assert_eq!(app.focus, Focus::Sidebar, "the click focused the control");

    app.focus = Focus::Detail;
    app.detail_index = index;
    app.handle_key(key(KeyCode::Enter));
    assert!(app.notifications.is_empty(), "Enter ran the action");
}

/// A background event drained in the same batch as a click must not remap
/// the clicked index onto a different control: activation resolves against
/// the snapshot of the frame the user saw.
#[tokio::test]
async fn a_click_resolves_against_the_drawn_frame_not_the_live_list() {
    let (_dir, mut app) = app_with_fixture();
    app.sessions = Some(Vec::new());
    app.issues = Some(Vec::new());
    snapshot_drawn(&mut app);

    // On the drawn frame (zero issues), this index is Remove Repository.
    let index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Action(Action::RemoveRepository))
        .expect("remove control present");
    app.push_hit(rect(40, 5, 20, 1), HitTarget::DetailItem(index));

    // Issues finish loading before the click is handled; the live list
    // grows and `index` would now point at StartWorkOnIssue(0).
    let path = app
        .state
        .selected_repository()
        .expect("repository selected")
        .source_path
        .clone();
    app.handle_event(AppEvent::Issues {
        path,
        issues: vec![
            serde_json::from_str(
                r#"{"number":1,"title":"t","createdAt":"2026-01-01T00:00:00Z",
                        "updatedAt":"2026-01-01T00:00:00Z","labels":[]}"#,
            )
            .expect("issue fixture"),
        ],
    });
    assert_eq!(
        app.detail_items().get(index),
        Some(&DetailItem::Action(Action::StartWorkOnIssue(0))),
        "precondition: the live list did remap the index"
    );

    app.handle_mouse(click(45, 5));
    // Remove Repository opens the confirm modal; StartWorkOnIssue would
    // instead have started a branch load with no modal.
    assert!(
        matches!(app.modals.last(), Some(Modal::Confirm(_))),
        "the click fired the remapped control instead of the drawn one"
    );
}

#[tokio::test]
async fn clicking_a_detail_field_only_moves_focus() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    app.sessions = Some(Vec::new());
    snapshot_drawn(&mut app);

    let index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Field(Field::BranchName))
        .expect("rename field present");
    app.push_hit(rect(40, 9, 20, 1), HitTarget::DetailItem(index));
    app.handle_mouse(click(42, 9));

    assert_eq!(app.detail_index, index);
    assert!(app.notifications.is_empty(), "a field is not an action");
}

#[tokio::test]
async fn a_modal_swallows_clicks_on_the_panes_behind_it() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Char('a')));
    assert_eq!(app.modals.len(), 1);

    let before = app.sidebar_index;
    app.push_hit(rect(0, 1, 30, 1), HitTarget::SidebarRow(1));
    app.handle_mouse(click(4, 1));

    assert_eq!(app.sidebar_index, before, "the modal keeps the click");
    assert_eq!(app.modals.len(), 1);
}

#[tokio::test]
async fn clicking_confirm_delete_runs_the_deletion() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
    app.handle_key(key(KeyCode::Char('d')));
    assert!(matches!(app.modals.last(), Some(Modal::Confirm(_))));

    // Index 1 is Delete; index 0 would be Cancel.
    app.push_hit(
        rect(10, 10, 10, 1),
        HitTarget::ModalControl {
            index: 1,
            click: ModalClick::Activate,
        },
    );
    app.handle_mouse(click(12, 10));

    assert!(app.modals.is_empty());
    // Removal is asynchronous now; the click only dispatches it. The entry
    // goes when the git result is folded.
    assert!(app.state.find_worktree(worktree_id).is_some());
    let change = app.worktree_sync.change_ticket_for_test(worktree_id);
    app.handle_event(AppEvent::WorktreeRemoveResult {
        change,
        outcome: Ok(git::RemoveOutcome::Removed),
    });
    assert!(app.state.find_worktree(worktree_id).is_none());
}

#[tokio::test]
async fn clicking_a_field_or_cycle_does_not_submit_the_modal() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Char('S')));
    assert_eq!(app.modals.len(), 1);

    // Focus index 1 is the branch-prefix input. Clicking it must focus, not save.
    app.push_hit(
        rect(0, 0, 10, 3),
        HitTarget::ModalControl {
            index: 1,
            click: ModalClick::Focus,
        },
    );
    app.handle_mouse(click(2, 1));
    assert_eq!(app.modals.len(), 1, "clicking a field closed the dialog");

    // Index 0 is the editor cycle: clicking advances it, still without saving.
    let before = match app.modals.last() {
        Some(Modal::Settings(m)) => m.editor_index,
        _ => panic!("settings modal expected"),
    };
    app.push_hit(
        rect(0, 5, 20, 1),
        HitTarget::ModalControl {
            index: 0,
            click: ModalClick::Cycle(Direction::Next),
        },
    );
    app.handle_mouse(click(3, 5));
    assert_eq!(app.modals.len(), 1, "clicking a cycle closed the dialog");
    let advanced = match app.modals.last() {
        Some(Modal::Settings(m)) => {
            assert_ne!(m.editor_index, before, "cycle did not advance");
            m.editor_index
        }
        _ => panic!("settings modal expected"),
    };

    // `Previous` steps back to exactly where it started — not merely
    // "somewhere else", which a second forward step would also satisfy.
    app.push_hit(
        rect(0, 5, 20, 1),
        HitTarget::ModalControl {
            index: 0,
            click: ModalClick::Cycle(Direction::Previous),
        },
    );
    app.handle_mouse(click(3, 5));
    match app.modals.last() {
        Some(Modal::Settings(m)) => assert_eq!(
            m.editor_index, before,
            "stepping back from {advanced} landed on {} instead of {before}",
            m.editor_index
        ),
        _ => panic!("settings modal expected"),
    }
}

#[tokio::test]
async fn clicking_a_branch_row_selects_that_row() {
    use crate::modal::AddWorktreeModal;
    let (_dir, mut app) = app_with_fixture();
    let repo = app.state.repositories()[0].clone();
    let branches = vec!["main".into(), "release-2".into(), "feat/other".into()];
    let mut modal = AddWorktreeModal::new(
        &repo,
        branches,
        vec![],
        PathBuf::from("/forest"),
        "feat/".into(),
    );
    modal.new_branch = false;
    app.modals.push(Modal::AddWorktree(Box::new(modal)));

    app.push_hit(
        rect(0, 7, 40, 1),
        HitTarget::ModalControl {
            index: 3,
            click: ModalClick::Row(2),
        },
    );
    app.handle_mouse(click(5, 7));

    match app.modals.last() {
        Some(Modal::AddWorktree(m)) => {
            assert_eq!(
                m.selected_branch(),
                "feat/other",
                "the clicked row was not selected"
            );
            assert_eq!(m.focus, 3, "focus did not move to the results list");
        }
        _ => panic!("add-worktree modal expected"),
    }
}

#[tokio::test]
async fn a_burst_of_events_is_handled_before_one_repaint() {
    // The draw loop redraws once per iteration, so everything already
    // queued has to be consumed in a single pass or a click repaints the
    // screen several times over — which is what flickering looked like.
    let dir = tempfile::tempdir().expect("tempdir");
    let mut state = AppState::load_from(dir.path().join(".forestui-config.json"));
    state.add_repository(Repository::new("demo".into(), "/tmp/demo".into()));

    let (tx, mut rx) = crate::event::start();
    let mut app = App::with_state(tx.clone(), state, Settings::default());

    for _ in 0..5 {
        tx.info("burst");
    }
    // Let the sends land in the channel before draining.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let handled = app.drain(&mut rx);
    assert!(handled >= 5, "drained only {handled} events");
    assert!(rx.try_recv().is_err(), "events left queued after draining");
}

#[tokio::test]
async fn pointer_motion_is_ignored() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    app.push_hit(rect(0, 1, 30, 1), HitTarget::SidebarRow(1));

    let at = |kind| MouseEvent {
        kind,
        column: 4,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    // Motion and release must not act; only the press does.
    app.handle_mouse(at(MouseEventKind::Moved));
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left)));
    app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left)));
    assert_eq!(app.sidebar_index, 0, "motion changed the selection");

    app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left)));
    assert_eq!(app.sidebar_index, 1);
}

/// The wheel follows the pointer, not the keyboard focus. Scrolling the
/// sidebar because the mouse happened to be over the detail pane was the
/// reported bug.
#[tokio::test]
async fn scroll_wheel_follows_the_pointer_not_the_focus() {
    use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    app.sidebar_rect = rect(0, 0, 35, 40);
    app.detail_rect = rect(35, 0, 115, 40);
    let wheel = |kind, column| MouseEvent {
        kind,
        column,
        row: 10,
        modifiers: KeyModifiers::NONE,
    };

    // Over the sidebar: the sidebar moves.
    app.handle_mouse(wheel(MouseEventKind::ScrollDown, 5));
    assert_eq!(app.sidebar_index, 1);
    app.handle_mouse(wheel(MouseEventKind::ScrollUp, 5));
    assert_eq!(app.sidebar_index, 0);

    // Over the detail pane, while the sidebar still has focus: the detail
    // pane scrolls and the selection is left alone.
    assert_eq!(app.focus, Focus::Sidebar);
    app.handle_mouse(wheel(MouseEventKind::ScrollDown, 90));
    assert_eq!(app.sidebar_index, 0, "the wheel moved the wrong pane");
    assert_eq!(app.detail_scroll, SCROLL_STEP as u16);

    // And it never scrolls above the top.
    app.handle_mouse(wheel(MouseEventKind::ScrollUp, 90));
    app.handle_mouse(wheel(MouseEventKind::ScrollUp, 90));
    assert_eq!(app.detail_scroll, 0);
}

/// Dragging the scrollbar thumb scrolls the pane, and picking the thumb up
/// part-way down must not snap it to the pointer.
#[tokio::test]
async fn dragging_the_scrollbar_thumb_scrolls_the_pane() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    // A 20-row track over 120 lines of content: thumb 3 rows, travel 17,
    // max offset 100.
    let geom = ScrollbarGeom::new(rect(148, 2, 2, 20), 120).expect("content overflows");
    assert_eq!((geom.thumb, geom.travel, geom.max_offset), (3, 17, 100));
    app.scrollbar = Some(geom);
    app.detail_scroll = 0;

    let at = |kind, row| MouseEvent {
        kind,
        column: 148,
        row,
        modifiers: KeyModifiers::NONE,
    };

    // Grab the thumb one row below its top; the offset must not move yet.
    app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), 3));
    assert_eq!(app.scroll_drag, Some(1));
    assert_eq!(app.detail_scroll, 0, "grabbing the thumb moved the pane");

    // Drag down ten rows: the thumb top follows the grab point, not the
    // pointer, so it lands at row 10 of the track.
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 13));
    assert_eq!(app.detail_scroll, geom.offset_at(10));

    // Past the bottom of the track it pins to the end rather than running on.
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 200));
    assert_eq!(app.detail_scroll, geom.max_offset);

    // Above the top it pins to the start.
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 0));
    assert_eq!(app.detail_scroll, 0);

    // The release applies its own position before ending the gesture, so a
    // flick that ends past the last drag report is not thrown away.
    app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left), 9));
    assert_eq!(app.detail_scroll, geom.offset_at(9 - 2 - 1));
    assert_eq!(app.scroll_drag, None);
    app.detail_scroll = 0;
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 18));
    assert_eq!(app.detail_scroll, 0, "the pane scrolled after the release");
}

/// Pressing the bare track pages to the pointer instead of doing nothing.
#[tokio::test]
async fn pressing_the_scrollbar_track_pages_to_the_pointer() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    let geom = ScrollbarGeom::new(rect(148, 2, 2, 20), 120).expect("content overflows");
    app.scrollbar = Some(geom);
    app.detail_scroll = 0;
    let focus_before = app.focus;

    // Row 14 of the track, well below the resting thumb.
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 148,
        row: 16,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(app.detail_scroll, geom.offset_at(14 - geom.thumb / 2));
    assert!(app.scroll_drag.is_some(), "the press did not begin a drag");
    assert_eq!(app.focus, focus_before, "the scrollbar stole focus");
}

/// A drag that began somewhere else must not move the pane.
#[tokio::test]
async fn a_drag_outside_the_scrollbar_does_not_scroll() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    app.scrollbar = Some(ScrollbarGeom::new(rect(148, 2, 2, 20), 120).expect("overflows"));
    app.detail_scroll = 7;

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 40,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(app.detail_scroll, 7);
}

/// The thumb the renderer draws and the offset a drag computes have to be
/// inverses, or the bar drifts under the pointer over a long drag.
#[tokio::test]
async fn thumb_position_and_offset_round_trip() {
    let geom = ScrollbarGeom::new(rect(0, 0, 2, 20), 120).expect("content overflows");
    for top in 0..=geom.travel {
        let offset = geom.offset_at(top);
        assert_eq!(
            geom.thumb_top(offset),
            top,
            "thumb top {top} did not survive"
        );
    }
}

/// Hover is what makes a control look clickable; it also must not repaint
/// the app for every pointer report, which is why motion is tracked at all.
#[tokio::test]
async fn hover_tracks_the_pointer_and_only_repaints_on_a_change() {
    use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    app.push_hit(rect(0, 0, 10, 1), HitTarget::SidebarRow(0));
    let moved = |column, row| MouseEvent {
        kind: MouseEventKind::Moved,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };

    app.redraw = false;
    app.handle_mouse(moved(2, 0));
    assert_eq!(app.hovered, Some(HitTarget::SidebarRow(0)));
    assert!(app.redraw, "entering a control has to repaint it");

    // Moving within the same control changes nothing on screen.
    app.redraw = false;
    app.handle_mouse(moved(4, 0));
    assert!(!app.redraw, "a no-op move repainted the app");

    app.redraw = false;
    app.handle_mouse(moved(50, 30));
    assert_eq!(app.hovered, None);
    assert!(app.redraw, "leaving a control has to repaint it");
}

/// A no-op pointer move must not cancel a repaint another event in the same
/// drain already earned.
#[tokio::test]
async fn a_no_op_move_does_not_swallow_another_events_repaint() {
    use ratatui::crossterm::event::{Event, KeyModifiers, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    app.redraw = false;

    app.handle_event(AppEvent::Notify("hi".into(), Severity::Information));
    app.handle_event(AppEvent::Term(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 200,
        row: 200,
        modifiers: KeyModifiers::NONE,
    })));

    assert!(app.redraw, "the notify's repaint was swallowed by the move");
}

/// Ticks arrive ten times a second; an idle frame must not repaint on them,
/// or the app keeps the terminal permanently busy doing nothing.
#[tokio::test]
async fn an_idle_tick_does_not_repaint() {
    let (_dir, mut app) = app_with_fixture();
    // A worktree selection has no spinner on screen.
    app.handle_key(key(KeyCode::Down));
    app.sessions = Some(Vec::new());
    app.spinner_index = 0;

    // Nine idle ticks paint nothing; the tenth refreshes the on-screen
    // relative times, which have second granularity.
    for tick in 1..=10 {
        app.redraw = false;
        app.handle_event(AppEvent::Tick);
        if tick < 10 {
            assert!(!app.redraw, "idle tick {tick} repainted the app");
        } else {
            assert!(app.redraw, "the once-a-second refresh never fired");
        }
    }
}

/// While the issue spinner is on screen the tick is what animates it.
#[tokio::test]
async fn a_tick_repaints_while_the_issue_spinner_is_visible() {
    let (_dir, mut app) = app_with_fixture();
    assert!(app.state.selection.is_repository());
    app.issues = None;
    app.redraw = false;

    app.handle_event(AppEvent::Tick);
    assert!(app.redraw, "the spinner froze");

    // Once the issues have landed there is nothing animating. Away from
    // the whole-second boundary, a loaded pane paints nothing.
    app.issues = Some(Vec::new());
    app.sessions = Some(Vec::new());
    app.notifications.clear();
    app.spinner_index = 1;
    app.redraw = false;
    app.handle_event(AppEvent::Tick);
    assert!(!app.redraw, "a loaded pane repainted on the tick");
}

/// A toast leaving the screen is a frame change, and the tick that expires
/// it has to pay for the repaint.
#[tokio::test]
async fn a_tick_repaints_when_a_notification_expires() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    app.sessions = Some(Vec::new());
    app.notifications.push(Notification {
        text: "old".into(),
        severity: Severity::Information,
        created: Instant::now() - NOTIFICATION_TTL - Duration::from_secs(1),
    });
    app.redraw = false;

    app.handle_event(AppEvent::Tick);
    assert!(app.notifications.is_empty());
    assert!(app.redraw, "the expired toast was left on screen");
}

/// A newer version marks the title, not a toast, and the mark is sticky:
/// ticks that expire every toast on screen leave it exactly where it was,
/// because the process stays the old build until it restarts.
#[tokio::test]
async fn a_finished_update_marks_the_title_until_restart() {
    use crate::version_check::UpdateStatus;

    let (_dir, mut app) = app_with_fixture();
    assert_eq!(app.title_suffix(false), None, "nothing pending at launch");
    app.redraw = false;

    app.handle_event(AppEvent::UpdateChecked(UpdateStatus::Installed(
        "9.9.9".into(),
    )));
    assert!(app.redraw, "the title changed, so the frame must repaint");
    assert!(
        app.notifications.is_empty(),
        "the title carries the news; a toast on top is noise"
    );
    assert_eq!(
        app.title_suffix(false).as_deref(),
        Some("(v9.9.9 ready — restart to update)")
    );

    // Well past any toast's lifetime, with other traffic in between.
    app.notifications.push(Notification {
        text: "old".into(),
        severity: Severity::Information,
        created: Instant::now() - NOTIFICATION_TTL - Duration::from_secs(1),
    });
    for _ in 0..50 {
        app.handle_event(AppEvent::Tick);
    }
    app.handle_key(key(KeyCode::Down));
    assert!(app.notifications.is_empty());
    assert_eq!(
        app.title_suffix(false).as_deref(),
        Some("(v9.9.9 ready — restart to update)"),
        "the mark outlived nothing but the toasts should"
    );
}

/// Only outcomes that leave a newer build to run mark the title. A failed
/// install is a toast with its reason, and silence changes nothing.
#[tokio::test]
async fn only_a_newer_build_marks_the_title() {
    use crate::version_check::UpdateStatus;

    let (_dir, mut app) = app_with_fixture();
    app.handle_event(AppEvent::UpdateChecked(UpdateStatus::Silent));
    assert_eq!(app.title_suffix(false), None);
    assert!(app.notifications.is_empty());

    app.handle_event(AppEvent::UpdateChecked(UpdateStatus::InstallFailed {
        version: "9.9.9".into(),
        reason: "permission denied".into(),
    }));
    assert_eq!(app.title_suffix(false), None);
    assert_eq!(app.notifications.len(), 1);
    assert_eq!(app.notifications[0].severity, Severity::Error);
    assert!(app.notifications[0].text.contains("permission denied"));

    let (_dir, mut app) = app_with_fixture();
    app.handle_event(AppEvent::UpdateChecked(UpdateStatus::Available(
        "9.9.9".into(),
    )));
    assert_eq!(
        app.title_suffix(false).as_deref(),
        Some("(v9.9.9 available — cargo install forestui)")
    );
    assert!(app.notifications.is_empty());
}

/// The sidebar's branch column earns its space or does not appear.
#[test]
fn only_a_branch_that_differs_from_the_name_is_shown() {
    // What `w` creates: prefix + name, and so pure repetition.
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

/// Clicking a footer entry is the same as pressing its key.
#[tokio::test]
async fn clicking_the_footer_runs_the_binding() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    app.push_hit(rect(0, 39, 10, 1), HitTarget::FooterKey('a'));

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 3,
        row: 39,
        modifiers: KeyModifiers::NONE,
    });

    assert!(
        matches!(app.modals.last(), Some(Modal::AddRepository(_))),
        "clicking `a` in the footer did not open Add Repository"
    );
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

// ---------------------------------------------------------------- start work

fn open_issue(number: i64, title: &str) -> crate::models::GitHubIssue {
    serde_json::from_value(serde_json::json!({
        "number": number,
        "title": title,
        "createdAt": "2026-01-01T00:00:00Z",
        "updatedAt": "2026-01-01T00:00:00Z",
    }))
    .expect("issue fixture")
}

/// The fixture's repository with `issues` already in the GitHub cache, so
/// opening Start work never has to ask `gh`.
fn app_with_issues(issues: Vec<crate::models::GitHubIssue>) -> (tempfile::TempDir, App, String) {
    let (dir, app) = app_with_fixture();
    let path = app.state.repositories()[0].source_path.clone();
    github::seed_cache_for_test(&path, issues, chrono::Utc::now());
    (dir, app, path)
}

fn start_work(app: &App) -> &crate::modal::StartWorkModal {
    match app.modals.last() {
        Some(Modal::StartWork(flow)) => flow,
        other => panic!("Start work expected, found {other:?}"),
    }
}

fn confirm_screen(app: &App) -> &crate::modal::StartWorkConfirmModal {
    match app.modals.last() {
        Some(Modal::StartWorkConfirm(confirm)) => confirm,
        other => panic!("the confirm screen expected, found {other:?}"),
    }
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
}

/// What the branch load for the confirm screen delivers, as git would.
fn branches_loaded(app: &App) -> AppEvent {
    AppEvent::Branches {
        repo_id: app.state.repositories()[0].id,
        list: crate::event::BranchList {
            branches: vec!["develop".into(), "main".into(), "origin/main".into()],
            remotes: vec!["origin".into()],
            remote_default: Some("origin/main".into()),
            current_branch: "develop".into(),
        },
        target: crate::event::BranchTarget::StartWorkConfirm,
    }
}

/// Mode, then item, then confirm: the whole keyboard path to the screen
/// Launch is pressed on.
fn walk_to_confirm(app: &mut App, query: &str) {
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    type_text(app, query);
    app.handle_key(key(KeyCode::Enter));
    let loaded = branches_loaded(app);
    app.handle_event(loaded);
}

#[tokio::test]
async fn s_opens_start_work_on_the_mode_step() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);

    app.handle_key(key(KeyCode::Char('s')));

    let flow = start_work(&app);
    assert_eq!(flow.step, crate::modal::StartWorkStep::Mode);
    assert_eq!(flow.mode, crate::work::WorkMode::Issue);
    assert!(
        keys::BINDINGS.contains(&('s', "Start")),
        "the footer must offer it"
    );
}

/// Settings moved to `S` to make room; `s` must not open it any more.
#[tokio::test]
async fn settings_moved_to_capital_s() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('S')));
    assert!(matches!(app.modals.last(), Some(Modal::Settings(_))));
}

/// Start work runs for the repository a worktree belongs to as well, so
/// it does not matter which of its rows is selected.
#[tokio::test]
async fn start_work_opens_from_a_worktree_row_for_its_repository() {
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    app.handle_key(key(KeyCode::Down));
    assert!(app.state.selection.is_worktree());

    app.handle_key(key(KeyCode::Char('s')));

    assert_eq!(start_work(&app).repo_path, path);
}

#[tokio::test]
async fn the_start_work_control_leads_the_repository_pane() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    assert_eq!(
        app.detail_items().first(),
        Some(&DetailItem::Action(Action::StartWork))
    );

    app.run_action(Action::StartWork);

    assert!(matches!(app.modals.last(), Some(Modal::StartWork(_))));
}

/// `pr-review` does not exist yet: it is listed, but the highlight stops
/// on `feature` and a click on it picks nothing, so Enter can never
/// choose it.
#[tokio::test]
async fn pr_review_cannot_be_chosen_yet() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('s')));

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(start_work(&app).mode, crate::work::WorkMode::Feature);

    if let Some(modal) = app.modals.last_mut() {
        modal.set_row(2);
    }
    assert_eq!(start_work(&app).mode, crate::work::WorkMode::Feature);
}

fn labelled_issue(number: i64, title: &str, labels: &[&str]) -> crate::models::GitHubIssue {
    let mut issue = open_issue(number, title);
    issue.labels = labels
        .iter()
        .map(|name| crate::models::GitHubLabel {
            name: (*name).into(),
        })
        .collect();
    issue
}

fn listed(app: &App) -> Vec<i64> {
    let picker = start_work(app).picker.as_ref().expect("issues loaded");
    picker.entries().map(|e| e.issue.number).collect()
}

/// The two modes split the issues by the feature label: a `bug` that is
/// also an `enhancement` is a feature, and appears only there.
#[tokio::test]
async fn issue_and_feature_list_disjoint_issues() {
    let (_dir, mut app, _) = app_with_issues(vec![
        labelled_issue(1, "Crash on save", &["bug"]),
        labelled_issue(2, "Dark mode", &["enhancement"]),
        labelled_issue(3, "Faster startup", &["bug", "enhancement"]),
        open_issue(4, "Typo in README"),
    ]);
    app.handle_key(key(KeyCode::Char('s')));
    assert_eq!(listed(&app), vec![1, 4]);

    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    let flow = start_work(&app);
    assert_eq!(flow.mode, crate::work::WorkMode::Feature);
    assert_eq!(flow.step, crate::modal::StartWorkStep::Item);
    assert_eq!(listed(&app), vec![2, 3]);

    // Back to the mode step and over to `issue` swaps the list back.
    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(listed(&app), vec![1, 4]);
}

/// A feature is named `feat-<n>-<slug>` on `feat/...` and started with the
/// plan-first template.
#[tokio::test]
async fn a_feature_confirms_with_feat_names_and_the_feature_prompt() {
    let (_dir, mut app, _) = app_with_issues(vec![
        labelled_issue(42, "Fix login bug", &["bug"]),
        labelled_issue(7, "Dark mode", &["enhancement"]),
    ]);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        app.pending_issue
            .as_ref()
            .map(|(i, mode)| (i.number, *mode)),
        Some((7, crate::work::WorkMode::Feature))
    );
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    let confirm = confirm_screen(&app);
    assert_eq!(confirm.mode, crate::work::WorkMode::Feature);
    assert_eq!(confirm.name.value(), "feat-7-dark-mode");
    assert_eq!(confirm.branch.value(), "feat/feat-7-dark-mode");
    let prompt = confirm.prompt();
    assert!(prompt.contains("feature from GitHub issue #7"), "{prompt}");
    assert!(prompt.contains("approved the plan"), "{prompt}");
}

/// Every issue card carries Start work, and Create WT is gone.
#[tokio::test]
async fn the_issue_card_offers_start_work() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.sessions = Some(Vec::new());
    app.issues = Some(vec![
        labelled_issue(42, "Fix login bug", &["bug"]),
        labelled_issue(7, "Dark mode", &["enhancement"]),
    ]);

    let items = app.detail_items();
    assert!(items.contains(&DetailItem::Action(Action::StartWorkOnIssue(0))));
    assert!(items.contains(&DetailItem::Action(Action::StartWorkOnIssue(1))));
    let screen = rendered(&mut app);
    assert!(!screen.contains("Create WT"), "{screen}");
    // The pane's own Start work, then the first card's — the second card
    // is below the fold of the test terminal.
    assert_eq!(screen.matches("Start work").count(), 2, "{screen}");
}

/// The card skips the mode and item steps: the confirm screen opens on
/// its own, in the mode the issue's labels put it in.
#[tokio::test]
async fn the_issue_card_opens_the_confirm_screen_in_its_mode() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.issues = Some(vec![
        labelled_issue(42, "Fix login bug", &["bug"]),
        labelled_issue(7, "Dark mode", &["Enhancement"]),
    ]);

    app.run_action(Action::StartWorkOnIssue(1));
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    let confirm = confirm_screen(&app);
    assert_eq!(confirm.mode, crate::work::WorkMode::Feature);
    assert_eq!(confirm.issue_number, 7);
    assert_eq!(confirm.name.value(), "feat-7-dark-mode");
    assert!(!confirm.has_parent, "nothing beneath to go back to");
    assert_eq!(app.modals.len(), 1);
    app.handle_key(key(KeyCode::Esc));
    assert!(app.modals.is_empty());

    app.run_action(Action::StartWorkOnIssue(0));
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    let confirm = confirm_screen(&app);
    assert_eq!(confirm.mode, crate::work::WorkMode::Issue);
    assert_eq!(confirm.name.value(), "issue-42-fix-login-bug");
}

#[tokio::test]
async fn mode_then_item_then_confirm() {
    let (_dir, mut app, _) = app_with_issues(vec![
        open_issue(42, "Fix login bug"),
        open_issue(7, "Crash on save"),
    ]);

    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    let flow = start_work(&app);
    assert_eq!(flow.step, crate::modal::StartWorkStep::Item);
    let picker = flow.picker.as_ref().expect("issues were cached");
    assert_eq!(picker.entry_count(), 2);

    type_text(&mut app, "crash");
    let picked = start_work(&app)
        .picker
        .as_ref()
        .and_then(|p| p.selected())
        .map(|e| e.issue.number);
    assert_eq!(picked, Some(7));

    app.handle_key(key(KeyCode::Enter));
    // The confirm screen waits for the branches; the item step stays up.
    assert_eq!(app.pending_issue.as_ref().map(|(i, _)| i.number), Some(7));
    assert!(matches!(app.modals.last(), Some(Modal::StartWork(_))));

    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    let confirm = confirm_screen(&app);
    assert_eq!(confirm.issue_number, 7);
    assert_eq!(confirm.name.value(), "issue-7-crash-on-save");
    assert_eq!(confirm.branch.value(), "fix/issue-7-crash-on-save");
    assert_eq!(
        confirm.base_branch.value(),
        "origin/main",
        "the remote's default branch, not the checkout's develop"
    );
    assert!(confirm.pull_first, "pull first is on by default");
    assert!(confirm.has_parent);
    let prompt = confirm.prompt();
    assert!(prompt.contains("#7: Crash on save"), "{prompt}");
    assert!(prompt.contains("`issue-7-crash-on-save`"), "{prompt}");
    assert_eq!(app.modals.len(), 2, "the item step stays beneath");
}

/// Esc walks back one step at a time, and only closes from the first.
#[tokio::test]
async fn escape_steps_back_through_the_flow() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    walk_to_confirm(&mut app, "login");
    assert!(matches!(
        app.modals.last(),
        Some(Modal::StartWorkConfirm(_))
    ));

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Item);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Mode);
    app.handle_key(key(KeyCode::Esc));
    assert!(app.modals.is_empty());
}

/// Launch hands over the prompt the screen showed and closes the whole
/// flow, item step included.
#[tokio::test]
async fn launching_closes_the_whole_flow() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    walk_to_confirm(&mut app, "login");

    // A repository nobody tracks: the launch is folded exactly as a real
    // one, but stops before it reaches git or the forest directory.
    if let Some(Modal::StartWorkConfirm(confirm)) = app.modals.last_mut() {
        confirm.repo_id = uuid::Uuid::new_v4();
        confirm.focus = crate::modal::StartWorkConfirmModal::FOCUS_LAUNCH;
    }
    app.handle_key(key(KeyCode::Enter));

    assert!(app.modals.is_empty(), "{:?}", app.modals);
}

/// The submitted result carries the issue, the names and the prompt as
/// they stood on screen — edits to the name reach the prompt too.
#[tokio::test]
async fn launch_submits_what_the_confirm_screen_shows() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    walk_to_confirm(&mut app, "login");
    type_text(&mut app, "-2");

    let modal = app.modals.last_mut().expect("the confirm screen");
    let Modal::StartWorkConfirm(confirm) = modal else {
        panic!("the confirm screen expected, found {modal:?}");
    };
    confirm.focus = crate::modal::StartWorkConfirmModal::FOCUS_LAUNCH;
    let shown = confirm.prompt();
    // Asked of the modal directly: through the app, the launch would go
    // on to create a real worktree.
    let outcome = modal.handle_key(key(KeyCode::Enter));

    let crate::modal::ModalOutcome::Submit(crate::modal::ModalResult::WorktreeFromIssue {
        issue_number,
        name,
        branch,
        pull_first,
        base_branch,
        first_prompt,
        ..
    }) = outcome
    else {
        panic!("a launch expected, got {outcome:?}");
    };
    assert_eq!(issue_number, 42);
    assert_eq!(name, "issue-42-fix-login-bug-2");
    assert_eq!(branch, "fix/issue-42-fix-login-bug");
    assert!(pull_first);
    assert_eq!(base_branch.as_deref(), Some("origin/main"));
    assert_eq!(first_prompt, shown);
    assert!(first_prompt.contains("`issue-42-fix-login-bug-2`"));
}

fn confirm_screen_mut(app: &mut App) -> &mut crate::modal::StartWorkConfirmModal {
    match app.modals.last_mut() {
        Some(Modal::StartWorkConfirm(confirm)) => confirm,
        other => panic!("the confirm screen expected, found {other:?}"),
    }
}

/// Launch the confirm screen as it stands, asked of the modal directly:
/// through the app, the launch would go on to create a real worktree.
fn submitted(app: &mut App) -> crate::modal::ModalResult {
    confirm_screen_mut(app).focus = crate::modal::StartWorkConfirmModal::FOCUS_LAUNCH;
    let modal = app.modals.last_mut().expect("the confirm screen");
    match modal.handle_key(key(KeyCode::Enter)) {
        crate::modal::ModalOutcome::Submit(result) => result,
        outcome => panic!("a launch expected, got {outcome:?}"),
    }
}

/// Edit prompt hands the text to an editor; what comes back is what
/// Claude starts with, and no template is touched on the way.
#[tokio::test]
async fn an_edited_prompt_is_what_launches() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    walk_to_confirm(&mut app, "login");
    let original = confirm_screen_mut(&mut app).prompt();
    let repo_id = app.state.repositories()[0].id;

    confirm_screen_mut(&mut app).focus = crate::modal::StartWorkConfirmModal::FOCUS_EDIT;
    app.handle_key(key(KeyCode::Enter));
    assert!(confirm_screen_mut(&mut app).is_editing);

    // What the editor task delivers once the editor closes.
    app.handle_event(AppEvent::PromptEdited {
        repo_id,
        issue_number: 42,
        result: Ok("Only fix the login bug.\n\nNothing else.\n".into()),
    });
    let confirm = confirm_screen_mut(&mut app);
    assert!(!confirm.is_editing);
    assert_eq!(confirm.prompt(), "Only fix the login bug.\n\nNothing else.");

    let crate::modal::ModalResult::WorktreeFromIssue { first_prompt, .. } = submitted(&mut app)
    else {
        panic!("a launch from the issue expected");
    };
    assert_eq!(first_prompt, "Only fix the login bug.\n\nNothing else.");

    // The built-in template is what a fresh confirm screen fills again.
    assert_eq!(
        crate::work::WorkMode::Issue.template(),
        crate::prompt::ISSUE_TEMPLATE
    );
    app.modals.clear();
    walk_to_confirm(&mut app, "login");
    assert_eq!(confirm_screen_mut(&mut app).prompt(), original);
}

/// A failed or emptied edit keeps the prompt the screen showed, and a
/// result for a confirm screen that has gone is dropped.
#[tokio::test]
async fn an_edit_that_fails_or_strays_changes_nothing() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    walk_to_confirm(&mut app, "login");
    let original = confirm_screen_mut(&mut app).prompt();
    let repo_id = app.state.repositories()[0].id;

    for result in [Err("no editor".to_string()), Ok("  \n".to_string())] {
        confirm_screen_mut(&mut app).is_editing = true;
        app.handle_event(AppEvent::PromptEdited {
            repo_id,
            issue_number: 42,
            result,
        });
        assert!(!confirm_screen_mut(&mut app).is_editing);
        assert_eq!(confirm_screen_mut(&mut app).prompt(), original);
    }

    // Saved as it was: still the template, so it keeps following the
    // name fields rather than freezing at the name it was opened with.
    confirm_screen_mut(&mut app).is_editing = true;
    app.handle_event(AppEvent::PromptEdited {
        repo_id,
        issue_number: 42,
        result: Ok(format!("{original}\n")),
    });
    assert!(confirm_screen_mut(&mut app).edited_prompt.is_none());

    app.handle_event(AppEvent::PromptEdited {
        repo_id,
        issue_number: 7,
        result: Ok("Another issue's prompt".into()),
    });
    assert_eq!(confirm_screen_mut(&mut app).prompt(), original);
}

/// The yolo toggle starts off and reaches the launch when switched on.
#[tokio::test]
async fn the_yolo_toggle_reaches_the_launch() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    walk_to_confirm(&mut app, "login");
    let crate::modal::ModalResult::WorktreeFromIssue { yolo, .. } = submitted(&mut app) else {
        panic!("a launch from the issue expected");
    };
    assert!(!yolo, "yolo is off by default");

    confirm_screen_mut(&mut app).focus = crate::modal::StartWorkConfirmModal::FOCUS_YOLO;
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(confirm_screen_mut(&mut app).yolo);
    let crate::modal::ModalResult::WorktreeFromIssue { yolo, .. } = submitted(&mut app) else {
        panic!("a launch from the issue expected");
    };
    assert!(yolo);
}

/// With a cold cache the mode step opens at once and the issue list fills
/// in when the fetch folds.
#[tokio::test]
async fn issues_that_arrive_later_fill_the_picker() {
    let (_dir, mut app) = app_with_fixture();
    let path = app.state.repositories()[0].source_path.clone();
    github::invalidate_cache(&path);

    app.handle_key(key(KeyCode::Char('s')));
    assert!(start_work(&app).picker.is_none());

    app.handle_event(AppEvent::Issues {
        path,
        issues: vec![open_issue(42, "Fix login bug")],
    });

    let picker = start_work(&app).picker.as_ref().expect("filled");
    assert_eq!(picker.entry_count(), 1);
}

/// Picking an issue and then leaving must not open its confirm screen
/// over nothing once the slow branch load lands.
#[tokio::test]
async fn a_flow_closed_before_the_branches_load_opens_nothing() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Enter));
    assert!(app.pending_issue.is_some());

    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Esc));
    assert!(app.modals.is_empty());
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    assert!(app.modals.is_empty(), "{:?}", app.modals);
}

/// Clicking a row picks it, the way Enter does: a mouse-only user can go
/// from mode to confirm without the keyboard.
#[tokio::test]
async fn clicking_rows_walks_the_flow() {
    let (_dir, mut app, _) = app_with_issues(vec![
        open_issue(42, "Fix login bug"),
        open_issue(7, "Crash on save"),
    ]);
    app.handle_key(key(KeyCode::Char('s')));

    let pick = |row| HitTarget::ModalControl {
        index: crate::modal::StartWorkModal::FOCUS_LIST,
        click: ModalClick::Pick(row),
    };
    app.push_hit(rect(10, 10, 30, 1), pick(0));
    app.handle_mouse(click(12, 10));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Item);

    app.push_hit(rect(10, 11, 30, 1), pick(1));
    app.handle_mouse(click(12, 11));
    assert_eq!(app.pending_issue.as_ref().map(|(i, _)| i.number), Some(7));
}

/// The flow draws on a real frame: every step renders its title and the
/// picked issue's prompt shows on the confirm screen.
#[tokio::test]
async fn every_step_renders() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    let backend = ratatui::backend::TestBackend::new(140, 50);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    let screen =
        |app: &mut App, terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>| {
            terminal
                .draw(|frame| crate::ui::draw(frame, app))
                .expect("draw");
            let buffer: &ratatui::buffer::Buffer = terminal.backend().buffer();
            buffer
                .content
                .chunks(buffer.area.width as usize)
                .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };

    app.handle_key(key(KeyCode::Char('s')));
    let mode = screen(&mut app, &mut terminal);
    assert!(mode.contains("Start Work"), "{mode}");
    assert!(mode.contains("pr-review"), "{mode}");

    app.handle_key(key(KeyCode::Enter));
    let item = screen(&mut app, &mut terminal);
    assert!(item.contains("Start Work: issue"), "{item}");
    assert!(item.contains("#42 Fix login bug"), "{item}");

    app.handle_key(key(KeyCode::Enter));
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);
    let confirm = screen(&mut app, &mut terminal);
    assert!(confirm.contains("Start Work: Issue #42"), "{confirm}");
    assert!(confirm.contains("gh issue view 42 --comments"), "{confirm}");
    assert!(confirm.contains("Launch"), "{confirm}");
}

// ------------------------------------------------- one work item, one worktree

/// Record on the fixture's worktree `name` that it was started for issue
/// `number`, as a launch would. Returns its id and path.
fn link_worktree(app: &mut App, name: &str, number: i64) -> (Uuid, String) {
    let (id, path) = app.state.repositories()[0]
        .worktrees
        .iter()
        .find(|w| w.name == name)
        .map(|w| (w.id, w.path.clone()))
        .expect("fixture worktree");
    app.state.update_worktree(id, |w| {
        w.work_item = Some(crate::models::WorkItemLink::Issue { number });
    });
    (id, path)
}

fn rendered(app: &mut App) -> String {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 50)).expect("terminal");
    terminal
        .draw(|frame| crate::ui::draw(frame, app))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    buffer
        .content
        .chunks(buffer.area.width as usize)
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn notified(app: &App) -> Vec<&str> {
    app.notifications.iter().map(|n| n.text.as_str()).collect()
}

/// Pick issue #7 ("Crash on save") from the `issue` list.
fn pick_crash(app: &mut App) {
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    type_text(app, "crash");
    app.handle_key(key(KeyCode::Enter));
}

fn two_issues() -> Vec<crate::models::GitHubIssue> {
    vec![
        open_issue(42, "Fix login bug"),
        open_issue(7, "Crash on save"),
    ]
}

#[tokio::test]
async fn an_issue_with_a_worktree_is_marked_in_the_list() {
    let (_dir, mut app, _) = app_with_issues(two_issues());
    link_worktree(&mut app, "beta", 7);

    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));

    let picker = start_work(&app).picker.as_ref().expect("issues loaded");
    let marks: Vec<_> = picker
        .entries()
        .map(|e| (e.issue.number, e.worktree.as_deref()))
        .collect();
    assert_eq!(marks, vec![(42, None), (7, Some("beta"))]);
    let screen = rendered(&mut app);
    let row = screen
        .lines()
        .find(|l| l.contains("#7 Crash on save"))
        .expect("the issue is listed");
    assert!(row.contains("→ beta"), "{row}");
    let other = screen
        .lines()
        .find(|l| l.contains("#42 Fix login bug"))
        .expect("the issue is listed");
    assert!(!other.contains('→'), "{other}");
}

/// Picking a started item selects its worktree, closes the flow and
/// creates nothing. With no Claude running there, that is all it does.
#[tokio::test]
async fn picking_a_started_issue_jumps_to_its_worktree() {
    let (_dir, mut app, _) = app_with_issues(two_issues());
    let (beta, path) = link_worktree(&mut app, "beta", 7);
    let worktrees = app.state.repositories()[0].worktrees.len();

    pick_crash(&mut app);

    assert!(app.modals.is_empty(), "no confirm screen: {:?}", app.modals);
    assert!(
        app.pending_issue.is_none(),
        "no branch load for a confirm screen"
    );
    assert_eq!(app.state.selection.worktree_id, Some(beta));
    assert_eq!(app.meta.path, path);
    assert_eq!(app.state.repositories()[0].worktrees.len(), worktrees);
    assert!(
        notified(&app).contains(&"Issue #7 is already in worktree 'beta'"),
        "{:?}",
        notified(&app)
    );

    // Both scans answer: a session exists there, but none is running.
    app.handle_event(AppEvent::Sessions {
        path,
        sessions: Some(vec![a_session("stopped")]),
    });
    app.handle_event(AppEvent::LiveSessions {
        sessions: vec![a_live_window("somewhere-else")],
    });

    assert!(app.pending_jump.is_none(), "the jump is settled");
    assert!(app.modals.is_empty(), "nothing is launched or offered");
    assert_eq!(notified(&app).len(), 1, "{:?}", notified(&app));
}

/// A Claude running in one of our windows is switched to, once both the
/// worktree's sessions and the live map are in, and not before.
#[tokio::test]
async fn the_jump_switches_to_a_live_session_window() {
    let (_dir, mut app, _) = app_with_issues(two_issues());
    let (_, path) = link_worktree(&mut app, "beta", 7);

    pick_crash(&mut app);
    app.handle_event(AppEvent::Sessions {
        path,
        sessions: Some(vec![a_session("stopped"), a_session("running")]),
    });
    assert!(app.pending_jump.is_some(), "still waiting for the live map");

    app.handle_event(AppEvent::LiveSessions {
        sessions: vec![a_live_window("running")],
    });

    // Unit tests never reach tmux, so the switch reports the window as
    // gone — which still shows it was aimed at the session's window.
    assert!(
        notified(&app).iter().any(|n| n.contains("claude:demo:wt")),
        "{:?}",
        notified(&app)
    );
    assert!(app.pending_jump.is_none());
}

/// One running outside this tmux session cannot be switched to; the jump
/// says where it is instead.
#[tokio::test]
async fn a_session_running_elsewhere_is_pointed_out() {
    let (_dir, mut app, _) = app_with_issues(two_issues());
    let (_, path) = link_worktree(&mut app, "beta", 7);

    pick_crash(&mut app);
    app.handle_event(AppEvent::LiveSessions {
        sessions: vec![a_live_elsewhere("running", 4242)],
    });
    app.handle_event(AppEvent::Sessions {
        path,
        sessions: Some(vec![a_session("running")]),
    });

    assert!(
        notified(&app).iter().any(|n| n.contains("pid 4242")),
        "{:?}",
        notified(&app)
    );
}

/// Moving on before the scans land drops the jump: switching windows
/// out from under a selection the user has left would be a surprise.
#[tokio::test]
async fn a_jump_the_user_moved_away_from_does_not_switch() {
    let (_dir, mut app, _) = app_with_issues(two_issues());
    let (_, path) = link_worktree(&mut app, "beta", 7);

    pick_crash(&mut app);
    app.handle_key(key(KeyCode::Up));
    app.handle_event(AppEvent::Sessions {
        path,
        sessions: Some(vec![a_session("running")]),
    });
    app.handle_event(AppEvent::LiveSessions {
        sessions: vec![a_live_window("running")],
    });

    assert!(
        !notified(&app).iter().any(|n| n.contains("claude:demo:wt")),
        "{:?}",
        notified(&app)
    );
}

/// The card keeps one work item to one worktree too: a started issue
/// jumps to its worktree instead of opening the confirm screen.
#[tokio::test]
async fn the_issue_card_jumps_to_a_started_issue() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.issues = Some(two_issues());
    let (beta, path) = link_worktree(&mut app, "beta", 7);
    let worktrees = app.state.repositories()[0].worktrees.len();

    app.run_action(Action::StartWorkOnIssue(1));

    assert!(app.modals.is_empty(), "no confirm screen: {:?}", app.modals);
    assert!(app.pending_issue.is_none(), "no branch load");
    assert_eq!(app.state.selection.worktree_id, Some(beta));
    assert_eq!(app.meta.path, path);
    assert_eq!(app.state.repositories()[0].worktrees.len(), worktrees);
    assert!(
        notified(&app).contains(&"Issue #7 is already in worktree 'beta'"),
        "{:?}",
        notified(&app)
    );
}

/// The link is not derived from names: renaming the worktree and its
/// branch keeps the item pointing at it.
#[tokio::test]
async fn the_jump_survives_a_rename() {
    let (_dir, mut app, _) = app_with_issues(two_issues());
    let (beta, _) = link_worktree(&mut app, "beta", 7);
    app.state.update_worktree(beta, |w| {
        w.name = "crash-hunt".into();
        w.branch = "fix/crash-hunt".into();
    });

    pick_crash(&mut app);

    assert!(app.modals.is_empty());
    assert_eq!(app.state.selection.worktree_id, Some(beta));
    assert!(
        notified(&app).contains(&"Issue #7 is already in worktree 'crash-hunt'"),
        "{:?}",
        notified(&app)
    );
}

/// An archived worktree still holds the work; the jump shows the archived
/// section so the cursor lands on a row rather than on nothing.
#[tokio::test]
async fn the_jump_reveals_an_archived_worktree() {
    let (_dir, mut app, _) = app_with_issues(two_issues());
    let (old, _) = link_worktree(&mut app, "old", 7);
    assert!(!app.state.show_archived);

    pick_crash(&mut app);

    assert!(app.state.show_archived);
    assert_eq!(app.state.selection.worktree_id, Some(old));
    assert_eq!(
        app.rows.get(app.sidebar_index).and_then(|row| row.ids()),
        Some((app.state.repositories()[0].id, Some(old)))
    );
}
