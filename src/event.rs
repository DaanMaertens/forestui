//! The single event channel that drives the app.
//!
//! Terminal input arrives from a blocking reader thread; background work
//! (git, gh, session scanning) arrives from tokio tasks.
//! Everything lands in one `mpsc::UnboundedReceiver<AppEvent>`, so the main loop
//! stays a plain `while let Some(event) = rx.recv().await`.

use crate::models::{ClaudeSession, GitHubIssue, Worktree};
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
    AddWorktree,
    /// Start work's confirm screen, for the issue in `App::pending_issue`.
    StartWorkConfirm,
    /// Refresh of the branch list inside an already-open modal, after a fetch.
    RefetchOpenModal,
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
    Issues {
        path: String,
        issues: Vec<GitHubIssue>,
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
        repo_id: Uuid,
        worktree: Box<Worktree>,
        /// Start Claude in the new worktree with this as its first message.
        /// Launched by the fold, so a result it drops starts nothing.
        first_prompt: Option<String>,
        /// That launch uses the built-in YOLO flag.
        yolo: bool,
    },
    /// The editor opened on Start work's prompt closed. `Ok` is the text it
    /// left; the fold applies it only to the confirm screen it was opened
    /// from, if that is still on top.
    PromptEdited {
        repo_id: Uuid,
        issue_number: i64,
        result: Result<String, String>,
    },
    /// A `git worktree list` scan finished; the fold reconciles the config
    /// against it (git is the source of truth for which worktrees exist).
    /// `None` means git itself failed — the fold releases the in-flight guard
    /// and keeps state untouched, because "could not list" must never read as
    /// "there are none" and prune every tracked worktree.
    /// Single-writer for the same reason as [`AppEvent::WorktreeAdded`].
    ///
    /// `epoch` is the repository's mutation counter at the moment the scan
    /// was spawned. A create, remove, or rename folding while git ran bumps
    /// the counter, and the fold discards a mismatched result: that listing
    /// describes a state the config has already moved past, and reconciling
    /// against it would revert the newer change.
    WorktreesScanned {
        repo_id: Uuid,
        epoch: u64,
        listed: Option<Vec<crate::services::git::WorktreeInfo>>,
    },
    /// A rename task ended without renaming (the target existed, or the move
    /// itself failed). Exists so the fold can release the rename's
    /// reconcile-protection; the error itself was already notified.
    WorktreeRenameAborted { worktree_id: Uuid },
    /// A worktree's directory was renamed on disk; fold the new name and path
    /// into state. Single-writer for the same reason as
    /// [`AppEvent::WorktreeAdded`].
    WorktreeRenamed {
        worktree_id: Uuid,
        name: String,
        path: String,
    },
    /// A worktree's branch was renamed by git. Folded only on success: the
    /// config used to take the new name before `git branch -m` ran and keep it
    /// when the rename failed, leaving a branch recorded that does not exist.
    WorktreeBranchRenamed { worktree_id: Uuid, branch: String },
    /// A worktree removal attempt finished. Success is folded into state on
    /// the main loop (single-writer, same reason as [`AppEvent::WorktreeAdded`]);
    /// a dirty refusal opens the second confirmation; an error surfaces git's
    /// message and leaves the entry in place.
    WorktreeRemoveResult {
        worktree_id: Uuid,
        outcome: Result<crate::services::git::RemoveOutcome, String>,
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
mod tests {
    use super::*;

    #[tokio::test]
    async fn events_reach_the_receiver() {
        let (tx, mut rx) = start();
        tx.info("hello");
        let received = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out")
            .expect("channel closed");
        // A tick may arrive first; drain until the notification shows up.
        let mut event = received;
        for _ in 0..50 {
            if let AppEvent::Notify(text, severity) = &event {
                assert_eq!(text, "hello");
                assert_eq!(*severity, Severity::Information);
                return;
            }
            event = rx.recv().await.expect("channel closed");
        }
        panic!("notification never arrived");
    }
}
