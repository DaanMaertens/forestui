use super::*;
use crate::modal::StartWorkSettingsModal as Dialog;

/// The Start Work Settings dialog on top.
fn dialog(app: &App) -> &Dialog {
    match app.modals.last() {
        Some(Modal::StartWorkSettings(dialog)) => dialog,
        other => panic!("the Start Work Settings dialog expected, found {other:?}"),
    }
}

fn focus(app: &mut App, index: usize) {
    if let Some(modal) = app.modals.last_mut() {
        modal.set_focus(index);
    }
    assert_eq!(dialog(app).focus, index, "focus {index} is not in the ring");
}

/// Replace the focused text field's value with `text`.
fn retype(app: &mut App, index: usize, text: &str) {
    focus(app, index);
    app.handle_key(key(KeyCode::End));
    app.handle_key(ctrl(KeyCode::Char('u')));
    type_text(app, text);
}

fn text(app: &App, index: usize) -> String {
    match &dialog(app).fields[index] {
        crate::modal::FieldState::Text(input) => input.value().to_string(),
        other => panic!("a text field expected, found {other:?}"),
    }
}

/// `S`, then Start Work Settings...
fn open_global(app: &mut App) {
    app.handle_key(key(KeyCode::Char('S')));
    if let Some(Modal::Settings(settings)) = app.modals.last_mut() {
        settings.focus = crate::modal::SettingsModal::FOCUS_START_WORK;
    }
    app.handle_key(key(KeyCode::Enter));
    assert!(!dialog(app).is_repository());
}

fn save(app: &mut App) {
    focus(app, Dialog::FOCUS_SAVE);
    app.handle_key(key(KeyCode::Enter));
}

/// What the settings file on disk holds.
fn saved() -> crate::models::Settings {
    crate::services::settings::load_settings()
}

/// Remove this thread's settings file, so a test starts from none.
fn no_settings_file() {
    let _ = std::fs::remove_file(crate::services::settings::settings_path());
}

// --------------------------------------------------------------- Global

/// The Settings dialog opens the global dialog, every field loaded from
/// the global settings and the placeholders the patterns take shown.
#[tokio::test]
async fn settings_opens_the_dialog_on_the_global_values() {
    let mut settings = crate::models::Settings::default();
    settings.start_work.issue.branch_prefix = "bug/".into();
    settings.start_work.session_naming.timeout_secs = 7;
    let (_dir, mut app, _) =
        app_with_settings_and_issues(settings, crate::models::Scope::Mine, Vec::new());
    let screen = rendered(&mut app);
    assert!(!screen.contains("Start Work Settings..."), "not yet open");
    app.handle_key(key(KeyCode::Char('S')));
    assert!(rendered(&mut app).contains("Start Work Settings..."));
    open_global(&mut app);

    assert_eq!(text(&app, Dialog::FOCUS_ISSUE_PREFIX), "bug/");
    assert_eq!(
        text(&app, Dialog::FOCUS_ISSUE_NAME),
        "issue-{number}-{slug}"
    );
    assert_eq!(text(&app, Dialog::FOCUS_FEATURE_LABELS), "enhancement");
    assert_eq!(text(&app, Dialog::FOCUS_SESSION_PREFIX), "session/");
    assert_eq!(
        text(&app, Dialog::FOCUS_PR_REVIEW_NAME),
        "pr-{number}-{slug}"
    );
    assert_eq!(text(&app, Dialog::FOCUS_NAMING_MODEL), "haiku");
    assert_eq!(text(&app, Dialog::FOCUS_NAMING_TIMEOUT), "7");
    assert_eq!(text(&app, Dialog::FOCUS_NAMING_TOOLS), "");
    let screen = rendered(&mut app);
    for expected in [
        "Start Work Settings",
        "Repositories can override these settings in Repository Settings.",
        "◂ Mine ▸",
        "[x] Enabled",
        "{number} {slug}",
        "pr-review",
        "SESSION NAME SUGGESTION",
    ] {
        assert!(screen.contains(expected), "{expected}:\n{screen}");
    }
}

/// Save writes every field to `settings.json` through the App, and the
/// Settings dialog beneath carries the new values, so its own Save keeps
/// them.
#[tokio::test]
async fn saving_writes_the_global_settings() {
    no_settings_file();
    let (_dir, mut app) = app_with_fixture();
    open_global(&mut app);

    focus(&mut app, Dialog::FOCUS_SCOPE);
    app.handle_key(key(KeyCode::Enter));
    retype(
        &mut app,
        Dialog::FOCUS_FEATURE_LABELS,
        "kind/feature,  enhancement ,",
    );
    retype(&mut app, Dialog::FOCUS_ISSUE_PREFIX, "bug/");
    retype(&mut app, Dialog::FOCUS_FEATURE_BRANCH, "");
    retype(&mut app, Dialog::FOCUS_SESSION_NAME, "s-{slug}");
    focus(&mut app, Dialog::FOCUS_NAMING_ENABLED);
    app.handle_key(key(KeyCode::Char(' ')));
    retype(&mut app, Dialog::FOCUS_NAMING_MODEL, "sonnet");
    retype(&mut app, Dialog::FOCUS_NAMING_TIMEOUT, "45");
    retype(&mut app, Dialog::FOCUS_NAMING_TOOLS, "Read, Grep");
    save(&mut app);

    let start_work = saved().start_work;
    assert_eq!(start_work.scope, crate::models::Scope::All);
    assert_eq!(start_work.feature_labels, ["kind/feature", "enhancement"]);
    assert_eq!(start_work.issue.branch_prefix, "bug/");
    assert_eq!(start_work.issue.name_pattern, "issue-{number}-{slug}");
    assert_eq!(
        start_work.feature.branch_pattern, "",
        "an empty pattern is allowed"
    );
    assert_eq!(start_work.session.name_pattern, "s-{slug}");
    assert!(!start_work.session_naming.enabled);
    assert_eq!(start_work.session_naming.model, "sonnet");
    assert_eq!(start_work.session_naming.timeout_secs, 45);
    assert_eq!(start_work.session_naming.allowed_tools, ["Read", "Grep"]);
    assert_eq!(app.settings.start_work, start_work);
    assert!(notified(&app).contains(&"Start work settings saved"));

    // Back on Settings, whose Save must not put the old values back.
    let Some(Modal::Settings(_)) = app.modals.last() else {
        panic!("Settings expected beneath");
    };
    if let Some(Modal::Settings(settings)) = app.modals.last_mut() {
        settings.focus = crate::modal::SettingsModal::FOCUS_SAVE;
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(saved().start_work, start_work);
}

/// Cancel and Esc throw the edits away and write nothing.
#[tokio::test]
async fn cancel_leaves_the_settings_untouched() {
    no_settings_file();
    let (_dir, mut app) = app_with_fixture();
    open_global(&mut app);
    retype(&mut app, Dialog::FOCUS_ISSUE_PREFIX, "bug/");
    focus(&mut app, Dialog::FOCUS_CANCEL);
    app.handle_key(key(KeyCode::Enter));
    assert!(matches!(app.modals.last(), Some(Modal::Settings(_))));

    open_global_again(&mut app);
    retype(&mut app, Dialog::FOCUS_ISSUE_PREFIX, "other/");
    app.handle_key(key(KeyCode::Esc));

    assert!(!crate::services::settings::settings_path().exists());
    assert_eq!(app.settings.start_work.issue.branch_prefix, "fix/");
}

fn open_global_again(app: &mut App) {
    if let Some(Modal::Settings(settings)) = app.modals.last_mut() {
        settings.focus = crate::modal::SettingsModal::FOCUS_START_WORK;
    }
    app.handle_key(key(KeyCode::Enter));
}

/// A timeout that is not a positive whole number is refused beside the
/// field, and Save stays disabled until it is fixed.
#[tokio::test]
async fn a_bad_timeout_disables_save() {
    no_settings_file();
    let (_dir, mut app) = app_with_fixture();
    open_global(&mut app);

    for bad in ["0", "ten", "", "-3"] {
        retype(&mut app, Dialog::FOCUS_NAMING_TIMEOUT, bad);
        assert!(!dialog(&app).can_save(), "{bad:?}");
        let screen = rendered(&mut app);
        assert!(
            screen.contains("must be a positive whole number"),
            "{bad:?}:\n{screen}"
        );
        save(&mut app);
        assert!(matches!(
            app.modals.last(),
            Some(Modal::StartWorkSettings(_))
        ));
    }
    assert!(!crate::services::settings::settings_path().exists());

    retype(&mut app, Dialog::FOCUS_NAMING_TIMEOUT, "5");
    assert!(!rendered(&mut app).contains("must be a positive whole number"));
    save(&mut app);
    assert_eq!(saved().start_work.session_naming.timeout_secs, 5);
}

/// Where `needle` is drawn on the 140×50 screen, as a click target.
fn find(app: &mut App, needle: &str) -> (u16, u16) {
    let screen = rendered(app);
    screen
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            line.find(needle)
                .map(|byte| (line[..byte].chars().count() as u16, row as u16))
        })
        .unwrap_or_else(|| panic!("{needle} not drawn:\n{screen}"))
}

/// Every field and button answers a click: a field takes the focus, a
/// choice cycles, and Cancel closes.
#[tokio::test]
async fn fields_and_buttons_take_clicks() {
    let (_dir, mut app) = app_with_fixture();
    open_global(&mut app);

    let (x, y) = find(&mut app, "pr-{number}-{slug}");
    app.handle_mouse(click(x, y));
    assert_eq!(dialog(&app).focus, Dialog::FOCUS_PR_REVIEW_NAME);

    let (x, y) = find(&mut app, "◂ Mine ▸");
    app.handle_mouse(click(x + 2, y));
    assert_eq!(dialog(&app).focus, Dialog::FOCUS_SCOPE);
    assert!(rendered(&mut app).contains("◂ Everyone ▸"));

    let (x, y) = find(&mut app, "[x] Enabled");
    app.handle_mouse(click(x, y));
    assert!(rendered(&mut app).contains("[ ] Enabled"));

    let (x, y) = find(&mut app, "Cancel");
    app.handle_mouse(click(x, y));
    assert!(matches!(app.modals.last(), Some(Modal::Settings(_))));
}

/// On a terminal too short for every field, the dialog scrolls to keep the
/// focused one in view, and the buttons stay on screen.
#[tokio::test]
async fn a_short_terminal_scrolls_to_the_focused_field() {
    let (_dir, mut app) = app_with_fixture();
    open_global(&mut app);
    let short = |app: &mut App| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 24)).expect("terminal");
        terminal
            .draw(|frame| crate::ui::draw(frame, app))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };

    let top = short(&mut app);
    assert!(top.contains("Default scope"), "{top}");
    assert!(!top.contains("Allowed tools"), "{top}");
    assert!(top.contains("Save"), "{top}");

    focus(&mut app, Dialog::FOCUS_NAMING_TOOLS);
    let bottom = short(&mut app);
    assert!(bottom.contains("Allowed tools"), "{bottom}");
    assert!(!bottom.contains("Default scope"), "{bottom}");
    assert!(bottom.contains("Save"), "{bottom}");

    // Tab walks the whole ring, and each field is drawn — clickable — as
    // it is reached.
    focus(&mut app, Dialog::FOCUS_SCOPE);
    for index in 0..crate::modal::START_WORK_FIELDS.len() {
        assert_eq!(dialog(&app).focus, index);
        short(&mut app);
        let drawn = app.hits.iter().any(
            |hit| matches!(hit.target, HitTarget::ModalControl { index: i, .. } if i == index),
        );
        assert!(drawn, "field {index} is out of view");
        app.handle_key(key(KeyCode::Tab));
    }
    assert_eq!(dialog(&app).focus, Dialog::FOCUS_SAVE);
}

/// Saving the global dialog changes no repository's overrides: an
/// overridden value keeps winning in its repository.
#[tokio::test]
async fn a_repository_override_still_wins_after_a_global_save() {
    no_settings_file();
    let repos = ReposDir::new();
    repos.write(
        "settings.json",
        r#"{"start_work": {"issue": {"branch_prefix": "repo/"}}}"#,
    );
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    on_github(&mut app, &path);
    open_global(&mut app);
    retype(&mut app, Dialog::FOCUS_ISSUE_PREFIX, "global/");
    retype(&mut app, Dialog::FOCUS_ISSUE_NAME, "i-{number}");
    save(&mut app);

    let effective = app.start_work_for(&path);
    assert_eq!(effective.issue.branch_prefix, "repo/");
    assert_eq!(effective.issue.name_pattern, "i-{number}");
    assert_eq!(
        std::fs::read_to_string(repos.repo().join("settings.json")).unwrap(),
        r#"{"start_work": {"issue": {"branch_prefix": "repo/"}}}"#
    );
}

// ----------------------------------------------------------- Repository

/// Repository Settings, its Start work settings row, for `octo/demo`.
fn open_repository(app: &mut App, path: &str) {
    on_github(app, path);
    app.sessions = Some(Vec::new());
    let items = detail::items(&detail::content(app));
    let index = items
        .iter()
        .position(|item| *item == DetailItem::Action(Action::RepositorySettings))
        .expect("the control is in the pane");
    // Enter resolves against the frame last drawn.
    rendered(app);
    app.focus = Focus::Detail;
    app.detail_index = index;
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(template_picker(app).selected(), ConfigFile::Settings);
    app.handle_key(key(KeyCode::Enter));
    assert!(dialog(app).is_repository());
}

fn repository_file(repos: &ReposDir) -> serde_json::Value {
    let raw = std::fs::read_to_string(repos.repo().join("settings.json")).expect("a file");
    serde_json::from_str(&raw).expect("JSON")
}

/// The row opens the dialog for the repository, every field inheriting:
/// a text field is empty and shows the global value dimmed, a choice says
/// what it inherits.
#[tokio::test]
async fn the_repository_row_opens_the_dialog_inheriting_everything() {
    let repos = ReposDir::new();
    let mut settings = crate::models::Settings::default();
    settings.start_work.issue.branch_prefix = "bug/".into();
    let (_dir, mut app, path) =
        app_with_settings_and_issues(settings, crate::models::Scope::Mine, Vec::new());
    open_repository(&mut app, &path);

    assert_eq!(text(&app, Dialog::FOCUS_ISSUE_PREFIX), "");
    assert_eq!(text(&app, Dialog::FOCUS_NAMING_TIMEOUT), "");
    assert!(dialog(&app).can_save(), "an empty timeout inherits");
    let screen = rendered(&mut app);
    for expected in [
        "Start Work Settings: octo/demo",
        "Overrides of the global settings",
        "bug/",
        "◂ Inherit (Mine) ▸",
        "◂ Inherit (On) ▸",
        "Edit settings.json...",
    ] {
        assert!(screen.contains(expected), "{expected}:\n{screen}");
    }
    assert!(
        !repos.repo().join("settings.json").exists(),
        "opening writes nothing"
    );
}

/// Typing overrides, clearing inherits again, and a choice cycles through
/// Inherit and each value.
#[tokio::test]
async fn typing_overrides_and_clearing_inherits() {
    let _repos = ReposDir::new();
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    open_repository(&mut app, &path);

    retype(&mut app, Dialog::FOCUS_ISSUE_PREFIX, "repo/");
    let changes = dialog(&app).changes();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].0, ["issue", "branch_prefix"]);
    retype(&mut app, Dialog::FOCUS_ISSUE_PREFIX, "");
    assert!(dialog(&app).changes().is_empty());
    retype(&mut app, Dialog::FOCUS_ISSUE_PREFIX, "   ");
    assert!(dialog(&app).changes().is_empty(), "spaces are no override");

    focus(&mut app, Dialog::FOCUS_SCOPE);
    let mut seen = Vec::new();
    for _ in 0..3 {
        app.handle_key(key(KeyCode::Enter));
        seen.push(dialog(&app).choice_label(Dialog::FOCUS_SCOPE));
    }
    assert_eq!(seen, ["Mine", "Everyone", "Inherit (Mine)"]);
    app.handle_key(key(KeyCode::Left));
    assert_eq!(dialog(&app).choice_label(Dialog::FOCUS_SCOPE), "Everyone");
}

/// Saving writes only the overridden keys — one half of a naming on its
/// own — and keeps every other key in the file, inside `start_work` or
/// not. The next Start work in the repository uses the new overrides.
#[tokio::test]
async fn saving_writes_only_the_overrides_and_keeps_the_rest() {
    let repos = ReposDir::new();
    repos.write(
        "settings.json",
        r#"{"future": {"x": 1}, "start_work": {"base_branch": "develop", "bug_labels": ["defect"], "issue": {"name_pattern": "i-{number}-{slug}"}, "unknown": true}}"#,
    );
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    open_repository(&mut app, &path);
    assert_eq!(text(&app, Dialog::FOCUS_ISSUE_NAME), "i-{number}-{slug}");

    retype(&mut app, Dialog::FOCUS_ISSUE_PREFIX, "repo/");
    retype(&mut app, Dialog::FOCUS_NAMING_TIMEOUT, "9");
    focus(&mut app, Dialog::FOCUS_NAMING_ENABLED);
    app.handle_key(key(KeyCode::Right));
    app.handle_key(key(KeyCode::Right));
    save(&mut app);

    assert_eq!(
        repository_file(&repos),
        serde_json::json!({
            "future": {"x": 1},
            "start_work": {
                "base_branch": "develop",
                "bug_labels": ["defect"],
                "issue": {"name_pattern": "i-{number}-{slug}", "branch_prefix": "repo/"},
                "unknown": true,
                "session_naming": {"timeout_secs": 9, "enabled": false}
            }
        })
    );
    let effective = app.start_work_for(&path);
    assert_eq!(effective.issue.branch_prefix, "repo/");
    assert_eq!(effective.issue.name_pattern, "i-{number}-{slug}");
    assert_eq!(effective.issue.branch_pattern, "issue-{number}-{slug}");
    assert_eq!(effective.session_naming.timeout_secs, 9);
    assert!(!effective.session_naming.enabled);
    assert!(
        app.session_name_call(&path, "Dark mode").is_none(),
        "the suggestion is off in this repository now"
    );
    assert_eq!(app.base_branch_for(&path).as_deref(), Some("develop"));
}

/// Inheriting the last override removes the `start_work` section rather
/// than leaving it empty; the rest of the file stays.
#[tokio::test]
async fn a_section_left_with_no_overrides_is_removed() {
    let repos = ReposDir::new();
    repos.write(
        "settings.json",
        r#"{"start_work": {"scope": "all", "session": {"branch_prefix": "s/"}}, "other": 1}"#,
    );
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    open_repository(&mut app, &path);
    focus(&mut app, Dialog::FOCUS_SCOPE);
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        dialog(&app).choice_label(Dialog::FOCUS_SCOPE),
        "Inherit (Mine)"
    );
    retype(&mut app, Dialog::FOCUS_SESSION_PREFIX, "");
    save(&mut app);

    assert_eq!(repository_file(&repos), serde_json::json!({"other": 1}));
    assert_eq!(app.start_work_for(&path).scope, crate::models::Scope::Mine);
}

/// A field the dialog cannot show as written — an explicitly empty list —
/// is left in the file when the user does not touch it.
#[tokio::test]
async fn an_untouched_empty_override_stays_in_the_file() {
    let repos = ReposDir::new();
    repos.write("settings.json", r#"{"start_work": {"feature_labels": []}}"#);
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    open_repository(&mut app, &path);
    assert!(
        rendered(&mut app).contains("(set empty in settings.json)"),
        "an empty override does not pass for inherited"
    );
    retype(&mut app, Dialog::FOCUS_NAMING_MODEL, "opus");
    save(&mut app);

    assert_eq!(
        repository_file(&repos),
        serde_json::json!({"start_work": {"feature_labels": [], "session_naming": {"model": "opus"}}})
    );
}

/// A file that cannot be parsed is never written over: the dialog says why
/// and offers only Cancel and the editor.
#[tokio::test]
async fn a_broken_file_is_never_overwritten() {
    let repos = ReposDir::new();
    repos.write("settings.json", "{ \"start_work\": ");
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    open_repository(&mut app, &path);

    assert!(!dialog(&app).can_save());
    assert_eq!(
        dialog(&app).ring(),
        [Dialog::FOCUS_EDIT_FILE, Dialog::FOCUS_CANCEL]
    );
    let screen = rendered(&mut app);
    assert!(screen.contains("does not parse"), "{screen}");
    assert!(!screen.contains("Save"), "{screen}");
    focus(&mut app, Dialog::FOCUS_CANCEL);
    app.handle_key(key(KeyCode::Enter));
    assert!(!matches!(
        app.modals.last(),
        Some(Modal::StartWorkSettings(_))
    ));

    // A save that gets as far as the App — the file broke after opening —
    // is refused there too.
    app.save_repository_start_work(
        test_key(),
        vec![(&["scope"], Some(serde_json::json!("all")))],
    );
    assert_eq!(
        std::fs::read_to_string(repos.repo().join("settings.json")).unwrap(),
        "{ \"start_work\": "
    );
    assert!(
        notified(&app)
            .iter()
            .any(|n| n.starts_with("Could not save octo/demo's settings")),
        "{:?}",
        notified(&app)
    );
}

/// Edit settings.json... opens the file in the editor, written from the
/// global settings if there is none yet, and the dialog stays up.
#[tokio::test]
async fn edit_settings_json_opens_the_file() {
    let repos = ReposDir::new();
    let mut settings = crate::models::Settings::default();
    settings.start_work.feature_labels = vec!["kind/feature".into()];
    let (_dir, mut app, path) =
        app_with_settings_and_issues(settings, crate::models::Scope::Mine, Vec::new());
    open_repository(&mut app, &path);
    focus(&mut app, Dialog::FOCUS_EDIT_FILE);
    app.handle_key(key(KeyCode::Enter));

    let loaded = crate::repo_settings::load(&repos.repo());
    assert_eq!(
        loaded.overrides.feature_labels,
        Some(vec!["kind/feature".to_string()])
    );
    assert!(matches!(
        app.modals.last(),
        Some(Modal::StartWorkSettings(_))
    ));
}
