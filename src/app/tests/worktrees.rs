use super::*;

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
        branch: None,
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
        outcome: Ok(git::RemoveOutcome::Refused(git::Refusal {
            changes: git::Changes::Uncommitted("2 modified, 1 untracked".into()),
            lock: None,
        })),
        branch: None,
    });

    let Some(Modal::Confirm(confirm)) = app.modals.last() else {
        panic!("expected the second confirmation");
    };
    assert!(confirm.message.contains("2 modified, 1 untracked"));
    assert_eq!(confirm.action.confirm_label(), "Delete anyway");
    assert!(matches!(
        confirm.action,
        crate::modal::ConfirmAction::ForceDeleteWorktree(id, git::Force::Changes)
            if id == worktree_id
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

/// The confirm a background `Refused` result opens for the selected worktree.
fn refusal_confirm(refusal: git::Refusal) -> crate::modal::ConfirmModal {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
    let change = begun(&mut app, worktree_id, WorktreeSync::begin_removal);
    app.handle_event(AppEvent::WorktreeRemoveResult {
        change,
        outcome: Ok(git::RemoveOutcome::Refused(refusal)),
        branch: None,
    });
    // Nothing destroyed, nothing dropped from state.
    assert!(app.state.find_worktree(worktree_id).is_some());
    match app.modals.pop() {
        Some(Modal::Confirm(confirm)) => confirm,
        _ => panic!("a refusal must offer to delete anyway, not end in an error"),
    }
}

#[tokio::test]
async fn a_worktree_whose_changes_cannot_be_listed_offers_delete_anyway() {
    let confirm = refusal_confirm(git::Refusal {
        changes: git::Changes::Unknown("index file smaller than expected".into()),
        lock: None,
    });
    assert!(
        confirm.message.contains("Could not list"),
        "{}",
        confirm.message
    );
    assert!(confirm.message.contains("index file smaller than expected"));
    assert_eq!(confirm.action.confirm_label(), "Delete anyway");
    assert!(matches!(
        confirm.action,
        crate::modal::ConfirmAction::ForceDeleteWorktree(_, git::Force::Changes)
    ));
}

#[tokio::test]
async fn a_locked_worktree_offers_to_unlock_and_delete() {
    let confirm = refusal_confirm(git::Refusal {
        changes: git::Changes::Uncommitted("1 untracked".into()),
        lock: Some(git::Lock {
            reason: Some("claude agent a1 (pid 42)".into()),
        }),
    });
    assert!(confirm.message.contains("claude agent a1 (pid 42)"));
    assert!(confirm.message.contains("1 untracked"));
    assert_eq!(confirm.action.confirm_label(), "Unlock and delete");
    assert!(matches!(
        confirm.action,
        crate::modal::ConfirmAction::ForceDeleteWorktree(_, git::Force::ChangesAndLock)
    ));
}

#[test]
fn a_long_git_reason_wraps_inside_the_dialog_instead_of_clipping() {
    let why = format!("{}: index file smaller than expected", "/x".repeat(60));
    let (_, message) = refusal_dialog(
        "wt",
        &git::Refusal {
            changes: git::Changes::Unknown(why),
            lock: None,
        },
    );
    assert!(
        message.lines().all(|line| line.chars().count() <= 72),
        "{message}"
    );
    // Nothing of the reason is lost, and its words are not split.
    assert!(
        message
            .replace('\n', " ")
            .contains("index file smaller than expected"),
        "{message}"
    );
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
        branch: None,
    });

    assert!(app.state.find_worktree(worktree_id).is_some());
    assert_eq!(app.notifications.len(), 1);
    assert!(app.notifications[0].text.contains("worktree is locked"));
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
        branch: None,
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

/// Start work makes a fresh branch under the same name the next time, so
/// the confirm says the branch goes too, and on what condition.
#[tokio::test]
async fn the_delete_confirm_names_the_branch_it_deletes() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Char('d')));
    let Some(Modal::Confirm(confirm)) = app.modals.last() else {
        panic!("expected the delete confirm");
    };
    assert!(confirm.message.contains("feat/beta"), "{}", confirm.message);
    assert!(confirm.message.contains("unpushed"), "{}", confirm.message);
}

/// A detached worktree has no branch of its own, so none is promised.
#[tokio::test]
async fn the_delete_confirm_of_a_detached_worktree_names_no_branch() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
    app.state
        .update_worktree(worktree_id, |worktree| worktree.branch = "HEAD".into());
    app.handle_key(key(KeyCode::Char('d')));
    let Some(Modal::Confirm(confirm)) = app.modals.last() else {
        panic!("expected the delete confirm");
    };
    assert!(!confirm.message.contains("branch"), "{}", confirm.message);
}

#[tokio::test]
async fn a_delete_that_took_the_branch_says_so() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
    let change = begun(&mut app, worktree_id, WorktreeSync::begin_removal);
    app.handle_event(AppEvent::WorktreeRemoveResult {
        change,
        outcome: Ok(git::RemoveOutcome::Removed),
        branch: Some(crate::prune::BranchFate::Deleted("feat/beta".into())),
    });
    assert!(app.state.find_worktree(worktree_id).is_none());
    let note = app.notifications.last().expect("a notification");
    assert_eq!(note.text, "Deleted 'beta' and its branch 'feat/beta'.");
    assert_eq!(note.severity, Severity::Information);
}

/// The worktree is gone either way; a kept branch is a warning, not a
/// failed delete.
#[tokio::test]
async fn a_delete_that_kept_the_branch_says_why() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
    let change = begun(&mut app, worktree_id, WorktreeSync::begin_removal);
    app.handle_event(AppEvent::WorktreeRemoveResult {
        change,
        outcome: Ok(git::RemoveOutcome::Removed),
        branch: Some(crate::prune::BranchFate::Kept {
            branch: "feat/beta".into(),
            why: "3 unpushed commits".into(),
        }),
    });
    assert!(app.state.find_worktree(worktree_id).is_none());
    let note = app.notifications.last().expect("a notification");
    assert_eq!(
        note.text,
        "Deleted 'beta'; kept branch 'feat/beta' — 3 unpushed commits."
    );
    assert_eq!(note.severity, Severity::Warning);
}
