//! `[` / `]` step between worktrees from either pane (#133).

use super::*;

/// The fixture plus a second repository, so a repository row sits between
/// two worktrees: `demo, alpha, beta, other, x`.
fn app_with_two_repositories() -> (tempfile::TempDir, App) {
    let (dir, mut app) = app_with_fixture();
    let mut other = Repository::new("other".into(), "/tmp/other".into());
    other
        .worktrees
        .push(Worktree::new("x".into(), "x".into(), "/tmp/other-x".into()));
    app.state.add_repository(other);
    app.rebuild_rows();
    (dir, app)
}

/// The name of the selected worktree, or `None` on a repository.
fn selected(app: &App) -> Option<String> {
    app.state
        .selected_worktree()
        .map(|(_repo, worktree)| worktree.name.clone())
}

/// Put the sidebar cursor on the row with this label and select it.
fn select_row(app: &mut App, label: &str) {
    let index = app
        .rows
        .iter()
        .position(|row| match row {
            SidebarRow::Repository { name, .. } | SidebarRow::Worktree { name, .. } => {
                name == label
            }
            _ => false,
        })
        .unwrap_or_else(|| panic!("no row named {label}"));
    app.sidebar_index = index;
    app.select_current_row();
}

/// The visible worktree rows' names, top to bottom.
fn worktree_rows(app: &App) -> Vec<String> {
    app.rows
        .iter()
        .filter_map(|row| match row {
            SidebarRow::Worktree { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn bracket_keys_step_between_worktrees_skipping_repository_rows() {
    let (_dir, mut app) = app_with_two_repositories();
    // demo's last worktree, then the `other` repository row, then `x`.
    let worktrees = worktree_rows(&app);
    assert_eq!(worktrees.len(), 3);
    select_row(&mut app, &worktrees[1]);

    app.handle_key(key(KeyCode::Char(']')));
    assert_eq!(selected(&app).as_deref(), Some("x"));

    app.handle_key(key(KeyCode::Char('[')));
    assert_eq!(selected(&app), Some(worktrees[1].clone()));
}

#[tokio::test]
async fn bracket_keys_wrap_around_the_ends() {
    let (_dir, mut app) = app_with_two_repositories();
    let worktrees = worktree_rows(&app);
    select_row(&mut app, "x");

    app.handle_key(key(KeyCode::Char(']')));
    assert_eq!(selected(&app), Some(worktrees[0].clone()));

    app.handle_key(key(KeyCode::Char('[')));
    assert_eq!(selected(&app).as_deref(), Some("x"));
}

/// From a repository row the nearest worktree in that direction wins: the
/// repository's own first worktree, or — for a repository with none
/// visible — the next one in the list, wrapping if needed.
#[tokio::test]
async fn bracket_keys_from_a_repository_row_go_to_the_nearest_worktree() {
    let (_dir, mut app) = app_with_two_repositories();
    let worktrees = worktree_rows(&app);

    select_row(&mut app, "demo");
    app.handle_key(key(KeyCode::Char(']')));
    assert_eq!(selected(&app), Some(worktrees[0].clone()));

    // Collapse `demo`: its row is now followed directly by `other`.
    let demo = app.state.selection.repository_id.expect("a repository");
    app.collapsed.insert(demo);
    app.rebuild_rows();
    select_row(&mut app, "demo");
    app.handle_key(key(KeyCode::Char(']')));
    assert_eq!(selected(&app).as_deref(), Some("x"));

    select_row(&mut app, "demo");
    app.handle_key(key(KeyCode::Char('[')));
    assert_eq!(selected(&app).as_deref(), Some("x"), "wraps to the last");
}

#[tokio::test]
async fn bracket_keys_work_from_the_detail_pane_and_hand_focus_to_the_sidebar() {
    let (_dir, mut app) = app_with_two_repositories();
    let worktrees = worktree_rows(&app);
    select_row(&mut app, &worktrees[0]);
    app.focus = Focus::Detail;

    app.handle_key(key(KeyCode::Char(']')));
    assert_eq!(selected(&app), Some(worktrees[1].clone()));
    assert_eq!(app.focus, Focus::Sidebar);

    // So the arrows carry on through the sidebar from there.
    app.handle_key(key(KeyCode::Down));
    assert_eq!(selected(&app), None, "Down landed on the `other` row");
}

#[tokio::test]
async fn bracket_keys_do_nothing_with_a_single_worktree_selected() {
    let (_dir, mut app) = app_with_fixture();
    let worktrees = worktree_rows(&app);
    // Archive one, leaving a single visible worktree.
    select_row(&mut app, &worktrees[1]);
    let archived = app.state.selection.worktree_id.expect("a worktree");
    app.state.set_archived(archived, true);
    app.rebuild_rows();
    select_row(&mut app, &worktrees[0]);
    app.focus = Focus::Detail;

    app.handle_key(key(KeyCode::Char(']')));
    assert_eq!(selected(&app), Some(worktrees[0].clone()));
}

#[tokio::test]
async fn a_focused_rename_field_types_brackets_instead_of_switching() {
    let (_dir, mut app) = app_with_two_repositories();
    let worktrees = worktree_rows(&app);
    select_row(&mut app, &worktrees[0]);
    app.sessions = Some(Vec::new());
    snapshot_drawn(&mut app);
    app.focus = Focus::Detail;
    app.detail_index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Field(Field::WorktreeName))
        .expect("rename field present");

    app.handle_key(key(KeyCode::Char(']')));
    assert!(app.name_input.value().ends_with(']'));
    assert_eq!(selected(&app), Some(worktrees[0].clone()));
}

fn focus_gained(app: &mut App) {
    app.handle_event(AppEvent::Term(
        ratatui::crossterm::event::Event::FocusGained,
    ));
}

/// Coming back from a Claude window opened from a session card left the
/// detail pane focused, so the arrows no longer switched worktree.
#[tokio::test]
async fn returning_to_forestui_hands_focus_back_to_the_sidebar() {
    let (_dir, mut app) = app_with_two_repositories();
    let worktrees = worktree_rows(&app);
    select_row(&mut app, &worktrees[0]);
    app.focus = Focus::Detail;

    focus_gained(&mut app);
    assert_eq!(app.focus, Focus::Sidebar);

    app.handle_key(key(KeyCode::Down));
    assert_eq!(selected(&app), Some(worktrees[1].clone()));
}

#[tokio::test]
async fn returning_to_forestui_discards_an_unsaved_rename() {
    let (_dir, mut app) = app_with_two_repositories();
    let worktrees = worktree_rows(&app);
    select_row(&mut app, &worktrees[0]);
    app.sessions = Some(Vec::new());
    snapshot_drawn(&mut app);
    app.focus = Focus::Detail;
    app.detail_index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Field(Field::BranchName))
        .expect("branch field present");
    let original = app.branch_input.value().to_string();
    app.handle_key(key(KeyCode::Char('z')));
    assert_ne!(app.branch_input.value(), original);

    focus_gained(&mut app);
    assert_eq!(app.branch_input.value(), original);
    assert_eq!(app.focus, Focus::Sidebar);
}

#[tokio::test]
async fn returning_to_forestui_leaves_an_open_modal_in_charge() {
    let (_dir, mut app) = app_with_two_repositories();
    app.handle_key(key(KeyCode::Char('a')));
    assert_eq!(app.modals.len(), 1);

    focus_gained(&mut app);
    assert_eq!(app.modals.len(), 1);
    // 'q' still types into the modal rather than quitting.
    app.handle_key(key(KeyCode::Char('q')));
    assert!(!app.should_quit);
}

#[tokio::test]
async fn bracket_keys_leave_modals_and_modified_presses_alone() {
    let (_dir, mut app) = app_with_two_repositories();
    let worktrees = worktree_rows(&app);
    select_row(&mut app, &worktrees[0]);

    app.handle_key(ctrl(KeyCode::Char(']')));
    assert_eq!(selected(&app), Some(worktrees[0].clone()));

    app.handle_key(key(KeyCode::Char('a')));
    app.handle_key(key(KeyCode::Char(']')));
    assert_eq!(app.modals.len(), 1);
    assert_eq!(selected(&app), Some(worktrees[0].clone()));
}
