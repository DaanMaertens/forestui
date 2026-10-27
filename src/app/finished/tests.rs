use super::*;

/// Selection changes rescan far faster than `gh` answers; one check per
/// repository runs at a time, and a new one waits out [`MIN_INTERVAL`].
#[test]
fn checks_are_one_at_a_time_and_spaced_out() {
    let mut finished = FinishedWorktrees::default();
    let repo = Uuid::new_v4();
    let now = Instant::now();

    let ticket = finished.start(repo, now).expect("first check");
    assert!(
        finished.start(repo, now + MIN_INTERVAL).is_none(),
        "one running"
    );
    finished.release(ticket);
    assert!(finished.start(repo, now + Duration::from_secs(1)).is_none());
    assert!(finished.start(repo, now + MIN_INTERVAL).is_some());
    assert!(
        finished.start(Uuid::new_v4(), now).is_some(),
        "per repository"
    );
}

fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn session(branch: &str, created_from: &str, base: Option<&str>) -> LinkedWorktree {
    LinkedWorktree {
        id: Uuid::new_v4(),
        link: WorkItemLink::Session {
            prompt: "tidy up".into(),
        },
        branch: branch.into(),
        created_from: Some(created_from.into()),
        base: base.map(str::to_string),
        parent_issue: None,
    }
}

/// Work started from a base branch is done once it is merged into that
/// branch, not into the remote's default. A worktree from before the base
/// was recorded is judged against the default, as it always was.
#[tokio::test]
async fn a_session_is_finished_once_merged_into_its_base_branch() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("upstream");
    std::fs::create_dir_all(&upstream).unwrap();
    git_in(&upstream, &["init", "-q", "-b", "main"]);
    git_in(&upstream, &["config", "user.email", "t@example.com"]);
    git_in(&upstream, &["config", "user.name", "t"]);
    git_in(&upstream, &["commit", "-q", "--allow-empty", "-m", "init"]);
    git_in(&upstream, &["branch", "develop"]);
    git_in(
        dir.path(),
        &["clone", "-q", &upstream.to_string_lossy(), "clone"],
    );
    let clone = dir.path().join("clone");
    git_in(&clone, &["config", "user.email", "t@example.com"]);
    git_in(&clone, &["config", "user.name", "t"]);

    let created_from = git_in(&clone, &["rev-parse", "origin/develop"]);
    git_in(
        &clone,
        &["switch", "-q", "-c", "feat/tidy", "origin/develop"],
    );
    git_in(&clone, &["commit", "-q", "--allow-empty", "-m", "work"]);
    git_in(&clone, &["push", "-q", "origin", "feat/tidy:develop"]);
    git_in(&clone, &["fetch", "-q"]);

    let on_base = session("feat/tidy", &created_from, Some("origin/develop"));
    let unrecorded = session("feat/tidy", &created_from, None);
    let found = check(
        &clone.to_string_lossy(),
        &[on_base.clone(), unrecorded.clone()],
    )
    .await;
    assert_eq!(found.get(&on_base.id), Some(&true));
    assert_eq!(found.get(&unrecorded.id), Some(&false));
}
