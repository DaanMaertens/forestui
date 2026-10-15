//! Git operations, executed by shelling out to the `git` binary.
//!
//! Shelling out keeps the argv identical to the Textual build, so behaviour
//! (including how `git` resolves config, hooks and credentials) is unchanged.

use chrono::{DateTime, TimeZone, Utc};
use std::fmt;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct GitError(pub String);

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GitError {}

pub type GitResult<T> = Result<T, GitError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: String,
    pub head: String,
    pub branch: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub short_hash: String,
    pub timestamp: DateTime<Utc>,
}

/// A signal-killed process must not read as success; -1 fails every
/// `code != 0` check below.
fn exit_code(status: std::process::ExitStatus) -> i32 {
    crate::util::exit_code(status, -1)
}

/// Run a git command, returning `(exit_code, stdout, stderr)`.
///
/// Returns [`GitError`] when the process cannot be spawned at all — most
/// commonly a stale worktree whose directory has been deleted, which makes
/// `cwd` invalid.
async fn run_git(args: &[&str], cwd: Option<&Path>) -> GitResult<(i32, String, String)> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd.output().await.map_err(|e| {
        GitError(format!(
            "Failed to run git in {}: {e}",
            cwd.map(|p| p.display().to_string()).unwrap_or_default()
        ))
    })?;
    Ok((
        exit_code(output.status),
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
    ))
}

fn expand(path: &str) -> PathBuf {
    crate::util::expanduser(path)
}

pub async fn get_current_branch(path: &str) -> GitResult<String> {
    let dir = expand(path);
    let (code, stdout, stderr) = run_git(&["branch", "--show-current"], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to get current branch: {stderr}")));
    }
    Ok(if stdout.is_empty() {
        "HEAD".to_string()
    } else {
        stdout
    })
}

pub async fn list_remotes(path: &str) -> GitResult<Vec<String>> {
    let dir = expand(path);
    let (code, stdout, stderr) = run_git(&["remote"], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to list remotes: {stderr}")));
    }
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

async fn safe_list_remotes(path: &str) -> Vec<String> {
    list_remotes(path).await.unwrap_or_default()
}

/// List branches. Remote branches keep their `<remote>/` prefix.
pub async fn list_branches(path: &str) -> GitResult<Vec<String>> {
    let dir = expand(path);
    // `git branch -a` returns remote refs and the bare remote names alongside
    // the local branches, and neither can be told apart without knowing what
    // the remotes are called.
    let remotes = safe_list_remotes(path).await;

    let (code, stdout, stderr) =
        run_git(&["branch", "-a", "--format=%(refname:short)"], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to list branches: {stderr}")));
    }

    let mut branches = Vec::new();
    for raw in stdout.lines() {
        let line = raw.trim();
        if line.is_empty() || line.ends_with("/HEAD") {
            continue;
        }
        // Skip bare remote names (e.g. "origin" without a branch).
        if remotes.iter().any(|r| r == line) {
            continue;
        }
        branches.push(line.to_string());
    }
    branches.sort();
    Ok(branches)
}

/// Create a new worktree.
///
/// * `new_branch` — create `branch` fresh, optionally stemming from `base_branch`.
/// * otherwise — check out an existing branch, creating a local tracking branch
///   when the given branch is a remote one, to avoid a detached HEAD.
///
/// Returns the branch the worktree has checked out: for a remote branch that
/// is the local tracking branch, so the caller records `develop`, not
/// `origin/develop`.
pub async fn create_worktree(
    repo_path: &str,
    worktree_path: &Path,
    branch: &str,
    new_branch: bool,
    base_branch: Option<&str>,
) -> GitResult<String> {
    let repo = expand(repo_path);
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| GitError(format!("cannot create its parent directory: {e}")))?;
    }
    let wt = worktree_path.to_string_lossy().to_string();
    let remotes = safe_list_remotes(repo_path).await;

    let mut checked_out = branch;
    let (code, stderr) = if new_branch {
        let mut args = vec!["worktree", "add", "-b", branch, wt.as_str()];
        if let Some(base) = base_branch {
            args.push(base);
        }
        let (code, _out, stderr) = run_git(&args, Some(&repo)).await?;

        // A remote base can make git auto-set an upstream (branch.autoSetupMerge).
        // Unset it: new branches should be pushed with `git push -u origin <branch>`.
        if code == 0
            && let Some(base) = base_branch
            && remotes.iter().any(|r| base.starts_with(&format!("{r}/")))
        {
            let _ = run_git(&["branch", "--unset-upstream", branch], Some(worktree_path)).await;
        }
        (code, stderr)
    } else {
        let remote_prefix = remotes
            .iter()
            .find(|r| branch.starts_with(&format!("{r}/")))
            .map(|r| format!("{r}/"));

        if let Some(prefix) = remote_prefix {
            let local_branch = &branch[prefix.len()..];
            checked_out = local_branch;
            let (code, _out, stderr) = run_git(
                &[
                    "worktree",
                    "add",
                    "--track",
                    "-b",
                    local_branch,
                    wt.as_str(),
                    branch,
                ],
                Some(&repo),
            )
            .await?;
            (code, stderr)
        } else {
            let (code, _out, stderr) =
                run_git(&["worktree", "add", wt.as_str(), branch], Some(&repo)).await?;
            (code, stderr)
        }
    };

    if code != 0 {
        return Err(GitError(reason(&stderr)));
    }
    Ok(checked_out.to_string())
}

/// git's reason for refusing, without its `fatal: ` and with the progress
/// lines (`Preparing worktree …`) it writes to the same stream dropped. The
/// caller says what failed; this says why.
fn reason(stderr: &str) -> String {
    let reasons: Vec<&str> = stderr
        .lines()
        .filter_map(|line| {
            line.strip_prefix("fatal: ")
                .or_else(|| line.strip_prefix("error: "))
        })
        .collect();
    if reasons.is_empty() {
        stderr.trim().to_string()
    } else {
        reasons.join("; ")
    }
}

/// Add a worktree on a detached `HEAD`, for something else to check a
/// branch out in — `gh pr checkout`, which knows where a PR's head lives.
pub async fn create_detached_worktree(repo_path: &str, worktree_path: &Path) -> GitResult<()> {
    let repo = expand(repo_path);
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| GitError(format!("cannot create its parent directory: {e}")))?;
    }
    let wt = worktree_path.to_string_lossy().to_string();
    let (code, _out, stderr) =
        run_git(&["worktree", "add", "--detach", wt.as_str()], Some(&repo)).await?;
    if code != 0 {
        return Err(GitError(reason(&stderr)));
    }
    Ok(())
}

/// What a plain (non-forced) removal attempt decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveOutcome {
    Removed,
    /// git refused because the tree holds uncommitted work. The summary is
    /// human-readable, e.g. "3 modified, 2 untracked".
    Dirty(String),
}

/// Remove a worktree without destroying uncommitted work.
///
/// `git worktree remove` refuses a tree with uncommitted changes or untracked
/// files. That refusal is a safety check, so it is surfaced as
/// [`RemoveOutcome::Dirty`] for the caller to confirm rather than papered over
/// with `--force`. `--force` runs only when there is provably nothing to lose:
/// the directory is already gone, or `git status` read the tree and found it
/// clean (a clean tree can still be refused, e.g. when it contains a
/// submodule). A tree whose status *cannot* be read is an error, not a force —
/// resolving that ambiguity in the destructive direction is the exact bug this
/// function exists to prevent. A locked worktree is also an error: git wants
/// `--force --force` for those, and offering "delete anyway" only to fail on
/// the lock would mislead.
pub async fn remove_worktree(repo_path: &str, worktree_path: &str) -> GitResult<RemoveOutcome> {
    let repo = expand(repo_path);
    let wt_dir = expand(worktree_path);
    let wt = wt_dir.to_string_lossy().to_string();

    let (code, _out, _err) = run_git(&["worktree", "remove", wt.as_str()], Some(&repo)).await?;
    if code == 0 {
        return Ok(RemoveOutcome::Removed);
    }

    if worktree_entry(&repo, &wt_dir).await == Some(WorktreeEntry::Locked) {
        return Err(GitError(
            "worktree is locked — unlock it first (git worktree unlock)".into(),
        ));
    }

    // Ask the tree itself instead of parsing stderr, whose wording is not a
    // stable interface across git versions.
    if wt_dir.exists() {
        match tree_state(&wt_dir).await {
            TreeState::Dirty(summary) => return Ok(RemoveOutcome::Dirty(summary)),
            TreeState::Clean => {}
            TreeState::Unreadable(reason) => {
                return Err(GitError(format!(
                    "cannot verify the worktree is clean: {reason}"
                )));
            }
        }
    }
    force_remove_worktree(repo_path, worktree_path).await?;
    Ok(RemoveOutcome::Removed)
}

/// Remove a worktree discarding whatever is in it. Only for after the user has
/// seen what they are about to lose (or there is provably nothing to lose).
pub async fn force_remove_worktree(repo_path: &str, worktree_path: &str) -> GitResult<()> {
    let repo = expand(repo_path);
    let wt_dir = expand(worktree_path);
    let wt = wt_dir.to_string_lossy().to_string();
    let (code, _out, stderr) =
        run_git(&["worktree", "remove", "--force", wt.as_str()], Some(&repo)).await?;
    if code == 0 {
        return Ok(());
    }
    // A pruned or gc'd entry fails even `--force` ("is not a working tree").
    // When git no longer knows the path and the directory is gone there is
    // nothing left to remove — failing here would strand the entry in the
    // sidebar with no way to ever delete it.
    if !wt_dir.exists() && worktree_entry(&repo, &wt_dir).await.is_none() {
        return Ok(());
    }
    Err(GitError(format!("Failed to remove worktree: {stderr}")))
}

/// What would be lost by removing the tree at `dir`: `None` when it is
/// clean, a summary like "2 modified" when it is not. A status git cannot
/// read is an error, never clean.
pub async fn uncommitted_changes(dir: &str) -> GitResult<Option<String>> {
    match tree_state(&expand(dir)).await {
        TreeState::Clean => Ok(None),
        TreeState::Dirty(summary) => Ok(Some(summary)),
        TreeState::Unreadable(reason) => Err(GitError(format!(
            "cannot verify the worktree is clean: {reason}"
        ))),
    }
}

/// How many commits reachable from any of `revs` are on no remote-tracking
/// branch — work that exists only here. Any remote branch counts, so a branch whose
/// own remote branch was deleted after its PR merged is still pushed: its
/// commits are on the default branch.
pub async fn unpushed_commits(dir: &str, revs: &[&str]) -> GitResult<u64> {
    let mut args = vec!["rev-list", "--count"];
    args.extend_from_slice(revs);
    args.extend(["--not", "--remotes"]);
    let (code, stdout, stderr) = run_git(&args, Some(&expand(dir))).await?;
    if code != 0 {
        return Err(GitError(format!(
            "Failed to count unpushed commits: {stderr}"
        )));
    }
    stdout
        .trim()
        .parse()
        .map_err(|_| GitError(format!("Unexpected rev-list output: {stdout}")))
}

/// Delete a local branch whatever its merge state — for after the caller
/// has checked nothing on it is unpushed. A squash-merged branch is never
/// "merged" to git, so `-d` would refuse every one of them.
pub async fn delete_branch(repo_path: &str, branch: &str) -> GitResult<()> {
    let (code, _out, stderr) = run_git(&["branch", "-D", branch], Some(&expand(repo_path))).await?;
    if code != 0 {
        return Err(GitError(stderr));
    }
    Ok(())
}

/// What `git status` said about a tree that refused a plain removal.
enum TreeState {
    Clean,
    /// Summary like "3 modified, 2 untracked".
    Dirty(String),
    /// Status could not be read (dubious ownership, corrupt index, spawn
    /// failure) — the tree cannot be assumed clean.
    Unreadable(String),
}

async fn tree_state(dir: &Path) -> TreeState {
    // `--ignore-submodules=none`: without it a dirty submodule reads as clean,
    // and clean is the destructive direction here. git's own pre-removal
    // cleanliness check passes the same flag.
    let args = ["status", "--porcelain", "--ignore-submodules=none"];
    match run_git(&args, Some(dir)).await {
        Ok((0, stdout, _)) if stdout.is_empty() => TreeState::Clean,
        Ok((0, stdout, _)) => {
            let untracked = stdout.lines().filter(|l| l.starts_with("??")).count();
            let modified = stdout.lines().count() - untracked;
            let mut parts = Vec::new();
            if modified > 0 {
                parts.push(format!("{modified} modified"));
            }
            if untracked > 0 {
                parts.push(format!("{untracked} untracked"));
            }
            TreeState::Dirty(parts.join(", "))
        }
        Ok((_, _, stderr)) => TreeState::Unreadable(stderr),
        Err(error) => TreeState::Unreadable(error.to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorktreeEntry {
    Listed,
    Locked,
}

/// Whether `git worktree list` knows the given directory, and whether it is
/// locked. `None` when git does not list it (or the list itself failed, which
/// callers must treat as "unknown", never as permission to force).
async fn worktree_entry(repo: &Path, wt_dir: &Path) -> Option<WorktreeEntry> {
    let (code, stdout, _err) = run_git(&["worktree", "list", "--porcelain"], Some(repo))
        .await
        .ok()?;
    if code != 0 {
        return None;
    }
    let target = std::fs::canonicalize(wt_dir).unwrap_or_else(|_| wt_dir.to_path_buf());
    for block in stdout.split("\n\n") {
        let mut path: Option<&str> = None;
        let mut locked = false;
        for line in block.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("worktree ") {
                path = Some(rest);
            } else if line == "locked" || line.starts_with("locked ") {
                locked = true;
            }
        }
        if let Some(p) = path {
            let p = Path::new(p);
            let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
            if p == target {
                return Some(if locked {
                    WorktreeEntry::Locked
                } else {
                    WorktreeEntry::Listed
                });
            }
        }
    }
    None
}

pub async fn rename_branch(path: &str, old_name: &str, new_name: &str) -> GitResult<()> {
    let dir = expand(path);
    let (code, _out, stderr) = run_git(&["branch", "-m", old_name, new_name], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to rename branch: {stderr}")));
    }
    Ok(())
}

/// Repair worktree references after the directory was moved.
pub async fn repair_worktree(repo_path: &str, worktree_path: &Path) -> GitResult<()> {
    let repo = expand(repo_path);
    let wt = worktree_path.to_string_lossy().to_string();
    let (code, _out, stderr) = run_git(&["worktree", "repair", wt.as_str()], Some(&repo)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to repair worktree: {stderr}")));
    }
    Ok(())
}

pub async fn list_worktrees(repo_path: &str) -> GitResult<Vec<WorktreeInfo>> {
    let repo = expand(repo_path);
    let (code, stdout, stderr) = run_git(&["worktree", "list", "--porcelain"], Some(&repo)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to list worktrees: {stderr}")));
    }
    Ok(parse_worktree_porcelain(&stdout))
}

/// git's entry for the worktree at `worktree_path`, or `None` when git does
/// not list it or cannot be asked. Asking the directory itself would, in a
/// tree git no longer knows, answer for whatever repository encloses it.
pub async fn listed_worktree(repo_path: &str, worktree_path: &str) -> Option<WorktreeInfo> {
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let target = real(&expand(worktree_path));
    list_worktrees(repo_path)
        .await
        .ok()?
        .into_iter()
        .find(|info| real(Path::new(&info.path)) == target)
}

/// Parse `git worktree list --porcelain` output.
///
/// A bare repository's own entry has no `HEAD` line — just `worktree <path>`
/// and `bare` — and must still be emitted: git puts the main entry first, and
/// a caller that skips the main checkout *by position* would otherwise skip a
/// real worktree in a bare-plus-worktrees layout instead.
pub fn parse_worktree_porcelain(stdout: &str) -> Vec<WorktreeInfo> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_head: Option<String> = None;
    let mut current_branch: Option<String> = None;
    let mut current_bare = false;

    let flush = |worktrees: &mut Vec<WorktreeInfo>,
                 path: &mut Option<String>,
                 head: &mut Option<String>,
                 branch: &mut Option<String>,
                 bare: &mut bool| {
        if let Some(p) = path.take()
            && (head.is_some() || *bare)
        {
            worktrees.push(WorktreeInfo {
                path: p,
                head: head.clone().unwrap_or_default(),
                branch: branch.clone(),
            });
        }
        *path = None;
        *head = None;
        *branch = None;
        *bare = false;
    };

    for raw in stdout.split('\n') {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("worktree ") {
            flush(
                &mut worktrees,
                &mut current_path,
                &mut current_head,
                &mut current_branch,
                &mut current_bare,
            );
            current_path = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            current_head = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            current_branch = Some(rest.replace("refs/heads/", ""));
        } else if line == "bare" {
            current_bare = true;
        } else if line.is_empty() {
            // Entries are blank-line separated; a blank with nothing pending
            // flushes nothing.
            flush(
                &mut worktrees,
                &mut current_path,
                &mut current_head,
                &mut current_branch,
                &mut current_bare,
            );
        }
    }
    flush(
        &mut worktrees,
        &mut current_path,
        &mut current_head,
        &mut current_branch,
        &mut current_bare,
    );
    worktrees
}

/// The branch a remote calls its default, as `<remote>/<branch>`.
///
/// Read from `refs/remotes/<remote>/HEAD`, which a clone records and a remote
/// added by hand only gets from `git remote set-head`. `origin` is asked
/// first, as the remote a clone names; `None` when no remote records one.
pub async fn remote_default_branch(path: &str, remotes: &[String]) -> Option<String> {
    let dir = expand(path);
    let mut ordered: Vec<&String> = remotes.iter().filter(|r| *r == "origin").collect();
    ordered.extend(remotes.iter().filter(|r| *r != "origin"));
    for remote in ordered {
        let head = format!("refs/remotes/{remote}/HEAD");
        let Ok((code, stdout, _)) =
            run_git(&["symbolic-ref", "--quiet", "--short", &head], Some(&dir)).await
        else {
            continue;
        };
        if code == 0 && !stdout.is_empty() {
            return Some(stdout);
        }
    }
    None
}

/// How far a branch has come relative to the default branch it will be
/// merged into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BranchProgress {
    /// The branch tip is contained in `target`.
    pub merged: bool,
    /// The branch has commits beyond `created_from`.
    pub has_commits: bool,
}

/// Where `branch` stands against `target` (e.g. `origin/main`), counting
/// its own commits from `created_from`. `None` when git cannot answer — a
/// branch deleted under the worktree, say — which must not read as unmerged.
///
/// Only local refs are read: no fetch, so `target` is as fresh as the
/// user's last fetch or pull.
pub async fn branch_progress(
    path: &str,
    branch: &str,
    target: &str,
    created_from: &str,
) -> Option<BranchProgress> {
    let dir = expand(path);
    let (code, _, _) = run_git(&["merge-base", "--is-ancestor", branch, target], Some(&dir))
        .await
        .ok()?;
    // 1 is "not an ancestor"; anything else is git failing to answer.
    let merged = match code {
        0 => true,
        1 => false,
        _ => return None,
    };
    let range = format!("{created_from}..{branch}");
    let (code, stdout, _) = run_git(&["rev-list", "--count", &range], Some(&dir))
        .await
        .ok()?;
    if code != 0 {
        return None;
    }
    let count: u64 = stdout.trim().parse().ok()?;
    Some(BranchProgress {
        merged,
        has_commits: count > 0,
    })
}

/// Short commit hash for a ref, or `None` when the ref cannot be resolved.
pub async fn get_ref(path: &str, reference: &str) -> Option<String> {
    let dir = expand(path);
    let (code, stdout, _err) = run_git(&["rev-parse", "--short", reference], Some(&dir))
        .await
        .ok()?;
    if code != 0 || stdout.trim().is_empty() {
        return None;
    }
    Some(stdout.trim().to_string())
}

pub async fn get_latest_commit(path: &str) -> GitResult<CommitInfo> {
    let dir = expand(path);
    let (code, stdout, stderr) = run_git(&["log", "-1", "--format=%H|%h|%ct"], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to get latest commit: {stderr}")));
    }
    let parts: Vec<&str> = stdout.split('|').collect();
    if parts.len() != 3 {
        return Err(GitError("Unexpected git log output format".into()));
    }
    let secs: i64 = parts[2]
        .parse()
        .map_err(|_| GitError("Unexpected git log output format".into()))?;
    let timestamp = Utc
        .timestamp_opt(secs, 0)
        .single()
        .ok_or_else(|| GitError("Unexpected git log timestamp".into()))?;
    // parts[0] is the full hash; the UI only ever shows the short one.
    Ok(CommitInfo {
        short_hash: parts[1].to_string(),
        timestamp,
    })
}

pub async fn fetch(path: &str) -> GitResult<()> {
    let dir = expand(path);
    let (code, _out, stderr) = run_git(&["fetch"], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to fetch: {stderr}")));
    }
    Ok(())
}

pub async fn pull(path: &str) -> GitResult<()> {
    let dir = expand(path);
    let (code, _out, stderr) = run_git(&["pull"], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to pull: {stderr}")));
    }
    Ok(())
}

/// Whether the current branch has a remote tracking branch.
pub async fn has_remote_tracking(path: &str) -> GitResult<bool> {
    let dir = expand(path);
    let (code, stdout, _err) = run_git(
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        Some(&dir),
    )
    .await?;
    Ok(code == 0 && !stdout.trim().is_empty())
}

#[cfg(test)]
mod tests;
