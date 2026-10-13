use super::*;

#[test]
fn install_then_status_then_uninstall() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("skills").join(PLUGIN_NAME);

    assert_eq!(status_in(&dir), Status::NotInstalled);
    install_in(&dir, false).expect("a clean install");
    assert_eq!(status_in(&dir), Status::Installed);

    // Installing twice is not an error and not a change.
    install_in(&dir, false).expect("re-install over an identical copy");
    assert_eq!(status_in(&dir), Status::Installed);

    uninstall_in(&dir).expect("uninstall");
    assert!(!dir.exists());
    // Uninstalling what is not there is a no-op, not a failure.
    uninstall_in(&dir).expect("second uninstall");
}

/// An install by an older forestui is not the user's work. Reporting it as
/// drift would make every upgrade look like something to be careful about.
#[test]
fn an_older_install_is_outdated_not_drifted() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(PLUGIN_NAME);
    install_in(&dir, false).unwrap();

    let manifest = dir.join(".claude-plugin/plugin.json");
    let old = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace(SHIPPED_VERSION, "1");
    std::fs::write(&manifest, old).unwrap();

    match status_in(&dir) {
        Status::Outdated { installed } => assert_eq!(installed, "1"),
        other => panic!("expected Outdated, got {other:?}"),
    }
    // Upgrading is not clobbering, so it needs no overwrite flag.
    install_in(&dir, false).expect("an upgrade installs without confirmation");
    assert_eq!(status_in(&dir), Status::Installed);
}

/// A hand-edited hook is the user's work. Reporting it and stopping is the
/// whole point of hashing the files.
#[test]
fn a_hand_edited_file_is_drift_and_is_not_clobbered() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(PLUGIN_NAME);
    install_in(&dir, false).unwrap();

    let script = dir.join(EXECUTABLES[0]);
    std::fs::write(&script, "#!/bin/sh\n# my own version\n").unwrap();

    match status_in(&dir) {
        Status::Drifted(files) => assert_eq!(files, vec![EXECUTABLES[0].to_string()]),
        other => panic!("expected drift, got {other:?}"),
    }

    let refused = install_in(&dir, false).expect_err("install must refuse to clobber");
    assert!(refused.contains(EXECUTABLES[0]));
    assert!(
        std::fs::read_to_string(&script)
            .unwrap()
            .contains("my own version")
    );

    install_in(&dir, true).expect("an explicit overwrite is allowed");
    assert_eq!(status_in(&dir), Status::Installed);
}

/// The startup nag fires for an outdated install and nothing else:
/// not-installed was never chosen, drift is the user's own edit, and a
/// current install has nothing to say.
#[test]
fn only_an_outdated_install_earns_the_startup_notice() {
    let outdated = Status::Outdated {
        installed: "3".into(),
    };
    let notice = upgrade_notice(&outdated).expect("outdated must notify");
    assert!(notice.contains("v3"), "{notice}");
    assert!(notice.contains(SHIPPED_VERSION), "{notice}");
    assert!(
        notice.contains("Settings > Claude Code Integration"),
        "{notice}"
    );

    assert_eq!(upgrade_notice(&Status::NotInstalled), None);
    assert_eq!(upgrade_notice(&Status::Installed), None);
    assert_eq!(
        upgrade_notice(&Status::Drifted(vec!["hooks/tmux-title.sh".into()])),
        None
    );
}

/// And the real status of a freshly written old-version install produces
/// that notice end to end.
#[test]
fn an_old_install_reads_as_outdated_and_notifies() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(PLUGIN_NAME);
    install_in(&dir, false).unwrap();
    std::fs::write(
        dir.join(".claude-plugin/plugin.json"),
        format!("{{\"name\":\"{PLUGIN_NAME}\",\"version\":\"3\"}}"),
    )
    .unwrap();

    let status = status_in(&dir);
    assert!(matches!(status, Status::Outdated { .. }), "{status:?}");
    assert!(upgrade_notice(&status).is_some());
}

/// A recursive delete guarded by a substring match would take
/// "forestui-integration-fork" with it.
#[test]
fn uninstall_refuses_a_plugin_whose_name_merely_contains_ours() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("forestui-integration-fork");
    std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
    std::fs::write(
        dir.join(".claude-plugin/plugin.json"),
        "{\"name\":\"forestui-integration-fork\"}",
    )
    .unwrap();

    uninstall_in(&dir).expect_err("must refuse a different plugin");
    assert!(dir.exists());
}

#[test]
fn uninstall_refuses_a_directory_it_does_not_own() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("someone-elses-plugin");
    std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
    std::fs::write(
        dir.join(".claude-plugin/plugin.json"),
        "{\"name\":\"something-else\"}",
    )
    .unwrap();

    uninstall_in(&dir).expect_err("must refuse");
    assert!(dir.exists());
}

/// The hook is run as a command by Claude; a non-executable file is a
/// silent no-op that would look exactly like the feature not working.
#[cfg(unix)]
#[test]
fn the_hook_script_is_installed_executable() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join(PLUGIN_NAME);
    install_in(&dir, false).unwrap();

    for rel in EXECUTABLES {
        let mode = std::fs::metadata(dir.join(rel))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "{rel}: owner/group/other execute");
    }
}

/// Every script `hooks.json` registers has to be shipped and executable —
/// a hook pointing at a file the install never wrote fails silently inside
/// Claude, which reads exactly like the feature not working.
#[test]
fn every_registered_hook_script_is_shipped_and_executable() {
    let hooks: serde_json::Value =
        serde_json::from_str(include_str!("../../../assets/claude-plugin/hooks.json")).unwrap();
    let mut scripts: Vec<String> = Vec::new();
    for groups in hooks["hooks"].as_object().unwrap().values() {
        for group in groups.as_array().unwrap() {
            for hook in group["hooks"].as_array().unwrap() {
                let command = hook["command"].as_str().unwrap();
                let script = command
                    .strip_prefix("${CLAUDE_PLUGIN_ROOT}/")
                    .and_then(|rest| rest.split_whitespace().next())
                    .unwrap_or_else(|| panic!("unexpected hook command {command:?}"));
                scripts.push(script.to_string());
            }
        }
    }
    assert!(scripts.iter().any(|s| s == "hooks/state.sh"));
    for script in scripts {
        assert!(
            asset_names().contains(&script.as_str()),
            "{script} is not shipped"
        );
        assert!(
            EXECUTABLES.contains(&script.as_str()),
            "{script} is not executable"
        );
    }
}
