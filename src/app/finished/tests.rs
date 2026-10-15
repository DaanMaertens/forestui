use super::*;

fn ids(list: &[Uuid]) -> HashSet<Uuid> {
    list.iter().copied().collect()
}

fn verdicts(list: &[(Uuid, bool)]) -> HashMap<Uuid, bool> {
    list.iter().copied().collect()
}

#[test]
fn a_check_marks_the_worktrees_it_found_finished() {
    let mut finished = FinishedWorktrees::default();
    let repo = Uuid::new_v4();
    let (done, open) = (Uuid::new_v4(), Uuid::new_v4());

    let ticket = finished.start(repo, Instant::now()).expect("none running");
    let changed = finished.fold(
        ticket,
        &verdicts(&[(done, true), (open, false)]),
        Some(&ids(&[done, open])),
    );

    assert!(changed);
    assert!(finished.contains(done));
    assert!(!finished.contains(open));
}

/// The sidebar repaints off this answer, so the common sweep that finds
/// nothing new must say so.
#[test]
fn a_check_that_changes_nothing_reports_no_change() {
    let mut finished = FinishedWorktrees::default();
    let repo = Uuid::new_v4();
    let done = Uuid::new_v4();
    let now = Instant::now();

    let ticket = finished.start(repo, now).unwrap();
    assert!(finished.fold(ticket, &verdicts(&[(done, true)]), Some(&ids(&[done]))));
    let ticket = finished.start(repo, now + MIN_INTERVAL).unwrap();
    assert!(!finished.fold(ticket, &verdicts(&[(done, true)]), Some(&ids(&[done]))));
}

/// A reopened issue takes its worktree back out of the finished set.
#[test]
fn a_worktree_found_open_again_is_unmarked() {
    let mut finished = FinishedWorktrees::default();
    let repo = Uuid::new_v4();
    let wt = Uuid::new_v4();
    let now = Instant::now();

    let ticket = finished.start(repo, now).unwrap();
    finished.fold(ticket, &verdicts(&[(wt, true)]), Some(&ids(&[wt])));
    let ticket = finished.start(repo, now + MIN_INTERVAL).unwrap();
    assert!(finished.fold(ticket, &verdicts(&[(wt, false)]), Some(&ids(&[wt]))));
    assert!(!finished.contains(wt));
}

/// `gh` offline or a branch git cannot read: no verdict, and a worktree
/// already marked stays marked rather than flickering off for a blip.
#[test]
fn a_worktree_without_a_verdict_keeps_its_mark() {
    let mut finished = FinishedWorktrees::default();
    let repo = Uuid::new_v4();
    let wt = Uuid::new_v4();
    let now = Instant::now();

    let ticket = finished.start(repo, now).unwrap();
    finished.fold(ticket, &verdicts(&[(wt, true)]), Some(&ids(&[wt])));
    let ticket = finished.start(repo, now + MIN_INTERVAL).unwrap();
    assert!(!finished.fold(ticket, &verdicts(&[]), Some(&ids(&[wt]))));
    assert!(finished.contains(wt));
}

/// A result that lands after its repository was removed changes nothing.
#[test]
fn a_result_for_a_removed_repository_is_discarded() {
    let mut finished = FinishedWorktrees::default();
    let repo = Uuid::new_v4();
    let wt = Uuid::new_v4();

    let ticket = finished.start(repo, Instant::now()).unwrap();
    assert!(!finished.fold(ticket, &verdicts(&[(wt, true)]), None));
    assert!(!finished.contains(wt));
}

/// Verdicts about worktrees the repository no longer has — removed while
/// the check ran, or belonging to another repository — are dropped.
#[test]
fn verdicts_for_worktrees_outside_the_repository_are_discarded() {
    let mut finished = FinishedWorktrees::default();
    let repo = Uuid::new_v4();
    let (kept, gone) = (Uuid::new_v4(), Uuid::new_v4());

    let ticket = finished.start(repo, Instant::now()).unwrap();
    finished.fold(
        ticket,
        &verdicts(&[(kept, true), (gone, true)]),
        Some(&ids(&[kept])),
    );
    assert!(finished.contains(kept));
    assert!(!finished.contains(gone));
}

/// Each repository's result replaces only its own marks.
#[test]
fn a_check_leaves_other_repositories_marks_alone() {
    let mut finished = FinishedWorktrees::default();
    let (repo_a, repo_b) = (Uuid::new_v4(), Uuid::new_v4());
    let (wt_a, wt_b) = (Uuid::new_v4(), Uuid::new_v4());
    let now = Instant::now();

    let ticket = finished.start(repo_a, now).unwrap();
    finished.fold(ticket, &verdicts(&[(wt_a, true)]), Some(&ids(&[wt_a])));
    let ticket = finished.start(repo_b, now).unwrap();
    finished.fold(ticket, &verdicts(&[(wt_b, false)]), Some(&ids(&[wt_b])));

    assert!(finished.contains(wt_a));
}

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
    finished.fold(ticket, &HashMap::new(), Some(&HashSet::new()));
    assert!(finished.start(repo, now + Duration::from_secs(1)).is_none());
    assert!(finished.start(repo, now + MIN_INTERVAL).is_some());
    assert!(
        finished.start(Uuid::new_v4(), now).is_some(),
        "per repository"
    );
}

/// The ticket frees the repository even when the caller drops the result.
#[test]
fn a_discarded_result_still_frees_the_repository() {
    let mut finished = FinishedWorktrees::default();
    let repo = Uuid::new_v4();
    let now = Instant::now();

    let ticket = finished.start(repo, now).unwrap();
    finished.fold(ticket, &HashMap::new(), None);
    assert!(finished.start(repo, now + MIN_INTERVAL).is_some());
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
