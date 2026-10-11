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

/// A repository's scope is learned with its key, after the pane listed the
/// global one's issues: the list is fetched again for the new scope, and a
/// list still arriving for the old one is not shown.
#[tokio::test]
async fn a_repository_scope_relists_the_issues() {
    use crate::models::Scope;
    let repos = ReposDir::new();
    repos.write("settings.json", r#"{"start_work": {"scope": "all"}}"#);
    let (_dir, mut app, path) = app_with_issues(vec![open_issue(1, "Mine")]);
    app.reload_detail();
    assert!(app.issues.is_some(), "the mine list, from the cache");

    on_github(&mut app, &path);

    assert!(app.issues.is_none(), "the mine list is not the one wanted");
    assert!(app.issues_in_flight.contains(&(path.clone(), Scope::All)));
    app.handle_event(AppEvent::Issues {
        path: path.clone(),
        scope: Scope::Mine,
        issues: vec![open_issue(1, "Mine")],
    });
    assert!(app.issues.is_none(), "a late mine list is dropped");
    let screen = rendered(&mut app);
    assert!(screen.contains("OPEN GITHUB ISSUES"), "{screen}");
    assert!(!screen.contains("MY OPEN GITHUB ISSUES"), "{screen}");
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

/// An issue card splits by the repository's feature labels, as the picker
/// does.
#[tokio::test]
async fn the_issue_card_follows_the_repositorys_feature_labels() {
    let repos = ReposDir::new();
    repos.write(
        "settings.json",
        r#"{"start_work": {"feature_labels": ["kind/feature"]}}"#,
    );
    let (_dir, mut app, path) = app_with_issues(Vec::new());
    on_github(&mut app, &path);
    app.issues = Some(vec![labelled_issue(7, "Dark mode", &["kind/feature"])]);

    app.run_action(Action::StartWorkOnIssue(0));
    let loaded = branches_loaded(&app);
    app.handle_event(loaded);

    assert_eq!(confirm_screen(&app).mode, crate::work::WorkMode::Feature);
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
