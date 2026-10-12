use super::*;

/// `s`, then the `session` mode, then `prompt` typed and submitted.
fn type_session_prompt(app: &mut App, prompt: &str) {
    app.handle_key(key(KeyCode::Char('s')));
    highlight_mode(app, crate::work::WorkMode::Session);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(app).step, crate::modal::StartWorkStep::Prompt);
    type_text(app, prompt);
    app.handle_key(key(KeyCode::Enter));
}

fn session(prompt: &str) -> crate::work::WorkItem {
    crate::work::WorkItem::Session {
        prompt: prompt.into(),
    }
}

/// `session` is listed between the issue modes and `pr-review`, with what it
/// is for.
#[tokio::test]
async fn the_mode_step_lists_session() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('s')));

    assert_eq!(
        crate::work::WorkMode::ALL.map(crate::work::WorkMode::label),
        ["issue", "feature", "session", "pr-review", "worktree"]
    );
    let screen = rendered(&mut app);
    assert!(
        screen.contains("session    A Claude session for anything else"),
        "{screen}"
    );
    let session = screen.find("session    ").expect("listed");
    let review = screen.find("pr-review").expect("listed");
    assert!(session < review, "{screen}");
}

/// The `feature` list holds issues and nothing else; the old wording is gone.
#[tokio::test]
async fn the_feature_list_has_no_free_text_entry() {
    let (_dir, mut app, _) =
        app_with_issues(vec![labelled_issue(7, "Dark mode", &["enhancement"])]);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Down));
    let screen = rendered(&mut app);
    assert!(!screen.contains("own words"), "{screen}");
    app.handle_key(key(KeyCode::Enter));

    let picker = start_work(&app).picker.as_ref().expect("issues loaded");
    assert_eq!(picker.entry_count(), 1);
    let screen = rendered(&mut app);
    assert!(!screen.contains("Free-text"), "{screen}");
    assert!(screen.contains("1 feature"), "{screen}");
}

/// A session is named from its prompt's first words by the `session`
/// naming, and started with the `session` template: the prompt as typed.
#[tokio::test]
async fn a_session_confirms_with_its_own_names_and_prompt() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    type_session_prompt(&mut app, "Why is the sidebar so slow today?");

    let pending = app.pending_start.as_ref().expect("waiting on branches");
    assert_eq!(pending.item, session("Why is the sidebar so slow today?"));
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    let confirm = confirm_screen(&app);
    assert_eq!(confirm.mode, crate::work::WorkMode::Session);
    assert_eq!(confirm.name.value(), "session-why-is-the-sidebar-so-slow");
    assert_eq!(
        confirm.branch.value(),
        "session/session-why-is-the-sidebar-so-slow"
    );
    assert_eq!(confirm.prompt(), "Why is the sidebar so slow today?\n");
    let screen = rendered(&mut app);
    assert!(screen.contains("Start Work: Session"), "{screen}");

    // Launch links the worktree to the prompt, not to any issue.
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
        WorkItemLink::Session {
            prompt: "Why is the sidebar so slow today?".into()
        }
    );
    assert_eq!(name, "session-why-is-the-sidebar-so-slow");
}

/// The global settings name sessions, down to one half of the naming.
#[tokio::test]
async fn the_session_naming_comes_from_the_settings() {
    let mut settings = crate::models::Settings::default();
    settings.start_work.session.branch_prefix = "spike/".into();
    settings.start_work.session.name_pattern = "s-{slug}".into();
    let (_dir, mut app, _) =
        app_with_settings_and_issues(settings, crate::models::Scope::Mine, Vec::new());
    type_session_prompt(&mut app, "Dark mode");

    let pending = app.pending_start.as_ref().expect("waiting on branches");
    assert_eq!(pending.names.worktree, "s-dark-mode");
    assert_eq!(pending.names.branch, "spike/s-dark-mode");
}

/// A global `session.md` replaces the built-in `{body}`, with the other
/// placeholders filled in.
#[tokio::test]
async fn a_global_session_template_fills_the_first_prompt() {
    let prompts = PromptsDir::new();
    prompts.write(
        crate::prompt::Template::Session,
        b"{body}\n\nYou are in {worktree} of {repo}, on {branch}.",
    );
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    type_session_prompt(&mut app, "Dark mode");
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    let confirm = confirm_screen(&app);
    assert_eq!(
        confirm.prompt(),
        format!(
            "Dark mode\n\nYou are in session-dark-mode of {}, on session/session-dark-mode.",
            confirm.repo_name
        )
    );
}

/// The old `free-feature.md` override is no longer read: a "build this
/// feature" prompt must not turn up in a general session.
#[tokio::test]
async fn an_old_free_feature_template_is_not_used() {
    let prompts = PromptsDir::new();
    std::fs::create_dir_all(&prompts.0).unwrap();
    std::fs::write(
        prompts.0.join("free-feature.md"),
        "Build this feature: {body}",
    )
    .unwrap();
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    type_session_prompt(&mut app, "Dark mode");
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    assert_eq!(confirm_screen(&app).prompt(), "Dark mode\n");
}

/// A prompt with nothing to name a worktree after cannot be confirmed.
#[tokio::test]
async fn an_unnameable_prompt_goes_nowhere() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    type_session_prompt(&mut app, "???");

    assert!(app.pending_start.is_none());
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Prompt);
}

/// Esc and Back from the prompt both return to the mode step, with
/// `session` still highlighted.
#[tokio::test]
async fn back_from_the_prompt_returns_to_the_modes() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    type_session_prompt(&mut app, "");
    app.handle_key(key(KeyCode::Esc));
    let flow = start_work(&app);
    assert_eq!(flow.step, crate::modal::StartWorkStep::Mode);
    assert_eq!(flow.mode, crate::work::WorkMode::Session);

    app.handle_key(key(KeyCode::Enter));
    if let Some(modal) = app.modals.last_mut() {
        modal.set_focus(crate::modal::StartWorkModal::FOCUS_BACK);
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Mode);
}

/// Name `beta` as a session's slug would.
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
    take_slug(&mut app, "session-dark-mode");

    type_session_prompt(&mut app, "Dark mode");

    let flow = start_work(&app);
    assert_eq!(flow.step, crate::modal::StartWorkStep::Clash);
    assert!(app.pending_start.is_none());
    let screen = rendered(&mut app);
    for choice in [
        "Start Work: Session",
        "Open existing",
        "Create copy",
        "Edit name",
        "create session-dark-mode-2 instead",
        "edit the prompt",
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
async fn edit_name_goes_back_to_the_prompt_as_typed() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    take_slug(&mut app, "session-dark-mode");
    type_session_prompt(&mut app, "Dark mode");

    if let Some(modal) = app.modals.last_mut() {
        modal.set_focus(crate::modal::StartWorkModal::FOCUS_EDIT);
    }
    app.handle_key(key(KeyCode::Enter));

    let flow = start_work(&app);
    assert_eq!(flow.step, crate::modal::StartWorkStep::Prompt);
    assert_eq!(flow.prompt.value(), "Dark mode");
    assert!(app.pending_start.is_none());
}

#[tokio::test]
async fn create_copy_confirms_under_the_next_free_suffix() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    take_slug(&mut app, "session-dark-mode");
    type_session_prompt(&mut app, "Dark mode");

    if let Some(modal) = app.modals.last_mut() {
        modal.set_focus(crate::modal::StartWorkModal::FOCUS_COPY);
    }
    app.handle_key(key(KeyCode::Enter));
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    let confirm = confirm_screen(&app);
    assert_eq!(confirm.name.value(), "session-dark-mode-2");
    assert_eq!(confirm.branch.value(), "session/session-dark-mode-2");
    assert_eq!(confirm.item, session("Dark mode"));
}

#[tokio::test]
async fn open_existing_jumps_to_the_worktree_and_creates_nothing() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    let taken = take_slug(&mut app, "session-dark-mode");
    let worktrees = app.state.repositories()[0].worktrees.len();
    type_session_prompt(&mut app, "Dark mode");

    // The first choice is focused, so Enter opens.
    app.handle_key(key(KeyCode::Enter));

    assert!(app.modals.is_empty(), "{:?}", app.modals);
    assert!(app.pending_start.is_none());
    assert_eq!(app.state.selection.worktree_id, Some(taken));
    assert_eq!(app.state.repositories()[0].worktrees.len(), worktrees);
}

/// The link records a prompt, but a prompt is not an identity: a worktree
/// started for the same words is not jumped to.
#[tokio::test]
async fn a_session_is_never_matched_by_its_link() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    let id = take_slug(&mut app, "renamed-since");
    app.state.update_worktree(id, |w| {
        w.work_item = Some(WorkItemLink::Session {
            prompt: "Dark mode".into(),
        });
    });

    type_session_prompt(&mut app, "Dark mode");

    assert_eq!(
        app.pending_start.as_ref().map(|p| &p.item),
        Some(&session("Dark mode"))
    );
    assert_ne!(app.state.selection.worktree_id, Some(id));
}

/// Without a GitHub remote only a session or a plain worktree can be
/// started: the other modes are greyed out and the highlight cannot reach
/// them.
#[tokio::test]
async fn a_repository_without_github_offers_only_session_and_worktree() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('s')));
    no_github(&mut app, &path);

    let flow = start_work(&app);
    assert_eq!(flow.mode, crate::work::WorkMode::Session);
    for mode in crate::work::WorkMode::ALL {
        let enabled = matches!(
            mode,
            crate::work::WorkMode::Session | crate::work::WorkMode::Worktree
        );
        assert_eq!(flow.mode_enabled(mode), enabled, "{mode:?}");
    }
    app.handle_key(key(KeyCode::Up));
    if let Some(modal) = app.modals.last_mut() {
        modal.set_row(0);
    }
    assert_eq!(start_work(&app).mode, crate::work::WorkMode::Session);
    let screen = rendered(&mut app);
    assert!(screen.contains("(needs GitHub)"), "{screen}");

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Prompt);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Mode);
}

/// The probe can answer after the user already moved to `issue`'s list;
/// the flow falls back to `session`, which still works, back on the mode
/// step, since `session` has no list to stay on.
#[tokio::test]
async fn a_late_no_github_answer_leaves_the_issue_list() {
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(start_work(&app).mode, crate::work::WorkMode::Issue);

    no_github(&mut app, &path);

    let flow = start_work(&app);
    assert_eq!(flow.mode, crate::work::WorkMode::Session);
    assert_eq!(flow.step, crate::modal::StartWorkStep::Mode);
    assert!(flow.picker.is_none(), "a session has no list");
    let screen = rendered(&mut app);
    assert!(!screen.contains("Loading issues"), "{screen}");
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
