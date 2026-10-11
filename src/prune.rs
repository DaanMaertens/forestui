//! Prune: remove a finished worktree together with its local branch,
//! unless that would lose work.
//!
//! The routine knows nothing about the App, so the repository pane's
//! **Prune finished** and `forestui prune` run the same steps and word their
//! outcome the same way. Which worktrees are finished is not decided by
//! [`prune_worktree`] — the caller passes the ones it judged finished.

use crate::event::Severity;
use crate::services::git;
use crate::state::AppState;
use uuid::Uuid;

/// What pruning one worktree came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The worktree is gone. `branch_kept` is git's reason when its branch
    /// could not be deleted — checked out somewhere else, say.
    Removed {
        branch_kept: Option<String>,
    },
    Skipped(Skip),
    Failed(String),
}

/// Why a worktree was left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// Uncommitted changes or untracked files, summarised by git.
    Uncommitted(String),
    /// Commits that are on no remote branch.
    Unpushed(u64),
    /// The repository has no remote, so nothing in it was ever pushed.
    NoRemote,
    /// Another change (a rename, a removal) is already running on it. Only
    /// the App sees this one: the routine never runs on a busy worktree.
    Busy,
}

/// Remove the worktree at `worktree_path` and delete `branch`, or say why
/// not. Both checks run before anything is touched, and a check git cannot
/// answer fails the prune rather than reading as "nothing to lose".
pub async fn prune_worktree(repo_path: &str, worktree_path: &str, branch: &str) -> Outcome {
    let branch_ref = format!("refs/heads/{branch}");
    let has_branch = !branch.is_empty() && git::get_ref(repo_path, &branch_ref).await.is_some();
    let on_disk = crate::util::expanduser(worktree_path).exists();

    if on_disk {
        match git::uncommitted_changes(worktree_path).await {
            Ok(Some(summary)) => return Outcome::Skipped(Skip::Uncommitted(summary)),
            Ok(None) => {}
            Err(error) => return Outcome::Failed(error.to_string()),
        }
    }
    // The worktree's HEAD may have moved off the branch it was made on, so
    // both are asked about.
    let (dir, mut revs) = if on_disk {
        (worktree_path, vec!["HEAD"])
    } else {
        (repo_path, Vec::new())
    };
    if has_branch {
        revs.push(&branch_ref);
    }
    if !revs.is_empty() {
        // With no remote every commit counts as unpushed, main's history
        // included, and that number would only mislead.
        match git::list_remotes(repo_path).await {
            Ok(remotes) if remotes.is_empty() => return Outcome::Skipped(Skip::NoRemote),
            Ok(_) => {}
            Err(error) => return Outcome::Failed(error.to_string()),
        }
        match git::unpushed_commits(dir, &revs).await {
            Ok(0) => {}
            Ok(count) => return Outcome::Skipped(Skip::Unpushed(count)),
            Err(error) => return Outcome::Failed(error.to_string()),
        }
    }

    match git::remove_worktree(repo_path, worktree_path).await {
        Ok(git::RemoveOutcome::Removed) => {}
        // Written to between the check and the removal.
        Ok(git::RemoveOutcome::Dirty(summary)) => {
            return Outcome::Skipped(Skip::Uncommitted(summary));
        }
        Err(error) => return Outcome::Failed(error.to_string()),
    }
    let branch_kept = if has_branch {
        git::delete_branch(repo_path, branch)
            .await
            .err()
            .map(|error| error.to_string())
    } else {
        None
    };
    Outcome::Removed { branch_kept }
}

/// A prune's outcome in words: one sentence per line, worst last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<String>,
    pub severity: Severity,
}

/// Word the outcomes of one prune, each paired with its worktree's name.
pub fn report(outcomes: &[(String, Outcome)]) -> Report {
    let removed: Vec<&str> = outcomes
        .iter()
        .filter(|(_, outcome)| matches!(outcome, Outcome::Removed { .. }))
        .map(|(name, _)| name.as_str())
        .collect();
    let mut lines = vec![match removed.as_slice() {
        [] => "Pruned nothing.".to_string(),
        [name] => format!("Pruned '{name}'."),
        all => format!("Pruned {} worktrees.", all.len()),
    }];
    let mut severity = Severity::Information;

    for (name, outcome) in outcomes {
        if let Outcome::Removed {
            branch_kept: Some(why),
        } = outcome
        {
            lines.push(format!("Kept the branch of '{name}' ({}).", sentence(why)));
            severity = Severity::Warning;
        }
    }
    let skipped: Vec<String> = outcomes
        .iter()
        .filter_map(|(name, outcome)| match outcome {
            Outcome::Skipped(skip) => Some(format!("'{name}' ({})", reason(skip))),
            _ => None,
        })
        .collect();
    if !skipped.is_empty() {
        lines.push(format!("Skipped {}.", skipped.join(", ")));
        severity = Severity::Warning;
    }
    for (name, outcome) in outcomes {
        if let Outcome::Failed(why) = outcome {
            lines.push(format!("Could not prune '{name}': {}.", sentence(why)));
            severity = Severity::Error;
        }
    }
    Report { lines, severity }
}

fn reason(skip: &Skip) -> String {
    match skip {
        Skip::Uncommitted(summary) => format!("uncommitted changes: {summary}"),
        Skip::Unpushed(1) => "1 unpushed commit".to_string(),
        Skip::Unpushed(count) => format!("{count} unpushed commits"),
        Skip::NoRemote => "no remote to push to".to_string(),
        Skip::Busy => "still being changed".to_string(),
    }
}

/// git's message as part of a sentence: one line, no closing full stop.
fn sentence(message: &str) -> String {
    let flat = message.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.trim_end_matches('.').to_string()
}

/// Prune tracked worktrees one after another, without the App: each one git
/// removed leaves `state`, which persists it with the atomic writer. The
/// App does not use this — its removals go through `WorktreeSync`, and
/// `forestui prune` has no other change in flight to race.
pub async fn prune_tracked(state: &mut AppState, worktree_ids: &[Uuid]) -> Vec<(String, Outcome)> {
    let mut outcomes = Vec::with_capacity(worktree_ids.len());
    for &worktree_id in worktree_ids {
        let Some((repo, worktree)) = state.find_worktree(worktree_id) else {
            continue;
        };
        let (repo_path, name) = (repo.source_path.clone(), worktree.name.clone());
        let (path, branch) = (worktree.path.clone(), worktree.branch.clone());
        let outcome = prune_worktree(&repo_path, &path, &branch).await;
        if matches!(outcome, Outcome::Removed { .. }) {
            state.remove_worktree(worktree_id);
        }
        outcomes.push((name, outcome));
    }
    outcomes
}

/// One repository's share of a forest-wide prune.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryPrune {
    pub name: String,
    /// Empty when nothing in it was finished.
    pub outcomes: Vec<(String, Outcome)>,
}

/// `forestui prune`: every repository in the forest, brought in step with
/// git the way a worktree scan would, then its finished worktrees pruned.
///
/// Finished is judged by the same check the sidebar marks come from, so a
/// worktree nobody could answer for — `gh` signed out, no remote default
/// branch — is left alone, as it would be unmarked in the App.
pub async fn prune_forest(state: &mut AppState) -> Vec<RepositoryPrune> {
    let repo_ids: Vec<Uuid> = state.repositories().iter().map(|repo| repo.id).collect();
    let mut results = Vec::with_capacity(repo_ids.len());
    for repo_id in repo_ids {
        let Some(source) = state
            .find_repository(repo_id)
            .map(|repo| repo.source_path.clone())
        else {
            continue;
        };
        // A branch switched since the App last scanned would otherwise have
        // the wrong branch deleted. A listing git cannot give leaves the
        // config as it is, as in the App.
        if let Ok(listed) = git::list_worktrees(&source).await {
            state.reconcile_worktrees(repo_id, &listed, &Default::default());
        }
        let Some(repo) = state.find_repository(repo_id) else {
            continue;
        };
        let name = repo.name.clone();
        let verdicts =
            crate::app::finished::check(&source, &crate::app::finished::linked(repo)).await;
        let finished: Vec<Uuid> = repo
            .worktrees
            .iter()
            .map(|worktree| worktree.id)
            .filter(|id| verdicts.get(id) == Some(&true))
            .collect();
        let outcomes = prune_tracked(state, &finished).await;
        results.push(RepositoryPrune { name, outcomes });
    }
    results
}

/// What `forestui prune` prints, and whether it exits non-zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub lines: Vec<String>,
    /// A worktree that could not be pruned. A skip is not a failure: it is
    /// the routine declining to lose work, which is what it is for.
    pub failed: bool,
}

pub fn summary(repositories: &[RepositoryPrune]) -> Summary {
    let mut lines = Vec::new();
    let mut failed = false;
    for repository in repositories {
        if repository.outcomes.is_empty() {
            continue;
        }
        failed |= repository
            .outcomes
            .iter()
            .any(|(_, outcome)| matches!(outcome, Outcome::Failed(_)));
        lines.extend(
            report(&repository.outcomes)
                .lines
                .into_iter()
                .map(|line| format!("{}: {line}", repository.name)),
        );
    }
    if lines.is_empty() {
        lines.push("No finished worktrees.".to_string());
    }
    Summary { lines, failed }
}

#[cfg(test)]
mod tests;
