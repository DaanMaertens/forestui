use super::*;

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

/// Pick issue #7 ("Crash on save") from the `issue` list.
fn pick_crash(app: &mut App) {
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    type_text(app, "crash");
    app.handle_key(key(KeyCode::Enter));
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
        .filter_map(issue_of)
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
        app.pending_start.is_none(),
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
