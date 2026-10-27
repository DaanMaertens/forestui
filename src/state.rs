//! Persisted application state: the repositories tracked in a forest.
//!
//! Stored as `.forestui-config.json` inside the forest directory itself, which
//! is what makes multiple independent forests possible.

use crate::models::{AppStateData, Repository, Selection, WorkItemLink, Worktree};
use crate::services::git::WorktreeInfo;
use crate::services::settings::get_forest_path;
use crate::util::resolve_path;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// What a reconcile against `git worktree list` changed, for the toast.
/// Branch-only refreshes are counted so the caller knows a save happened,
/// but they are not worth announcing.
#[derive(Debug, Default)]
pub struct WorktreeReconcile {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Worktrees whose recorded branch was corrected to what git reports.
    pub branch_updated: Vec<Uuid>,
}

impl WorktreeReconcile {
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.branch_updated.is_empty()
    }
}

pub struct AppState {
    repositories: Vec<Repository>,
    /// Pinned Claude sessions per path, in pin order — see
    /// [`crate::models::AppStateData::pinned_sessions`].
    pinned_sessions: std::collections::HashMap<String, Vec<String>>,
    pub selection: Selection,
    pub show_archived: bool,
    config_path: PathBuf,
    /// Why the last [`AppState::save`] failed, if it did.
    ///
    /// A failed write used to be discarded outright, so the app went on to
    /// announce "Created worktree 'x'" for a worktree that never reached the
    /// config — the loudest possible lie, because a corrupt or missing file
    /// loads as *empty* state and the entry is simply gone on the next launch.
    /// Callers that announce success drain this first.
    save_error: Option<String>,
}

impl AppState {
    /// Load state for the active forest, creating the forest directory if needed.
    pub fn load() -> Self {
        let forest_dir = get_forest_path();
        let _ = std::fs::create_dir_all(&forest_dir);
        Self::load_from(forest_dir.join(".forestui-config.json"))
    }

    /// Load state from an explicit config path. Corrupt files are ignored, the
    /// same as the Textual build, so a bad file never blocks startup.
    pub fn load_from(config_path: PathBuf) -> Self {
        let data = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<AppStateData>(&raw).ok())
            .unwrap_or_default();

        Self {
            repositories: data.repositories,
            pinned_sessions: data.pinned_sessions,
            selection: Selection::default(),
            show_archived: false,
            config_path,
            save_error: None,
        }
    }

    fn save(&mut self) {
        if let Some(parent) = self.config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let data = AppStateData {
            repositories: self.repositories.clone(),
            pinned_sessions: self.pinned_sessions.clone(),
        };
        self.save_error = match serde_json::to_string_pretty(&data) {
            Ok(json) => crate::util::write_atomically(&self.config_path, &json)
                .err()
                .map(|error| error.to_string()),
            Err(error) => Some(error.to_string()),
        };
    }

    /// Take the last save failure, if the most recent write did not land.
    ///
    /// Draining it means one failure is reported once, by whoever was about to
    /// claim the change succeeded.
    pub fn take_save_error(&mut self) -> Option<String> {
        self.save_error.take()
    }

    pub fn repositories(&self) -> &[Repository] {
        &self.repositories
    }

    pub fn add_repository(&mut self, repository: Repository) {
        self.repositories.push(repository);
        self.save();
    }

    pub fn remove_repository(&mut self, repo_id: Uuid) {
        self.repositories.retain(|r| r.id != repo_id);
        if self.selection.repository_id == Some(repo_id) {
            self.selection = Selection::default();
        }
        self.save();
    }

    pub fn find_repository(&self, repo_id: Uuid) -> Option<&Repository> {
        self.repositories.iter().find(|r| r.id == repo_id)
    }

    /// Find a worktree and the repository that owns it.
    pub fn find_worktree(&self, worktree_id: Uuid) -> Option<(&Repository, &Worktree)> {
        self.repositories.iter().find_map(|repo| {
            repo.worktrees
                .iter()
                .find(|w| w.id == worktree_id)
                .map(|w| (repo, w))
        })
    }

    /// The worktree a work item was started in, archived or not. Matched on
    /// the recorded link, never on names, so a renamed worktree still answers.
    pub fn linked_worktree(&self, repo_id: Uuid, link: &WorkItemLink) -> Option<&Worktree> {
        self.find_repository(repo_id)?
            .worktrees
            .iter()
            .find(|w| w.work_item.as_ref() == Some(link))
    }

    /// The worktree of `repo_id` that has `branch` checked out, if any.
    pub fn worktree_on_branch(&self, repo_id: Uuid, branch: &str) -> Option<&Worktree> {
        self.find_repository(repo_id)?
            .worktrees
            .iter()
            .find(|w| w.branch == branch)
    }

    pub fn add_worktree(&mut self, repo_id: Uuid, worktree: Worktree) {
        if let Some(repo) = self.repositories.iter_mut().find(|r| r.id == repo_id) {
            // A reconcile scan can adopt this path in the window between git
            // creating the tree and the create task reporting back. The create
            // flow's entry wins — it carries the user's chosen name and base
            // branch — but the scan's must go, or the sidebar shows the same
            // worktree twice.
            let resolved = resolve_path(&worktree.path);
            repo.worktrees.retain(|w| resolve_path(&w.path) != resolved);
            repo.worktrees.push(worktree);
            self.save();
        }
    }

    /// Reconcile a repository's tracked worktrees against `git worktree list`.
    ///
    /// Git is the source of truth for which worktrees *exist*; the config only
    /// annotates them (name, archived flag, sort order). Listed paths the
    /// config has never seen are adopted, tracked paths git no longer lists
    /// are dropped, and a survivor's branch is corrected when it drifted —
    /// worktrees created, removed, or switched by anything else (an agent
    /// running `git worktree add`, a manual `git worktree remove`) all
    /// converge into the sidebar this way.
    ///
    /// Returns `None` — and does not save — when nothing changed, so the
    /// periodic scan is free while reality already matches.
    ///
    /// `protected` names worktrees with a removal or rename still in flight:
    /// the listing cannot be trusted about them (it may predate the mutation
    /// or observe it half-done), so they are neither pruned nor
    /// branch-corrected — their own fold will land the truth.
    pub fn reconcile_worktrees(
        &mut self,
        repo_id: Uuid,
        listed: &[WorktreeInfo],
        protected: &std::collections::HashSet<Uuid>,
    ) -> Option<WorktreeReconcile> {
        // Paths that belong to *other* tracked repositories (their checkouts
        // and their worktrees). `git worktree list` from a repository that is
        // itself a linked worktree prints its siblings too; adopting those
        // here would show one directory as two rows owned by two entries,
        // where deleting either destroys the other's tree.
        let foreign: std::collections::HashSet<PathBuf> = self
            .repositories
            .iter()
            .filter(|r| r.id != repo_id)
            .flat_map(|r| {
                std::iter::once(resolve_path(&r.source_path))
                    .chain(r.worktrees.iter().map(|w| resolve_path(&w.path)))
            })
            .collect();

        let repo = self.repositories.iter_mut().find(|r| r.id == repo_id)?;
        let source = resolve_path(&repo.source_path);

        // Drop the main checkout: it is the repository row, not a worktree.
        // Git prints it first — the parser keeps a bare repository's entry
        // for exactly this reason — and it only differs from `source` when
        // the "repository" the user added is itself a linked worktree, so
        // skipping by position covers that case too, instead of adopting the
        // real checkout as a deletable row.
        let listed: Vec<(PathBuf, &WorktreeInfo)> = listed
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != 0)
            .map(|(_, info)| (resolve_path(&info.path), info))
            .filter(|(resolved, _)| *resolved != source)
            .collect();
        let listed_by_path: std::collections::HashMap<&PathBuf, &WorktreeInfo> =
            listed.iter().map(|(path, info)| (path, *info)).collect();

        let mut outcome = WorktreeReconcile::default();

        // Each tracked path is canonicalized exactly once, keyed by id; the
        // loops below are lookups, not repeated realpath syscalls on the
        // event loop.
        let tracked: std::collections::HashMap<Uuid, PathBuf> = repo
            .worktrees
            .iter()
            .map(|w| (w.id, resolve_path(&w.path)))
            .collect();

        let mut removed_ids = Vec::new();
        for worktree in &repo.worktrees {
            let Some(resolved) = tracked.get(&worktree.id) else {
                continue;
            };
            // Unlisted alone is not enough to prune: the listing is a
            // snapshot, and a rename or create can fold in between git
            // running and this landing — the entry's directory existing is
            // the proof it is newer than the listing. A real removal
            // (`git worktree remove`) deletes the directory, so it still
            // prunes; a directory deleted by hand stays *listed* by git and
            // is not this case.
            if !protected.contains(&worktree.id)
                && !listed_by_path.contains_key(resolved)
                && !resolved.exists()
            {
                removed_ids.push(worktree.id);
                outcome.removed.push(worktree.name.clone());
            }
        }
        repo.worktrees.retain(|w| !removed_ids.contains(&w.id));

        for worktree in repo.worktrees.iter_mut() {
            if protected.contains(&worktree.id) {
                continue;
            }
            let Some(resolved) = tracked.get(&worktree.id) else {
                continue;
            };
            if let Some(info) = listed_by_path.get(resolved) {
                let branch = info.branch.clone().unwrap_or_else(|| "HEAD".to_string());
                if worktree.branch != branch {
                    worktree.branch = branch;
                    outcome.branch_updated.push(worktree.id);
                }
            }
        }

        let known: std::collections::HashSet<&PathBuf> = repo
            .worktrees
            .iter()
            .filter_map(|w| tracked.get(&w.id))
            .collect();
        for (resolved, info) in &listed {
            // A listed path whose directory is gone is a prunable stub —
            // adopting it would add a row whose every action fails.
            if known.contains(resolved) || foreign.contains(resolved) || !resolved.exists() {
                continue;
            }
            let base = Path::new(&info.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| info.path.clone());
            // Names key tmux windows (`repo:name`), so two adoptions sharing
            // a basename would open and focus each other's editors; suffix
            // the later one instead.
            let mut name = base.clone();
            let mut suffix = 2;
            while repo.worktrees.iter().any(|w| w.name == name) {
                name = format!("{base}-{suffix}");
                suffix += 1;
            }
            let branch = info.branch.clone().unwrap_or_else(|| "HEAD".to_string());
            outcome.added.push(name.clone());
            repo.worktrees
                .push(Worktree::new(name, branch, info.path.clone()));
        }

        if outcome.is_empty() {
            return None;
        }
        // The pruned worktree can be the one on screen.
        if let Some(selected) = self.selection.worktree_id
            && removed_ids.contains(&selected)
        {
            self.selection.worktree_id = None;
        }
        self.save();
        Some(outcome)
    }

    pub fn remove_worktree(&mut self, worktree_id: Uuid) {
        for repo in &mut self.repositories {
            repo.worktrees.retain(|w| w.id != worktree_id);
        }
        if self.selection.worktree_id == Some(worktree_id) {
            self.selection = Selection {
                repository_id: self.selection.repository_id,
                worktree_id: None,
            };
        }
        self.save();
    }

    /// Apply a mutation to a worktree and persist.
    pub fn update_worktree<F: FnOnce(&mut Worktree)>(&mut self, worktree_id: Uuid, edit: F) {
        for repo in &mut self.repositories {
            if let Some(worktree) = repo.worktrees.iter_mut().find(|w| w.id == worktree_id) {
                edit(worktree);
                self.save();
                return;
            }
        }
    }

    pub fn set_finished(&mut self, worktree_id: Uuid, finished: bool) {
        self.update_worktree(worktree_id, |w| w.finished = finished);
    }

    /// Fold a finished check of `repo_id` into its worktrees' saved marks —
    /// `true` finished, `false` not, missing for "could not tell", which
    /// changes nothing. Verdicts about worktrees the repository no longer
    /// has are dropped. Saves, and returns true, only when a mark or a last
    /// answer changed.
    pub fn fold_finished_verdicts(
        &mut self,
        repo_id: Uuid,
        verdicts: &std::collections::HashMap<Uuid, bool>,
    ) -> bool {
        let Some(repo) = self.repositories.iter_mut().find(|r| r.id == repo_id) else {
            return false;
        };
        let mut changed = false;
        for worktree in &mut repo.worktrees {
            if let Some(&finished) = verdicts.get(&worktree.id) {
                changed |= worktree.fold_finished_check(finished);
            }
        }
        if changed {
            self.save();
        }
        changed
    }

    pub fn set_archived(&mut self, worktree_id: Uuid, archived: bool) {
        self.update_worktree(worktree_id, |w| w.is_archived = archived);
    }

    pub fn select_repository(&mut self, repo_id: Uuid) {
        self.selection = Selection {
            repository_id: Some(repo_id),
            worktree_id: None,
        };
    }

    pub fn select_worktree(&mut self, repo_id: Uuid, worktree_id: Uuid) {
        self.selection = Selection {
            repository_id: Some(repo_id),
            worktree_id: Some(worktree_id),
        };
    }

    pub fn selected_repository(&self) -> Option<&Repository> {
        self.selection
            .repository_id
            .and_then(|id| self.find_repository(id))
    }

    pub fn selected_worktree(&self) -> Option<(&Repository, &Worktree)> {
        self.selection
            .worktree_id
            .and_then(|id| self.find_worktree(id))
    }

    // -------------------------------------------------------- session pins

    /// Pinned session ids for a path, in pin order.
    pub fn pinned_for(&self, path: &str) -> Vec<String> {
        self.pinned_sessions.get(path).cloned().unwrap_or_default()
    }

    pub fn is_pinned(&self, path: &str, session_id: &str) -> bool {
        self.pinned_sessions
            .get(path)
            .is_some_and(|ids| ids.iter().any(|id| id == session_id))
    }

    /// Pin or unpin a session. Returns whether it is pinned afterwards.
    /// A new pin appends, so pin order is the order the user pinned in.
    pub fn toggle_pin(&mut self, path: &str, session_id: &str) -> bool {
        let ids = self.pinned_sessions.entry(path.to_string()).or_default();
        let pinned = if let Some(index) = ids.iter().position(|id| id == session_id) {
            ids.remove(index);
            false
        } else {
            ids.push(session_id.to_string());
            true
        };
        if self.pinned_sessions.get(path).is_some_and(Vec::is_empty) {
            // An empty list would sit in the config file forever.
            self.pinned_sessions.remove(path);
        }
        self.save();
        pinned
    }

    /// Drop a pin without toggling — for a session that was deleted.
    pub fn remove_pin(&mut self, path: &str, session_id: &str) {
        if let Some(ids) = self.pinned_sessions.get_mut(path) {
            let before = ids.len();
            ids.retain(|id| id != session_id);
            let now_empty = ids.is_empty();
            let changed = ids.len() != before;
            if now_empty {
                self.pinned_sessions.remove(path);
            }
            if changed {
                self.save();
            }
        }
    }

    /// Move a pinned session within the pinned order. Returns whether anything
    /// moved — unpinned sessions are ordered by recency and cannot be moved.
    pub fn move_pin(&mut self, path: &str, session_id: &str, delta: isize) -> bool {
        let Some(ids) = self.pinned_sessions.get_mut(path) else {
            return false;
        };
        let Some(index) = ids.iter().position(|id| id == session_id) else {
            return false;
        };
        let target = index as isize + delta;
        if target < 0 || target as usize >= ids.len() {
            return false;
        }
        ids.swap(index, target as usize);
        self.save();
        true
    }

    pub fn has_archived_worktrees(&self) -> bool {
        self.repositories
            .iter()
            .any(|r| r.worktrees.iter().any(|w| w.is_archived))
    }

    /// Path of the currently selected repository or worktree.
    pub fn selected_path(&self) -> Option<String> {
        if let Some((_repo, worktree)) = self.selected_worktree() {
            return Some(worktree.path.clone());
        }
        self.selected_repository().map(|r| r.source_path.clone())
    }

    /// tmux window name for a path: `repo:worktree`, or just `repo`.
    pub fn tmux_window_name(&self, path: &str) -> String {
        for repo in &self.repositories {
            for worktree in &repo.worktrees {
                if worktree.path == path {
                    return format!("{}:{}", repo.name, worktree.name);
                }
            }
            if repo.source_path == path {
                return repo.name.clone();
            }
        }
        "session".to_string()
    }
}

#[cfg(test)]
mod tests;
