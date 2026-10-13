use super::test_support::{app_with_fixture, ctrl, key};
use super::worktree_sync::{ChangeTicket, WorktreeSync};
use super::*;
use crate::models::{CustomClaudeButton, Repository, WorkItemLink, Worktree};
use crate::repo_settings::ConfigFile;
use ratatui::crossterm::event::KeyCode;
use std::path::PathBuf;

mod blocked_issues;
mod finished_worktrees;
mod issue_filter;
mod linked_worktrees;
mod mouse;
mod navigation;
mod plain_worktree;
mod pr_review;
mod prompt_templates;
mod prune;
mod repaint;
mod repository_settings;
mod session_mode;
mod session_states;
mod sessions;
mod start_work;
mod start_work_scope;
mod update_notice;
mod worktrees;

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

fn a_live_window(session_id: &str) -> crate::services::live::LiveSession {
    crate::services::live::LiveSession {
        session_id: session_id.into(),
        place: crate::services::live::LivePlace::Window {
            window_id: "@7".into(),
            window_name: "claude:demo:wt".into(),
        },
        state: None,
        cwd: None,
    }
}

fn a_live_elsewhere(session_id: &str, pid: u32) -> crate::services::live::LiveSession {
    crate::services::live::LiveSession {
        session_id: session_id.into(),
        place: crate::services::live::LivePlace::Elsewhere { pid },
        state: None,
        cwd: None,
    }
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
    github::seed_cache_for_test(
        &path,
        crate::models::Scope::Mine,
        issues,
        chrono::Utc::now(),
    );
    (dir, app, path)
}

/// `gh`'s whole answer for `pr-review`: these PRs, listed for `viewer`.
fn pr_answer(
    prs: Vec<crate::models::GitHubPullRequest>,
    viewer: Option<String>,
) -> crate::services::github::PullRequests {
    crate::services::github::PullRequests {
        prs,
        viewer,
        incomplete: false,
    }
}

/// An open PR, as `gh pr list` returns it.
fn open_pr(number: i64, title: &str, author: &str, head: &str) -> crate::models::GitHubPullRequest {
    serde_json::from_value(serde_json::json!({
        "number": number,
        "title": title,
        "author": {"login": author},
        "isDraft": false,
        "headRefName": head,
        "createdAt": "2026-01-01T00:00:00Z",
    }))
    .expect("pull request fixture")
}

fn confirm_number(confirm: &crate::modal::StartWorkConfirmModal) -> Option<i64> {
    match &confirm.item {
        crate::work::WorkItem::Issue { number, .. } => Some(*number),
        crate::work::WorkItem::PullRequest { .. } | crate::work::WorkItem::Session { .. } => None,
    }
}

fn issue_of(entry: &crate::work::ItemEntry) -> Option<&crate::work::IssueEntry> {
    match entry {
        crate::work::ItemEntry::Issue(issue) => Some(issue),
        _ => None,
    }
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
    picker
        .entries()
        .filter_map(issue_of)
        .map(|e| e.issue.number)
        .collect()
}

/// The fixture's repository with `issues` cached for `scope`, run with
/// `settings`.
fn app_with_settings_and_issues(
    settings: crate::models::Settings,
    scope: crate::models::Scope,
    issues: Vec<crate::models::GitHubIssue>,
) -> (tempfile::TempDir, App, String) {
    let (dir, app) = test_support::app_with_settings(settings);
    let path = app.state.repositories()[0].source_path.clone();
    github::seed_cache_for_test(&path, scope, issues, chrono::Utc::now());
    (dir, app, path)
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

fn two_issues() -> Vec<crate::models::GitHubIssue> {
    vec![
        open_issue(42, "Fix login bug"),
        open_issue(7, "Crash on save"),
    ]
}

/// Move Start work's mode highlight down to `mode`, by key, so a test does
/// not count rows the mode list may gain.
fn highlight_mode(app: &mut App, mode: crate::work::WorkMode) {
    for _ in crate::work::WorkMode::ALL {
        if start_work(app).mode == mode {
            return;
        }
        app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(start_work(app).mode, mode);
}

/// What the GitHub probe reports for a repository with no GitHub remote.
fn no_github(app: &mut App, path: &str) {
    app.handle_event(AppEvent::GitHubChecked {
        path: path.to_string(),
        repo: None,
        for_start_work: true,
    });
}

/// This test thread's global prompts directory, removed again on drop so a
/// template written by one test cannot reach another run.
struct PromptsDir(PathBuf);

impl PromptsDir {
    fn new() -> Self {
        let dir = settings_service::prompts_dir();
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }

    fn write(&self, template: crate::prompt::Template, contents: &[u8]) {
        std::fs::create_dir_all(&self.0).unwrap();
        std::fs::write(self.0.join(template.file_name()), contents).unwrap();
    }
}

impl Drop for PromptsDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn template_picker(app: &App) -> &crate::modal::PromptTemplatesModal {
    match app.modals.last() {
        Some(Modal::PromptTemplates(picker)) => picker,
        other => panic!("the template picker expected, found {other:?}"),
    }
}
