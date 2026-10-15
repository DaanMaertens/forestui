//! Which worktrees are finished: their work item is done.
//!
//! [`check`] runs in the background on the worktree sweep and asks `gh` and
//! git; [`FinishedWorktrees`] keeps its answers. They are derived state —
//! never saved — and are judged per repository, so a result can only change
//! the marks of the repository it was taken for, and only of worktrees that
//! repository still has. Whether an answer means finished is
//! [`work::is_finished`], a pure rule.

use crate::models::{Repository, WorkItemLink};
use crate::services::git;
use crate::services::github::{self, AuthStatus};
use crate::work::{self, Observed};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// The least time between two checks of one repository. Every selection
/// change rescans the repository it lands on, and a check costs a `gh`
/// round trip per linked worktree, so arrowing through the sidebar must not
/// turn into a stream of them.
pub const MIN_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Default)]
pub struct FinishedWorktrees {
    /// Repositories with a check running.
    running: HashSet<Uuid>,
    last_started: HashMap<Uuid, Instant>,
    /// The finished worktrees, by repository.
    finished: HashMap<Uuid, HashSet<Uuid>>,
}

/// Handed out when a check starts and handed back with its verdicts. Not
/// `Clone`, as in [`super::worktree_sync`]: the task that runs the check
/// sends it back on every exit path.
#[derive(Debug)]
pub struct FinishedTicket {
    repo_id: Uuid,
}

impl FinishedTicket {
    pub fn repo_id(&self) -> Uuid {
        self.repo_id
    }
}

impl FinishedWorktrees {
    /// `None` while a check of this repository is running, or one started
    /// less than [`MIN_INTERVAL`] ago.
    pub fn start(&mut self, repo_id: Uuid, now: Instant) -> Option<FinishedTicket> {
        if self.running.contains(&repo_id)
            || self
                .last_started
                .get(&repo_id)
                .is_some_and(|last| now.saturating_duration_since(*last) < MIN_INTERVAL)
        {
            return None;
        }
        self.running.insert(repo_id);
        self.last_started.insert(repo_id, now);
        Some(FinishedTicket { repo_id })
    }

    /// Fold a check's verdicts — `true` finished, `false` not, missing for
    /// "could not tell" — into the repository's marks. `current` is the
    /// repository's worktrees now, `None` once it is gone. Returns whether
    /// any mark changed, which is what earns a repaint.
    pub fn fold(
        &mut self,
        ticket: FinishedTicket,
        verdicts: &HashMap<Uuid, bool>,
        current: Option<&HashSet<Uuid>>,
    ) -> bool {
        self.running.remove(&ticket.repo_id);
        let Some(current) = current else {
            return self.finished.remove(&ticket.repo_id).is_some();
        };
        let before = self.finished.remove(&ticket.repo_id).unwrap_or_default();
        let after: HashSet<Uuid> = current
            .iter()
            .copied()
            .filter(|id| verdicts.get(id).copied().unwrap_or(before.contains(id)))
            .collect();
        let changed = after != before;
        if !after.is_empty() {
            self.finished.insert(ticket.repo_id, after);
        }
        changed
    }

    pub fn contains(&self, worktree_id: Uuid) -> bool {
        self.finished.values().any(|set| set.contains(&worktree_id))
    }
}

/// Hands App tests the ticket a running check would carry back, starting
/// one now if none is running — without the throttle, which a test that
/// delivers two results in a row would otherwise trip.
#[cfg(test)]
impl FinishedWorktrees {
    pub fn ticket_for_test(&mut self, repo_id: Uuid) -> FinishedTicket {
        self.running.insert(repo_id);
        FinishedTicket { repo_id }
    }
}

/// A worktree with a work item link, as the background check needs it.
#[derive(Debug, Clone)]
pub struct LinkedWorktree {
    id: Uuid,
    link: WorkItemLink,
    branch: String,
    created_from: Option<String>,
    /// The base branch it was started from, which a session's branch is
    /// merged into once its work is done.
    base: Option<String>,
}

/// The repository's worktrees that have a work item link. Without one a
/// worktree is never finished, so it is not asked about at all.
pub fn linked(repo: &Repository) -> Vec<LinkedWorktree> {
    repo.worktrees
        .iter()
        .filter_map(|worktree| {
            Some(LinkedWorktree {
                id: worktree.id,
                link: worktree.work_item.clone()?,
                branch: worktree.branch.clone(),
                created_from: worktree.created_from_ref.clone(),
                base: worktree.base_branch.clone(),
            })
        })
        .collect()
}

/// Ask `gh` and git about each linked worktree of the repository at
/// `source`. A worktree with no verdict is one nobody could answer for —
/// `gh` missing or signed out, a branch git cannot resolve, no remote
/// default branch — and keeps whatever mark it had.
///
/// A session's branch counts as merged only when its tip is in the base
/// branch it was started from — the remote's default branch for a worktree
/// from before the base was recorded — so a squash or rebase merge on
/// GitHub goes unnoticed, as does a worktree from before `created_from_ref`
/// was recorded.
pub async fn check(source: &str, linked: &[LinkedWorktree]) -> HashMap<Uuid, bool> {
    let is_session = |item: &&LinkedWorktree| matches!(item.link, WorkItemLink::Session { .. });
    let signed_in = linked.iter().any(|item| !is_session(&item))
        && github::get_auth_status().await.0 == AuthStatus::Authenticated;
    let remote_default = if linked
        .iter()
        .any(|item| is_session(&item) && item.base.is_none())
    {
        let remotes = git::list_remotes(source).await.unwrap_or_default();
        git::remote_default_branch(source, &remotes).await
    } else {
        None
    };

    let mut verdicts = HashMap::new();
    for item in linked {
        let observed = match &item.link {
            WorkItemLink::Issue { number } if signed_in => github::issue_state(source, *number)
                .await
                .map(Observed::State),
            WorkItemLink::PullRequest { number } if signed_in => {
                github::pull_request_state(source, *number)
                    .await
                    .map(Observed::State)
            }
            WorkItemLink::Session { .. } => match (
                item.base.as_ref().or(remote_default.as_ref()),
                &item.created_from,
            ) {
                (Some(target), Some(created_from)) => {
                    git::branch_progress(source, &item.branch, target, created_from)
                        .await
                        .map(|progress| Observed::Branch {
                            merged: progress.merged,
                            has_commits: progress.has_commits,
                        })
                }
                _ => None,
            },
            WorkItemLink::Issue { .. } | WorkItemLink::PullRequest { .. } => None,
        };
        if let Some(finished) =
            observed.and_then(|observed| work::is_finished(&item.link, observed))
        {
            verdicts.insert(item.id, finished);
        }
    }
    verdicts
}

#[cfg(test)]
mod tests;
