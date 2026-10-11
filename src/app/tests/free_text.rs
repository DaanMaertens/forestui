use super::*;

/// `s`, then the `feature` mode, then the free-text entry — first in the
/// list — and the description step.
fn describe(app: &mut App, description: &str) {
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(app).step, crate::modal::StartWorkStep::Describe);
    type_text(app, description);
    app.handle_key(key(KeyCode::Enter));
}

fn free_text(description: &str) -> crate::work::WorkItem {
    crate::work::WorkItem::FreeText {
        description: description.into(),
    }
}

/// The free-text entry leads the `feature` list, ahead of every issue.
#[tokio::test]
async fn the_feature_picker_leads_with_free_text() {
    let (_dir, mut app, _) =
        app_with_issues(vec![labelled_issue(7, "Dark mode", &["enhancement"])]);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));

    let picker = start_work(&app).picker.as_ref().expect("issues loaded");
    assert!(matches!(
        picker.selected(),
        Some(crate::work::ItemEntry::FreeText)
    ));
    assert_eq!(picker.entry_count(), 2);
    let screen = rendered(&mut app);
    let free = screen.find("Free-text feature").expect("listed");
    let issue = screen.find("#7 Dark mode").expect("listed");
    assert!(free < issue, "{screen}");
    assert!(
        screen.contains("1 feature"),
        "the count is of issues: {screen}"
    );
}

/// A free-text feature is named from its description and started with the
/// `free-feature` template, which carries the description as `{body}`.
#[tokio::test]
async fn a_free_text_feature_confirms_with_its_own_names_and_prompt() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    describe(&mut app, "Dark mode for the sidebar");

    let pending = app.pending_start.as_ref().expect("waiting on branches");
    assert_eq!(pending.item, free_text("Dark mode for the sidebar"));
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    let confirm = confirm_screen(&app);
    assert_eq!(confirm.mode, crate::work::WorkMode::Feature);
    assert_eq!(confirm.name.value(), "feat-dark-mode-for-the-sidebar");
    assert_eq!(
        confirm.branch.value(),
        "feat/feat-dark-mode-for-the-sidebar"
    );
    let prompt = confirm.prompt();
    assert!(prompt.contains("\nDark mode for the sidebar\n"), "{prompt}");
    assert!(!prompt.contains("gh issue"), "{prompt}");
    let screen = rendered(&mut app);
    assert!(screen.contains("Start Work: Free-text feature"), "{screen}");

    // Launch links the worktree to the description, not to any issue.
    let modal = app.modals.last_mut().expect("the confirm screen");
    modal.set_focus(crate::modal::StartWorkConfirmModal::FOCUS_LAUNCH);
    let outcome = modal.handle_key(key(KeyCode::Enter));
    let crate::modal::ModalOutcome::Submit(crate::modal::ModalResult::WorktreeForWorkItem {
        link,
        name,
        ..
    }) = outcome
    else {
        panic!("a launch expected, got {outcome:?}");
    };
    assert_eq!(
        link,
        crate::models::WorkItemLink::FreeText {
            description: "Dark mode for the sidebar".into()
        }
    );
    assert_eq!(name, "feat-dark-mode-for-the-sidebar");
}

/// A description with nothing to name a worktree after cannot be confirmed.
#[tokio::test]
async fn an_unnameable_description_goes_nowhere() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    describe(&mut app, "???");

    assert!(app.pending_start.is_none());
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Describe);
}

/// Esc from the description goes back to the list it was picked from.
#[tokio::test]
async fn escape_from_the_description_returns_to_the_list() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    describe(&mut app, "");
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Item);
}

/// Name `beta` as a free-text feature's slug would.
fn take_slug(app: &mut App, slug: &str) -> Uuid {
    let id = app.state.repositories()[0]
        .worktrees
        .iter()
        .find(|w| w.name == "beta")
        .map(|w| w.id)
        .expect("fixture worktree");
    app.state.update_worktree(id, |w| w.name = slug.into());
    id
}

/// A slug that is taken is a question, never a silent match: nothing is
/// loaded until the user picks one of exactly three answers.
#[tokio::test]
async fn a_taken_slug_asks_what_to_do() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    take_slug(&mut app, "feat-dark-mode");

    describe(&mut app, "Dark mode");

    let flow = start_work(&app);
    assert_eq!(flow.step, crate::modal::StartWorkStep::Clash);
    assert!(app.pending_start.is_none());
    let screen = rendered(&mut app);
    for choice in [
        "Open existing",
        "Create copy",
        "Edit name",
        "create feat-dark-mode-2 instead",
    ] {
        assert!(screen.contains(choice), "{choice}: {screen}");
    }
    assert_eq!(
        start_work(&app).ring(),
        &[
            crate::modal::StartWorkModal::FOCUS_OPEN,
            crate::modal::StartWorkModal::FOCUS_COPY,
            crate::modal::StartWorkModal::FOCUS_EDIT,
        ]
    );
}

#[tokio::test]
async fn edit_name_goes_back_to_the_description_as_typed() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    take_slug(&mut app, "feat-dark-mode");
    describe(&mut app, "Dark mode");

    if let Some(modal) = app.modals.last_mut() {
        modal.set_focus(crate::modal::StartWorkModal::FOCUS_EDIT);
    }
    app.handle_key(key(KeyCode::Enter));

    let flow = start_work(&app);
    assert_eq!(flow.step, crate::modal::StartWorkStep::Describe);
    assert_eq!(flow.description.value(), "Dark mode");
    assert!(app.pending_start.is_none());
}

#[tokio::test]
async fn create_copy_confirms_under_the_next_free_suffix() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    take_slug(&mut app, "feat-dark-mode");
    describe(&mut app, "Dark mode");

    if let Some(modal) = app.modals.last_mut() {
        modal.set_focus(crate::modal::StartWorkModal::FOCUS_COPY);
    }
    app.handle_key(key(KeyCode::Enter));
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    let confirm = confirm_screen(&app);
    assert_eq!(confirm.name.value(), "feat-dark-mode-2");
    assert_eq!(confirm.branch.value(), "feat/feat-dark-mode-2");
    assert_eq!(confirm.item, free_text("Dark mode"));
}

#[tokio::test]
async fn open_existing_jumps_to_the_worktree_and_creates_nothing() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    let taken = take_slug(&mut app, "feat-dark-mode");
    let worktrees = app.state.repositories()[0].worktrees.len();
    describe(&mut app, "Dark mode");

    // The first choice is focused, so Enter opens.
    app.handle_key(key(KeyCode::Enter));

    assert!(app.modals.is_empty(), "{:?}", app.modals);
    assert!(app.pending_start.is_none());
    assert_eq!(app.state.selection.worktree_id, Some(taken));
    assert_eq!(app.state.repositories()[0].worktrees.len(), worktrees);
}

/// The link records a description, but a description is not an identity:
/// a worktree started for the same words is not jumped to.
#[tokio::test]
async fn free_text_is_never_matched_by_its_link() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    let id = take_slug(&mut app, "renamed-since");
    app.state.update_worktree(id, |w| {
        w.work_item = Some(crate::models::WorkItemLink::FreeText {
            description: "Dark mode".into(),
        });
    });

    describe(&mut app, "Dark mode");

    assert_eq!(
        app.pending_start.as_ref().map(|p| &p.item),
        Some(&free_text("Dark mode"))
    );
    assert_ne!(app.state.selection.worktree_id, Some(id));
}

/// Without a GitHub remote only a free-text feature or a plain worktree
/// can be started: the other modes are greyed out and the highlight cannot
/// reach them.
#[tokio::test]
async fn a_repository_without_github_offers_only_free_text_and_worktree() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('s')));
    no_github(&mut app, &path);

    let flow = start_work(&app);
    assert_eq!(flow.mode, crate::work::WorkMode::Feature);
    assert!(!flow.mode_enabled(crate::work::WorkMode::Issue));
    assert!(!flow.mode_enabled(crate::work::WorkMode::PrReview));
    assert!(flow.mode_enabled(crate::work::WorkMode::Worktree));
    app.handle_key(key(KeyCode::Up));
    if let Some(modal) = app.modals.last_mut() {
        modal.set_row(0);
    }
    assert_eq!(start_work(&app).mode, crate::work::WorkMode::Feature);
    let screen = rendered(&mut app);
    assert!(screen.contains("(needs GitHub)"), "{screen}");

    // There is nothing to pick from, so `feature` goes straight to the
    // description, and Esc straight back.
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Describe);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Mode);
}

/// The probe can answer after the user already moved to `issue`'s list;
/// the flow falls back to the one mode that still works.
#[tokio::test]
async fn a_late_no_github_answer_leaves_the_issue_list() {
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).mode, crate::work::WorkMode::Issue);

    no_github(&mut app, &path);

    let flow = start_work(&app);
    assert_eq!(flow.mode, crate::work::WorkMode::Feature);
    let picker = flow.picker.as_ref().expect("free text needs no issues");
    assert_eq!(picker.entry_count(), 1);
    assert!(matches!(
        picker.selected(),
        Some(crate::work::ItemEntry::FreeText)
    ));
}

/// A yes is what the flow already assumed, so it must not rebuild the list
/// under a half-typed query.
#[tokio::test]
async fn a_github_answer_keeps_the_typed_query() {
    let (_dir, mut app, path) = app_with_issues(two_issues());
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    type_text(&mut app, "crash");

    app.handle_event(AppEvent::GitHubChecked {
        path,
        repo: crate::repo_settings::RepoKey::new("owner", "repo"),
        for_start_work: true,
    });

    let picker = start_work(&app).picker.as_ref().expect("issues loaded");
    assert_eq!(picker.query().value(), "crash");
}

/// An answer for another repository's checkout changes nothing here.
#[tokio::test]
async fn a_github_answer_for_another_path_is_ignored() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('s')));
    no_github(&mut app, "/somewhere/else");
    assert!(start_work(&app).mode_enabled(crate::work::WorkMode::Issue));
}
