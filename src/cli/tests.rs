use super::*;

#[test]
fn window_names() {
    assert_eq!(window_name(false), "forestui");
    assert!(window_name(true).starts_with("forestui-dev-"));
    assert!(is_forestui_window("forestui"));
    assert!(is_forestui_window("forestui-dev-0930"));
    assert!(!is_forestui_window("claude:demo"));
}

#[test]
fn session_names_follow_the_forest_folder() {
    assert_eq!(session_name(None), "forestui-forest");
    assert_eq!(session_name(Some("/tmp/My Work")), "forestui-my-work");
}

#[test]
fn self_command_quotes_and_carries_flags() {
    let args = Args {
        forest_path: Some("/tmp/my forest".into()),
        no_self_update: true,
        debug_mode: false,
        dev_mode: true,
        no_hover: true,
        no_focus_events: false,
        claude_plugin: None,
    };
    let command = self_command(&args, Path::new("/usr/local/bin/forestui"));
    assert!(command.starts_with("/usr/local/bin/forestui"));
    assert!(command.contains("--no-self-update"));
    assert!(command.contains("--dev"));
    assert!(
        command.contains("--no-hover"),
        "a flag the re-exec drops is a flag that does nothing"
    );
    assert!(!command.contains("--no-focus-events"));
    assert!(!command.contains("--debug"));
    assert!(command.contains("'/tmp/my forest'"));
}

/// `--claude-plugin` reports and exits before anything re-executes into
/// tmux. Carrying it through would hand the new process the same one-shot
/// action, which exits and re-executes again — a loop that never draws a UI.
/// Each flag takes away exactly its own mode. They are near-identical
/// booleans one negation apart, so a swap would leave `--no-hover` quietly
/// turning off focus reporting instead.
#[test]
fn each_flag_declines_only_its_own_mode() {
    let args = |no_hover, no_focus_events| Args {
        forest_path: None,
        no_self_update: false,
        debug_mode: false,
        dev_mode: false,
        no_hover,
        no_focus_events,
        claude_plugin: None,
    };

    assert_eq!(
        args(false, false).input_modes(),
        crate::terminal::InputModes::default()
    );
    assert_eq!(
        args(true, false).input_modes(),
        crate::terminal::InputModes {
            hover: false,
            focus: true
        }
    );
    assert_eq!(
        args(false, true).input_modes(),
        crate::terminal::InputModes {
            hover: true,
            focus: false
        }
    );
}

#[test]
fn a_one_shot_action_is_not_carried_into_the_re_exec() {
    let args = Args {
        forest_path: None,
        no_self_update: false,
        debug_mode: false,
        dev_mode: false,
        no_hover: false,
        no_focus_events: false,
        claude_plugin: Some(ClaudePluginAction::Install),
    };
    let command = self_command(&args, Path::new("/usr/local/bin/forestui"));
    assert!(
        !command.contains("claude-plugin"),
        "the re-exec would run the action again: {command}"
    );
}

#[test]
fn self_command_quotes_paths_with_spaces_in_the_program() {
    let args = Args {
        forest_path: None,
        no_self_update: false,
        debug_mode: false,
        dev_mode: false,
        no_hover: false,
        no_focus_events: false,
        claude_plugin: None,
    };
    let command = self_command(&args, Path::new("/tmp/my dir/forestui"));
    assert_eq!(command, "'/tmp/my dir/forestui'");
}
