use super::*;
use std::collections::HashMap;

/// A finished check's result for the fixture repository, carrying the
/// ticket the background task would.
fn checked(app: &mut App, verdicts: &[(Uuid, bool)]) -> AppEvent {
    let repo_id = app.state.repositories()[0].id;
    AppEvent::WorktreesFinished {
        ticket: app.finished.ticket_for_test(repo_id),
        verdicts: verdicts.iter().copied().collect::<HashMap<_, _>>(),
    }
}

fn marked(app: &App, worktree_id: Uuid) -> bool {
    app.state
        .find_worktree(worktree_id)
        .is_some_and(|(_, w)| w.finished)
}

/// The detail pane's finished toggle, by label.
fn toggle_label(app: &App) -> Option<String> {
    detail::content(app)
        .into_iter()
        .find_map(|node| match node {
            detail::DetailNode::Controls { controls, .. } => controls.into_iter().find_map(|c| {
                (c.item == detail::DetailItem::Action(detail::Action::ToggleFinished))
                    .then_some(c.label)
            }),
            _ => None,
        })
}

fn row_finished(app: &App, worktree_id: Uuid) -> bool {
    app.rows.iter().any(
        |row| matches!(row, SidebarRow::Worktree { id, finished: true, .. } if *id == worktree_id),
    )
}

#[tokio::test]
async fn a_finished_verdict_marks_the_sidebar_row() {
    let (_dir, mut app) = app_with_fixture();
    let worktree_id = app.state.repositories()[0].worktrees[0].id;
    assert!(!row_finished(&app, worktree_id));

    app.redraw = false;
    let event = checked(&mut app, &[(worktree_id, true)]);
    app.handle_event(event);

    assert!(row_finished(&app, worktree_id));
    assert!(app.redraw, "a changed mark must repaint");
}

/// The sweep re-checks every 30 seconds and almost always finds what it
/// found last time; that must not cost a frame.
#[tokio::test]
async fn an_unchanged_verdict_does_not_repaint() {
    let (_dir, mut app) = app_with_fixture();
    let worktree_id = app.state.repositories()[0].worktrees[0].id;
    let event = checked(&mut app, &[(worktree_id, true)]);
    app.handle_event(event);

    app.redraw = false;
    let event = checked(&mut app, &[(worktree_id, true)]);
    app.handle_event(event);
    assert!(!app.redraw, "a check that changed nothing repainted");
}

/// A check that lands after its repository was removed changes nothing.
#[tokio::test]
async fn a_result_for_a_removed_repository_is_discarded() {
    let (_dir, mut app) = app_with_fixture();
    let repo_id = app.state.repositories()[0].id;
    let worktree_id = app.state.repositories()[0].worktrees[0].id;
    let event = checked(&mut app, &[(worktree_id, true)]);
    app.state.remove_repository(repo_id);
    app.rebuild_rows();

    app.handle_event(event);
    assert!(!marked(&app, worktree_id));
    assert!(
        app.finished
            .start(repo_id, Instant::now() + finished::MIN_INTERVAL)
            .is_some(),
        "a discarded result did not free the repository for the next check"
    );
}

/// A verdict about a worktree another repository owns is not this
/// repository's to give.
#[tokio::test]
async fn a_verdict_about_another_repositorys_worktree_is_discarded() {
    let (_dir, mut app) = app_with_fixture();
    let mut other = Repository::new("other".into(), "/tmp/other".into());
    let foreign = Worktree::new("x".into(), "x".into(), "/tmp/other-x".into());
    let foreign_id = foreign.id;
    other.worktrees.push(foreign);
    app.state.add_repository(other);
    app.rebuild_rows();

    let event = checked(&mut app, &[(foreign_id, true)]);
    app.handle_event(event);
    assert!(!marked(&app, foreign_id));
}

/// The check rides on the worktree scans: every trigger that rescans a
/// repository also re-checks which of its worktrees are finished.
#[tokio::test]
async fn a_worktree_scan_starts_a_finished_check() {
    let (_dir, mut app) = app_with_fixture();
    let repo_id = app.state.repositories()[0].id;
    app.scan_worktrees(repo_id);
    assert!(
        app.finished.start(repo_id, Instant::now()).is_none(),
        "no finished check is running"
    );
}

/// The sidebar draws a finished worktree dimmed, with a ✓ badge.
#[tokio::test]
async fn the_sidebar_draws_a_finished_worktree_dimmed_with_a_badge() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let _guard = crate::theme::test_lock();

    let (_dir, mut app) = app_with_fixture();
    let SidebarRow::Worktree { id, name, .. } = app.rows[1].clone() else {
        panic!("fixture row 1 is not a worktree");
    };
    let worktree_id = id;
    let event = checked(&mut app, &[(worktree_id, true)]);
    app.handle_event(event);

    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    // Header row + the 3-row gh-status box put the first tree row at y=4;
    // the fixture's first worktree is the row below it.
    let row: String = (0..buffer.area.width)
        .map(|col| buffer[(col, 5)].symbol().to_string())
        .collect();
    let badge = row.find('✓').expect("no ✓ badge on the finished row");
    let name_at = row.find(&name).expect("the name is drawn");
    assert!(badge < name_at, "the badge leads the name: {row:?}");

    let name_col = row[..name_at].chars().count() as u16;
    assert_eq!(
        buffer[(name_col, 5)].fg,
        crate::theme::finished()
            .fg
            .expect("finished style has a colour"),
        "the finished name is not dimmed"
    );
}

/// Any worktree can be marked by hand — the fixture's are plain, with no
/// work item for the check to ask about — and `f` marks and unmarks it.
#[tokio::test]
async fn f_toggles_the_selected_worktrees_mark() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");

    app.handle_key(key(KeyCode::Char('f')));
    assert!(marked(&app, worktree_id));
    assert!(
        row_finished(&app, worktree_id),
        "the sidebar row is not dimmed"
    );
    let repo_id = app.state.repositories()[0].id;
    assert_eq!(
        app.finished_worktrees(repo_id).len(),
        1,
        "Prune does not count it"
    );
    assert!(app.notifications.is_empty(), "a toggle is not announced");

    app.handle_key(key(KeyCode::Char('f')));
    assert!(!marked(&app, worktree_id));
    assert!(!row_finished(&app, worktree_id));
}

#[tokio::test]
async fn an_archived_worktree_can_be_marked() {
    let (_dir, mut app) = app_with_fixture();
    let (repo_id, archived_id) = {
        let repo = &app.state.repositories()[0];
        let archived = repo.worktrees.iter().find(|w| w.is_archived).unwrap();
        (repo.id, archived.id)
    };
    app.state.select_worktree(repo_id, archived_id);
    app.run_action(detail::Action::ToggleFinished);
    assert!(marked(&app, archived_id));
}

/// The whole worktree pane: MANAGE sits below the fold of [`rendered`].
fn drawn_tall(app: &mut App) -> String {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 120)).expect("terminal");
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

/// The button sits beside Archive and Delete and says what pressing it
/// will do.
#[tokio::test]
async fn the_button_label_follows_the_mark() {
    let _guard = crate::theme::test_lock();
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    assert_eq!(toggle_label(&app).as_deref(), Some("Mark finished"));
    assert!(drawn_tall(&mut app).contains("│ Mark finished │"));

    app.handle_key(key(KeyCode::Char('f')));
    assert_eq!(toggle_label(&app).as_deref(), Some("Unmark finished"));
    assert!(drawn_tall(&mut app).contains("│ Unmark finished │"));
}

/// The user's Unmark of a worktree the check found finished sticks while
/// the check keeps finding it finished.
#[tokio::test]
async fn an_unmark_survives_the_next_check() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
    let event = checked(&mut app, &[(worktree_id, true)]);
    app.handle_event(event);
    app.handle_key(key(KeyCode::Char('f')));
    assert!(!marked(&app, worktree_id));

    let event = checked(&mut app, &[(worktree_id, true)]);
    app.handle_event(event);
    assert!(!marked(&app, worktree_id));
}
