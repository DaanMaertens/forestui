use super::*;
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
