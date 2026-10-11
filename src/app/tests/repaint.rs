use super::*;

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
