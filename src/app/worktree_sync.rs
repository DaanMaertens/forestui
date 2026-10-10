//! Keeps worktree scans from undoing forestui's own changes.
//!
//! A scan is a snapshot of `git worktree list`, taken in the background. If
//! one of forestui's own changes (create, rename, removal) finishes while it
//! runs, the snapshot describes a state the config has already moved past,
//! and reconciling against it would revert the change — a renamed branch
//! back to its old name, most visibly. This module owns the rule that
//! prevents that, so no flow can forget a step of it:
//!
//! - Each repository has an epoch. Every finished change moves it on,
//!   success or not, and a scan that started under an older epoch is ignored.
//! - A change in flight protects the worktree it touches: the listing may
//!   catch it half done, so the reconcile leaves that worktree alone. A create
//!   has no worktree id until it lands, so it holds back its whole repository.
//! - A worktree takes one change at a time.
//!
//! Both kinds of ticket are deliberately not `Clone`: a change is started
//! once and finished once, and the task that runs it carries the ticket back
//! in its result event. That means every exit path of the task must send an
//! event carrying its ticket — a path that only toasts leaves the change in
//! flight for good.
//!
//! Pure bookkeeping: nothing here touches state, git or the config file.

use crate::services::git::WorktreeInfo;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Debug, Default)]
pub struct WorktreeSync {
    /// Repositories with a scan running — one at a time, since selection
    /// changes arrive far faster than a listing returns.
    scanning: HashSet<Uuid>,
    /// Missing means 0: a repository enters the map at its first change.
    epochs: HashMap<Uuid, u64>,
    /// Renames and removals in flight, keyed by worktree.
    changes: HashMap<Uuid, Change>,
    /// Creates in flight per repository; more than one can run at once.
    creates: HashMap<Uuid, usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeKind {
    Rename,
    Removal,
}

#[derive(Debug)]
struct Change {
    repo_id: Uuid,
    kind: ChangeKind,
}

/// Handed out when a scan starts and handed back with its listing.
#[derive(Debug)]
pub struct ScanTicket {
    repo_id: Uuid,
    epoch: u64,
}

impl ScanTicket {
    pub fn repo_id(&self) -> Uuid {
        self.repo_id
    }
}

/// A rename or removal of one worktree, from start to fold.
#[derive(Debug)]
pub struct ChangeTicket {
    repo_id: Uuid,
    worktree_id: Uuid,
}

impl ChangeTicket {
    pub fn worktree_id(&self) -> Uuid {
        self.worktree_id
    }
}

/// A worktree create, from start to fold.
#[derive(Debug)]
pub struct CreateTicket {
    repo_id: Uuid,
}

impl CreateTicket {
    pub fn repo_id(&self) -> Uuid {
        self.repo_id
    }
}

#[derive(Debug)]
pub enum ScanVerdict {
    /// The listing is stale, or a create is running on the repository.
    Ignore,
    /// Git could not list. "Could not list" must never read as "there are
    /// none" and prune every tracked worktree.
    KeepState,
    /// Reconcile against `listed`, leaving the `protected` worktrees alone:
    /// their own fold lands the truth about them.
    Reconcile {
        listed: Vec<WorktreeInfo>,
        protected: HashSet<Uuid>,
    },
}

impl WorktreeSync {
    /// `None` while a scan of this repository is already running.
    pub fn start_scan(&mut self, repo_id: Uuid) -> Option<ScanTicket> {
        if !self.scanning.insert(repo_id) {
            return None;
        }
        Some(ScanTicket {
            repo_id,
            epoch: self.epoch(repo_id),
        })
    }

    /// Releases the scan guard whatever the verdict, so a result the caller
    /// drops for its own reasons (the repository is gone) still frees it.
    pub fn judge_scan(
        &mut self,
        ticket: ScanTicket,
        listed: Option<Vec<WorktreeInfo>>,
    ) -> ScanVerdict {
        self.scanning.remove(&ticket.repo_id);
        if ticket.epoch != self.epoch(ticket.repo_id) || self.creates.contains_key(&ticket.repo_id)
        {
            return ScanVerdict::Ignore;
        }
        let Some(listed) = listed else {
            return ScanVerdict::KeepState;
        };
        let protected = self
            .changes
            .iter()
            .filter(|(_, change)| change.repo_id == ticket.repo_id)
            .map(|(worktree_id, _)| *worktree_id)
            .collect();
        ScanVerdict::Reconcile { listed, protected }
    }

    /// `None` while the worktree already has a change running.
    pub fn begin_rename(&mut self, repo_id: Uuid, worktree_id: Uuid) -> Option<ChangeTicket> {
        self.begin(repo_id, worktree_id, ChangeKind::Rename)
    }

    /// `None` while the worktree already has a change running.
    pub fn begin_removal(&mut self, repo_id: Uuid, worktree_id: Uuid) -> Option<ChangeTicket> {
        self.begin(repo_id, worktree_id, ChangeKind::Removal)
    }

    pub fn finish(&mut self, ticket: ChangeTicket) {
        self.changes.remove(&ticket.worktree_id);
        self.bump(ticket.repo_id);
    }

    pub fn begin_create(&mut self, repo_id: Uuid) -> CreateTicket {
        *self.creates.entry(repo_id).or_insert(0) += 1;
        CreateTicket { repo_id }
    }

    pub fn finish_create(&mut self, ticket: CreateTicket) {
        if let Some(running) = self.creates.get_mut(&ticket.repo_id) {
            *running -= 1;
            if *running == 0 {
                self.creates.remove(&ticket.repo_id);
            }
        }
        self.bump(ticket.repo_id);
    }

    pub fn change_running(&self, worktree_id: Uuid) -> bool {
        self.changes.contains_key(&worktree_id)
    }

    pub fn is_removing(&self, worktree_id: Uuid) -> bool {
        self.changes
            .get(&worktree_id)
            .is_some_and(|change| change.kind == ChangeKind::Removal)
    }

    /// Holds quit: an orphaned git child would finish removing the tree on
    /// disk after the config was last saved, leaving a phantom entry.
    pub fn any_removal_running(&self) -> bool {
        self.changes
            .values()
            .any(|change| change.kind == ChangeKind::Removal)
    }

    fn begin(
        &mut self,
        repo_id: Uuid,
        worktree_id: Uuid,
        kind: ChangeKind,
    ) -> Option<ChangeTicket> {
        if self.changes.contains_key(&worktree_id) {
            return None;
        }
        self.changes.insert(worktree_id, Change { repo_id, kind });
        Some(ChangeTicket {
            repo_id,
            worktree_id,
        })
    }

    fn epoch(&self, repo_id: Uuid) -> u64 {
        self.epochs.get(&repo_id).copied().unwrap_or(0)
    }

    fn bump(&mut self, repo_id: Uuid) {
        let epoch = self.epochs.entry(repo_id).or_insert(0);
        *epoch = epoch.wrapping_add(1);
    }
}

/// App tests stand in for the background tasks, which own the real tickets
/// and never run under a test that does not await them. These hand a test
/// the ticket such a task would carry back. This module's own tests do not
/// use them: they go through the interface like any caller.
#[cfg(test)]
impl WorktreeSync {
    /// The ticket of a scan running on `repo_id` — started now if none is —
    /// as a listing that raced nothing would carry it.
    pub fn scan_ticket_for_test(&mut self, repo_id: Uuid) -> ScanTicket {
        self.scanning.insert(repo_id);
        ScanTicket {
            repo_id,
            epoch: self.epoch(repo_id),
        }
    }

    /// The ticket held by the task running a change on `worktree_id`.
    pub fn change_ticket_for_test(&self, worktree_id: Uuid) -> ChangeTicket {
        let change = self
            .changes
            .get(&worktree_id)
            .expect("no change is running on this worktree");
        ChangeTicket {
            repo_id: change.repo_id,
            worktree_id,
        }
    }
}

#[cfg(test)]
mod tests;
