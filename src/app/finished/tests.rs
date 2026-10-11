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
