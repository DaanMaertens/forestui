use super::*;

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
