use super::*;
use crate::models::{Repository, Worktree};
use std::path::Path;

fn git_in(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    std::process::Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(repo)
        .output()
        .unwrap()
        .status
        .success()
}

/// A clone of an upstream with one worktree on `branch`, its commit pushed.
struct Fixture {
    _dir: tempfile::TempDir,
    clone: std::path::PathBuf,
    worktree: std::path::PathBuf,
}

impl Fixture {
    fn new(branch: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let upstream = dir.path().join("upstream");
        std::fs::create_dir_all(&upstream).unwrap();
        git_in(&upstream, &["init", "-q", "-b", "main"]);
        git_in(&upstream, &["config", "user.email", "t@example.com"]);
        git_in(&upstream, &["config", "user.name", "t"]);
        git_in(&upstream, &["commit", "-q", "--allow-empty", "-m", "init"]);
        git_in(
            dir.path(),
            &["clone", "-q", &upstream.to_string_lossy(), "clone"],
        );
        let clone = dir.path().join("clone");
        git_in(&clone, &["config", "user.email", "t@example.com"]);
        git_in(&clone, &["config", "user.name", "t"]);
        let worktree = dir.path().join("wt");
        git_in(
            &clone,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                &worktree.to_string_lossy(),
            ],
        );
        git_in(&worktree, &["commit", "-q", "--allow-empty", "-m", "work"]);
        git_in(&worktree, &["push", "-q", "origin", branch]);
        Self {
            _dir: dir,
            clone,
            worktree,
        }
    }

    async fn prune(&self, branch: &str) -> Outcome {
        prune_worktree(
            &self.clone.to_string_lossy(),
            &self.worktree.to_string_lossy(),
            branch,
        )
        .await
    }
}

#[tokio::test]
async fn a_clean_pushed_worktree_is_removed_with_its_branch() {
    let fixture = Fixture::new("fix/done");
    assert_eq!(
        fixture.prune("fix/done").await,
        Outcome::Removed { branch_kept: None }
    );
    assert!(!fixture.worktree.exists());
    assert!(!branch_exists(&fixture.clone, "fix/done"));
}

/// Uncommitted work is never touched, untracked files included.
#[tokio::test]
async fn a_worktree_with_uncommitted_changes_is_skipped() {
    let fixture = Fixture::new("fix/dirty");
    std::fs::write(fixture.worktree.join("scratch.txt"), "unsaved").unwrap();
    assert_eq!(
        fixture.prune("fix/dirty").await,
        Outcome::Skipped(Skip::Uncommitted("1 untracked".into()))
    );
    assert!(fixture.worktree.join("scratch.txt").exists());
    assert!(branch_exists(&fixture.clone, "fix/dirty"));
}

#[tokio::test]
async fn a_worktree_with_unpushed_commits_is_skipped() {
    let fixture = Fixture::new("fix/ahead");
    git_in(
        &fixture.worktree,
        &["commit", "-q", "--allow-empty", "-m", "local only"],
    );
    git_in(
        &fixture.worktree,
        &["commit", "-q", "--allow-empty", "-m", "local too"],
    );
    assert_eq!(
        fixture.prune("fix/ahead").await,
        Outcome::Skipped(Skip::Unpushed(2))
    );
    assert!(fixture.worktree.exists());
    assert!(branch_exists(&fixture.clone, "fix/ahead"));
}

/// A merged PR whose remote branch was deleted (and pruned locally) still
/// has its commits on the default branch: nothing is lost by removing it.
#[tokio::test]
async fn commits_on_any_remote_branch_count_as_pushed() {
    let fixture = Fixture::new("fix/merged");
    let upstream = fixture.clone.with_file_name("upstream");
    git_in(&upstream, &["merge", "-q", "--ff-only", "fix/merged"]);
    git_in(&upstream, &["branch", "-q", "-D", "fix/merged"]);
    git_in(&fixture.clone, &["fetch", "-q", "--prune", "origin"]);
    assert_eq!(
        fixture.prune("fix/merged").await,
        Outcome::Removed { branch_kept: None }
    );
}

/// The worktree is gone either way; a branch git would not delete is
/// reported, not a reason to keep the worktree.
#[tokio::test]
async fn a_branch_that_cannot_be_deleted_is_reported() {
    let fixture = Fixture::new("fix/held");
    // The branch is also checked out in the main clone, so git refuses.
    git_in(&fixture.worktree, &["switch", "-q", "--detach"]);
    git_in(&fixture.clone, &["switch", "-q", "fix/held"]);
    let outcome = fixture.prune("fix/held").await;
    assert!(
        matches!(
            outcome,
            Outcome::Removed {
                branch_kept: Some(_)
            }
        ),
        "got {outcome:?}"
    );
    assert!(!fixture.worktree.exists());
}

fn named(name: &str, outcome: Outcome) -> (String, Outcome) {
    (name.to_string(), outcome)
}

#[test]
fn the_report_names_every_skip_with_its_reason() {
    let report = report(&[
        named("a", Outcome::Removed { branch_kept: None }),
        named(
            "b",
            Outcome::Skipped(Skip::Uncommitted("2 modified".into())),
        ),
        named("c", Outcome::Skipped(Skip::Unpushed(1))),
        named("d", Outcome::Skipped(Skip::Busy)),
    ]);
    assert_eq!(
        report.lines,
        vec![
            "Pruned 'a'.".to_string(),
            "Skipped 'b' (uncommitted changes: 2 modified), 'c' (1 unpushed commit), \
             'd' (still being changed)."
                .to_string(),
        ]
    );
    assert_eq!(report.severity, Severity::Warning);
}

#[test]
fn a_clean_run_is_information() {
    let report = report(&[
        named("a", Outcome::Removed { branch_kept: None }),
        named("b", Outcome::Removed { branch_kept: None }),
    ]);
    assert_eq!(report.lines, vec!["Pruned 2 worktrees.".to_string()]);
    assert_eq!(report.severity, Severity::Information);
}

#[test]
fn a_failure_is_an_error_and_says_why() {
    let report = report(&[
        named("a", Outcome::Failed("worktree is locked".into())),
        named(
            "b",
            Outcome::Removed {
                branch_kept: Some("fix/b: checked out elsewhere".into()),
            },
        ),
    ]);
    assert_eq!(
        report.lines,
        vec![
            "Pruned 'b'.".to_string(),
            "Kept the branch of 'b' (fix/b: checked out elsewhere).".to_string(),
            "Could not prune 'a': worktree is locked.".to_string(),
        ]
    );
    assert_eq!(report.severity, Severity::Error);
}

#[test]
fn nothing_pruned_says_so() {
    let report = report(&[named("b", Outcome::Skipped(Skip::Unpushed(3)))]);
    assert_eq!(
        report.lines,
        vec![
            "Pruned nothing.".to_string(),
            "Skipped 'b' (3 unpushed commits).".to_string(),
        ]
    );
}

/// Without a remote nothing was ever pushed; a count would include the
/// default branch's whole history, so the reason says so instead.
#[tokio::test]
async fn a_repository_without_a_remote_is_skipped_as_such() {
    let fixture = Fixture::new("fix/local");
    git_in(&fixture.clone, &["remote", "remove", "origin"]);
    assert_eq!(
        fixture.prune("fix/local").await,
        Outcome::Skipped(Skip::NoRemote)
    );
    assert!(fixture.worktree.exists());
}

/// The command line's prune over a tracked repository: what git removed
/// leaves the state file, what was skipped stays in it.
#[tokio::test]
async fn pruning_tracked_worktrees_drops_only_the_removed_from_the_state_file() {
    let fixture = Fixture::new("fix/done");
    let dirty = fixture.clone.with_file_name("dirty");
    git_in(
        &fixture.clone,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "fix/dirty",
            &dirty.to_string_lossy(),
        ],
    );
    std::fs::write(dirty.join("scratch.txt"), "unsaved").unwrap();

    let config = fixture.clone.with_file_name(".forestui-config.json");
    let mut state = AppState::load_from(config.clone());
    let mut repo = Repository::new("clone".into(), fixture.clone.to_string_lossy().into());
    let done = Worktree::new(
        "done".into(),
        "fix/done".into(),
        fixture.worktree.to_string_lossy().into(),
    );
    let kept = Worktree::new(
        "dirty".into(),
        "fix/dirty".into(),
        dirty.to_string_lossy().into(),
    );
    let (done_id, kept_id) = (done.id, kept.id);
    repo.worktrees = vec![done, kept];
    let repo_id = repo.id;
    state.add_repository(repo);

    let outcomes = prune_tracked(&mut state, &[done_id, kept_id]).await;

    assert_eq!(
        outcomes,
        vec![
            named("done", Outcome::Removed { branch_kept: None }),
            named(
                "dirty",
                Outcome::Skipped(Skip::Uncommitted("1 untracked".into()))
            ),
        ]
    );
    assert_eq!(state.take_save_error(), None);
    let ids = |state: &AppState| -> Vec<uuid::Uuid> {
        state
            .find_repository(repo_id)
            .unwrap()
            .worktrees
            .iter()
            .map(|w| w.id)
            .collect()
    };
    assert_eq!(ids(&state), vec![kept_id]);
    assert_eq!(
        ids(&AppState::load_from(config)),
        vec![kept_id],
        "written to disk"
    );
}

fn repository(name: &str, outcomes: Vec<(String, Outcome)>) -> RepositoryPrune {
    RepositoryPrune {
        name: name.to_string(),
        outcomes,
    }
}

/// Skips are the routine doing its job; only a real failure fails the run.
#[test]
fn the_command_line_fails_only_on_a_failure() {
    let skipped = summary(&[repository(
        "app",
        vec![
            named("a", Outcome::Removed { branch_kept: None }),
            named("b", Outcome::Skipped(Skip::Unpushed(1))),
        ],
    )]);
    assert_eq!(
        skipped.lines,
        vec![
            "app: Pruned 'a'.".to_string(),
            "app: Skipped 'b' (1 unpushed commit).".to_string(),
        ]
    );
    assert!(!skipped.failed);

    let failed = summary(&[
        repository("app", vec![named("a", Outcome::Failed("locked".into()))]),
        repository("lib", Vec::new()),
    ]);
    assert!(failed.failed);
    assert_eq!(
        failed.lines,
        vec![
            "app: Pruned nothing.".to_string(),
            "app: Could not prune 'a': locked.".to_string(),
        ],
        "a repository with nothing finished says nothing"
    );
}

#[test]
fn a_forest_with_nothing_finished_says_so() {
    let quiet = summary(&[repository("app", Vec::new())]);
    assert_eq!(quiet.lines, vec!["No finished worktrees.".to_string()]);
    assert!(!quiet.failed);
}

/// A Delete from the App: the worktree is already gone, and its branch
/// follows only when nothing on it would be lost.
#[tokio::test]
async fn a_deleted_worktrees_pushed_branch_is_deleted() {
    let fixture = Fixture::new("fix/again");
    git_in(
        &fixture.clone,
        &["worktree", "remove", &fixture.worktree.to_string_lossy()],
    );
    assert_eq!(
        delete_branch_if_pushed(&fixture.clone.to_string_lossy(), "fix/again", None).await,
        Some(BranchFate::Deleted("fix/again".into()))
    );
    assert!(!branch_exists(&fixture.clone, "fix/again"));
}

#[tokio::test]
async fn a_deleted_worktrees_unpushed_branch_is_kept_with_the_count() {
    let fixture = Fixture::new("fix/ahead");
    git_in(
        &fixture.worktree,
        &["commit", "-q", "--allow-empty", "-m", "local"],
    );
    git_in(
        &fixture.worktree,
        &["commit", "-q", "--allow-empty", "-m", "local"],
    );
    git_in(
        &fixture.clone,
        &["worktree", "remove", &fixture.worktree.to_string_lossy()],
    );
    assert_eq!(
        delete_branch_if_pushed(&fixture.clone.to_string_lossy(), "fix/ahead", None).await,
        Some(BranchFate::Kept {
            branch: "fix/ahead".into(),
            why: "2 unpushed commits".into(),
        })
    );
    assert!(branch_exists(&fixture.clone, "fix/ahead"));
}

#[tokio::test]
async fn without_a_remote_a_deleted_worktrees_branch_is_kept() {
    let fixture = Fixture::new("fix/local");
    git_in(
        &fixture.clone,
        &["worktree", "remove", &fixture.worktree.to_string_lossy()],
    );
    git_in(&fixture.clone, &["remote", "remove", "origin"]);
    assert_eq!(
        delete_branch_if_pushed(&fixture.clone.to_string_lossy(), "fix/local", None).await,
        Some(BranchFate::Kept {
            branch: "fix/local".into(),
            why: "no remote to push to".into(),
        })
    );
    assert!(branch_exists(&fixture.clone, "fix/local"));
}

/// Checked out elsewhere: git refuses, and its reason is what is reported.
#[tokio::test]
async fn a_branch_git_will_not_delete_is_kept_with_gits_reason() {
    let fixture = Fixture::new("fix/held");
    git_in(
        &fixture.clone,
        &["worktree", "remove", &fixture.worktree.to_string_lossy()],
    );
    git_in(&fixture.clone, &["switch", "-q", "fix/held"]);
    let fate = delete_branch_if_pushed(&fixture.clone.to_string_lossy(), "fix/held", None).await;
    assert!(
        matches!(&fate, Some(BranchFate::Kept { branch, why }) if branch == "fix/held" && !why.is_empty()),
        "got {fate:?}"
    );
    assert!(branch_exists(&fixture.clone, "fix/held"));
}

#[tokio::test]
async fn a_branch_that_is_already_gone_has_no_fate() {
    let fixture = Fixture::new("fix/gone");
    assert_eq!(
        delete_branch_if_pushed(&fixture.clone.to_string_lossy(), "fix/elsewhere", None).await,
        None
    );
    assert_eq!(
        delete_branch_if_pushed(&fixture.clone.to_string_lossy(), "", None).await,
        None
    );
    // The state's word for a detached worktree.
    assert_eq!(
        delete_branch_if_pushed(&fixture.clone.to_string_lossy(), "HEAD", None).await,
        None
    );
}

/// A worktree made on the default branch hands that branch back, it does not
/// take it: deleting `main` would be surprising, even fully pushed.
#[tokio::test]
async fn the_default_branch_is_kept() {
    let fixture = Fixture::new("fix/any");
    git_in(&fixture.clone, &["switch", "-q", "--detach"]);
    let clone = fixture.clone.to_string_lossy().to_string();
    let kept = Some(BranchFate::Kept {
        branch: "main".into(),
        why: "the default branch".into(),
    });
    assert_eq!(delete_branch_if_pushed(&clone, "main", None).await, kept);
    // With no default recorded for the remote, the usual names are spared.
    git_in(&fixture.clone, &["remote", "set-head", "origin", "-d"]);
    assert_eq!(delete_branch_if_pushed(&clone, "main", None).await, kept);
    assert!(branch_exists(&fixture.clone, "main"));
}

/// The base branch the repository settings name is handed back like the
/// default branch, whichever way it is written.
#[tokio::test]
async fn the_base_branch_is_kept() {
    let fixture = Fixture::new("develop");
    git_in(
        &fixture.clone,
        &["worktree", "remove", &fixture.worktree.to_string_lossy()],
    );
    let clone = fixture.clone.to_string_lossy().to_string();
    let kept = Some(BranchFate::Kept {
        branch: "develop".into(),
        why: "the base branch".into(),
    });
    assert_eq!(
        delete_branch_if_pushed(&clone, "develop", Some("origin/develop")).await,
        kept
    );
    assert_eq!(
        delete_branch_if_pushed(&clone, "develop", Some("develop")).await,
        kept
    );
    assert!(branch_exists(&fixture.clone, "develop"));
    // Another base keeps nothing of this branch's.
    assert_eq!(
        delete_branch_if_pushed(&clone, "develop", Some("origin/main")).await,
        Some(BranchFate::Deleted("develop".into()))
    );
}

/// A branch that is both is named for the default branch, the stronger
/// reason.
#[tokio::test]
async fn a_base_branch_that_is_the_default_branch_is_kept_as_the_default() {
    let fixture = Fixture::new("fix/any");
    git_in(&fixture.clone, &["switch", "-q", "--detach"]);
    let clone = fixture.clone.to_string_lossy().to_string();
    assert_eq!(
        delete_branch_if_pushed(&clone, "main", Some("origin/main")).await,
        Some(BranchFate::Kept {
            branch: "main".into(),
            why: "the default branch".into(),
        })
    );
}

#[test]
fn a_base_branch_is_matched_by_its_local_name() {
    let remotes = ["origin".to_string(), "upstream".to_string()];
    assert_eq!(local_name("origin/feat/x", &remotes), "feat/x");
    assert_eq!(local_name("upstream/develop", &remotes), "develop");
    assert_eq!(local_name("develop", &remotes), "develop");
    // Only a known remote is a prefix: these are local names.
    assert_eq!(local_name("feature/develop", &remotes), "feature/develop");
    assert_eq!(local_name("originx/develop", &remotes), "originx/develop");
}
