use super::*;

fn listing(path: &str) -> Vec<WorktreeInfo> {
    vec![WorktreeInfo {
        path: path.into(),
        head: "abc123".into(),
        branch: Some("main".into()),
    }]
}

#[test]
fn a_scan_that_raced_nothing_is_reconciled() {
    let mut sync = WorktreeSync::default();
    let repo = Uuid::new_v4();

    let ticket = sync.start_scan(repo).expect("no scan was running");
    assert_eq!(ticket.repo_id(), repo);

    match sync.judge_scan(ticket, Some(listing("/r"))) {
        ScanVerdict::Reconcile { listed, protected } => {
            assert_eq!(listed[0].path, "/r");
            assert!(protected.is_empty());
        }
        other => panic!("expected a reconcile, got {other:?}"),
    }
}

/// "Could not list" must never read as "there are none".
#[test]
fn a_failed_listing_keeps_state() {
    let mut sync = WorktreeSync::default();
    let ticket = sync.start_scan(Uuid::new_v4()).unwrap();
    assert!(matches!(
        sync.judge_scan(ticket, None),
        ScanVerdict::KeepState
    ));
}

/// Selection changes arrive faster than a listing returns; a second scan
/// of the same repository would only duplicate the first.
#[test]
fn one_scan_per_repository_runs_at_a_time() {
    let mut sync = WorktreeSync::default();
    let repo = Uuid::new_v4();

    let first = sync.start_scan(repo).expect("first scan starts");
    assert!(sync.start_scan(repo).is_none(), "a duplicate scan started");
    assert!(
        sync.start_scan(Uuid::new_v4()).is_some(),
        "another repository's scan was held back"
    );

    // Judging releases the guard, whatever the verdict.
    sync.judge_scan(first, None);
    assert!(
        sync.start_scan(repo).is_some(),
        "the guard was never released"
    );
}

/// The listing predates the rename: reconciling it would put the old
/// branch name back.
#[test]
fn a_scan_older_than_a_finished_change_is_ignored() {
    let mut sync = WorktreeSync::default();
    let repo = Uuid::new_v4();
    let worktree = Uuid::new_v4();

    let change = sync.begin_rename(repo, worktree).expect("rename starts");
    let stale = sync.start_scan(repo).unwrap();
    sync.finish(change);
    assert!(matches!(
        sync.judge_scan(stale, Some(listing("/r"))),
        ScanVerdict::Ignore
    ));

    // A scan started after the change finished describes the new state.
    let fresh = sync.start_scan(repo).unwrap();
    assert!(matches!(
        sync.judge_scan(fresh, Some(listing("/r"))),
        ScanVerdict::Reconcile { .. }
    ));
}

/// Even a change that failed moves the epoch on: one "finish" for every
/// outcome, so no caller can pick the wrong one.
#[test]
fn a_failed_change_also_makes_older_scans_stale() {
    let mut sync = WorktreeSync::default();
    let repo = Uuid::new_v4();

    let change = sync.begin_removal(repo, Uuid::new_v4()).unwrap();
    let stale = sync.start_scan(repo).unwrap();
    sync.finish(change); // the fold does not say whether git succeeded
    assert!(matches!(
        sync.judge_scan(stale, Some(listing("/r"))),
        ScanVerdict::Ignore
    ));
}

/// The epoch belongs to the repository the change was started on, recorded
/// at the start: a worktree already gone from state still moves it on.
#[test]
fn a_change_moves_only_its_own_repositorys_epoch() {
    let mut sync = WorktreeSync::default();
    let (repo, other) = (Uuid::new_v4(), Uuid::new_v4());

    let change = sync.begin_rename(repo, Uuid::new_v4()).unwrap();
    let other_scan = sync.start_scan(other).unwrap();
    sync.finish(change);
    assert!(matches!(
        sync.judge_scan(other_scan, Some(listing("/o"))),
        ScanVerdict::Reconcile { .. }
    ));
}

/// A listing may predate a change or catch it half done, so the worktrees
/// it touches are not the listing's to prune or branch-correct.
#[test]
fn worktrees_with_a_change_in_flight_are_protected() {
    let mut sync = WorktreeSync::default();
    let repo = Uuid::new_v4();
    let (renaming, removing, idle) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let rename = sync.begin_rename(repo, renaming).unwrap();
    let _removal = sync.begin_removal(repo, removing).unwrap();
    let _elsewhere = sync.begin_rename(Uuid::new_v4(), Uuid::new_v4()).unwrap();

    let scan = sync.start_scan(repo).unwrap();
    let ScanVerdict::Reconcile { protected, .. } = sync.judge_scan(scan, Some(listing("/r")))
    else {
        panic!("expected a reconcile");
    };
    assert_eq!(protected, HashSet::from([renaming, removing]));
    assert!(!protected.contains(&idle));

    // Once finished, the worktree is the listing's to judge again.
    sync.finish(rename);
    let scan = sync.start_scan(repo).unwrap();
    let ScanVerdict::Reconcile { protected, .. } = sync.judge_scan(scan, Some(listing("/r")))
    else {
        panic!("expected a reconcile");
    };
    assert_eq!(protected, HashSet::from([removing]));
}

/// A rename and a removal racing on one worktree, or two of either, can
/// only end with one of them working on a path the other moved.
#[test]
fn a_worktree_takes_one_change_at_a_time() {
    let mut sync = WorktreeSync::default();
    let (repo, worktree) = (Uuid::new_v4(), Uuid::new_v4());

    let rename = sync.begin_rename(repo, worktree).unwrap();
    assert!(sync.begin_rename(repo, worktree).is_none());
    assert!(sync.begin_removal(repo, worktree).is_none());
    assert!(sync.change_running(worktree));

    sync.finish(rename);
    assert!(!sync.change_running(worktree));
    assert!(sync.begin_removal(repo, worktree).is_some());
}

/// The Delete control and the quit check ask about removals only: a
/// rename running is not a reason to hold quit.
#[test]
fn removals_are_told_apart_from_renames() {
    let mut sync = WorktreeSync::default();
    let repo = Uuid::new_v4();
    let (renaming, removing) = (Uuid::new_v4(), Uuid::new_v4());

    let _rename = sync.begin_rename(repo, renaming).unwrap();
    assert!(!sync.any_removal_running());
    assert!(!sync.is_removing(renaming));

    let removal = sync.begin_removal(repo, removing).unwrap();
    assert!(sync.any_removal_running());
    assert!(sync.is_removing(removing));

    sync.finish(removal);
    assert!(!sync.any_removal_running());
    assert!(!sync.is_removing(removing));
}

/// A create has no worktree id until it lands, so it guards its whole
/// repository: a scan landing mid-create would adopt the new worktree
/// and announce it as found outside forestui.
#[test]
fn a_create_running_holds_back_its_repositorys_scans() {
    let mut sync = WorktreeSync::default();
    let (repo, other) = (Uuid::new_v4(), Uuid::new_v4());

    let first = sync.begin_create(repo);
    let second = sync.begin_create(repo);

    let during = sync.start_scan(repo).unwrap();
    assert!(matches!(
        sync.judge_scan(during, Some(listing("/r"))),
        ScanVerdict::Ignore
    ));
    let elsewhere = sync.start_scan(other).unwrap();
    assert!(matches!(
        sync.judge_scan(elsewhere, Some(listing("/o"))),
        ScanVerdict::Reconcile { .. }
    ));

    // One of two creates finishing still leaves the other running.
    sync.finish_create(first);
    let scan = sync.start_scan(repo).unwrap();
    assert!(matches!(
        sync.judge_scan(scan, Some(listing("/r"))),
        ScanVerdict::Ignore
    ));

    // A scan that started before the last create finished is stale too.
    let stale = sync.start_scan(repo).unwrap();
    sync.finish_create(second);
    assert!(matches!(
        sync.judge_scan(stale, Some(listing("/r"))),
        ScanVerdict::Ignore
    ));
    let fresh = sync.start_scan(repo).unwrap();
    assert!(matches!(
        sync.judge_scan(fresh, Some(listing("/r"))),
        ScanVerdict::Reconcile { .. }
    ));
}
