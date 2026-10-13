use super::*;

/// This test thread's repository settings directory, removed again on drop
/// so a file written by one test cannot reach another run.
struct ReposDir(PathBuf);

impl ReposDir {
    fn new() -> Self {
        let dir = settings_service::repos_dir();
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }

    /// The directory of the repository every test here resolves to.
    fn repo(&self) -> PathBuf {
        test_key().dir(&self.0)
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.repo().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
}

impl Drop for ReposDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn test_key() -> crate::repo_settings::RepoKey {
    crate::repo_settings::RepoKey::new("octo", "demo").unwrap()
}

/// What the probe reports for a checkout that resolves to `octo/demo`.
fn on_github(app: &mut App, path: &str) {
    app.handle_event(AppEvent::GitHubChecked {
        path: path.to_string(),
        repo: Some(test_key()),
        for_start_work: false,
    });
}

fn repository_settings_control(app: &App) -> (usize, bool) {
    let items = detail::items(&detail::content(app));
    let index = items
        .iter()
        .position(|item| *item == DetailItem::Action(Action::RepositorySettings))
        .expect("the control is in the pane");
    let enabled = detail::content(app)
        .iter()
        .filter_map(|node| match node {
            detail::DetailNode::Controls { controls, .. } => Some(controls.clone()),
            _ => None,
        })
        .flatten()
        .find(|c| c.item == DetailItem::Action(Action::RepositorySettings))
        .map(|c| c.enabled)
        .unwrap();
    (index, enabled)
}

/// Without a GitHub repository there is no key to keep settings under: the
/// control stays in its slot, disabled, and the pane says why.
#[tokio::test]
async fn repository_settings_explain_why_a_repository_without_github_has_none() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    app.sessions = Some(Vec::new());

    let (_, enabled) = repository_settings_control(&app);
    assert!(!enabled, "not before gh has answered");

    no_github(&mut app, &path);
    let (index, enabled) = repository_settings_control(&app);
    assert!(!enabled);
    let screen = rendered(&mut app);
    assert!(
        screen.contains("No GitHub repository: the global settings apply"),
        "{screen}"
    );

    // Activating it anyway opens nothing.
    app.focus = Focus::Detail;
    app.detail_index = index;
    app.handle_key(key(KeyCode::Enter));
    assert!(app.modals.is_empty(), "{:?}", app.modals);
}

/// The control opens a chooser of the repository's files; picking the
/// settings file writes it from the global settings, outside the checkout.
#[tokio::test]
async fn repository_settings_prefill_the_chosen_file_outside_the_checkout() {
    let repos = ReposDir::new();
    let mut settings = crate::models::Settings::default();
    settings.start_work.feature_labels = vec!["kind/feature".into()];
    let (_dir, mut app, path) =
        app_with_settings_and_issues(settings, crate::models::Scope::Mine, Vec::new());
    std::fs::create_dir_all(&path).unwrap();
    app.sessions = Some(Vec::new());
    on_github(&mut app, &path);

    let (index, enabled) = repository_settings_control(&app);
    assert!(enabled);
    assert!(rendered(&mut app).contains("Settings for octo/demo"));
    app.focus = Focus::Detail;
    app.detail_index = index;
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(template_picker(&app).selected(), ConfigFile::Settings);
    app.handle_key(key(KeyCode::Enter));

    let written = repos.repo().join("settings.json");
    let loaded = crate::repo_settings::load(&repos.repo());
    assert_eq!(loaded.warning, None);
    assert_eq!(
        loaded.overrides.feature_labels,
        Some(vec!["kind/feature".to_string()]),
        "pre-filled from the global settings: {}",
        std::fs::read_to_string(&written).unwrap()
    );

    // A template is pre-filled from the global one, here the built-in
    // default, and lands in the repository's own prompts directory.
    app.handle_key(key(KeyCode::Down));
    assert_eq!(
        template_picker(&app).selected(),
        ConfigFile::Template(crate::prompt::Template::Issue)
    );
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(
        std::fs::read_to_string(repos.repo().join("prompts/issue.md")).unwrap(),
        crate::prompt::ISSUE_TEMPLATE
    );

    // Nothing reached the checkout, so git has nothing to report.
    assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
}

/// A repository's settings override the global ones in Start work: here
/// the issue branch prefix, while the pattern the file leaves out stays.
#[tokio::test]
async fn repository_settings_name_the_worktrees_of_their_repository() {
    let repos = ReposDir::new();
    repos.write(
        "settings.json",
        r#"{"start_work": {"issue": {"branch_prefix": "bug/"}}}"#,
    );
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    on_github(&mut app, &path);

    walk_to_confirm(&mut app, "login");

    let confirm = confirm_screen(&app);
    assert_eq!(confirm.branch.value(), "bug/issue-42-fix-login-bug");
    assert_eq!(confirm.name.value(), "issue-42-fix-login-bug");
}

/// A repository's own feature labels decide how its issues start: a
/// `kind/feature` issue is a feature there, and an `enhancement` one, a
/// feature under the global labels, is not.
#[tokio::test]
async fn a_repositorys_feature_labels_decide_how_its_issues_start() {
    let repos = ReposDir::new();
    repos.write(
        "settings.json",
        r#"{"start_work": {"feature_labels": ["kind/feature"]}}"#,
    );
    let issues = vec![
        labelled_issue(7, "Dark mode", &["kind/feature"]),
        labelled_issue(8, "Export", &["enhancement"]),
    ];
    for (query, branch) in [
        ("dark", "feat/7-dark-mode"),
        ("export", "fix/issue-8-export"),
    ] {
        let (_dir, mut app, path) = app_with_issues(issues.clone());
        on_github(&mut app, &path);

        walk_to_confirm(&mut app, query);

        assert_eq!(confirm_screen(&app).branch.value(), branch);
    }
}

/// A repository's own bug labels decide what its Bugs filter lists.
#[tokio::test]
async fn a_repositorys_bug_labels_decide_what_its_bugs_filter_lists() {
    let repos = ReposDir::new();
    repos.write(
        "settings.json",
        r#"{"start_work": {"bug_labels": ["kind/defect"]}}"#,
    );
    let settings = crate::models::Settings {
        issue_filter: crate::models::IssueFilter::Bugs,
        ..Default::default()
    };
    let (_dir, mut app, path) = app_with_settings_and_issues(
        settings,
        crate::models::Scope::Mine,
        vec![
            labelled_issue(1, "Global bug", &["bug"]),
            labelled_issue(2, "Repository bug", &["kind/defect"]),
        ],
    );
    on_github(&mut app, &path);
    app.handle_key(key(KeyCode::Char('s')));
    app.handle_key(key(KeyCode::Enter));

    assert_eq!(listed(&app), vec![2]);
}

/// A repository's template wins over the global one.
#[tokio::test]
async fn a_repository_template_fills_the_confirm_screens_prompt() {
    let repos = ReposDir::new();
    let prompts = PromptsDir::new();
    prompts.write(crate::prompt::Template::Issue, b"global #{number}");
    repos.write("prompts/issue.md", "repository #{number}");
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    on_github(&mut app, &path);

    walk_to_confirm(&mut app, "login");

    assert_eq!(confirm_screen(&app).prompt(), "repository #42");
}

/// A repository can name its sessions and give them its own first prompt,
/// one half of the naming at a time.
#[tokio::test]
async fn a_repository_overrides_the_session_naming_and_template() {
    let repos = ReposDir::new();
    let _prompts = PromptsDir::new();
    repos.write(
        "settings.json",
        r#"{"start_work": {"session": {"branch_prefix": "spike/"}}}"#,
    );
    repos.write("prompts/session.md", "In {worktree}: {body}");
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    on_github(&mut app, &path);

    app.handle_key(key(KeyCode::Char('s')));
    highlight_mode(&mut app, crate::work::WorkMode::Session);
    app.handle_key(key(KeyCode::Enter));
    type_text(&mut app, "Dark mode");
    app.handle_key(key(KeyCode::Enter));
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    let confirm = confirm_screen(&app);
    assert_eq!(confirm.name.value(), "session-dark-mode");
    assert_eq!(confirm.branch.value(), "spike/dark-mode");
    assert_eq!(confirm.prompt(), "In session-dark-mode: Dark mode");
}

/// A repository's scope is learned with its key, after the cache was
/// warmed with the global one's issues: the new scope is prefetched too, so
/// Start work's list is the one it will open on.
#[tokio::test]
async fn a_repository_scope_prefetches_its_issues() {
    use crate::models::Scope;
    let repos = ReposDir::new();
    repos.write("settings.json", r#"{"start_work": {"scope": "all"}}"#);
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(1, "Mine")]);
    app.reload_detail();
    assert!(
        !app.issues_in_flight.contains(&(path.clone(), Scope::All)),
        "nothing asked for the all list yet"
    );

    on_github(&mut app, &path);

    assert!(app.issues_in_flight.contains(&(path.clone(), Scope::All)));
}

/// A settings file that does not parse overrides nothing, and is pointed
/// out once rather than on every selection.
#[tokio::test]
async fn a_broken_repository_settings_file_warns_once() {
    let repos = ReposDir::new();
    repos.write("settings.json", "{ oops");
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(42, "Fix login bug")]);
    on_github(&mut app, &path);
    let warnings = |app: &App| {
        app.notifications
            .iter()
            .filter(|n| n.severity == Severity::Warning)
            .count()
    };
    assert_eq!(warnings(&app), 1);
    assert_eq!(
        app.start_work_for(&path),
        crate::models::StartWorkSettings::default()
    );

    app.reload_detail();
    app.reload_detail();
    assert_eq!(warnings(&app), 1);
}

/// `gh` answers no when it fails, too; a key already known is kept, and the
/// repository keeps its own settings.
#[tokio::test]
async fn a_failed_probe_keeps_the_known_repository_settings() {
    let repos = ReposDir::new();
    repos.write(
        "settings.json",
        r#"{"start_work": {"feature_labels": ["kind/feature"]}}"#,
    );
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    on_github(&mut app, &path);

    no_github(&mut app, &path);

    assert_eq!(app.repo_key(&path), Some(Some(&test_key())));
    assert_eq!(
        app.start_work_for(&path).feature_labels,
        vec!["kind/feature".to_string()]
    );
}

/// "No GitHub repository" is asked again on the next selection: a remote
/// may have been added since. A known key is not.
#[tokio::test]
async fn selecting_a_repository_asks_again_only_without_a_key() {
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    no_github(&mut app, &path);
    app.reload_detail();
    assert!(app.repo_probes.contains(&path));

    on_github(&mut app, &path);
    app.reload_detail();
    assert!(!app.repo_probes.contains(&path));
}

/// A repository's `session-name.md` and naming-call settings are the ones
/// its sessions' name suggestion uses, a key at a time.
#[tokio::test]
async fn a_repository_overrides_the_name_suggestion() {
    let repos = ReposDir::new();
    let prompts = PromptsDir::new();
    prompts.write(crate::prompt::Template::SessionName, b"global: {body}");
    repos.write(
        "settings.json",
        r#"{"start_work": {"session_naming": {"model": "opus"}}}"#,
    );
    repos.write("prompts/session-name.md", "repository: {body}");
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    on_github(&mut app, &path);

    let call = app.session_name_call(&path, "Dark mode").expect("a call");
    assert_eq!(call.instruction, "repository: Dark mode");
    assert_eq!(call.naming.model, "opus");
    assert_eq!(call.naming.timeout_secs, 20);

    repos.write(
        "settings.json",
        r#"{"start_work": {"session_naming": {"enabled": false}}}"#,
    );
    on_github(&mut app, &path);
    assert!(app.session_name_call(&path, "Dark mode").is_none());
}
