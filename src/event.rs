//! The single event channel that drives the app.
//!
//! Terminal input arrives from a blocking reader thread; background work
//! (git, gh, session scanning) arrives from tokio tasks.
//! Everything lands in one `mpsc::UnboundedReceiver<AppEvent>`, so the main loop
//! stays a plain `while let Some(event) = rx.recv().await`.

use crate::app::finished::FinishedTicket;
use crate::app::worktree_sync::{ChangeTicket, CreateTicket, ScanTicket};
use crate::models::{ClaudeSession, Worktree};
use crate::services::github::AuthStatus;
use chrono::{DateTime, Utc};
use ratatui::crossterm::event::{self, Event};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use uuid::Uuid;

/// Severity of a transient notification, mirroring Textual's `notify()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Information,
    Warning,
    Error,
}

/// Where a freshly loaded branch list should go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchTarget {
    /// The form of Start work's `worktree` mode.
    StartWorkWorktree,
    /// Start work's confirm screen, for the work item in `App::pending_start`.
    StartWorkConfirm,
    /// Refresh of the branch list inside an already-open modal, after a fetch.
    RefetchOpenModal,
}

/// How the Claude a new worktree was created for starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeStart {
    /// Its first message; none for a blank session prompt, which starts
    /// Claude ready to be talked to.
    pub first_prompt: Option<String>,
    /// The built-in YOLO flag.
    pub yolo: bool,
}

/// A repository's branches, as one load saw them.
#[derive(Debug, Clone, Default)]
pub struct BranchList {
    /// Local and remote branches; remote ones keep their `<remote>/`.
    pub branches: Vec<String>,
    pub remotes: Vec<String>,
    /// The branch the remote calls its default (`origin/main`), when it
    /// records one: what a new piece of work starts from.
    pub remote_default: Option<String>,
    /// The branch the repository's own checkout is on.
    pub current_branch: String,
}

/// Header data for the detail pane, loaded in the background.
#[derive(Debug, Clone, Default)]
pub struct DetailMeta {
    pub path: String,
    pub branch: Option<String>,
    pub commit_hash: Option<String>,
    pub commit_time: Option<DateTime<Utc>>,
    pub has_remote: bool,
    pub path_exists: bool,
}

#[derive(Debug)]
pub enum AppEvent {
    /// A raw terminal event (key, resize, focus, mouse).
    Term(Event),
    /// tmux was made to match the back key setting `key`.
    BackKeyApplied {
        key: Option<String>,
        claim: crate::services::tmux::back_key::Claim,
    },
    /// Periodic tick, used for spinner animation and notification expiry.
    Tick,
    /// `gh auth status` finished.
    GhStatus(AuthStatus, Option<String>),
    /// Claude sessions for a path finished loading. `None` means the scan
    /// failed (a panicked blocking task) — the fold must release its in-flight
    /// guard but keep whatever content is already on screen rather than
    /// overwriting it with an authoritative-looking empty list.
    /// One transcript was re-read. Carries its id so the fold can replace that
    /// card alone, and `None` when the session no longer exists.
    SessionRefreshed {
        path: String,
        id: String,
        session: Box<Option<ClaudeSession>>,
    },
    Sessions {
        path: String,
        sessions: Option<Vec<ClaudeSession>>,
    },
    /// The live scan finished: where every Claude session on the machine is
    /// running right now — stamped tmux windows plus plugin heartbeats. A
    /// snapshot of the whole view, so the fold replaces rather than merges —
    /// a session that ended simply stops being listed.
    LiveSessions {
        sessions: Vec<crate::services::live::LiveSession>,
    },
    /// The once-a-second state read finished: what each live session it
    /// asked about is doing, `None` where its heartbeat carries no state.
    SessionStates {
        states: std::collections::HashMap<String, Option<crate::services::live::SessionState>>,
    },
    /// A transcript deletion finished. Folded on the main loop so the list,
    /// the pin state and the toast all change together.
    SessionDeleted {
        path: String,
        session_id: String,
        title: String,
        result: Result<(), String>,
    },
    /// A stopped session's transcript was renamed on disk. Carries the re-read
    /// session so the card updates in the same fold that announces it.
    SessionRenamed {
        path: String,
        session_id: String,
        session: Box<Option<ClaudeSession>>,
        result: Result<(), String>,
    },
    /// GitHub issues for a repository finished loading.
    /// `scope` is the one they were listed for: a repository's own settings
    /// can change it while the fetch runs, and the list is then not the one
    /// wanted.
    /// `issues` is an error when `gh` could not list them.
    Issues {
        path: String,
        scope: crate::models::Scope,
        issues: crate::services::github::IssuesResult,
    },
    /// Start work's `pr-review` list finished loading: the open PRs in
    /// `scope`, and the login `gh` is authenticated as, which decides each
    /// one's review kind — or why there is no list.
    PullRequests {
        path: String,
        scope: crate::models::Scope,
        result: Result<crate::services::github::PullRequests, crate::services::github::ListError>,
    },
    /// `gh` was asked which GitHub repository a checkout resolves to:
    /// `repo` keys its repository settings, and without one only a
    /// session or a plain worktree can be started. `for_start_work` when
    /// Start work asked, which then wants the PRs for the scope that turns
    /// out to be in effect.
    GitHubChecked {
        path: String,
        repo: Option<crate::repo_settings::RepoKey>,
        for_start_work: bool,
    },
    /// Detail-pane header data finished loading.
    Meta(Box<DetailMeta>),
    /// Branch and remote lists finished loading.
    Branches {
        repo_id: Uuid,
        list: BranchList,
        target: BranchTarget,
    },
    /// A background fetch inside Start work's confirm screen failed.
    FetchFailed(String),
    /// Show a transient message.
    Notify(String, Severity),
    /// The startup update check concluded. Folded on the main loop rather
    /// than turned into a toast by the task, because a newer version is
    /// sticky state — the title bar carries it until the process restarts.
    UpdateChecked(crate::version_check::UpdateStatus),
    /// A background task finished creating a worktree. The entry is folded into
    /// state on the main loop — background tasks never write the config file
    /// themselves, so there is exactly one writer and a user action mid-flight
    /// cannot clobber a task's save (or the other way around).
    WorktreeAdded {
        create: CreateTicket,
        worktree: Box<Worktree>,
        /// Start Claude in the new worktree. Launched by the fold, so a
        /// result it drops starts nothing.
        claude: Option<ClaudeStart>,
    },
    /// A create task ended without a worktree. The error itself was already
    /// notified; this hands the ticket back so the create stops holding back
    /// its repository's scans.
    WorktreeAddFailed { create: CreateTicket },
    /// The editor opened on Start work's prompt closed. `Ok` is the text it
    /// left; the fold applies it only to the confirm screen it was opened
    /// from, if that is still on top.
    PromptEdited {
        repo_id: Uuid,
        link: crate::models::WorkItemLink,
        result: Result<String, String>,
    },
    /// The name suggestion a session's confirm screen asked for answered:
    /// Claude's raw reply, or `None` for a call that failed, timed out or
    /// could not run. The fold applies it only to the screen that asked, if
    /// that is still on top.
    SessionNameSuggested {
        request: Uuid,
        reply: Option<String>,
    },
    /// Start work asked `gh` for parent issue `number`, which its confirm
    /// screen names a worktree after if the parent has none: the issue, or
    /// `None` for a call that failed. Sent before the branches the screen
    /// waits on, so it is in place when they arrive.
    ParentIssueLoaded {
        number: i64,
        issue: Option<Box<crate::models::GitHubIssue>>,
    },
    /// A `git worktree list` scan finished; the fold reconciles the config
    /// against it (git is the source of truth for which worktrees exist).
    /// `None` means git itself failed — the fold releases the in-flight guard
    /// and keeps state untouched, because "could not list" must never read as
    /// "there are none" and prune every tracked worktree.
    /// Single-writer for the same reason as [`AppEvent::WorktreeAdded`].
    ///
    /// `scan` goes back to [`WorktreeSync`](crate::app::worktree_sync::WorktreeSync),
    /// which decides whether the listing is still current enough to reconcile.
    WorktreesScanned {
        scan: ScanTicket,
        listed: Option<Vec<crate::services::git::WorktreeInfo>>,
    },
    /// Which of a repository's linked worktrees are finished: `true`
    /// finished, `false` not, absent when the check could not tell.
    ///
    /// `ticket` goes back to
    /// [`FinishedWorktrees`](crate::app::finished::FinishedWorktrees); the
    /// verdicts fold into the marks saved on the worktrees, by
    /// [`AppState::fold_finished_verdicts`](crate::state::AppState::fold_finished_verdicts).
    WorktreesFinished {
        ticket: FinishedTicket,
        verdicts: std::collections::HashMap<Uuid, bool>,
    },
    /// A rename task ended without renaming (the target existed, or the move
    /// itself failed). Exists so the fold can release the rename's
    /// reconcile-protection; the error itself was already notified.
    WorktreeRenameAborted { change: ChangeTicket },
    /// A worktree's directory was renamed on disk; fold the new name and path
    /// into state. Single-writer for the same reason as
    /// [`AppEvent::WorktreeAdded`].
    WorktreeRenamed {
        change: ChangeTicket,
        name: String,
        path: String,
    },
    /// A worktree's branch was renamed by git. Folded only on success: the
    /// config used to take the new name before `git branch -m` ran and keep it
    /// when the rename failed, leaving a branch recorded that does not exist.
    WorktreeBranchRenamed {
        change: ChangeTicket,
        branch: String,
    },
    /// A worktree removal attempt finished. Success is folded into state on
    /// the main loop (single-writer, same reason as [`AppEvent::WorktreeAdded`]);
    /// a dirty refusal opens the second confirmation; an error surfaces git's
    /// message and leaves the entry in place. `branch` is what became of
    /// the worktree's local branch once git removed it; `None` when nothing
    /// was removed or there was no branch.
    WorktreeRemoveResult {
        change: ChangeTicket,
        outcome: Result<crate::services::git::RemoveOutcome, String>,
        branch: Option<crate::prune::BranchFate>,
    },
    /// A prune finished: each removal it started with its ticket, the
    /// worktree's name and what came of it, plus the names of the worktrees
    /// `WorktreeSync` refused because a change was already running on them.
    WorktreesPruned {
        removals: Vec<(ChangeTicket, String, crate::prune::Outcome)>,
        busy: Vec<String>,
    },
    /// Reload the detail pane only.
    ReloadDetail,
}

/// Clonable handle used by background tasks to push events back to the loop.
#[derive(Clone)]
pub struct EventTx(UnboundedSender<AppEvent>);

impl EventTx {
    pub fn send(&self, event: AppEvent) {
        let _ = self.0.send(event);
    }

    pub fn notify(&self, text: impl Into<String>, severity: Severity) {
        self.send(AppEvent::Notify(text.into(), severity));
    }

    pub fn info(&self, text: impl Into<String>) {
        self.notify(text, Severity::Information);
    }

    pub fn error(&self, text: impl Into<String>) {
        self.notify(text, Severity::Error);
    }
}

/// Create the channel and start the input reader and tick timer.
/// A bare channel for tests: no stdin-polling reader thread, no tick task.
/// [`start`] spawns both, and every test fixture that called it added another
/// detached thread competing for the test binary's stdin.
#[cfg(test)]
pub fn test_channel() -> (EventTx, UnboundedReceiver<AppEvent>) {
    let (tx, rx) = unbounded_channel::<AppEvent>();
    (EventTx(tx), rx)
}

pub fn start() -> (EventTx, UnboundedReceiver<AppEvent>) {
    let (tx, rx) = unbounded_channel::<AppEvent>();

    // Terminal input: crossterm's read() blocks, so it gets its own OS thread.
    let input_tx = tx.clone();
    std::thread::spawn(move || {
        loop {
            match event::poll(Duration::from_millis(200)) {
                Ok(true) => match event::read() {
                    Ok(ev) => {
                        if input_tx.send(AppEvent::Term(ev)).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });

    // Tick: drives spinners and notification expiry.
    let tick_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            interval.tick().await;
            if tick_tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });

    (EventTx(tx), rx)
}

#[cfg(test)]
mod tests;
