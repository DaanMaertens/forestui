use super::*;
use crate::fuzzy::Searchable;
use crate::modal::{ModalOutcome, ModalResult, StartWorkStep, WorktreeForm};
use crate::work::WorkMode;

/// What the branch load for the `worktree` step delivers, as git would.
fn worktree_branches_loaded(app: &App) -> AppEvent {
    AppEvent::Branches {
        repo_id: app.state.repositories()[0].id,
        list: crate::event::BranchList {
            branches: vec!["main".into(), "feat/login".into(), "origin/release".into()],
            remotes: vec!["origin".into()],
            remote_default: Some("origin/main".into()),
            current_branch: "main".into(),
        },
        target: crate::event::BranchTarget::StartWorkWorktree,
    }
}

/// `s`, the `worktree` mode — last in the list — and its form, loaded.
fn open_form(app: &mut App) {
    app.handle_key(key(KeyCode::Char('s')));
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down));
    }
    assert_eq!(start_work(app).mode, WorkMode::Worktree);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(app).step, StartWorkStep::Worktree);
    let loaded = worktree_branches_loaded(app);
    app.handle_event(loaded);
}

fn form(app: &App) -> &WorktreeForm {
    start_work(app)
        .worktree
        .as_deref()
        .expect("branches loaded")
}

fn form_mut(app: &mut App) -> &mut WorktreeForm {
    match app.modals.last_mut() {
        Some(Modal::StartWork(flow)) => flow.worktree.as_deref_mut().expect("branches loaded"),
        other => panic!("Start work expected, found {other:?}"),
    }
}

/// Enter on the form, asked of the modal directly: through the app, a
/// submit would go on to create a real worktree.
fn enter_on_form(app: &mut App) -> ModalOutcome {
    app.modals
        .last_mut()
        .expect("Start work")
        .handle_key(key(KeyCode::Enter))
}

#[tokio::test]
async fn the_mode_step_lists_worktree_after_pr_review() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('s')));

    let screen = rendered(&mut app);
    let review = screen.find("pr-review").expect("listed");
    let worktree = screen
        .find("worktree   A plain worktree: new or existing branch, no Claude")
        .expect("listed");
    assert!(review < worktree, "{screen}");
}

#[tokio::test]
async fn worktree_is_selectable_without_github() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('s')));
    no_github(&mut app, &path);

    assert!(start_work(&app).mode_enabled(WorkMode::Worktree));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(start_work(&app).mode, WorkMode::Worktree);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).step, StartWorkStep::Worktree);
}

#[tokio::test]
async fn the_form_waits_for_the_branches() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('s')));
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Enter));
    assert!(start_work(&app).worktree.is_none());
    assert!(rendered(&mut app).contains("Loading branches..."));

    let loaded = worktree_branches_loaded(&app);
    app.handle_event(loaded);
    let screen = rendered(&mut app);
    assert!(screen.contains("Start Work: worktree"), "{screen}");
    assert!(screen.contains("Worktree Name"), "{screen}");
    assert!(screen.contains("Create Worktree"), "{screen}");
}

/// A branch list for the confirm screen is not the form's.
#[tokio::test]
async fn a_branch_load_for_another_target_does_not_fill_the_form() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('s')));
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Enter));
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);
    assert!(start_work(&app).worktree.is_none());
}

#[tokio::test]
async fn a_new_branch_follows_the_name_under_the_prefix() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    open_form(&mut app);
    type_text(&mut app, "my thing");

    let prefix = app.settings.branch_prefix.clone();
    assert_eq!(form(&app).sanitized_name(), "mything");
    assert_eq!(form(&app).branch.value(), format!("{prefix}mything"));

    match enter_on_form(&mut app) {
        ModalOutcome::Submit(ModalResult::WorktreeCreated {
            name,
            branch,
            new_branch,
            ..
        }) => {
            assert_eq!(name, "mything");
            assert_eq!(branch, format!("{prefix}mything"));
            assert!(new_branch);
        }
        outcome => panic!("a create expected, got {outcome:?}"),
    }
}

#[tokio::test]
async fn an_existing_remote_branch_can_be_picked() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    open_form(&mut app);
    type_text(&mut app, "rel");
    // Tab to the toggle, flip it to Existing, Tab to the search.
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Tab));
    type_text(&mut app, "origin/release");

    match enter_on_form(&mut app) {
        ModalOutcome::Submit(ModalResult::WorktreeCreated {
            branch, new_branch, ..
        }) => {
            assert_eq!(branch, "origin/release");
            assert!(!new_branch);
        }
        outcome => panic!("a create expected, got {outcome:?}"),
    }
}

#[tokio::test]
async fn each_validation_error_is_shown_and_refuses_create() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    open_form(&mut app);

    // No name.
    assert!(matches!(enter_on_form(&mut app), ModalOutcome::None));
    assert_eq!(form(&app).error, "Worktree name is required");
    assert!(rendered(&mut app).contains("Worktree name is required"));

    // A name, but the branch emptied by hand.
    type_text(&mut app, "wt");
    form_mut(&mut app).branch.set_value("");
    assert!(matches!(enter_on_form(&mut app), ModalOutcome::None));
    assert_eq!(form(&app).error, "Branch name is required");

    // An existing branch that is not one.
    let f = form_mut(&mut app);
    f.new_branch = false;
    f.picker.set_query("nope");
    assert!(!form(&app).can_create());
    assert!(matches!(enter_on_form(&mut app), ModalOutcome::None));
    assert_eq!(form(&app).error, "Branch 'nope' does not exist");

    // A worktree path already on disk.
    let forest = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(forest.path().join(&form(&app).repo_name).join("wt")).unwrap();
    let f = form_mut(&mut app);
    f.forest_dir = forest.path().to_path_buf();
    f.picker.set_query("main");
    assert!(matches!(enter_on_form(&mut app), ModalOutcome::None));
    assert_eq!(form(&app).error, "Worktree path already exists");
}

#[tokio::test]
async fn esc_and_back_return_to_the_mode_list() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    open_form(&mut app);
    type_text(&mut app, "kept");
    form_mut(&mut app).error = "Worktree path already exists".into();
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(start_work(&app).step, StartWorkStep::Mode);
    assert_eq!(start_work(&app).mode, WorkMode::Worktree);

    // Back in again: what was typed is still there, the old complaint is not.
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).step, StartWorkStep::Worktree);
    assert_eq!(form(&app).name.value(), "kept");
    assert!(form(&app).error.is_empty());
    let back = form(&app).back_index();
    form_mut(&mut app).focus = back;
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).step, StartWorkStep::Mode);
}

#[tokio::test]
async fn cancel_closes_the_flow() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    open_form(&mut app);
    let cancel = form(&app).cancel_index();
    form_mut(&mut app).focus = cancel;
    app.handle_key(key(KeyCode::Enter));
    assert!(app.modals.is_empty());
}

/// Clicking a branch row has to *pick* the branch, not just light it up:
/// `can_create` reads the query, so a mouse-only user was left with Create
/// permanently disabled and no way to choose.
#[tokio::test]
async fn clicking_a_branch_row_picks_that_branch() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    open_form(&mut app);
    type_text(&mut app, "wt");
    form_mut(&mut app).new_branch = false;
    assert!(!form(&app).can_create(), "nothing is picked yet");
    let row = form(&app)
        .picker
        .matches()
        .nth(1)
        .unwrap()
        .search_text()
        .to_string();

    app.push_hit(
        rect(0, 7, 40, 1),
        HitTarget::ModalControl {
            index: WorktreeForm::FOCUS_RESULTS,
            click: ModalClick::Row(1),
        },
    );
    app.handle_mouse(click(5, 7));

    assert_eq!(form(&app).selected_branch(), row);
    assert_eq!(form(&app).focus, WorktreeForm::FOCUS_RESULTS);
    assert!(
        form(&app).can_create(),
        "the clicked branch was never committed"
    );
}

#[tokio::test]
async fn the_repository_pane_has_no_add_worktree_and_w_does_nothing() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    let screen = rendered(&mut app);
    assert!(!screen.contains("Add Worktree"), "{screen}");
    assert!(!screen.contains("Add WT"), "{screen}");
    app.handle_key(key(KeyCode::Char('w')));
    assert!(app.modals.is_empty());
}
