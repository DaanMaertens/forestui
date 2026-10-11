use super::*;

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
