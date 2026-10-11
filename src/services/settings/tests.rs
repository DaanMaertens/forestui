use super::*;
use crate::models::{Scope, StartWorkSettings};

#[test]
fn missing_file_yields_defaults() {
    let s = load_settings_from(Path::new("/nonexistent/settings.json"));
    assert_eq!(s.default_editor, "vim");
    assert_eq!(s.branch_prefix, "feat/");
}

#[test]
fn partial_file_fills_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    std::fs::write(&p, r#"{"default_editor": "nvim"}"#).unwrap();
    let s = load_settings_from(&p);
    assert_eq!(s.default_editor, "nvim");
    assert_eq!(s.theme_name, "forest-dark");
    assert!(s.custom_buttons.is_empty());
    // A file from before the checkbox existed turns Remote Control on:
    // an update picks it up without anyone opening Settings.
    assert!(s.remote_control);
}

#[test]
fn remote_control_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    assert!(Settings::default().remote_control, "on for a new install");

    // Unticking is the one way to turn it off, and it has to stick: the
    // `false` is written out and read back rather than defaulted over.
    let s = Settings {
        remote_control: false,
        ..Settings::default()
    };
    save_settings_to(&p, &s).unwrap();
    let raw = std::fs::read_to_string(&p).unwrap();
    assert!(raw.contains("\"remote_control\": false"), "{raw}");
    assert!(!load_settings_from(&p).remote_control);
}

/// Keys this build no longer has — the old `theme` and `default_terminal`
/// — must not stop a file from loading, or an update would throw away the
/// user's settings; they are dropped on the next save.
#[test]
fn retired_keys_load_and_are_dropped_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    std::fs::write(
        &p,
        r#"{"default_editor": "nvim", "default_terminal": "", "theme": "dark",
                "theme_name": "nord", "custom_buttons": []}"#,
    )
    .unwrap();
    let s = load_settings_from(&p);
    assert_eq!(s.default_editor, "nvim");
    assert_eq!(s.theme_name, "nord", "the file loaded, not the defaults");

    save_settings_to(&p, &s).unwrap();
    let raw = std::fs::read_to_string(&p).unwrap();
    assert!(!raw.contains("default_terminal"), "{raw}");
    assert!(!raw.contains("\"theme\""), "{raw}");
}

#[test]
fn roundtrip_keeps_field_names() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    let s = Settings {
        default_editor: "hx".into(),
        ..Settings::default()
    };
    save_settings_to(&p, &s).unwrap();
    let raw = std::fs::read_to_string(&p).unwrap();
    assert!(raw.contains("\"default_editor\""));
    assert!(raw.contains("\"branch_prefix\""));
    assert!(raw.contains("\"custom_buttons\""));
    assert_eq!(load_settings_from(&p).default_editor, "hx");
}

/// A file written before Start work had settings loads with the behaviour
/// that shipped: my items, `enhancement` as the feature label, and the
/// names Start work has always made.
#[test]
fn a_file_without_start_work_loads_todays_behaviour() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    std::fs::write(&p, r#"{"default_editor": "nvim", "custom_buttons": []}"#).unwrap();
    let s = load_settings_from(&p);
    let start = &s.start_work;
    assert_eq!(start.scope, Scope::Mine);
    assert_eq!(start.feature_labels, vec!["enhancement".to_string()]);
    assert_eq!(start.issue.branch_prefix, "fix/");
    assert_eq!(start.issue.name_pattern, "issue-{number}-{slug}");
    assert_eq!(start.feature.branch_prefix, "feat/");
    assert_eq!(start.feature.name_pattern, "feat-{number}-{slug}");
    assert_eq!(start.free_feature.branch_prefix, "feat/");
    assert_eq!(start.free_feature.name_pattern, "feat-{slug}");
    assert_eq!(start.pr_review.name_pattern, "pr-{number}-{slug}");
    assert_eq!(*start, StartWorkSettings::default());
}

/// Each key that is there is taken, and each one that is not keeps its
/// default — down to one half of a mode's naming.
#[test]
fn a_partial_start_work_section_fills_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    std::fs::write(
        &p,
        r#"{"start_work": {
            "scope": "all",
            "issue": {"branch_prefix": "bug/"},
            "feature": {"name_pattern": "f{number}"}
        }}"#,
    )
    .unwrap();
    let start = load_settings_from(&p).start_work;
    assert_eq!(start.scope, Scope::All);
    assert_eq!(start.feature_labels, vec!["enhancement".to_string()]);
    assert_eq!(start.issue.branch_prefix, "bug/");
    assert_eq!(start.issue.name_pattern, "issue-{number}-{slug}");
    assert_eq!(start.feature.branch_prefix, "feat/");
    assert_eq!(start.feature.name_pattern, "f{number}");
    assert_eq!(
        start.free_feature,
        StartWorkSettings::default().free_feature
    );
}

/// A hand-edit that does not parse costs the Start work section, not the
/// whole file: the theme and the rest still load.
#[test]
fn a_malformed_start_work_section_loads_as_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    std::fs::write(
        &p,
        r#"{"theme_name": "nord", "start_work": {"scope": "everyone", "feature_labels": "feature"}}"#,
    )
    .unwrap();
    let s = load_settings_from(&p);
    assert_eq!(s.theme_name, "nord");
    assert_eq!(s.start_work, StartWorkSettings::default());
}

#[test]
fn start_work_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    let mut s = Settings::default();
    s.start_work.scope = Scope::All;
    s.start_work.feature_labels = vec!["feature".into(), "type: feature".into()];
    s.start_work.issue.branch_prefix = "bug/".into();
    s.start_work.pr_review.name_pattern = "review-{number}".into();
    save_settings_to(&p, &s).unwrap();
    let raw = std::fs::read_to_string(&p).unwrap();
    assert!(raw.contains("\"scope\": \"all\""), "{raw}");
    assert_eq!(load_settings_from(&p).start_work, s.start_work);
}
