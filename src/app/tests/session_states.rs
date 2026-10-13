//! What live sessions are doing: the once-a-second state read, its fold,
//! and the worktree rows that follow it.

use super::*;
use crate::services::live::{Activity, LivePlace, LiveSession, SessionState};

fn worktree_path(app: &App, name: &str) -> String {
    app.state
        .repositories()
        .iter()
        .flat_map(|repo| repo.worktrees.iter())
        .find(|worktree| worktree.name == name)
        .map(|worktree| worktree.path.clone())
        .expect("a fixture worktree")
}

fn in_window(sid: &str, cwd: &str, state: Option<SessionState>) -> LiveSession {
    LiveSession {
        session_id: sid.into(),
        place: LivePlace::Window {
            window_id: format!("@{sid}"),
            window_name: sid.into(),
        },
        state,
        cwd: Some(cwd.into()),
    }
}

fn activity_of(app: &App, name: &str) -> Option<Activity> {
    app.rows.iter().find_map(|row| match row {
        SidebarRow::Worktree {
            name: row_name,
            activity,
            ..
        } if row_name == name => Some(*activity),
        _ => None,
    })?
}

fn states(pairs: &[(&str, Option<SessionState>)]) -> AppEvent {
    AppEvent::SessionStates {
        states: pairs
            .iter()
            .map(|(sid, state)| (sid.to_string(), *state))
            .collect(),
    }
}

/// The full scan's fold puts each session's state on its worktree's row.
#[tokio::test]
async fn the_live_scan_rolls_states_onto_worktree_rows() {
    let (_dir, mut app) = app_with_fixture();
    app.rebuild_rows();
    let alpha = worktree_path(&app, "alpha");
    assert_eq!(activity_of(&app, "alpha"), None);

    app.handle_event(AppEvent::LiveSessions {
        sessions: vec![
            in_window("one", &alpha, Some(SessionState::Busy)),
            in_window("two", &alpha, Some(SessionState::NeedsYou)),
        ],
    });
    assert_eq!(
        activity_of(&app, "alpha"),
        Some(Activity {
            state: SessionState::NeedsYou,
            sessions: 2
        })
    );
    assert_eq!(activity_of(&app, "beta"), None);
}

/// The quick read moves a state within a second, repaints and re-rolls the
/// row only when a state actually changed, and never touches a session it
/// was not asked about.
#[tokio::test]
async fn the_state_read_repaints_only_on_a_change() {
    let (_dir, mut app) = app_with_fixture();
    let alpha = worktree_path(&app, "alpha");
    app.handle_event(AppEvent::LiveSessions {
        sessions: vec![
            in_window("one", &alpha, Some(SessionState::Busy)),
            in_window("two", &alpha, None),
        ],
    });

    app.redraw = false;
    app.handle_event(states(&[("one", Some(SessionState::Busy)), ("two", None)]));
    assert!(!app.redraw, "an unchanged state must not repaint");

    app.handle_event(states(&[("one", Some(SessionState::NeedsYou))]));
    assert!(app.redraw, "a changed state repaints");
    assert_eq!(app.live_sessions["one"].state, Some(SessionState::NeedsYou));
    assert_eq!(
        activity_of(&app, "alpha").map(|a| a.state),
        Some(SessionState::NeedsYou),
        "the row follows the state"
    );

    // A session the read was not asked about keeps what it had, and one the
    // live map no longer holds is not resurrected by a late read.
    app.redraw = false;
    app.handle_event(states(&[("gone", Some(SessionState::Busy))]));
    assert!(!app.redraw);
    assert!(!app.live_sessions.contains_key("gone"));
    assert_eq!(app.live_sessions["one"].state, Some(SessionState::NeedsYou));
}

/// The read runs about once a second, and only while there is a live
/// session to read for; one at a time.
#[tokio::test]
async fn the_state_read_is_paced_and_needs_a_live_session() {
    let (_dir, mut app) = app_with_fixture();
    app.last_state_read = Instant::now() - STATE_READ_INTERVAL;
    app.on_tick();
    assert!(!app.state_read_in_flight, "nothing live, nothing to read");

    let alpha = worktree_path(&app, "alpha");
    app.live_sessions
        .insert("one".into(), in_window("one", &alpha, None));
    app.on_tick();
    assert!(app.state_read_in_flight);

    // The answer clears the flag; the next tick inside the interval waits.
    app.handle_event(states(&[("one", None)]));
    assert!(!app.state_read_in_flight);
    app.on_tick();
    assert!(!app.state_read_in_flight, "a second has not passed");
}
