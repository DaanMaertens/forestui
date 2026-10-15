use super::*;

/// A stale worktree (deleted directory) must surface as GitError, not a panic.
#[tokio::test]
async fn missing_cwd_raises_git_error() {
    assert!(
        get_latest_commit("/nonexistent/stale/worktree")
            .await
            .is_err()
    );
    assert!(
        has_remote_tracking("/nonexistent/stale/worktree")
            .await
            .is_err()
    );
    assert!(
        get_current_branch("/nonexistent/stale/worktree")
            .await
            .is_err()
    );
}

/// A shell that SIGKILLs itself is the cheapest real signal-killed status.
fn killed_by_signal() -> std::process::ExitStatus {
    let status = std::process::Command::new("sh")
        .args(["-c", "kill -9 $$"])
        .status()
        .unwrap();
    assert!(status.code().is_none(), "expected a signal-killed process");
    status
}

/// A killed `git pull` must not report success.
#[test]
fn signal_killed_process_is_not_success() {
    assert_ne!(exit_code(killed_by_signal()), 0);
}

#[test]
fn normal_exit_codes_pass_through() {
    let ok = std::process::Command::new("sh")
        .args(["-c", "exit 3"])
        .status()
        .unwrap();
    assert_eq!(exit_code(ok), 3);
}

#[test]
fn porcelain_parsing() {
    let out = "worktree /repo\nHEAD abc123\nbranch refs/heads/main\n\
                   \nworktree /repo/wt\nHEAD def456\nbranch refs/heads/feat/x\n\
                   \nworktree /repo/detached\nHEAD 000111\ndetached\n";
    let parsed = parse_worktree_porcelain(out);
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0].path, "/repo");
    assert_eq!(parsed[0].branch.as_deref(), Some("main"));
    assert_eq!(parsed[1].branch.as_deref(), Some("feat/x"));
    assert_eq!(parsed[2].branch, None);
    assert_eq!(parsed[2].head, "000111");
}

/// A bare repository's entry has no HEAD line and must still come out
/// first — consumers skip the main checkout by position, and dropping the
/// bare entry would make them skip a real worktree instead.
#[test]
fn porcelain_parsing_keeps_the_bare_entry() {
    let out = "worktree /srv/repo.git\nbare\n\
                   \nworktree /srv/wt-one\nHEAD abc123\nbranch refs/heads/main\n";
    let parsed = parse_worktree_porcelain(out);
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].path, "/srv/repo.git");
    assert_eq!(parsed[0].branch, None);
    assert_eq!(parsed[1].path, "/srv/wt-one");
    assert_eq!(parsed[1].branch.as_deref(), Some("main"));
}

#[tokio::test]
async fn real_repo_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo_str = repo.to_string_lossy().to_string();

    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "t@example.com"],
        vec!["config", "user.name", "t"],
        vec!["commit", "--allow-empty", "-m", "init"],
    ] {
        let out = std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
    }

    assert!(list_worktrees(&repo_str).await.is_ok());
    assert_eq!(get_current_branch(&repo_str).await.unwrap(), "main");
    assert!(
        list_branches(&repo_str)
            .await
            .unwrap()
            .contains(&"main".to_string())
    );
    assert!(get_latest_commit(&repo_str).await.is_ok());
    assert!(!has_remote_tracking(&repo_str).await.unwrap());
    assert!(list_remotes(&repo_str).await.unwrap().is_empty());

    let wt = dir.path().join("forest").join("wt1");
    create_worktree(&repo_str, &wt, "feat/x", true, None)
        .await
        .unwrap();
    assert!(wt.exists());
    assert!(
        list_branches(&repo_str)
            .await
            .unwrap()
            .contains(&"feat/x".to_string())
    );

    let listed = list_worktrees(&repo_str).await.unwrap();
    assert_eq!(listed.len(), 2);

    let outcome = remove_worktree(&repo_str, &wt.to_string_lossy())
        .await
        .unwrap();
    assert_eq!(outcome, RemoveOutcome::Removed);
    assert!(!wt.exists());
}

#[tokio::test]
async fn a_dirty_worktree_is_refused_with_a_summary_not_destroyed() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    init_repo(&repo);
    let repo_str = repo.to_string_lossy().to_string();

    let wt = dir.path().join("wt-dirty");
    create_worktree(&repo_str, &wt, "feat/dirty", true, None)
        .await
        .unwrap();
    std::fs::write(wt.join("tracked.txt"), "committed").unwrap();
    git_in(&wt, &["add", "tracked.txt"]);
    git_in(&wt, &["commit", "-m", "add tracked"]);
    std::fs::write(wt.join("tracked.txt"), "edited but not committed").unwrap();
    std::fs::write(wt.join("scratch.txt"), "untracked").unwrap();

    let outcome = remove_worktree(&repo_str, &wt.to_string_lossy())
        .await
        .unwrap();
    assert_eq!(
        outcome,
        RemoveOutcome::Refused(Refusal {
            changes: Changes::Uncommitted("1 modified, 1 untracked".into()),
            lock: None,
        })
    );
    // The refusal left the work in place.
    assert!(wt.join("scratch.txt").exists());

    force_remove_worktree(&repo_str, &wt.to_string_lossy(), Force::Changes)
        .await
        .unwrap();
    assert!(!wt.exists());
}

#[tokio::test]
async fn a_locked_worktree_is_refused_with_its_lock_and_its_changes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    init_repo(&repo);
    let repo_str = repo.to_string_lossy().to_string();

    let wt = dir.path().join("wt-locked");
    let wt_str = wt.to_string_lossy().to_string();
    create_worktree(&repo_str, &wt, "feat/locked", true, None)
        .await
        .unwrap();
    git_in(&repo, &["worktree", "lock", "--reason", "in use", &wt_str]);
    std::fs::write(wt.join("scratch.txt"), "untracked").unwrap();

    let outcome = remove_worktree(&repo_str, &wt_str).await.unwrap();
    let RemoveOutcome::Refused(refusal) = outcome else {
        panic!("a locked tree must be refused, got {outcome:?}");
    };
    assert_eq!(
        refusal,
        Refusal {
            changes: Changes::Uncommitted("1 untracked".into()),
            lock: Some(Lock {
                reason: Some("in use".into()),
            }),
        }
    );
    assert_eq!(refusal.force(), Force::ChangesAndLock);

    // Forcing past the changes alone stops at the lock.
    assert!(
        force_remove_worktree(&repo_str, &wt_str, Force::Changes)
            .await
            .is_err()
    );
    assert!(wt.join("scratch.txt").exists());

    force_remove_worktree(&repo_str, &wt_str, refusal.force())
        .await
        .unwrap();
    assert!(!wt.exists());
}

#[tokio::test]
async fn a_clean_locked_worktree_is_refused_for_its_lock_alone() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    init_repo(&repo);
    let repo_str = repo.to_string_lossy().to_string();

    let wt = dir.path().join("wt-locked");
    let wt_str = wt.to_string_lossy().to_string();
    create_worktree(&repo_str, &wt, "feat/locked", true, None)
        .await
        .unwrap();
    git_in(&repo, &["worktree", "lock", &wt_str]);

    let outcome = remove_worktree(&repo_str, &wt_str).await.unwrap();
    assert_eq!(
        outcome,
        RemoveOutcome::Refused(Refusal {
            changes: Changes::None,
            lock: Some(Lock { reason: None }),
        })
    );
    assert!(wt.exists());
}

#[tokio::test]
async fn a_worktree_whose_status_cannot_be_read_is_refused_not_forced() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    init_repo(&repo);
    let repo_str = repo.to_string_lossy().to_string();

    let wt = dir.path().join("wt-corrupt");
    let wt_str = wt.to_string_lossy().to_string();
    create_worktree(&repo_str, &wt, "feat/corrupt", true, None)
        .await
        .unwrap();
    std::fs::write(wt.join("scratch.txt"), "untracked").unwrap();
    // A corrupt index makes both git's own clean check and `git status` fail.
    std::fs::write(repo.join(".git/worktrees/wt-corrupt/index"), "garbage").unwrap();

    let outcome = remove_worktree(&repo_str, &wt_str).await.unwrap();
    let RemoveOutcome::Refused(refusal) = outcome else {
        panic!("an unreadable tree must be refused, got {outcome:?}");
    };
    assert!(
        matches!(&refusal.changes, Changes::Unknown(why) if why.contains("index")),
        "got {refusal:?}"
    );
    assert_eq!(refusal.lock, None);
    assert!(wt.join("scratch.txt").exists());

    force_remove_worktree(&repo_str, &wt_str, refusal.force())
        .await
        .unwrap();
    assert!(!wt.exists());
}

#[tokio::test]
async fn a_worktree_with_a_deleted_directory_is_removed_without_a_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    init_repo(&repo);
    let repo_str = repo.to_string_lossy().to_string();

    let wt = dir.path().join("wt-gone");
    create_worktree(&repo_str, &wt, "feat/gone", true, None)
        .await
        .unwrap();
    std::fs::remove_dir_all(&wt).unwrap();

    // A stale entry must not read as "dirty" — it force-removes silently.
    let outcome = remove_worktree(&repo_str, &wt.to_string_lossy())
        .await
        .unwrap();
    assert_eq!(outcome, RemoveOutcome::Removed);
    assert_eq!(list_worktrees(&repo_str).await.unwrap().len(), 1);
}

/// Run git in `dir`, panicking with the argv on failure. Tests only.
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

fn init_repo(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git_in(path, &["init", "-b", "main"]);
    git_in(path, &["config", "user.email", "t@example.com"]);
    git_in(path, &["config", "user.name", "t"]);
    git_in(path, &["commit", "--allow-empty", "-m", "init"]);
}

/// `git branch -a` returns more than branches: the bare remote names and
/// `origin/HEAD` come back too, and neither is something to check out.
#[tokio::test]
async fn list_branches_keeps_remote_branches_but_not_remote_names() {
    let dir = tempfile::tempdir().unwrap();

    let upstream = dir.path().join("upstream");
    init_repo(&upstream);
    git_in(&upstream, &["branch", "feat/remote-only"]);

    let clone = dir.path().join("clone");
    init_repo(&clone);
    git_in(
        &clone,
        &["remote", "add", "origin", &upstream.to_string_lossy()],
    );
    git_in(&clone, &["fetch", "--no-tags", "origin"]);
    let clone_str = clone.to_string_lossy().to_string();

    let all = list_branches(&clone_str).await.unwrap();
    assert!(all.contains(&"main".to_string()));
    assert!(all.contains(&"origin/main".to_string()));
    assert!(all.contains(&"origin/feat/remote-only".to_string()));
    // The bare remote name is not a branch, and neither is its HEAD.
    assert!(!all.contains(&"origin".to_string()));
    assert!(!all.iter().any(|b| b.ends_with("/HEAD")));
}

/// Checking out a remote branch makes a local tracking branch, and that local
/// name is the one the worktree records — not `origin/develop`.
#[tokio::test]
async fn a_remote_branch_is_checked_out_under_its_local_name() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("upstream");
    init_repo(&upstream);
    git_in(&upstream, &["branch", "develop"]);
    git_in(
        dir.path(),
        &["clone", "-q", &upstream.to_string_lossy(), "clone"],
    );
    let clone_str = dir.path().join("clone").to_string_lossy().to_string();

    let wt = dir.path().join("forest").join("dev");
    let branch = create_worktree(&clone_str, &wt, "origin/develop", false, None)
        .await
        .unwrap();
    assert_eq!(branch, "develop");
    assert_eq!(
        get_current_branch(&wt.to_string_lossy()).await.unwrap(),
        "develop"
    );

    let wt = dir.path().join("forest").join("fresh");
    let branch = create_worktree(&clone_str, &wt, "feat/new", true, Some("origin/develop"))
        .await
        .unwrap();
    assert_eq!(branch, "feat/new");
}

/// A session's branch, from creation to merged upstream.
#[tokio::test]
async fn branch_progress_follows_a_branch_into_the_default_branch() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("upstream");
    init_repo(&upstream);
    let clone = dir.path().join("clone");
    git_in(
        dir.path(),
        &["clone", "-q", &upstream.to_string_lossy(), "clone"],
    );
    git_in(&clone, &["config", "user.email", "t@example.com"]);
    git_in(&clone, &["config", "user.name", "t"]);
    let clone_str = clone.to_string_lossy().to_string();
    let base = get_ref(&clone_str, "origin/main").await.unwrap();

    git_in(&clone, &["branch", "feat/thing", "origin/main"]);
    let fresh = branch_progress(&clone_str, "feat/thing", "origin/main", &base).await;
    assert_eq!(
        fresh,
        Some(BranchProgress {
            merged: true,
            has_commits: false
        })
    );

    git_in(&clone, &["switch", "-q", "feat/thing"]);
    git_in(&clone, &["commit", "-q", "--allow-empty", "-m", "work"]);
    let working = branch_progress(&clone_str, "feat/thing", "origin/main", &base).await;
    assert_eq!(
        working,
        Some(BranchProgress {
            merged: false,
            has_commits: true
        })
    );

    git_in(&clone, &["push", "-q", "origin", "feat/thing"]);
    git_in(&upstream, &["merge", "-q", "--ff-only", "feat/thing"]);
    git_in(&clone, &["fetch", "-q", "origin"]);
    let merged = branch_progress(&clone_str, "feat/thing", "origin/main", &base).await;
    assert_eq!(
        merged,
        Some(BranchProgress {
            merged: true,
            has_commits: true
        })
    );
}

/// A branch git cannot resolve is unknown, not "unmerged".
#[tokio::test]
async fn branch_progress_of_a_missing_branch_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    init_repo(dir.path());
    let repo = dir.path().to_string_lossy().to_string();
    let head = get_ref(&repo, "HEAD").await.unwrap();
    assert_eq!(branch_progress(&repo, "gone", "main", &head).await, None);
}

/// git's own record of a worktree, found by path — not the directory's
/// `HEAD`, which in a broken tree would be an enclosing repository's.
#[tokio::test]
async fn a_listed_worktree_is_found_by_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo_str = repo.to_string_lossy().to_string();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "t@example.com"],
        vec!["config", "user.name", "t"],
        vec!["commit", "--allow-empty", "-m", "init"],
    ] {
        let out = std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
    }
    let wt = dir.path().join("forest").join("wt1");
    create_worktree(&repo_str, &wt, "feat/x", true, None)
        .await
        .unwrap();
    let wt_str = wt.to_string_lossy().to_string();

    let found = listed_worktree(&repo_str, &wt_str).await.expect("listed");
    assert_eq!(found.branch.as_deref(), Some("feat/x"));

    let out = std::process::Command::new("git")
        .args(["switch", "--detach"])
        .current_dir(&wt)
        .output()
        .unwrap();
    assert!(out.status.success());
    let found = listed_worktree(&repo_str, &wt_str).await.expect("listed");
    assert_eq!(found.branch, None, "a detached worktree has no branch");

    let elsewhere = dir.path().join("forest").join("never-made");
    assert_eq!(
        listed_worktree(&repo_str, &elsewhere.to_string_lossy()).await,
        None
    );
}

/// The App puts "Failed to create worktree:" in front; the error itself is
/// git's reason alone, so the notification does not say it twice.
#[tokio::test]
async fn a_create_that_git_refuses_reports_gits_reason_alone() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo_str = repo.to_string_lossy().to_string();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "t@example.com"],
        vec!["config", "user.name", "t"],
        vec!["commit", "--allow-empty", "-m", "init"],
        vec!["branch", "fix/taken"],
    ] {
        let out = std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
    }
    let wt = dir.path().join("forest").join("wt1");
    let error = create_worktree(&repo_str, &wt, "fix/taken", true, None)
        .await
        .expect_err("the branch exists");
    assert_eq!(
        error.to_string(),
        "a branch named 'fix/taken' already exists"
    );
}
