use super::*;

#[tokio::test]
async fn a_global_template_file_fills_the_confirm_screens_prompt() {
    let prompts = PromptsDir::new();
    prompts.write(
        crate::prompt::Template::Issue,
        b"Look at #{number} ({title}) in {worktree} first.",
    );
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);

    walk_to_confirm(&mut app, "login");

    assert_eq!(
        confirm_screen(&app).prompt(),
        "Look at #42 (Fix login bug) in issue-42-fix-login-bug first."
    );
    assert!(app.notifications.is_empty());
}

#[tokio::test]
async fn an_unreadable_template_file_falls_back_to_the_default_with_a_notice() {
    let prompts = PromptsDir::new();
    prompts.write(crate::prompt::Template::Issue, &[0xff, 0xfe]);
    let (_dir, mut app, _) = app_with_issues(vec![open_issue(42, "Fix login bug")]);

    walk_to_confirm(&mut app, "login");

    let prompt = confirm_screen(&app).prompt();
    assert!(prompt.contains("#42: Fix login bug"), "{prompt}");
    let notice = app.notifications.last().expect("a notification");
    assert_eq!(notice.severity, Severity::Warning);
    assert!(notice.text.contains("issue.md"), "{}", notice.text);
}

#[tokio::test]
async fn prompt_templates_in_settings_creates_the_chosen_file_from_its_default() {
    let prompts = PromptsDir::new();
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Char('S')));
    if let Some(Modal::Settings(settings)) = app.modals.last_mut() {
        settings.focus = crate::modal::SettingsModal::FOCUS_TEMPLATES;
    }

    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        template_picker(&app).selected(),
        ConfigFile::Template(crate::prompt::Template::Issue)
    );
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));

    let file = prompts.0.join("feature.md");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        crate::prompt::FEATURE_TEMPLATE
    );
    assert!(
        !prompts.0.join("issue.md").exists(),
        "only the chosen template is written"
    );
    // The picker stays up for the next template, and Esc goes back to
    // Settings, which is still open beneath it.
    assert_eq!(
        template_picker(&app).selected(),
        ConfigFile::Template(crate::prompt::Template::Feature)
    );
    app.handle_key(key(KeyCode::Esc));
    assert!(matches!(app.modals.last(), Some(Modal::Settings(_))));
}

#[tokio::test]
async fn prompt_templates_in_settings_keeps_an_existing_file() {
    let prompts = PromptsDir::new();
    prompts.write(crate::prompt::Template::Issue, b"mine");
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Char('S')));
    if let Some(Modal::Settings(settings)) = app.modals.last_mut() {
        settings.focus = crate::modal::SettingsModal::FOCUS_TEMPLATES;
    }

    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(
        std::fs::read_to_string(prompts.0.join("issue.md")).unwrap(),
        "mine"
    );
}
