use super::*;

fn temp_state() -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".forestui-config.json");
    (dir, AppState::load_from(path))
}

#[test]
fn add_and_persist_roundtrip() {
    let (dir, mut state) = temp_state();
    let repo = Repository::new("demo".into(), "/tmp/demo".into());
    let repo_id = repo.id;
    state.add_repository(repo);
    state.add_worktree(
        repo_id,
        Worktree::new("wt".into(), "feat/x".into(), "/f/wt".into()),
    );

    let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
    assert_eq!(reloaded.repositories().len(), 1);
    assert_eq!(reloaded.repositories()[0].worktrees.len(), 1);
    assert_eq!(reloaded.repositories()[0].worktrees[0].branch, "feat/x");
}

/// The link is the match, not the name: a worktree renamed away from its
/// issue's slug still answers for it, and another repository's worktree
/// with the same link does not.
#[test]
fn a_work_item_finds_its_worktree_by_link() {
    let (_dir, mut state) = temp_state();
    let repo = Repository::new("demo".into(), "/tmp/demo".into());
    let other = Repository::new("other".into(), "/tmp/other".into());
    let (repo_id, other_id) = (repo.id, other.id);
    state.add_repository(repo);
    state.add_repository(other);
    let link = WorkItemLink::Issue { number: 7 };
    let mut linked = Worktree::new("renamed".into(), "b".into(), "/f/renamed".into());
    linked.work_item = Some(link.clone());
    let linked_id = linked.id;
    state.add_worktree(repo_id, linked);
    state.add_worktree(
        repo_id,
        Worktree::new("issue-7-x".into(), "fix/issue-7-x".into(), "/f/x".into()),
    );

    assert_eq!(
        state.linked_worktree(repo_id, &link).map(|w| w.id),
        Some(linked_id)
    );
    assert!(
        state
            .linked_worktree(repo_id, &WorkItemLink::Issue { number: 8 })
            .is_none()
    );
    assert!(state.linked_worktree(other_id, &link).is_none());
}

#[test]
fn corrupt_config_loads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".forestui-config.json");
    std::fs::write(&path, "{not json").unwrap();
    assert!(AppState::load_from(path).repositories().is_empty());
}

#[test]
fn removing_selection_clears_it() {
    let (_dir, mut state) = temp_state();
    let repo = Repository::new("demo".into(), "/tmp/demo".into());
    let repo_id = repo.id;
    state.add_repository(repo);
    let worktree = Worktree::new("wt".into(), "b".into(), "/f/wt".into());
    let wt_id = worktree.id;
    state.add_worktree(repo_id, worktree);

    state.select_worktree(repo_id, wt_id);
    state.remove_worktree(wt_id);
    assert_eq!(state.selection.worktree_id, None);
    assert_eq!(state.selection.repository_id, Some(repo_id));

    state.remove_repository(repo_id);
    assert_eq!(state.selection, Selection::default());
}

#[test]
fn tmux_window_names() {
    let (_dir, mut state) = temp_state();
    let repo = Repository::new("demo".into(), "/tmp/demo".into());
    let repo_id = repo.id;
    state.add_repository(repo);
    state.add_worktree(
        repo_id,
        Worktree::new("wt".into(), "b".into(), "/f/wt".into()),
    );

    assert_eq!(state.tmux_window_name("/f/wt"), "demo:wt");
    assert_eq!(state.tmux_window_name("/tmp/demo"), "demo");
    assert_eq!(state.tmux_window_name("/elsewhere"), "session");
}

fn info(path: &str, branch: Option<&str>) -> WorktreeInfo {
    WorktreeInfo {
        path: path.into(),
        head: "abc123".into(),
        branch: branch.map(str::to_string),
    }
}

fn unprotected() -> std::collections::HashSet<Uuid> {
    std::collections::HashSet::new()
}

/// The reconcile corrects a drifted branch, never adopts the main
/// checkout as a worktree, and reports nothing when reality already
/// matches.
#[test]
fn reconcile_updates_branches_and_skips_the_main_checkout() {
    let (dir, mut state) = temp_state();
    let source = dir.path().join("demo").to_string_lossy().to_string();
    let repo = Repository::new("demo".into(), source.clone());
    let repo_id = repo.id;
    state.add_repository(repo);
    let wt_path = dir.path().join("wt").to_string_lossy().to_string();
    state.add_worktree(
        repo_id,
        Worktree::new("wt".into(), "feat/old".into(), wt_path.clone()),
    );

    let listed = [
        info(&source, Some("main")),
        info(&wt_path, Some("feat/new")),
    ];
    let outcome = state
        .reconcile_worktrees(repo_id, &listed, &unprotected())
        .expect("a branch drift is a change");
    assert!(outcome.added.is_empty(), "adopted the main checkout");
    assert!(outcome.removed.is_empty());
    assert_eq!(outcome.branch_updated.len(), 1);
    assert_eq!(state.repositories()[0].worktrees[0].branch, "feat/new");

    assert!(
        state
            .reconcile_worktrees(repo_id, &listed, &unprotected())
            .is_none(),
        "an unchanged listing must be a silent no-op"
    );
}

/// An unlisted worktree whose directory still exists is kept: the listing
/// is a snapshot, and a rename that folded while git ran must not have its
/// result pruned by the stale scan.
#[test]
fn reconcile_keeps_unlisted_worktrees_whose_directory_exists() {
    let (dir, mut state) = temp_state();
    let source = dir.path().join("demo").to_string_lossy().to_string();
    let repo = Repository::new("demo".into(), source.clone());
    let repo_id = repo.id;
    state.add_repository(repo);
    let renamed = dir.path().join("just-renamed");
    std::fs::create_dir_all(&renamed).unwrap();
    state.add_worktree(
        repo_id,
        Worktree::new(
            "just-renamed".into(),
            "feat/r".into(),
            renamed.to_string_lossy().to_string(),
        ),
    );

    let listed = [info(&source, Some("main"))];
    assert!(
        state
            .reconcile_worktrees(repo_id, &listed, &unprotected())
            .is_none()
    );
    assert_eq!(state.repositories()[0].worktrees.len(), 1);
}

/// A listed path whose directory is gone is a prunable stub, not a row.
#[test]
fn reconcile_does_not_adopt_deleted_directories() {
    let (dir, mut state) = temp_state();
    let source = dir.path().join("demo").to_string_lossy().to_string();
    let repo = Repository::new("demo".into(), source.clone());
    let repo_id = repo.id;
    state.add_repository(repo);

    let ghost = dir.path().join("ghost").to_string_lossy().to_string();
    let listed = [info(&source, Some("main")), info(&ghost, Some("feat/g"))];
    assert!(
        state
            .reconcile_worktrees(repo_id, &listed, &unprotected())
            .is_none()
    );
    assert!(state.repositories()[0].worktrees.is_empty());
}

/// A protected worktree (removal or rename in flight) is neither pruned
/// nor branch-corrected, whatever the listing claims.
#[test]
fn reconcile_leaves_protected_worktrees_alone() {
    let (dir, mut state) = temp_state();
    let source = dir.path().join("demo").to_string_lossy().to_string();
    let repo = Repository::new("demo".into(), source.clone());
    let repo_id = repo.id;
    state.add_repository(repo);
    let worktree = Worktree::new("wt".into(), "feat/current".into(), "/gone/wt".into());
    let worktree_id = worktree.id;
    state.add_worktree(repo_id, worktree);

    let protected = std::collections::HashSet::from([worktree_id]);
    // The listing neither contains the path nor could its directory
    // exist — exactly what a half-done removal looks like.
    let listed = [info(&source, Some("main"))];
    assert!(
        state
            .reconcile_worktrees(repo_id, &listed, &protected)
            .is_none()
    );
    assert!(state.find_worktree(worktree_id).is_some());

    // And a listed-but-drifted branch is not "corrected" either.
    let wt_dir = dir.path().join("wt");
    std::fs::create_dir_all(&wt_dir).unwrap();
    state.update_worktree(worktree_id, |w| {
        w.path = wt_dir.to_string_lossy().to_string()
    });
    let listed = [
        info(&source, Some("main")),
        info(&wt_dir.to_string_lossy(), Some("feat/mid-rename")),
    ];
    assert!(
        state
            .reconcile_worktrees(repo_id, &listed, &protected)
            .is_none()
    );
    assert_eq!(
        state
            .find_worktree(worktree_id)
            .map(|(_, w)| w.branch.as_str()),
        Some("feat/current")
    );
}

/// Listing entry zero is the main checkout even when it differs from the
/// tracked source path — the "repository" the user added may itself be a
/// linked worktree — and paths owned by other tracked repositories are
/// never adopted as this one's worktrees.
#[test]
fn reconcile_does_not_adopt_the_main_checkout_or_other_repositories_trees() {
    let (dir, mut state) = temp_state();
    let real_main = dir.path().join("real-main");
    let linked = dir.path().join("linked-wt");
    let other_source = dir.path().join("other");
    let other_wt = dir.path().join("other-wt");
    for path in [&real_main, &linked, &other_source, &other_wt] {
        std::fs::create_dir_all(path).unwrap();
    }

    let other = Repository::new("other".into(), other_source.to_string_lossy().to_string());
    let other_id = other.id;
    state.add_repository(other);
    state.add_worktree(
        other_id,
        Worktree::new(
            "other-wt".into(),
            "feat/o".into(),
            other_wt.to_string_lossy().to_string(),
        ),
    );

    // The user added a linked worktree as a "repository": git lists the
    // real main checkout first, then the siblings.
    let pseudo = Repository::new("linked".into(), linked.to_string_lossy().to_string());
    let pseudo_id = pseudo.id;
    state.add_repository(pseudo);

    let listed = [
        info(&real_main.to_string_lossy(), Some("main")),
        info(&linked.to_string_lossy(), Some("feat/l")),
        info(&other_wt.to_string_lossy(), Some("feat/o")),
    ];
    assert!(
        state
            .reconcile_worktrees(pseudo_id, &listed, &unprotected())
            .is_none(),
        "adopted the main checkout or another repository's worktree"
    );
    assert!(state.repositories()[1].worktrees.is_empty());
}

/// The config may spell a path through a symlink while git records the
/// resolved form. The two must compare equal even after the directory is
/// deleted, or a by-hand `rm -rf` under a symlinked forest reads as
/// unlisted-and-gone and prunes a row git still tracks.
#[test]
fn reconcile_matches_symlinked_spellings_of_a_deleted_worktree() {
    let (dir, mut state) = temp_state();
    let source = dir.path().join("demo").to_string_lossy().to_string();
    let real = dir.path().join("real-forest");
    std::fs::create_dir_all(&real).unwrap();
    let link = dir.path().join("linked-forest");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let repo = Repository::new("demo".into(), source.clone());
    let repo_id = repo.id;
    state.add_repository(repo);
    // Tracked through the symlink; the directory does not exist.
    state.add_worktree(
        repo_id,
        Worktree::new(
            "wt".into(),
            "feat/x".into(),
            link.join("wt").to_string_lossy().to_string(),
        ),
    );

    // Git lists the resolved spelling, still, as a prunable stub.
    let listed = [
        info(&source, Some("main")),
        info(&real.join("wt").to_string_lossy(), Some("feat/x")),
    ];
    assert!(
        state
            .reconcile_worktrees(repo_id, &listed, &unprotected())
            .is_none(),
        "the two spellings of one deleted worktree compared unequal"
    );
    assert_eq!(state.repositories()[0].worktrees.len(), 1);
}

/// Two adoptions sharing a directory basename get distinct names — the
/// name keys the tmux windows, so a duplicate would focus the other
/// worktree's editor.
#[test]
fn reconcile_uniquifies_colliding_adopted_names() {
    let (dir, mut state) = temp_state();
    let source = dir.path().join("demo").to_string_lossy().to_string();
    let repo = Repository::new("demo".into(), source.clone());
    let repo_id = repo.id;
    state.add_repository(repo);
    let a = dir.path().join("a").join("feature");
    let b = dir.path().join("b").join("feature");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();

    let listed = [
        info(&source, Some("main")),
        info(&a.to_string_lossy(), Some("feat/a")),
        info(&b.to_string_lossy(), Some("feat/b")),
    ];
    let outcome = state
        .reconcile_worktrees(repo_id, &listed, &unprotected())
        .expect("two adoptions");
    assert_eq!(outcome.added, vec!["feature", "feature-2"]);
}

/// The create flow's entry replaces a scan-adopted one for the same path
/// rather than sitting next to it in the sidebar.
#[test]
fn add_worktree_replaces_a_scan_adopted_duplicate() {
    let (_dir, mut state) = temp_state();
    let repo = Repository::new("demo".into(), "/tmp/demo".into());
    let repo_id = repo.id;
    state.add_repository(repo);

    state.add_worktree(
        repo_id,
        Worktree::new("adopted".into(), "feat/x".into(), "/f/x".into()),
    );
    state.add_worktree(
        repo_id,
        Worktree::new("chosen-name".into(), "feat/x".into(), "/f/x".into()),
    );

    let worktrees = &state.repositories()[0].worktrees;
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0].name, "chosen-name");
}

/// Pins survive a relaunch and keep their order — that order *is* the
/// feature: it is what the user arranged with K/J.
#[test]
fn session_pins_toggle_reorder_and_persist() {
    let (dir, mut state) = temp_state();
    let path = "/tmp/forest/demo/wt";

    assert!(state.toggle_pin(path, "a"));
    assert!(state.toggle_pin(path, "b"));
    assert!(state.toggle_pin(path, "c"));
    assert_eq!(state.pinned_for(path), vec!["a", "b", "c"]);
    assert!(state.is_pinned(path, "b"));

    // Move "c" up past "b"; the edges refuse silently.
    assert!(state.move_pin(path, "c", -1));
    assert_eq!(state.pinned_for(path), vec!["a", "c", "b"]);
    assert!(!state.move_pin(path, "a", -1));
    assert!(!state.move_pin(path, "b", 1));
    assert!(!state.move_pin(path, "unpinned", 1));

    // Unpinning removes; deleting a session drops its pin the same way.
    assert!(!state.toggle_pin(path, "c"));
    state.remove_pin(path, "b");
    assert_eq!(state.pinned_for(path), vec!["a"]);

    let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
    assert_eq!(reloaded.pinned_for(path), vec!["a"]);
    assert!(reloaded.pinned_for("/other/path").is_empty());

    // Emptying a path's pins removes the key from the file entirely.
    let mut reloaded = reloaded;
    assert!(!reloaded.toggle_pin(path, "a"));
    let raw = std::fs::read_to_string(dir.path().join(".forestui-config.json")).unwrap_or_default();
    assert!(
        !raw.contains("/tmp/forest/demo/wt"),
        "an empty pin list lingers in the config: {raw}"
    );
}

/// The new field is additive: a config written before pins existed loads,
/// and one written with pins still carries its repositories.
#[test]
fn configs_without_pins_load_cleanly() {
    let json = r#"{"repositories": []}"#;
    let data: crate::models::AppStateData = serde_json::from_str(json).unwrap();
    assert!(data.pinned_sessions.is_empty());
}

#[test]
fn archive_toggle_persists() {
    let (dir, mut state) = temp_state();
    let repo = Repository::new("demo".into(), "/tmp/demo".into());
    let repo_id = repo.id;
    state.add_repository(repo);
    let worktree = Worktree::new("wt".into(), "b".into(), "/f/wt".into());
    let wt_id = worktree.id;
    state.add_worktree(repo_id, worktree);

    state.set_archived(wt_id, true);
    assert!(state.has_archived_worktrees());
    let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
    assert!(reloaded.repositories()[0].worktrees[0].is_archived);
}

/// A repository with one worktree, for the finished-mark tests.
fn state_with_worktree() -> (tempfile::TempDir, AppState, Uuid, Uuid) {
    let (dir, mut state) = temp_state();
    let repo = Repository::new("demo".into(), "/tmp/demo".into());
    let repo_id = repo.id;
    state.add_repository(repo);
    let worktree = Worktree::new("wt".into(), "b".into(), "/f/wt".into());
    let wt_id = worktree.id;
    state.add_worktree(repo_id, worktree);
    (dir, state, repo_id, wt_id)
}

fn verdict(wt_id: Uuid, finished: bool) -> std::collections::HashMap<Uuid, bool> {
    [(wt_id, finished)].into_iter().collect()
}

fn marked(state: &AppState, wt_id: Uuid) -> bool {
    state.find_worktree(wt_id).expect("tracked").1.finished
}

/// A config from before marks were saved: the first check sets each mark
/// from its verdict, as the derived marks used to be.
#[test]
fn the_first_verdict_sets_the_mark() {
    let (_dir, mut state, repo_id, wt_id) = state_with_worktree();
    assert!(state.fold_finished_verdicts(repo_id, &verdict(wt_id, true)));
    assert!(marked(&state, wt_id));
}

/// The sweep finds the same answer almost every time; that must report no
/// change, so it costs neither a repaint nor a save.
#[test]
fn the_same_verdict_again_changes_nothing() {
    let (_dir, mut state, repo_id, wt_id) = state_with_worktree();
    state.fold_finished_verdicts(repo_id, &verdict(wt_id, true));
    assert!(!state.fold_finished_verdicts(repo_id, &verdict(wt_id, true)));
}

/// The user's Unmark of a closed issue's worktree sticks while the check
/// keeps saying closed.
#[test]
fn a_hand_set_mark_survives_an_unchanged_verdict() {
    let (dir, mut state, repo_id, wt_id) = state_with_worktree();
    state.fold_finished_verdicts(repo_id, &verdict(wt_id, true));
    state.set_finished(wt_id, false);

    assert!(!state.fold_finished_verdicts(repo_id, &verdict(wt_id, true)));
    assert!(!marked(&state, wt_id));
    let mut reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
    assert!(!reloaded.fold_finished_verdicts(repo_id, &verdict(wt_id, true)));
    assert!(
        !marked(&reloaded, wt_id),
        "the Unmark did not survive a restart"
    );
}

/// Whichever changed the mark last wins: a hand mark on an open issue
/// stays until the issue's state changes, then follows it.
#[test]
fn a_changed_verdict_overrides_a_hand_set_mark() {
    let (_dir, mut state, repo_id, wt_id) = state_with_worktree();
    state.fold_finished_verdicts(repo_id, &verdict(wt_id, false));
    state.set_finished(wt_id, true);
    state.fold_finished_verdicts(repo_id, &verdict(wt_id, false));
    assert!(marked(&state, wt_id));

    assert!(state.fold_finished_verdicts(repo_id, &verdict(wt_id, true)));
    assert!(marked(&state, wt_id), "closed: still marked");
    assert!(state.fold_finished_verdicts(repo_id, &verdict(wt_id, false)));
    assert!(!marked(&state, wt_id), "reopened: the mark follows");
}

/// "Could not tell" — `gh` signed out, a branch git cannot resolve —
/// touches neither the mark nor the saved last answer.
#[test]
fn no_verdict_changes_nothing() {
    let (_dir, mut state, repo_id, wt_id) = state_with_worktree();
    state.fold_finished_verdicts(repo_id, &verdict(wt_id, true));
    assert!(!state.fold_finished_verdicts(repo_id, &Default::default()));
    assert!(marked(&state, wt_id));
    let worktree = state.find_worktree(wt_id).unwrap().1;
    assert_eq!(worktree.last_finished_check, Some(true));
}

/// Verdicts are judged per repository: one about a worktree elsewhere is
/// not this result's to fold.
#[test]
fn a_verdict_about_another_repositorys_worktree_is_ignored() {
    let (_dir, mut state, _repo_id, wt_id) = state_with_worktree();
    let other = Repository::new("other".into(), "/tmp/other".into());
    let other_id = other.id;
    state.add_repository(other);
    assert!(!state.fold_finished_verdicts(other_id, &verdict(wt_id, true)));
    assert!(!marked(&state, wt_id));
}

#[test]
fn a_finished_mark_persists() {
    let (dir, mut state, _repo_id, wt_id) = state_with_worktree();
    state.set_finished(wt_id, true);
    let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
    assert!(marked(&reloaded, wt_id));
}

#[test]
fn configs_without_finished_marks_load_cleanly() {
    let json = r#"{"name": "wt", "branch": "b", "path": "/f/wt"}"#;
    let worktree: Worktree = serde_json::from_str(json).unwrap();
    assert!(!worktree.finished);
    assert_eq!(worktree.last_finished_check, None);
}
