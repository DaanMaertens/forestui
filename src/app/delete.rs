//! Deleting a worktree once its first confirm is answered: the removal in
//! the background, the fold of its result, and the second confirm a refusal
//! opens.
//!
//! git refuses a plain removal that a lock forbids, or that would lose
//! uncommitted work or work it cannot list. That refusal never ends the
//! flow: it becomes a confirm saying what is in the way, and only that
//! confirm forces.

use super::{App, worktree_sync};
use crate::event::{AppEvent, Severity};
use crate::modal::{ConfirmAction, ConfirmModal, Modal};
use crate::services::git;
use uuid::Uuid;

impl App {
    /// Run a worktree removal in the background. The entry leaves state only
    /// when git has actually removed the worktree — the fold decides, so a
    /// refusal can stop with everything intact. `force` is `None` for a plain
    /// removal, and what to override once the user has confirmed a refusal.
    pub(super) fn spawn_worktree_removal(&mut self, worktree_id: Uuid, force: Option<git::Force>) {
        let Some((repo, worktree)) = self.state.find_worktree(worktree_id) else {
            return;
        };
        let name = worktree.name.clone();
        // A second confirm on the same worktree while git is still running
        // would race a duplicate removal and stack a second destructive modal;
        // a rename still running would be moving the tree out from under it.
        let Some(change) = self.worktree_sync.begin_removal(repo.id, worktree_id) else {
            self.refuse_busy(&name);
            return;
        };
        let repo_path = repo.source_path.clone();
        let worktree_path = worktree.path.clone();
        let tracked_branch = worktree.branch.clone();
        // Read here, not in the task: the main loop owns the settings.
        let base_branch = self.base_branch_for(&repo_path);
        // Immediate feedback: on a large tree the round trip takes seconds and
        // the frame would otherwise be identical to the one before confirming.
        self.notify(format!("Deleting '{name}'…"), Severity::Information);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            // git's word over the state's: a branch switched since the last
            // scan would otherwise have the wrong branch deleted. A detached
            // worktree has none of its own; the state's is the fallback for
            // an entry git has already forgotten.
            let branch = match git::listed_worktree(&repo_path, &worktree_path).await {
                Some(listed) => listed.branch.unwrap_or_default(),
                None => tracked_branch,
            };
            let outcome = match force {
                Some(force) => git::force_remove_worktree(&repo_path, &worktree_path, force)
                    .await
                    .map(|()| git::RemoveOutcome::Removed),
                None => git::remove_worktree(&repo_path, &worktree_path).await,
            }
            .map_err(|error| error.to_string());
            // After the removal, not before: a branch checked out in the
            // worktree is one git will not delete.
            let branch_fate = match outcome {
                Ok(git::RemoveOutcome::Removed) => {
                    crate::prune::delete_branch_if_pushed(
                        &repo_path,
                        &branch,
                        base_branch.as_deref(),
                    )
                    .await
                }
                _ => None,
            };
            tx.send(AppEvent::WorktreeRemoveResult {
                change,
                outcome,
                branch: branch_fate,
            });
        });
    }

    /// Fold a finished removal attempt. State changes only here — the confirm
    /// handler just spawns git, so a refusal arrives with the entry (and
    /// the user's uncommitted work) fully intact.
    pub(super) fn on_worktree_remove_result(
        &mut self,
        change: worktree_sync::ChangeTicket,
        outcome: Result<git::RemoveOutcome, String>,
        branch: Option<crate::prune::BranchFate>,
    ) {
        let worktree_id = change.worktree_id();
        self.worktree_sync.finish(change);
        // A quit that waited on this removal can go through now.
        if self.quit_pending && !self.worktree_sync.any_removal_running() {
            self.should_quit = true;
        }
        // The entry may have been removed by other means while git ran.
        let Some((_, worktree)) = self.state.find_worktree(worktree_id) else {
            return;
        };
        let name = worktree.name.clone();
        match outcome {
            Ok(git::RemoveOutcome::Removed) => {
                self.state.remove_worktree(worktree_id);
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
                self.report_save_error();
                match branch {
                    Some(crate::prune::BranchFate::Deleted(branch)) => self.notify(
                        format!("Deleted '{name}' and its branch '{branch}'."),
                        Severity::Information,
                    ),
                    Some(crate::prune::BranchFate::Kept { branch, why }) => self.notify(
                        format!("Deleted '{name}'; kept branch '{branch}' — {why}."),
                        Severity::Warning,
                    ),
                    None => {}
                }
            }
            Ok(git::RemoveOutcome::Refused(refusal)) => {
                let confirm = refusal_confirm(worktree_id, &name, &refusal);
                self.modals.push(Modal::Confirm(confirm));
            }
            Err(error) => {
                self.notify(
                    format!("Could not delete '{name}': {error}"),
                    Severity::Error,
                );
            }
        }
    }
}

/// The confirm a refused Delete opens: what deleting anyway would discard,
/// and the lock it would break. Every refusal gets one — a flat "could not
/// delete" would leave the user no way forward from the UI. Confirming
/// overrides exactly what the dialog showed, through [`git::Refusal::force`].
pub(super) fn refusal_confirm(
    worktree_id: Uuid,
    name: &str,
    refusal: &git::Refusal,
) -> ConfirmModal {
    // The name is capped so what matters — what would be lost — never clips
    // off the dialog edge.
    let display = crate::util::truncate(name, 40);
    let named = format!("'{display}'");
    let mut lines = Vec::new();
    // Once the lock line has named the worktree, the rest refers back to it.
    let (title, subject, object, unlock) = match &refusal.lock {
        Some(lock) => {
            lines.push(match &lock.reason {
                Some(reason) => {
                    format!(
                        "'{display}' is locked: {}",
                        crate::util::truncate(reason, 40)
                    )
                }
                None => format!("'{display}' is locked."),
            });
            ("Locked Worktree", "It", "it", "unlock it and ")
        }
        None if matches!(refusal.changes, git::Changes::Unknown(_)) => {
            ("Cannot List Changes", named.as_str(), named.as_str(), "")
        }
        None => ("Uncommitted Changes", named.as_str(), named.as_str(), ""),
    };
    match &refusal.changes {
        git::Changes::None => lines.push(format!("Deleting will {unlock}remove it.")),
        git::Changes::Uncommitted(summary) => {
            lines.push(format!("{subject} has uncommitted changes:"));
            lines.push(summary.clone());
            lines.push(format!("Deleting will {unlock}discard them permanently."));
        }
        git::Changes::Unknown(why) => {
            lines.push(format!("Could not list the changes in {object}:"));
            // The dialog does not wrap, and git's reason often leads with a
            // long path that would push the reason itself off the edge.
            lines.extend(crate::ui::wrap_words(why, 72).into_iter().take(4));
            lines.push(format!(
                "Deleting will {unlock}discard any changes in it permanently."
            ));
        }
    }
    ConfirmModal::new(
        title,
        lines.join("\n"),
        ConfirmAction::ForceDeleteWorktree(worktree_id, refusal.force()),
    )
    // Opened by a background result, not a user action — a keystroke
    // already in the queue must not confirm it.
    .with_arm_delay()
}
