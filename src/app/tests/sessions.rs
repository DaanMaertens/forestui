use super::*;

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

/// Resuming a session that is *running* must guard: a session in one of
/// our windows is switched to straight away — no dialog, no toast, since
/// going to the conversation is what Resume means — and one whose heartbeat
/// says it runs somewhere unreachable gets an eyes-open "resume anyway". A
/// session with no live entry resumes freely.
///
/// Exercised through `guard_open_session` directly, never through
/// `run_action(Resume…)`: the resume path really launches windows, and a
/// test binary inherits the developer's own `TMUX_PANE` — an earlier
/// version of this test opened a real window in a real tmux session.
/// The `cfg!(test)` gate in `services::tmux` now makes that impossible,
/// and this test stays off the launch path anyway.
#[tokio::test]
async fn resuming_an_open_session_switches_to_its_window() {
    let (_dir, mut app) = app_with_fixture();
    let path = app.state.selected_path().expect("a selection");
    app.sessions = Some(vec![a_session("one")]);
    app.handle_event(AppEvent::LiveSessions {
        sessions: vec![a_live_window("one")],
    });
    app.notifications.clear();

    assert!(app.guard_open_session(0, &path, false, None));
    assert!(
        app.modals.is_empty(),
        "a switch asks nothing: {:?}",
        app.modals.last()
    );
    // Under test tmux answers as if absent, so the select fails and the
    // only notice is the window-gone warning; a success would say nothing.
    assert!(
        app.notifications
            .iter()
            .all(|n| !n.text.starts_with("Switched")),
        "{:?}",
        app.notifications
            .iter()
            .map(|n| &n.text)
            .collect::<Vec<_>>()
    );

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

/// A session in a window of another tmux session on our server is offered
/// to be brought here: the window moves over, keeping its Claude, so there
/// is no second Claude to warn about. The dialog says which session loses
/// the window — and when it is that session's last, that it will close.
#[tokio::test]
async fn resuming_a_session_in_another_tmux_session_offers_to_bring_it_here() {
    let (_dir, mut app) = app_with_fixture();
    let path = app.state.selected_path().expect("a selection");
    app.sessions = Some(vec![a_session("one")]);
    app.handle_event(AppEvent::LiveSessions {
        sessions: vec![a_live_in_other_session("one", false)],
    });

    assert!(app.guard_open_session(0, &path, false, None));
    let Some(crate::modal::Modal::Confirm(confirm)) = app.modals.last() else {
        panic!("the guard did not open a dialog: {:?}", app.modals.last());
    };
    assert!(
        confirm.message.contains("'hand-made'"),
        "{}",
        confirm.message
    );
    assert!(confirm.message.contains("'work'"), "{}", confirm.message);
    assert!(!confirm.message.contains("close"), "{}", confirm.message);
    match &confirm.action {
        crate::modal::ConfirmAction::BringWindow {
            window_id,
            window_name,
            session_id,
            closes_session,
        } => {
            assert_eq!(window_id, "@12");
            assert_eq!(window_name, "hand-made");
            assert_eq!(session_id, "one");
            assert!(!closes_session);
        }
        other => panic!("expected BringWindow, got {other:?}"),
    }
    assert_eq!(confirm.action.confirm_label(), "Bring here");
    assert!(
        !confirm.action.is_destructive(),
        "moving a window loses nothing"
    );

    // Its session's only window: moving it closes that tmux session, which
    // the dialog says, and which earns the red.
    app.modals.clear();
    app.handle_event(AppEvent::LiveSessions {
        sessions: vec![a_live_in_other_session("one", true)],
    });
    assert!(app.guard_open_session(0, &path, false, None));
    let Some(crate::modal::Modal::Confirm(confirm)) = app.modals.last() else {
        panic!("no dialog");
    };
    assert!(
        confirm.message.contains("tmux session 'work' will close"),
        "{}",
        confirm.message
    );
    assert!(confirm.action.is_destructive());
}

/// Renaming renames the session's window, and a window in another tmux
/// session is the user's arrangement there: renaming it is refused, like
/// any session forestui cannot reach, until it is brought here.
#[tokio::test]
async fn renaming_a_session_in_another_tmux_session_is_refused() {
    let (_dir, mut app) = app_with_fixture();
    app.sessions = Some(vec![a_session("one")]);
    app.live_sessions
        .insert("one".into(), a_live_in_other_session("one", false));
    app.run_action(Action::RenameSession(0));
    assert!(app.modals.is_empty(), "no rename dialog");
    assert!(
        app.notifications
            .iter()
            .any(|n| n.text.contains("tmux session 'work'")),
        "{:?}",
        app.notifications
            .iter()
            .map(|n| &n.text)
            .collect::<Vec<_>>()
    );
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

/// The session card's launch row, as the content walk builds it: each
/// control's label, action and whether it can fire.
fn launch_row(app: &App) -> Vec<(String, DetailItem, bool)> {
    detail::content(app)
        .iter()
        .filter_map(|node| match node {
            detail::DetailNode::CardBody { rows, .. } => Some(rows.concat()),
            _ => None,
        })
        .flatten()
        .filter(|c| {
            matches!(
                c.item,
                DetailItem::Action(
                    Action::ResumeSession(0)
                        | Action::ResumeYolo(0)
                        | Action::ResumeCustom { session: 0, .. }
                )
            )
        })
        .map(|c| (c.label, c.item, c.enabled))
        .collect()
}

fn app_with_a_custom_button() -> (tempfile::TempDir, App) {
    let (dir, mut app) = app_with_fixture();
    app.settings.custom_buttons = vec![CustomClaudeButton {
        label: "Opus".into(),
        prefix: "opus".into(),
        command: "claude --model opus".into(),
    }];
    app.sessions = Some(vec![a_session("one")]);
    (dir, app)
}

/// The card's plain button resumes *this* conversation, so it says so;
/// the CLAUDE: NEW SESSION row above keeps "Claude", which starts one.
#[tokio::test]
async fn the_cards_plain_button_reads_resume() {
    let (_dir, app) = app_with_a_custom_button();
    let labels: Vec<String> = launch_row(&app).into_iter().map(|(l, ..)| l).collect();
    assert_eq!(labels, ["Resume", "YOLO", "Opus"]);

    let new_session = detail::content(&app)
        .iter()
        .filter_map(|node| match node {
            detail::DetailNode::Controls { controls, .. } => Some(controls.clone()),
            _ => None,
        })
        .flatten()
        .find(|c| c.item == DetailItem::Action(Action::ClaudeNew))
        .expect("the new-session row");
    assert_eq!(new_session.label, "Claude");
}

/// A card that is not live offers every launch.
#[tokio::test]
async fn a_free_card_enables_every_launch() {
    let (_dir, app) = app_with_a_custom_button();
    let row = launch_row(&app);
    assert_eq!(row.len(), 3);
    assert!(row.iter().all(|(.., enabled)| *enabled));
}

/// A live session is one conversation already running: Resume goes to it,
/// but YOLO or a custom button would only start a second Claude on it with
/// other flags. They keep their slots, disabled, wherever it runs.
#[tokio::test]
async fn a_live_card_disables_all_but_resume() {
    for live in [
        a_live_window("one"),
        a_live_in_other_session("one", false),
        a_live_elsewhere("one", 4242),
    ] {
        let (_dir, mut app) = app_with_a_custom_button();
        let place = format!("{:?}", live.place);
        app.handle_event(AppEvent::LiveSessions {
            sessions: vec![live],
        });
        let row = launch_row(&app);
        assert_eq!(row.len(), 3, "{place}: the row keeps its slots");
        let enabled: Vec<bool> = row.iter().map(|(.., e)| *e).collect();
        assert_eq!(enabled, [true, false, false], "{place}");
    }
}
