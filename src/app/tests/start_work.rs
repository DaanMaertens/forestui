use super::*;

/// The issue Start work is waiting to confirm, if it is waiting on one.
fn pending_number(app: &App) -> Option<i64> {
    match app.pending_start.as_ref().map(|p| &p.item) {
        Some(crate::work::WorkItem::Issue { number, .. }) => Some(*number),
        _ => None,
    }
}

#[tokio::test]
async fn s_opens_start_work_on_the_mode_step() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);

    app.handle_key(key(KeyCode::Char('s')));

    let flow = start_work(&app);
    assert_eq!(flow.step, crate::modal::StartWorkStep::Mode);
    assert_eq!(flow.mode, crate::work::WorkMode::Issue);
    assert!(
        keys::BINDINGS.contains(&('s', "Start")),
        "the footer must offer it"
    );
}

/// Settings moved to `S` to make room; `s` must not open it any more.
#[tokio::test]
async fn settings_moved_to_capital_s() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('S')));
    assert!(matches!(app.modals.last(), Some(Modal::Settings(_))));
}

/// Start work runs for the repository a worktree belongs to as well, so
/// it does not matter which of its rows is selected.
#[tokio::test]
async fn start_work_opens_from_a_worktree_row_for_its_repository() {
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    app.handle_key(key(KeyCode::Down));
    assert!(app.state.selection.is_worktree());

    app.handle_key(key(KeyCode::Char('s')));

    assert_eq!(start_work(&app).repo_path, path);
}

#[tokio::test]
async fn the_start_work_control_leads_the_repository_pane() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    assert_eq!(
        app.detail_items().first(),
        Some(&DetailItem::Action(Action::StartWork))
    );

    app.run_action(Action::StartWork);

    assert!(matches!(app.modals.last(), Some(Modal::StartWork(_))));
}

/// `issue` lists every open issue in one list, features and bugs alike,
/// and counts them all as issues.
#[tokio::test]
async fn the_issue_mode_lists_every_open_issue() {
    let (_dir, mut app, _) = app_with_issues(vec![
        labelled_issue(1, "Crash on save", &["bug"]),
        labelled_issue(2, "Dark mode", &["enhancement"]),
        labelled_issue(3, "Faster startup", &["bug", "enhancement"]),
        open_issue(4, "Typo in README"),
    ]);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));

    let flow = start_work(&app);
    assert_eq!(flow.mode, crate::work::WorkMode::Issue);
    assert_eq!(flow.step, crate::modal::StartWorkStep::Item);
    assert_eq!(listed(&app), vec![1, 2, 3, 4]);
    let screen = rendered(&mut app);
    assert!(screen.contains("4 issues"), "{screen}");
}

/// With nothing open, the list says there are no issues — not "no
/// non-feature issues".
#[tokio::test]
async fn an_empty_issue_list_says_no_open_issues() {
    let (_dir, mut app, _) = app_with_issues(Vec::new());
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    let screen = rendered(&mut app);
    assert!(screen.contains("No open issues"), "{screen}");
    assert!(!screen.contains("feature"), "{screen}");
}

fn with_scope(scope: crate::models::Scope) -> crate::models::Settings {
    let mut settings = crate::models::Settings::default();
    settings.start_work.scope = scope;
    settings
}

/// Under `all`, a list fetched for `mine` is not everything: Start work
/// waits for the `all` list rather than showing the short one as complete.
#[tokio::test]
async fn scope_all_does_not_list_the_mine_issues() {
    use crate::models::Scope;
    let (_dir, mut app, path) = app_with_settings_and_issues(
        with_scope(Scope::All),
        Scope::Mine,
        vec![open_issue(1, "Assigned to me")],
    );
    app.handle_key(key(KeyCode::Char('s')));
    assert!(
        start_work(&app).picker.is_none(),
        "the mine list stood in for all"
    );
    assert!(
        app.issues_in_flight.contains(&(path, Scope::All)),
        "all was never asked for"
    );
}

/// The list fetched for the configured scope is the one Start work shows,
/// for `all` as for `mine`.
#[tokio::test]
async fn start_work_lists_the_configured_scope() {
    use crate::models::Scope;
    for scope in [Scope::Mine, Scope::All] {
        let (_dir, mut app, _) = app_with_settings_and_issues(
            with_scope(scope),
            scope,
            vec![open_issue(1, "One"), open_issue(2, "Two")],
        );
        app.handle_key(key(KeyCode::Char('s')));
        assert_eq!(listed(&app), vec![1, 2], "{scope:?}");
    }
}

/// Pick the `nth` issue of the `issue` list and wait out the branches,
/// landing on the confirm screen.
fn confirm_nth_issue(app: &mut App, nth: usize) {
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    for _ in 0..nth {
        app.handle_key(key(KeyCode::Down));
    }
    app.handle_key(key(KeyCode::Enter));
    let loaded = branches_loaded(app);
    app.handle_event(loaded);
}

/// The configured feature labels decide how an issue starts, not whether
/// it is listed: both are in the one list.
#[tokio::test]
async fn the_feature_labels_setting_decides_how_an_issue_starts() {
    let mut settings = crate::models::Settings::default();
    settings.start_work.feature_labels = vec!["feature".into()];
    let issues = vec![
        labelled_issue(1, "Dark mode", &["enhancement"]),
        labelled_issue(2, "Export", &["Feature"]),
    ];
    let (_dir, mut app, _) =
        app_with_settings_and_issues(settings.clone(), crate::models::Scope::Mine, issues.clone());
    app.handle_key(key(KeyCode::Char('s')));
    assert_eq!(listed(&app), vec![1, 2]);

    confirm_nth_issue(&mut app, 0);
    assert_eq!(
        confirm_screen(&app).name.value(),
        "issue-1-dark-mode",
        "enhancement is no longer a feature"
    );

    let (_dir, mut app, _) =
        app_with_settings_and_issues(settings, crate::models::Scope::Mine, issues);
    confirm_nth_issue(&mut app, 1);
    assert_eq!(confirm_screen(&app).name.value(), "feat-2-export");
}

/// The confirm screen names the worktree and branch by the configured
/// prefix and pattern.
#[tokio::test]
async fn the_confirm_screen_uses_the_configured_naming() {
    let mut settings = crate::models::Settings::default();
    settings.start_work.issue = crate::models::Naming {
        branch_prefix: "bug/".into(),
        name_pattern: "{number}-{slug}".into(),
    };
    let (_dir, mut app, _) = app_with_settings_and_issues(
        settings,
        crate::models::Scope::Mine,
        vec![labelled_issue(42, "Fix login bug", &["bug"])],
    );
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Enter));
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    let confirm = confirm_screen(&app);
    assert_eq!(confirm.name.value(), "42-fix-login-bug");
    assert_eq!(confirm.branch.value(), "bug/42-fix-login-bug");
}

/// A feature-labelled issue, picked from the one `issue` list, is named
/// `feat-<n>-<slug>` on `feat/...`, started with the plan-first template,
/// and called a feature on the confirm screen.
#[tokio::test]
async fn a_feature_confirms_with_feat_names_and_the_feature_prompt() {
    let (_dir, mut app, _) = app_with_issues(vec![
        labelled_issue(42, "Fix login bug", &["bug"]),
        labelled_issue(7, "Dark mode", &["enhancement"]),
    ]);
    confirm_nth_issue(&mut app, 1);

    let confirm = confirm_screen(&app);
    assert!(matches!(
        confirm.item,
        crate::work::WorkItem::Issue {
            number: 7,
            kind: crate::work::IssueKind::Feature,
            ..
        }
    ));
    assert_eq!(confirm.name.value(), "feat-7-dark-mode");
    assert_eq!(confirm.branch.value(), "feat/feat-7-dark-mode");
    let prompt = confirm.prompt();
    assert!(prompt.contains("feature from GitHub issue #7"), "{prompt}");
    assert!(prompt.contains("approved the plan"), "{prompt}");
    let screen = rendered(&mut app);
    assert!(screen.contains("Start Work: Feature #7"), "{screen}");
}

/// A bug — and any other issue without a feature label — is named
/// `issue-<n>-<slug>` on `fix/...`, started with the issue template, and
/// called an issue on the confirm screen.
#[tokio::test]
async fn a_bug_confirms_with_issue_names_and_the_issue_prompt() {
    let (_dir, mut app, _) = app_with_issues(vec![
        labelled_issue(42, "Fix login bug", &["bug"]),
        labelled_issue(7, "Dark mode", &["enhancement"]),
    ]);
    confirm_nth_issue(&mut app, 0);

    let confirm = confirm_screen(&app);
    assert!(matches!(
        confirm.item,
        crate::work::WorkItem::Issue {
            number: 42,
            kind: crate::work::IssueKind::Issue,
            ..
        }
    ));
    assert_eq!(confirm.name.value(), "issue-42-fix-login-bug");
    assert_eq!(confirm.branch.value(), "fix/issue-42-fix-login-bug");
    let prompt = confirm.prompt();
    assert!(
        prompt.starts_with("Work on GitHub issue #42: Fix login bug"),
        "{prompt}"
    );
    let screen = rendered(&mut app);
    assert!(screen.contains("Start Work: Issue #42"), "{screen}");
}

/// A feature label wins over a bug label: an issue carrying both starts
/// as a feature.
#[tokio::test]
async fn a_feature_label_wins_over_a_bug_label() {
    let (_dir, mut app, _) = app_with_issues(vec![labelled_issue(
        3,
        "Faster startup",
        &["bug", "enhancement"],
    )]);
    confirm_nth_issue(&mut app, 0);
    assert_eq!(
        confirm_screen(&app).branch.value(),
        "feat/feat-3-faster-startup"
    );
}

/// The confirm screen only ever opens over Start work: closed while the
/// branches loaded, the flow is over and nothing appears in its place.
#[tokio::test]
async fn closing_start_work_before_the_branches_land_drops_the_confirm() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(pending_number(&app), Some(42));

    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Esc));
    assert!(app.modals.is_empty());
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);
    assert!(app.modals.is_empty(), "{:?}", app.modals);
}

#[tokio::test]
async fn mode_then_item_then_confirm() {
    let (_dir, mut app, _) = app_with_issues(vec![
        open_issue(42, "Fix login bug"),
        open_issue(7, "Crash on save"),
    ]);

    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    let flow = start_work(&app);
    assert_eq!(flow.step, crate::modal::StartWorkStep::Item);
    let picker = flow.picker.as_ref().expect("issues were cached");
    assert_eq!(picker.entry_count(), 2);

    type_text(&mut app, "crash");
    let picked = start_work(&app)
        .picker
        .as_ref()
        .and_then(|p| p.selected())
        .and_then(issue_of)
        .map(|e| e.issue.number);
    assert_eq!(picked, Some(7));

    app.handle_key(key(KeyCode::Enter));
    // The confirm screen waits for the branches; the item step stays up.
    assert_eq!(pending_number(&app), Some(7));
    assert!(matches!(app.modals.last(), Some(Modal::StartWork(_))));

    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    let confirm = confirm_screen(&app);
    assert_eq!(confirm_number(confirm), Some(7));
    assert_eq!(confirm.name.value(), "issue-7-crash-on-save");
    assert_eq!(confirm.branch.value(), "fix/issue-7-crash-on-save");
    assert_eq!(
        confirm.base_branch.value(),
        "origin/main",
        "the remote's default branch, not the checkout's develop"
    );
    assert!(confirm.pull_first, "pull first is on by default");
    let prompt = confirm.prompt();
    assert!(prompt.contains("#7: Crash on save"), "{prompt}");
    assert!(prompt.contains("`issue-7-crash-on-save`"), "{prompt}");
    assert_eq!(app.modals.len(), 2, "the item step stays beneath");
}

/// Esc walks back one step at a time, and only closes from the first.
#[tokio::test]
async fn escape_steps_back_through_the_flow() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    walk_to_confirm(&mut app, "login");
    assert!(matches!(
        app.modals.last(),
        Some(Modal::StartWorkConfirm(_))
    ));

    app.handle_key(key(KeyCode::Esc));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Item);
    app.handle_key(key(KeyCode::Esc));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Mode);
    app.handle_key(key(KeyCode::Esc));
    assert!(app.modals.is_empty());
}

/// Launch hands over the prompt the screen showed and closes the whole
/// flow, item step included.
#[tokio::test]
async fn launching_closes_the_whole_flow() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    walk_to_confirm(&mut app, "login");

    // A repository nobody tracks: the launch is folded exactly as a real
    // one, but stops before it reaches git or the forest directory.
    if let Some(Modal::StartWorkConfirm(confirm)) = app.modals.last_mut() {
        confirm.repo_id = uuid::Uuid::new_v4();
        confirm.focus = crate::modal::StartWorkConfirmModal::FOCUS_LAUNCH;
    }
    app.handle_key(key(KeyCode::Enter));

    assert!(app.modals.is_empty(), "{:?}", app.modals);
}

/// The submitted result carries the issue, the names and the prompt as
/// they stood on screen — edits to the name reach the prompt too.
#[tokio::test]
async fn launch_submits_what_the_confirm_screen_shows() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    walk_to_confirm(&mut app, "login");
    type_text(&mut app, "-2");

    let modal = app.modals.last_mut().expect("the confirm screen");
    let Modal::StartWorkConfirm(confirm) = modal else {
        panic!("the confirm screen expected, found {modal:?}");
    };
    confirm.focus = crate::modal::StartWorkConfirmModal::FOCUS_LAUNCH;
    let shown = confirm.prompt();
    // Asked of the modal directly: through the app, the launch would go
    // on to create a real worktree.
    let outcome = modal.handle_key(key(KeyCode::Enter));

    let crate::modal::ModalOutcome::Submit(crate::modal::ModalResult::WorktreeForWorkItem {
        link,
        name,
        branch,
        pull_first,
        base_branch,
        first_prompt,
        ..
    }) = outcome
    else {
        panic!("a launch expected, got {outcome:?}");
    };
    assert_eq!(link, crate::models::WorkItemLink::Issue { number: 42 });
    assert_eq!(name, "issue-42-fix-login-bug-2");
    assert_eq!(branch, "fix/issue-42-fix-login-bug");
    assert!(pull_first);
    assert_eq!(base_branch.as_deref(), Some("origin/main"));
    assert_eq!(first_prompt, shown);
    assert!(first_prompt.contains("`issue-42-fix-login-bug-2`"));
}

/// Edit prompt hands the text to an editor; what comes back is what
/// Claude starts with, and no template is touched on the way.
#[tokio::test]
async fn an_edited_prompt_is_what_launches() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    walk_to_confirm(&mut app, "login");
    let original = confirm_screen_mut(&mut app).prompt();
    let repo_id = app.state.repositories()[0].id;

    confirm_screen_mut(&mut app).focus = crate::modal::StartWorkConfirmModal::FOCUS_EDIT;
    app.handle_key(key(KeyCode::Enter));
    assert!(confirm_screen_mut(&mut app).is_editing);

    // What the editor task delivers once the editor closes.
    app.handle_event(AppEvent::PromptEdited {
        repo_id,
        link: crate::models::WorkItemLink::Issue { number: 42 },
        result: Ok("Only fix the login bug.\n\nNothing else.\n".into()),
    });
    let confirm = confirm_screen_mut(&mut app);
    assert!(!confirm.is_editing);
    assert_eq!(confirm.prompt(), "Only fix the login bug.\n\nNothing else.");

    let crate::modal::ModalResult::WorktreeForWorkItem { first_prompt, .. } = submitted(&mut app)
    else {
        panic!("a launch from the issue expected");
    };
    assert_eq!(first_prompt, "Only fix the login bug.\n\nNothing else.");

    // The built-in template is what a fresh confirm screen fills again.
    app.modals.clear();
    walk_to_confirm(&mut app, "login");
    assert_eq!(confirm_screen_mut(&mut app).prompt(), original);
}

/// A failed or emptied edit keeps the prompt the screen showed, and a
/// result for a confirm screen that has gone is dropped.
#[tokio::test]
async fn an_edit_that_fails_or_strays_changes_nothing() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    walk_to_confirm(&mut app, "login");
    let original = confirm_screen_mut(&mut app).prompt();
    let repo_id = app.state.repositories()[0].id;

    for result in [Err("no editor".to_string()), Ok("  \n".to_string())] {
        confirm_screen_mut(&mut app).is_editing = true;
        app.handle_event(AppEvent::PromptEdited {
            repo_id,
            link: crate::models::WorkItemLink::Issue { number: 42 },
            result,
        });
        assert!(!confirm_screen_mut(&mut app).is_editing);
        assert_eq!(confirm_screen_mut(&mut app).prompt(), original);
    }

    // Saved as it was: still the template, so it keeps following the
    // name fields rather than freezing at the name it was opened with.
    confirm_screen_mut(&mut app).is_editing = true;
    app.handle_event(AppEvent::PromptEdited {
        repo_id,
        link: crate::models::WorkItemLink::Issue { number: 42 },
        result: Ok(format!("{original}\n")),
    });
    assert!(confirm_screen_mut(&mut app).edited_prompt.is_none());

    app.handle_event(AppEvent::PromptEdited {
        repo_id,
        link: crate::models::WorkItemLink::Issue { number: 7 },
        result: Ok("Another issue's prompt".into()),
    });
    assert_eq!(confirm_screen_mut(&mut app).prompt(), original);
}

/// The yolo toggle starts off and reaches the launch when switched on.
#[tokio::test]
async fn the_yolo_toggle_reaches_the_launch() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    walk_to_confirm(&mut app, "login");
    let crate::modal::ModalResult::WorktreeForWorkItem { yolo, .. } = submitted(&mut app) else {
        panic!("a launch from the issue expected");
    };
    assert!(!yolo, "yolo is off by default");

    confirm_screen_mut(&mut app).focus = crate::modal::StartWorkConfirmModal::FOCUS_YOLO;
    app.handle_key(key(KeyCode::Char(' ')));
    assert!(confirm_screen_mut(&mut app).yolo);
    let crate::modal::ModalResult::WorktreeForWorkItem { yolo, .. } = submitted(&mut app) else {
        panic!("a launch from the issue expected");
    };
    assert!(yolo);
}

/// With a cold cache the mode step opens at once and the issue list fills
/// in when the fetch folds.
#[tokio::test]
async fn issues_that_arrive_later_fill_the_picker() {
    let (_dir, mut app) = app_with_fixture();
    let path = app.state.repositories()[0].source_path.clone();
    github::invalidate_cache(&path);

    app.handle_key(key(KeyCode::Char('s')));
    assert!(start_work(&app).picker.is_none());

    app.handle_event(AppEvent::Issues {
        path,
        scope: crate::models::Scope::Mine,
        issues: vec![open_issue(42, "Fix login bug")],
    });

    let picker = start_work(&app).picker.as_ref().expect("filled");
    assert_eq!(picker.entry_count(), 1);
}

/// Picking an issue and then leaving must not open its confirm screen
/// over nothing once the slow branch load lands.
#[tokio::test]
async fn a_flow_closed_before_the_branches_load_opens_nothing() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Enter));
    assert!(app.pending_start.is_some());

    app.handle_key(key(KeyCode::Esc));
    app.handle_key(key(KeyCode::Esc));
    assert!(app.modals.is_empty());
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    assert!(app.modals.is_empty(), "{:?}", app.modals);
}

/// Clicking a row picks it, the way Enter does: a mouse-only user can go
/// from mode to confirm without the keyboard.
#[tokio::test]
async fn clicking_rows_walks_the_flow() {
    let (_dir, mut app, _) = app_with_issues(vec![
        open_issue(42, "Fix login bug"),
        open_issue(7, "Crash on save"),
    ]);
    app.handle_key(key(KeyCode::Char('s')));

    let pick = |row| HitTarget::ModalControl {
        index: crate::modal::StartWorkModal::FOCUS_LIST,
        click: ModalClick::Pick(row),
    };
    app.push_hit(rect(10, 10, 30, 1), pick(0));
    app.handle_mouse(click(12, 10));
    assert_eq!(start_work(&app).step, crate::modal::StartWorkStep::Item);

    app.push_hit(rect(10, 11, 30, 1), pick(1));
    app.handle_mouse(click(12, 11));
    assert_eq!(pending_number(&app), Some(7));
}

/// The flow draws on a real frame: every step renders its title and the
/// picked issue's prompt shows on the confirm screen.
#[tokio::test]
async fn every_step_renders() {
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    let backend = ratatui::backend::TestBackend::new(140, 50);
    let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
    let screen =
        |app: &mut App, terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>| {
            terminal
                .draw(|frame| crate::ui::draw(frame, app))
                .expect("draw");
            let buffer: &ratatui::buffer::Buffer = terminal.backend().buffer();
            buffer
                .content
                .chunks(buffer.area.width as usize)
                .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };

    app.handle_key(key(KeyCode::Char('s')));
    let mode = screen(&mut app, &mut terminal);
    assert!(mode.contains("Start Work"), "{mode}");
    assert!(mode.contains("pr-review"), "{mode}");

    app.handle_key(key(KeyCode::Enter));
    let item = screen(&mut app, &mut terminal);
    assert!(item.contains("Start Work: issue"), "{item}");
    assert!(item.contains("#42 Fix login bug"), "{item}");

    app.handle_key(key(KeyCode::Enter));
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);
    let confirm = screen(&mut app, &mut terminal);
    assert!(confirm.contains("Start Work: Issue #42"), "{confirm}");
    assert!(confirm.contains("gh issue view 42 --comments"), "{confirm}");
    assert!(confirm.contains("Launch"), "{confirm}");
}
