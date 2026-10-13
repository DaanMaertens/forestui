use super::*;
use crate::models::{Naming, Scope, StartWorkSettings};

fn global() -> StartWorkSettings {
    StartWorkSettings {
        scope: Scope::All,
        feature_labels: vec!["feature".into()],
        ..StartWorkSettings::default()
    }
}

fn overrides_from(json: &str) -> PartialStartWork {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(SETTINGS_FILE), json).unwrap();
    let loaded = load(dir.path());
    assert_eq!(loaded.warning, None);
    loaded.overrides
}

#[test]
fn no_repository_file_leaves_the_global_settings_as_they_are() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = load(&dir.path().join("missing"));
    assert_eq!(loaded.warning, None);
    assert_eq!(effective(&global(), &loaded.overrides), global());
}

/// A repository's bug labels replace the global ones; left out, they are
/// the global ones.
#[test]
fn a_repository_can_name_its_own_bug_labels() {
    let overrides = overrides_from(r#"{"start_work": {"bug_labels": ["kind/defect"]}}"#);
    assert_eq!(
        effective(&global(), &overrides).bug_labels,
        vec!["kind/defect".to_string()]
    );
    let overrides = overrides_from(r#"{"start_work": {}}"#);
    assert_eq!(
        effective(&global(), &overrides).bug_labels,
        global().bug_labels
    );
}

#[test]
fn repository_values_are_laid_over_the_global_ones() {
    let overrides =
        overrides_from(r#"{"start_work": {"scope": "mine", "feature_labels": ["type: feature"]}}"#);
    let merged = effective(&global(), &overrides);
    assert_eq!(merged.scope, Scope::Mine);
    assert_eq!(merged.feature_labels, vec!["type: feature".to_string()]);
    // Everything the file leaves out is the global value.
    assert_eq!(merged.issue, global().issue);
    assert_eq!(merged.pr_review, global().pr_review);
}

#[test]
fn half_of_a_modes_naming_keeps_the_global_other_half() {
    let mut base = global();
    base.issue = Naming {
        branch_prefix: "bug/".into(),
        name_pattern: "bug-{number}".into(),
        branch_pattern: "bug-{number}".into(),
    };
    let overrides = overrides_from(r#"{"start_work": {"issue": {"branch_prefix": "fix/"}}}"#);
    let merged = effective(&base, &overrides);
    assert_eq!(merged.issue.branch_prefix, "fix/");
    assert_eq!(merged.issue.name_pattern, "bug-{number}");
}

/// A repository can rename its branches without touching its worktree names.
#[test]
fn a_repository_can_set_only_the_branch_pattern() {
    let overrides =
        overrides_from(r#"{"start_work": {"feature": {"branch_pattern": "f{number}"}}}"#);
    let merged = effective(&global(), &overrides);
    assert_eq!(merged.feature.branch_pattern, "f{number}");
    assert_eq!(merged.feature.branch_prefix, "feat/");
    assert_eq!(merged.feature.name_pattern, "feat-{number}-{slug}");
}

#[test]
fn keys_outside_start_work_are_ignored() {
    let overrides = overrides_from(r#"{"theme_name": "x", "start_work": {}}"#);
    assert_eq!(effective(&global(), &overrides), global());
}

#[test]
fn a_file_that_does_not_parse_overrides_nothing_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(SETTINGS_FILE), "{ not json").unwrap();
    let loaded = load(dir.path());
    assert_eq!(loaded.overrides, PartialStartWork::default());
    let warning = loaded.warning.expect("a warning");
    assert!(warning.contains(SETTINGS_FILE), "{warning}");
}

#[test]
fn the_settings_file_is_prefilled_from_the_global_settings() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("owner/name");
    let path = ensure_settings_file(&repo, &global()).unwrap();
    assert_eq!(path, repo.join(SETTINGS_FILE));
    // Read back, it reproduces the global settings exactly.
    let loaded = load(&repo);
    assert_eq!(loaded.warning, None);
    assert_eq!(
        effective(&StartWorkSettings::default(), &loaded.overrides),
        global()
    );
}

#[test]
fn an_existing_settings_file_is_never_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(SETTINGS_FILE), "mine").unwrap();
    ensure_settings_file(dir.path(), &global()).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap(),
        "mine"
    );
}

#[test]
fn a_key_is_an_owner_and_a_name_and_nothing_that_leaves_its_directory() {
    let key = RepoKey::new("DaanMaertens", "forestui").unwrap();
    assert_eq!(key.to_string(), "DaanMaertens/forestui");
    assert_eq!(
        key.dir(Path::new("/r")),
        Path::new("/r/DaanMaertens/forestui")
    );
    assert_eq!(RepoKey::new("", "x"), None);
    assert_eq!(RepoKey::new("..", "x"), None);
    assert_eq!(RepoKey::new("a", "."), None);
    assert_eq!(RepoKey::new("a/b", "x"), None);
    assert_eq!(RepoKey::new("a", "b\\c"), None);
}

#[test]
fn a_repository_template_is_prefilled_from_the_global_one() {
    let dir = tempfile::tempdir().unwrap();
    let global_prompts = dir.path().join("prompts");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&global_prompts).unwrap();
    std::fs::write(
        global_prompts.join(Template::Issue.file_name()),
        "global issue",
    )
    .unwrap();

    let path = ensure_file(
        &repo,
        ConfigFile::Template(Template::Issue),
        &global_prompts,
        &global(),
    )
    .unwrap();
    assert_eq!(path, repo.join("prompts").join("issue.md"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "global issue");

    // Without a global file, the built-in default.
    let path = ensure_file(
        &repo,
        ConfigFile::Template(Template::Feature),
        &global_prompts,
        &global(),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        crate::prompt::FEATURE_TEMPLATE
    );
}
