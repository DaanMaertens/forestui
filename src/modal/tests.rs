use super::*;
use ratatui::crossterm::event::{KeyEventKind, KeyModifiers};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: ratatui::crossterm::event::KeyEventState::NONE,
    }
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent {
        modifiers: KeyModifiers::CONTROL,
        ..key(code)
    }
}

fn typed(text: &str) -> Vec<KeyEvent> {
    text.chars().map(|c| key(KeyCode::Char(c))).collect()
}

#[test]
fn escape_closes_every_modal() {
    let mut modals: Vec<Modal> = vec![
        Modal::AddRepository(AddRepositoryModal::new()),
        Modal::Confirm(ConfirmModal::new(
            "t",
            "m",
            ConfirmAction::RemoveRepository(Uuid::new_v4()),
        )),
        Modal::Settings(Box::new(SettingsModal::new(&Settings::default()))),
        Modal::CustomButtons(CustomButtonsModal::new(vec![])),
        Modal::EditButton(Box::new(EditButtonModal::new(None, &[], None))),
        Modal::RenameSession(RenameSessionModal::new(
            "/tmp/x".into(),
            "sess".into(),
            "old name".into(),
            None,
        )),
    ];
    for modal in &mut modals {
        assert!(matches!(
            modal.handle_key(key(KeyCode::Esc)),
            ModalOutcome::Close
        ));
    }
}

#[test]
fn add_repository_requires_a_git_directory() {
    let dir = tempfile::tempdir().unwrap();
    let mut modal = AddRepositoryModal::new();
    modal
        .path
        .set_value(dir.path().to_string_lossy().to_string());
    assert_eq!(modal.status().0, "Not a git repository");
    assert!(matches!(
        modal.handle_key(key(KeyCode::Enter)),
        ModalOutcome::None
    ));

    std::fs::create_dir(dir.path().join(".git")).unwrap();
    assert!(modal.status().1);
    assert!(matches!(
        modal.handle_key(key(KeyCode::Enter)),
        ModalOutcome::Submit(ModalResult::RepositoryAdded { .. })
    ));
}

#[test]
fn worktree_form_autofills_branch_from_name() {
    let repo = Repository::new("demo".into(), "/tmp/demo".into());
    let mut modal = WorktreeForm::new(
        &repo,
        vec!["main".into()],
        vec![],
        PathBuf::from("/forest"),
        "feat/".into(),
    );
    for k in typed("my feature!") {
        modal.handle_key(k);
    }
    assert_eq!(modal.sanitized_name(), "myfeature");
    assert_eq!(modal.branch.value(), "feat/myfeature");
    assert_eq!(
        modal.path_preview().unwrap(),
        PathBuf::from("/forest/demo/myfeature")
    );
}

#[test]
fn worktree_form_rejects_unknown_existing_branch() {
    let repo = Repository::new("demo".into(), "/tmp/demo".into());
    let mut modal = WorktreeForm::new(
        &repo,
        vec!["main".into()],
        vec![],
        PathBuf::from("/forest"),
        "feat/".into(),
    );
    modal.name.set_value("wt");
    modal.new_branch = false;
    modal.picker.set_query("nope");
    assert!(!modal.can_create());
    modal.focus = modal.create_index();
    modal.handle_key(key(KeyCode::Enter));
    assert!(modal.error.contains("does not exist"));

    modal.picker.set_query("main");
    assert!(modal.can_create());
    assert!(matches!(
        modal.handle_key(key(KeyCode::Enter)),
        ModalOutcome::Submit(ModalResult::WorktreeCreated { .. })
    ));
}

#[test]
fn edit_button_prefix_follows_label_until_edited() {
    let mut modal = EditButtonModal::new(None, &[], None);
    for k in typed("Yolo Disc") {
        modal.handle_key(k);
    }
    assert_eq!(modal.prefix.value(), "yolo-disc");

    modal.focus = EditButtonModal::FOCUS_PREFIX;
    modal.handle_key(key(KeyCode::Backspace));
    assert!(!modal.follows);

    modal.focus = EditButtonModal::FOCUS_LABEL;
    modal.handle_key(key(KeyCode::Char('X')));
    assert_eq!(modal.prefix.value(), "yolo-dis");
}

#[test]
fn edit_button_rejects_duplicates_and_empty_command() {
    let existing = vec![CustomClaudeButton {
        label: "Opus".into(),
        prefix: "opus".into(),
        command: "claude --model opus".into(),
    }];
    let mut modal = EditButtonModal::new(None, &existing, None);
    modal.label.set_value("Opus");
    modal.prefix.set_value("other");
    modal.focus = EditButtonModal::FOCUS_SAVE;
    modal.handle_key(key(KeyCode::Enter));
    assert_eq!(modal.error, "Command cannot be empty");

    modal.command.set_value("claude");
    modal.handle_key(key(KeyCode::Enter));
    assert!(modal.error.contains("label"));

    modal.label.set_value("Fresh");
    assert!(matches!(
        modal.handle_key(key(KeyCode::Enter)),
        ModalOutcome::Submit(ModalResult::CustomButtonSaved(_))
    ));
}

#[test]
fn custom_buttons_reorder_and_delete() {
    let make = |name: &str| CustomClaudeButton {
        label: name.into(),
        prefix: name.to_lowercase(),
        command: "claude".into(),
    };
    let mut modal = CustomButtonsModal::new(vec![make("A"), make("B"), make("C")]);
    modal.selected = 2;
    modal.handle_key(key(KeyCode::Char('K')));
    assert_eq!(modal.buttons[1].label, "C");
    assert_eq!(modal.selected, 1);

    modal.handle_key(key(KeyCode::Char('d')));
    assert_eq!(modal.buttons.len(), 2);
    assert!(matches!(
        modal.handle_key(key(KeyCode::Char('s'))),
        ModalOutcome::Submit(ModalResult::CustomButtonsSaved(_))
    ));
}

/// The funnel in `Modal::handle_key`, for the same reason as the footer
/// bindings (Defect B of issue #51): the modals match shortcuts on the
/// character alone, so `Ctrl+D` used to delete the selected custom button.
/// The one exception is the text editor's own `Ctrl+U`, which must still
/// reach a focused field.
#[test]
fn modified_letters_never_fire_modal_shortcuts() {
    let make = |name: &str| CustomClaudeButton {
        label: name.into(),
        prefix: name.to_lowercase(),
        command: "claude".into(),
    };
    let mut modal = Modal::CustomButtons(CustomButtonsModal::new(vec![make("A"), make("B")]));
    for shortcut in ['d', 'a', 's', 'K', 'J'] {
        assert!(matches!(
            modal.handle_key(ctrl(KeyCode::Char(shortcut))),
            ModalOutcome::None
        ));
    }
    let Modal::CustomButtons(buttons) = &modal else {
        unreachable!("the modal was replaced");
    };
    assert_eq!(buttons.buttons.len(), 2, "a Ctrl combination deleted a row");
    assert_eq!(
        buttons.buttons[0].label, "A",
        "a Ctrl combination reordered"
    );

    let mut add = Modal::AddRepository(AddRepositoryModal::new());
    for event in typed("/tmp/repo") {
        add.handle_key(event);
    }
    add.handle_key(ctrl(KeyCode::Char('u')));
    let Modal::AddRepository(add) = &add else {
        unreachable!("the modal was replaced");
    };
    assert_eq!(add.path.value(), "", "Ctrl+U no longer reaches the field");
}

#[test]
fn child_results_flow_to_the_parent() {
    let mut settings = Modal::Settings(Box::new(SettingsModal::new(&Settings::default())));
    let button = CustomClaudeButton {
        label: "Opus".into(),
        prefix: "opus".into(),
        command: "claude --model opus".into(),
    };
    settings.receive_child(&ModalResult::CustomButtonsSaved(vec![button.clone()]));
    if let Modal::Settings(m) = &settings {
        assert_eq!(m.custom_buttons.len(), 1);
        assert_eq!(m.buttons_summary(), "1 custom button configured");
    } else {
        panic!("wrong modal");
    }

    let mut parent = Modal::CustomButtons(CustomButtonsModal::new(vec![]));
    parent.receive_child(&ModalResult::CustomButtonSaved(Box::new(button)));
    if let Modal::CustomButtons(m) = &parent {
        assert_eq!(m.buttons.len(), 1);
    } else {
        panic!("wrong modal");
    }
}

#[test]
fn settings_cycles_selects_and_saves() {
    let mut modal = SettingsModal::new(&Settings::default());
    let start = modal.editor_index;
    modal.handle_key(key(KeyCode::Right));
    assert_ne!(modal.editor_index, start);
    modal.handle_key(key(KeyCode::Left));
    assert_eq!(modal.editor_index, start);

    modal.focus = SettingsModal::FOCUS_SAVE;
    match modal.handle_key(key(KeyCode::Enter)) {
        ModalOutcome::Submit(ModalResult::SettingsSaved(s)) => {
            assert_eq!(s.default_editor, EDITORS[modal.editor_index].1);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

/// The dialog does not edit Start work, so it must hand back exactly what
/// it was opened with — or saving the theme would reset a hand-edited
/// scope, labels and naming to the defaults.
#[test]
fn saving_settings_keeps_the_start_work_section() {
    let mut settings = Settings::default();
    settings.start_work.scope = crate::models::Scope::All;
    settings.start_work.feature_labels = vec!["feature".into()];
    settings.start_work.issue.branch_prefix = "bug/".into();
    let mut modal = SettingsModal::new(&settings);
    modal.focus = SettingsModal::FOCUS_SAVE;
    let ModalOutcome::Submit(ModalResult::SettingsSaved(saved)) =
        modal.handle_key(key(KeyCode::Enter))
    else {
        panic!("Save must submit the settings");
    };
    assert_eq!(saved.start_work, settings.start_work);
}

/// The Remote Control checkbox opens showing the stored value, toggles on
/// Space and on Enter (a click sends Enter, and must not save and close),
/// and Save carries whatever it shows.
#[test]
fn settings_toggles_and_saves_remote_control() {
    let mut modal = SettingsModal::new(&Settings::default());
    assert!(modal.remote_control, "on unless the user turns it off");

    modal.focus = SettingsModal::FOCUS_REMOTE_CONTROL;
    assert!(matches!(
        modal.handle_key(key(KeyCode::Char(' '))),
        ModalOutcome::None
    ));
    assert!(!modal.remote_control);
    assert!(matches!(
        modal.handle_key(key(KeyCode::Enter)),
        ModalOutcome::None
    ));
    assert!(modal.remote_control, "Enter toggles, it does not submit");
    modal.handle_key(key(KeyCode::Enter));

    modal.focus = SettingsModal::FOCUS_SAVE;
    let ModalOutcome::Submit(ModalResult::SettingsSaved(saved)) =
        modal.handle_key(key(KeyCode::Enter))
    else {
        panic!("Save must submit the settings");
    };
    assert!(!saved.remote_control);

    let reopened = SettingsModal::new(&saved);
    assert!(!reopened.remote_control, "the dialog shows what was stored");
}

/// The rename dialog refuses what cannot become a name — empty, or text
/// with control characters, which would end a startup-file line — and
/// carries the live window through so the submit knows its mechanism.
#[test]
fn rename_session_validates_and_carries_the_mechanism() {
    let mut modal = RenameSessionModal::new(
        "/tmp/x".into(),
        "sess".into(),
        String::new(),
        Some(("@3".into(), "claude:wt".into())),
    );
    modal.focus = RenameSessionModal::FOCUS_SAVE;
    assert!(matches!(
        modal.handle_key(key(KeyCode::Enter)),
        ModalOutcome::None
    ));
    assert_eq!(modal.error, "Name cannot be empty");

    modal.name.set_value("bad\nname");
    modal.handle_key(key(KeyCode::Enter));
    assert!(modal.error.contains("control characters"));

    modal.name.set_value("  retry loop  ");
    let ModalOutcome::Submit(ModalResult::SessionRenamed {
        name,
        live_window_id,
        session_id,
        ..
    }) = modal.handle_key(key(KeyCode::Enter))
    else {
        panic!("a valid name must submit");
    };
    assert_eq!(name, "retry loop", "the name is trimmed");
    assert_eq!(live_window_id.as_deref(), Some("@3"));
    assert_eq!(session_id, "sess");

    // A stopped session submits with no window, which is the transcript
    // path on the other side.
    let mut stopped = RenameSessionModal::new("/tmp/x".into(), "sess".into(), "old".into(), None);
    stopped.focus = RenameSessionModal::FOCUS_SAVE;
    let ModalOutcome::Submit(ModalResult::SessionRenamed { live_window_id, .. }) =
        stopped.handle_key(key(KeyCode::Enter))
    else {
        panic!("a valid name must submit");
    };
    assert_eq!(live_window_id, None);
}

/// "Switch" is the safe choice and must not wear the destructive colour.
#[test]
fn only_destructive_confirms_read_as_destructive() {
    assert!(ConfirmAction::DeleteWorktree(Uuid::new_v4()).is_destructive());
    assert!(
        ConfirmAction::DeleteClaudeSession {
            path: "/x".into(),
            session_id: "s".into(),
            title: "t".into(),
        }
        .is_destructive()
    );
    let switch = ConfirmAction::SwitchToWindow {
        window_id: "@1".into(),
        window_name: "claude:wt".into(),
    };
    assert!(!switch.is_destructive());
    assert_eq!(switch.confirm_label(), "Switch");
}

#[test]
fn confirm_needs_an_explicit_yes() {
    let mut modal = ConfirmModal::new(
        "Delete",
        "sure?",
        ConfirmAction::DeleteWorktree(Uuid::new_v4()),
    );
    // Enter defaults to Cancel.
    assert!(matches!(
        modal.handle_key(key(KeyCode::Enter)),
        ModalOutcome::Close
    ));
    assert!(matches!(
        modal.handle_key(key(KeyCode::Char('y'))),
        ModalOutcome::Submit(ModalResult::Confirmed(_))
    ));
}

fn branch_list(
    branches: &[&str],
    remotes: &[&str],
    remote_default: Option<&str>,
    current: &str,
) -> BranchList {
    BranchList {
        branches: branches.iter().map(|b| b.to_string()).collect(),
        remotes: remotes.iter().map(|r| r.to_string()).collect(),
        remote_default: remote_default.map(str::to_string),
        current_branch: current.to_string(),
    }
}

#[test]
fn base_branch_default_prefers_remote() {
    let branches = ["main", "origin/main"];
    assert_eq!(
        default_base_branch(&branch_list(&branches, &["origin"], None, "main")),
        "origin/main"
    );
    assert_eq!(
        default_base_branch(&branch_list(&branches, &[], None, "main")),
        "main"
    );
    assert_eq!(
        default_base_branch(&branch_list(&branches, &[], None, "nope")),
        "main"
    );
    assert_eq!(
        default_base_branch(&branch_list(&[], &[], None, "main")),
        ""
    );
}

/// New work starts from what the remote calls its default branch, not
/// from whatever the repository's own checkout happens to be on.
#[test]
fn base_branch_default_is_the_remotes_default_branch() {
    let branches = ["develop", "main", "origin/develop", "origin/main"];
    let list = branch_list(&branches, &["origin"], Some("origin/main"), "develop");
    assert_eq!(default_base_branch(&list), "origin/main");

    // A recorded default that is not among the branches (pruned, or a
    // stale ref) falls back to the old order rather than naming nothing.
    let stale = branch_list(&branches, &["origin"], Some("origin/gone"), "develop");
    assert_eq!(default_base_branch(&stale), "origin/develop");
}

/// The picker's contract: moving the highlight applies the candidate to
/// the whole app, Esc restores what was active when it opened, and Enter
/// commits the slug while leaving the preview in place.
#[test]
fn theme_picker_previews_commits_and_reverts() {
    let _guard = crate::theme::test_lock();
    crate::theme::set_active("forest-dark");

    let mut picker = ThemePickerModal::new();
    picker.handle_key(key(KeyCode::Down));
    assert_eq!(crate::theme::active().slug, crate::theme::THEMES[1].slug);
    assert!(matches!(
        picker.handle_key(key(KeyCode::Esc)),
        ModalOutcome::Close
    ));
    assert_eq!(crate::theme::active().slug, "forest-dark");

    let mut picker = ThemePickerModal::new();
    picker.handle_key(key(KeyCode::Down));
    let ModalOutcome::Submit(ModalResult::ThemeChosen(slug)) =
        picker.handle_key(key(KeyCode::Enter))
    else {
        panic!("Enter must commit the highlighted theme");
    };
    assert_eq!(slug, crate::theme::THEMES[1].slug);
    assert_eq!(crate::theme::active().slug, slug);
    // The guard restores the pre-test theme on drop.
}

/// A previewed theme must not outlive the dialog it was previewed in:
/// closing Settings without saving restores the theme it opened with.
#[test]
fn cancelling_settings_abandons_a_previewed_theme() {
    let _guard = crate::theme::test_lock();
    crate::theme::set_active("forest-dark");

    let mut settings = SettingsModal::new(&Settings::default());
    // The picker previewed and committed a different slug into the dialog.
    crate::theme::set_active("dracula");
    settings.theme_slug = "dracula";

    assert!(matches!(
        settings.handle_key(key(KeyCode::Esc)),
        ModalOutcome::Close
    ));
    assert_eq!(crate::theme::active().slug, "forest-dark");
}

#[test]
fn saving_settings_carries_the_chosen_theme() {
    let _guard = crate::theme::test_lock();
    let mut settings = SettingsModal::new(&Settings::default());
    settings.theme_slug = "nord";
    settings.focus = SettingsModal::FOCUS_SAVE;

    let ModalOutcome::Submit(ModalResult::SettingsSaved(saved)) =
        settings.handle_key(key(KeyCode::Enter))
    else {
        panic!("Save must submit the settings");
    };
    assert_eq!(saved.theme_name, "nord");
}

/// An unknown stored slug resolves to the default when the dialog opens,
/// so a hand-edited or future-versioned file neither crashes nor persists
/// an unknown value forward.
#[test]
fn unknown_theme_names_resolve_to_the_default_slug() {
    let settings = Settings {
        theme_name: "some-future-theme".into(),
        ..Settings::default()
    };
    let modal = SettingsModal::new(&settings);
    assert_eq!(modal.theme_slug, "forest-dark");
}
