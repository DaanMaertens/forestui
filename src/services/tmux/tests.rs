use super::*;

/// The bug this replaced: a forestui opened on a second forest created its
/// windows in the *first* forestui's session, because ungrouped sessions
/// report an empty group and the scan then ranged over the whole server,
/// returning whichever terminal the user had touched most recently.
#[test]
fn a_client_outside_our_group_never_wins() {
    let clients = "100 $1 ours\n900 $2 someone-else\n300 $3 ours\n";
    assert_eq!(most_active_in_group(clients, "ours", "$9"), "$3");
}

/// An ungrouped session short-circuits before this is reached, but a group
/// whose clients have all detached must still answer with our own session
/// rather than someone else's.
#[test]
fn our_own_session_is_the_fallback() {
    assert_eq!(most_active_in_group("900 $2 other\n", "ours", "$7"), "$7");
    assert_eq!(most_active_in_group("", "ours", "$7"), "$7");
    assert_eq!(most_active_in_group("garbage\n", "ours", "$7"), "$7");
}

/// Within our own group the windows are shared, so the most recently active
/// client is the right one: the window opens where the user is looking.
#[test]
fn the_most_recently_active_client_in_our_group_wins() {
    let clients = "100 $1 ours\n500 $2 ours\n300 $3 ours\n";
    assert_eq!(most_active_in_group(clients, "ours", "$1"), "$2");
}

/// The window name and the session name are one string. Resuming must not
/// re-apply a prefix, or a session resumed a few times ends up called
/// `claude:claude:claude:thing`.
#[test]
fn a_resumed_session_keeps_its_name_verbatim() {
    assert_eq!(
        opening_window_name(Some("yolo:forestuiNAMESYNC"), "repo:wt", false, None),
        "yolo:forestuiNAMESYNC"
    );
    assert_eq!(
        opening_window_name(Some("retry loop"), "repo:wt", true, Some("opus")),
        "retry loop"
    );

    // Resuming the same session again is a fixed point.
    let once = opening_window_name(Some("claude:demo:wt"), "demo:wt", false, None);
    let twice = opening_window_name(Some(&once), "demo:wt", false, None);
    assert_eq!(once, twice);

    // A session with no name of its own still gets forestui's opening name.
    assert_eq!(
        opening_window_name(None, "repo:wt", true, None),
        "yolo:repo:wt"
    );
    assert_eq!(
        opening_window_name(None, "repo:wt", false, Some("opus")),
        "opus:repo:wt"
    );
}

/// The test binary inherits the developer's own `TMUX`/`TMUX_PANE`, and
/// the developer working on forestui is running forestui — so a test that
/// reaches any tmux command is touching their live server. The gate in
/// `tmux()` makes every such call answer as if there were no tmux at all.
/// This assertion is the proof precisely when the suite runs inside tmux
/// (where it once opened a real window), and trivially true outside.
#[test]
fn no_test_can_reach_a_real_tmux_server() {
    assert!(current_session().is_none(), "the cfg!(test) gate is gone");
    assert!(!rename_window_by_id("@0", "forestui-test-probe"));
    assert!(!select_window("@0"));
    assert!(list_claude_windows().is_empty());
    assert!(create_claude_window("forestui-test-probe", "/tmp", ClaudeLaunch::default()).is_none());
}

#[test]
fn tui_editor_detection() {
    assert!(is_tui_editor("vim"));
    assert!(is_tui_editor("emacs -nw"));
    assert!(!is_tui_editor("code"));
    assert!(!is_tui_editor(""));
}

#[test]
fn unique_window_names() {
    let existing = vec!["term:a".to_string(), "term:a:2".to_string()];
    assert_eq!(unique_name_among("term:b", &existing), "term:b");
    assert_eq!(unique_name_among("term:a", &existing), "term:a:3");
}

#[test]
fn claude_window_naming() {
    assert_eq!(
        claude_base_window_name("repo:wt", false, None),
        "claude:repo:wt"
    );
    assert_eq!(
        claude_base_window_name("repo:wt", true, None),
        "yolo:repo:wt"
    );
    assert_eq!(
        claude_base_window_name("repo:wt", true, Some("opus")),
        "opus:repo:wt"
    );
}

#[test]
fn claude_command_building() {
    let build = |resume_session_id, yolo, custom_command, custom_prefix| {
        claude_command_line(
            &ClaudeLaunch {
                resume_session_id,
                yolo,
                custom_command,
                custom_prefix,
                remote_control: false,
                first_prompt: None,
            },
            "claude:wt",
            None,
        )
    };

    // A fresh session is named at launch; a resumed one keeps its own name.
    assert_eq!(build(None, false, None, None), "claude -n 'claude:wt'");
    assert_eq!(
        build(None, true, None, None),
        "claude --dangerously-skip-permissions -n 'claude:wt'"
    );
    assert_eq!(
        build(Some("abc123"), false, None, None),
        "claude -r 'abc123'"
    );
    // A resume id is a transcript filename stem — read off disk, not
    // written by forestui — so it is quoted like every other foreign
    // string that reaches a shell.
    assert_eq!(
        build(Some("x; rm -rf ~"), false, None, None),
        "claude -r 'x; rm -rf ~'"
    );
    // A custom button never gets the YOLO flag appended.
    assert_eq!(
        build(None, true, Some("claude --model opus"), Some("opus")),
        "claude --model opus -n 'claude:wt'"
    );
}

/// A fresh session runs under the id forestui minted, so the window knows
/// which session it holds from birth. A resumed session must never get the
/// flag — it already *has* an id, and `--session-id` alongside `-r` would
/// be a contradiction for Claude to resolve.
#[test]
fn a_preminted_session_id_reaches_fresh_sessions_only() {
    assert_eq!(
        claude_command_line(&ClaudeLaunch::default(), "claude:wt", Some("id-123")),
        "claude -n 'claude:wt' --session-id 'id-123'"
    );
    let resume = ClaudeLaunch {
        resume_session_id: Some("abc"),
        ..ClaudeLaunch::default()
    };
    assert_eq!(
        claude_command_line(&resume, "claude:wt", Some("id-123")),
        "claude -r 'abc'"
    );
}

/// Remote Control is passed bare, never with a name. Claude titles the
/// remote session after the session itself — verified against a phone for
/// a fresh `-n` launch, a resume with no `-n`, and a `/rename` — so a name
/// here could only disagree with it, and on a resume would be the window's
/// possibly `:2`-suffixed name.
#[test]
fn remote_control_is_passed_bare_on_every_built_in_launch() {
    let rc = ClaudeLaunch {
        remote_control: true,
        ..ClaudeLaunch::default()
    };
    assert_eq!(
        claude_command_line(&rc, "claude:wt", Some("id-1")),
        "claude --remote-control -n 'claude:wt' --session-id 'id-1'"
    );
    assert_eq!(
        claude_command_line(&ClaudeLaunch { yolo: true, ..rc }, "yolo:wt", None),
        "claude --dangerously-skip-permissions --remote-control -n 'yolo:wt'"
    );
    let resume = ClaudeLaunch {
        resume_session_id: Some("abc"),
        ..rc
    };
    assert_eq!(
        claude_command_line(&resume, "claude:wt:2", None),
        "claude --remote-control -r 'abc'"
    );
    assert_eq!(
        claude_command_line(
            &ClaudeLaunch {
                yolo: true,
                ..resume
            },
            "yolo:wt",
            None
        ),
        "claude --dangerously-skip-permissions --remote-control -r 'abc'"
    );

    // Off means absent, not `--remote-control=false` or similar.
    assert!(
        !claude_command_line(&ClaudeLaunch::default(), "claude:wt", None)
            .contains("remote-control")
    );
}

/// `--remote-control [name]` takes an optional value, so whatever follows
/// the flag is at risk of becoming the remote session's name. It must
/// always be followed by another flag.
#[test]
fn remote_control_is_never_followed_by_a_bare_word() {
    let custom = [None, Some(("claude --model opus", "opus"))];
    for resume_session_id in [None, Some("abc")] {
        for (yolo, custom) in [false, true]
            .into_iter()
            .flat_map(|y| custom.map(|c| (y, c)))
        {
            let launch = ClaudeLaunch {
                resume_session_id,
                yolo,
                custom_command: custom.map(|(command, _)| command),
                custom_prefix: custom.map(|(_, prefix)| prefix),
                remote_control: true,
                first_prompt: None,
            };
            let line = claude_command_line(&launch, "claude:wt", Some("id-1"));
            let words: Vec<&str> = line.split(' ').collect();
            let at = words
                .iter()
                .position(|w| *w == "--remote-control")
                .expect("the flag is there");
            assert!(
                words.get(at + 1).is_some_and(|next| next.starts_with('-')),
                "{line:?}: the flag could take the next word as its name"
            );
        }
    }
}

/// The history line is how a session comes back after a Ctrl-C, so it
/// must come back with Remote Control still on — otherwise the one restart
/// the user did not plan is the one that leaves the phone unable to reach it.
#[test]
fn the_history_line_keeps_remote_control() {
    let fresh = ClaudeLaunch {
        yolo: true,
        remote_control: true,
        ..ClaudeLaunch::default()
    };
    let (line, remembered) = launch_lines(&fresh, "yolo:wt", "id-1");
    assert_eq!(
        line,
        "claude --dangerously-skip-permissions --remote-control -n 'yolo:wt' --session-id 'id-1'"
    );
    assert_eq!(
        remembered,
        "claude --dangerously-skip-permissions --remote-control -r 'id-1'"
    );

    let resume = ClaudeLaunch {
        resume_session_id: Some("id-2"),
        remote_control: true,
        ..ClaudeLaunch::default()
    };
    let (line, remembered) = launch_lines(&resume, "claude:wt", "id-2");
    assert_eq!(line, "claude --remote-control -r 'id-2'");
    assert_eq!(remembered, line);
}

/// A first prompt is foreign text — a filled-in template quoting an
/// issue's title — so it is quoted literally, flattened to one line, and
/// comes last: after `--remote-control`, so the flag never takes it as its
/// name, and after `-n`, which keeps the ordering the tests above hold.
#[test]
fn a_first_prompt_is_quoted_flattened_and_last() {
    let launch = ClaudeLaunch {
        remote_control: true,
        first_prompt: Some("Fix #3: it's `$(rm -rf ~)`!\n\nThen\twait  here.\r\n"),
        ..ClaudeLaunch::default()
    };
    assert_eq!(
        claude_command_line(&launch, "issue-3-fix", Some("id-1")),
        "claude --remote-control -n 'issue-3-fix' --session-id 'id-1' \
             'Fix #3: it'\\''s `$(rm -rf ~)`! Then wait  here.'"
    );
}

/// Start work's yolo toggle: the YOLO flag leads, the prompt still ends
/// the line, and the up arrow resumes in YOLO without the prompt.
#[test]
fn a_yolo_launch_keeps_its_first_prompt_last() {
    let launch = ClaudeLaunch {
        yolo: true,
        remote_control: true,
        first_prompt: Some("Work on issue 3"),
        ..ClaudeLaunch::default()
    };
    let (line, remembered) = launch_lines(&launch, "issue-3-fix", "id-1");
    assert_eq!(
        line,
        "claude --dangerously-skip-permissions --remote-control -n 'issue-3-fix' \
             --session-id 'id-1' 'Work on issue 3'"
    );
    assert_eq!(
        remembered,
        "claude --dangerously-skip-permissions --remote-control -r 'id-1'"
    );
}

/// The up arrow restarts the conversation; sending the first prompt again
/// would hand Claude the same work twice in the middle of it.
#[test]
fn the_history_line_never_repeats_the_first_prompt() {
    let launch = ClaudeLaunch {
        first_prompt: Some("Work on issue 3"),
        ..ClaudeLaunch::default()
    };
    let (line, remembered) = launch_lines(&launch, "issue-3-fix", "id-1");
    assert!(line.ends_with(" 'Work on issue 3'"), "{line}");
    assert_eq!(remembered, "claude -r 'id-1'");
}

/// A custom button is Claude too — it already gets `-n`, `-r` and
/// `--session-id` appended — so it gets Remote Control with the rest. A
/// command that asks for it already is not handed it twice.
#[test]
fn custom_buttons_get_remote_control_once() {
    let custom = ClaudeLaunch {
        custom_command: Some("claude --model opus"),
        custom_prefix: Some("opus"),
        remote_control: true,
        ..ClaudeLaunch::default()
    };
    assert_eq!(
        claude_command_line(&custom, "opus:wt", None),
        "claude --model opus --remote-control -n 'opus:wt'"
    );

    let already = ClaudeLaunch {
        custom_command: Some("claude --remote-control --model opus"),
        ..custom
    };
    assert_eq!(
        claude_command_line(&already, "opus:wt", None),
        "claude --remote-control --model opus -n 'opus:wt'"
    );

    // Asking by another spelling counts too; `--rc` is Claude's alias, and
    // a second flag would override the name the button gave it.
    for asked in [
        "claude --rc 'Build box'",
        "claude --rc",
        "claude --rc=box",
        "claude --remote-control=box",
    ] {
        let launch = ClaudeLaunch {
            custom_command: Some(asked),
            ..custom
        };
        assert_eq!(
            claude_command_line(&launch, "opus:wt", None),
            format!("{asked} -n 'opus:wt'"),
            "{asked:?} was handed a second flag"
        );
    }

    // Naming the remote sessions is not asking for Remote Control.
    let prefix_only = ClaudeLaunch {
        custom_command: Some("claude --remote-control-session-name-prefix box"),
        ..custom
    };
    assert_eq!(
        claude_command_line(&prefix_only, "opus:wt", None),
        "claude --remote-control-session-name-prefix box --remote-control -n 'opus:wt'"
    );

    // The YOLO flag still stays off a custom button, Remote Control or not.
    assert_eq!(
        claude_command_line(
            &ClaudeLaunch {
                yolo: true,
                ..custom
            },
            "opus:wt",
            None
        ),
        "claude --model opus --remote-control -n 'opus:wt'"
    );
}

/// The live scan trusts only stamped windows, and decides "running" from
/// the pane's foreground command: a shell at the prompt is an open window
/// whose Claude has exited, not a live session.
#[test]
fn claude_windows_parse_from_stamps_and_foreground_command() {
    let out = "@1\tnode\tsess-a\tclaude:wt\n\
                   @2\tzsh\tsess-b\tyolo:other\n\
                   @3\tvim\t\tedit:wt\n\
                   garbage-line\n";
    let windows = parse_claude_windows(out);
    assert_eq!(windows.len(), 2, "unstamped windows are not Claude's");
    assert_eq!(windows[0].session_id, "sess-a");
    assert!(windows[0].running);
    assert_eq!(windows[0].window_name, "claude:wt");
    assert_eq!(windows[1].session_id, "sess-b");
    assert!(!windows[1].running, "a shell at the prompt is not running");
}

/// Window names may contain tabs? They cannot — but the name field is last
/// in the format string on purpose, so a name containing the separator
/// still parses: `splitn(4)` leaves everything after the third tab intact.
#[test]
fn a_tab_in_a_window_name_does_not_shift_fields() {
    let windows = parse_claude_windows("@1\tnode\tsess-a\tweird\tname\n");
    assert_eq!(windows[0].window_name, "weird\tname");
}

/// Window names come from session titles, and a session title can be set by
/// a `SessionStart` hook living in a repository's own `.claude/settings.json`
/// — so cloning a repository must not be able to run code here. Proven by
/// handing each name to a real shell and reading back what it made of it.
#[test]
fn a_hostile_window_name_stays_data() {
    for hostile in [
        "$(touch /tmp/forestui-pwned)",
        "`touch /tmp/forestui-pwned`",
        "'; touch /tmp/forestui-pwned; '",
        "it's",
        "a!b",
        "back\\slash",
        "claude:wt",
    ] {
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf %s {}", sh_quote(hostile)))
            .output()
            .expect("the shell runs");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            hostile,
            "{hostile:?} did not survive as literal text"
        );
    }
    assert!(
        !std::path::Path::new("/tmp/forestui-pwned").exists(),
        "a window name executed a command"
    );

    // And the name is still literal in the line the window is given.
    assert_eq!(
        claude_command_line(&ClaudeLaunch::default(), "$(id); rm -rf /", None),
        "claude -n '$(id); rm -rf /'"
    );
}

/// The line becomes one line of a file a shell reads, so a newline in it
/// would end that line and leave the rest standing as a command of its
/// own — from a window name forestui did not write. Control characters
/// never survive that far.
#[test]
fn a_newline_in_a_name_cannot_smuggle_a_command() {
    let built = claude_command_line(&ClaudeLaunch::default(), "wt'\nrm -rf /\n#", None);
    assert!(!built.contains('\n'), "a newline survived: {built:?}");
    assert_eq!(built, "claude -n 'wt'\\''rm -rf /#'");
}

fn home() -> ShellHome {
    ShellHome {
        home: "/home/u".into(),
        zdotdir: None,
        env_file: None,
    }
}

/// Each hook replaces one of the user's own startup files, so each
/// generated file has to source the file it displaced. Getting this wrong
/// is invisible until someone's aliases or PATH quietly stop applying in
/// Claude windows only.
#[test]
fn a_generated_startup_file_sources_the_one_it_replaced() {
    let dir = Path::new("/tmp/launch");
    let tail = startup_tail("claude:wt", "sess-1", "claude");
    let tail = tail.as_str();

    let zsh = startup_for(
        "/usr/bin/zsh",
        dir,
        &home(),
        "claude:wt",
        "sess-1",
        "claude",
        "claude -r 'sess-1'",
    )
    .expect("zsh is known");
    assert_eq!(zsh.command, "env ZDOTDIR='/tmp/launch' '/usr/bin/zsh' -i");
    let names: Vec<_> = zsh.files.iter().map(|(p, _)| p.clone()).collect();
    assert_eq!(names, vec![dir.join(".zshenv"), dir.join(".zshrc")]);
    assert!(zsh.files[0].1.contains("'/home/u/.zshenv'"));
    // ZDOTDIR is put back before the user's own file runs.
    assert!(zsh.files[1].1.starts_with("ZDOTDIR='/home/u'\n"));
    assert!(zsh.files[1].1.contains("'/home/u/.zshrc'"));
    assert!(zsh.files[1].1.ends_with(tail));

    let bash = startup_for(
        "/bin/bash",
        dir,
        &home(),
        "claude:wt",
        "sess-1",
        "claude",
        "claude -r 'sess-1'",
    )
    .expect("bash is known");
    assert_eq!(bash.command, "'/bin/bash' --rcfile '/tmp/launch/rc' -i");
    assert!(bash.files[0].1.contains("'/home/u/.bashrc'"));
    assert!(bash.files[0].1.ends_with(tail));

    let sh = startup_for(
        "/bin/dash",
        dir,
        &home(),
        "claude:wt",
        "sess-1",
        "claude",
        "claude -r 'sess-1'",
    )
    .expect("dash is known");
    assert_eq!(sh.command, "env ENV='/tmp/launch/rc' '/bin/dash' -i");
    assert!(sh.files[0].1.ends_with(tail));

    // The command lands in the shell's history where there is a builtin
    // for it, so the up arrow restarts a session that was interrupted —
    // and what is remembered is the *resume* form, because a fresh
    // session's `--session-id` refuses to run a second time and the up
    // arrow exists precisely for the second run.
    let expected_zsh = format!("  print -s -- {}\n", sh_quote("claude -r 'sess-1'"));
    let expected_bash = format!("history -s {}", sh_quote("claude -r 'sess-1'"));
    assert!(zsh.files[1].1.contains(&expected_zsh), "{}", zsh.files[1].1);
    assert!(
        bash.files[0].1.contains(&expected_bash),
        "{}",
        bash.files[0].1
    );
    assert!(!sh.files[0].1.contains("history"));

    // A shell whose ENV already names a file keeps reading it.
    let inherited = ShellHome {
        env_file: Some("/home/u/.shinit".into()),
        ..home()
    };
    let sh = startup_for(
        "/bin/sh",
        dir,
        &inherited,
        "claude:wt",
        "sess-1",
        "claude",
        "claude -r 'sess-1'",
    )
    .expect("sh is known");
    assert!(sh.files[0].1.contains("'/home/u/.shinit'"));
}

/// The remembered line has to be the *first* thing the up arrow finds once
/// Claude is gone — above the history file the shell loads, and above a
/// line another window's shell wrote while this session ran. zsh reads its
/// history file only after the startup file that runs Claude, which buried
/// the line under the whole file and let the up arrow resume a different
/// conversation. Driven through the real shells, since the order is the
/// shell's doing, not ours; a shell that is not installed is skipped.
#[test]
fn the_remembered_line_is_the_newest_history_entry() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    for shell in ["zsh", "bash"] {
        let Some(path) = std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(shell))
                .find(|candidate| candidate.is_file())
        }) else {
            eprintln!("{shell} is not installed; skipping");
            continue;
        };
        let home = tempfile::tempdir().expect("tempdir");
        let root = home.path();
        // The startup file stamps the window through tmux. A stub answers
        // instead, so this can never reach a real server.
        let bin = root.join("bin");
        std::fs::create_dir(&bin).expect("bin dir");
        let stub = bin.join("tmux");
        std::fs::write(&stub, "#!/bin/sh\nexit 0\n").expect("tmux stub");
        let mut mode = std::fs::metadata(&stub).expect("stub").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut mode, 0o755);
        std::fs::set_permissions(&stub, mode).expect("chmod");

        let (history, rc, old, other) = if shell == "zsh" {
            (
                root.join(".zsh_history"),
                (
                    root.join(".zshrc"),
                    "HISTFILE=$HOME/.zsh_history; HISTSIZE=100; SAVEHIST=100\n\
                         setopt share_history extended_history hist_ignore_space\n",
                ),
                ": 1:0;echo old\n",
                ": 3:0;claude -r OTHER",
            )
        } else {
            (
                root.join(".bash_history"),
                (
                    root.join(".bashrc"),
                    "HISTFILE=$HOME/.bash_history; HISTSIZE=100; HISTCONTROL=ignorespace\n",
                ),
                "echo old\n",
                "claude -r OTHER",
            )
        };
        std::fs::write(&history, old).expect("history");
        std::fs::write(&rc.0, rc.1).expect("rc");

        // Claude's stand-in: another window's shell exits mid-session and
        // appends its own line to the shared history file.
        let running = format!(
            "printf '%s\\n' {} >> {}",
            sh_quote(other),
            sh_quote(history.to_str().expect("utf-8"))
        );
        let launch = root.join("launch");
        let home_env = ShellHome {
            home: root.to_str().expect("utf-8").into(),
            zdotdir: None,
            env_file: None,
        };
        let startup = startup_for(
            path.to_str().expect("utf-8"),
            &launch,
            &home_env,
            "claude:wt",
            "sess-1",
            &running,
            "claude -r 'MINE'",
        )
        .expect("a known shell");
        std::fs::create_dir(&launch).expect("launch dir");
        for (file, contents) in &startup.files {
            std::fs::write(file, contents).expect("startup file");
        }

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(&startup.command)
            .env_clear()
            .env("HOME", root)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("TMUX_TMPDIR", root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the shell starts");
        // Leading space: the query itself stays out of the history.
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(b" fc -ln 1\n exit\n")
            .expect("commands");
        let out = child.wait_with_output().expect("the shell exits");
        let listed = String::from_utf8_lossy(&out.stdout);
        let entries: Vec<&str> = listed
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(
            entries.last(),
            Some(&"claude -r 'MINE'"),
            "{shell}: the up arrow would not find this window's line first: {entries:?}"
        );
    }
}

/// Every generated file removes itself once the shell has it, so a session
/// left open for a week does not leave one behind.
#[test]
fn a_generated_startup_file_sweeps_itself_up() {
    let dir = Path::new("/tmp/launch");
    for shell in ["/usr/bin/zsh", "/bin/bash", "/bin/sh"] {
        let startup = startup_for(
            shell,
            dir,
            &home(),
            "claude:wt",
            "sess-1",
            "claude",
            "claude",
        )
        .expect("a known shell");
        let last = &startup.files.last().expect("a file").1;
        assert!(
            last.contains("rm -rf '/tmp/launch'"),
            "{shell} leaves its startup file behind: {last:?}"
        );
    }
}

/// An unknown shell is not a failure — the line is typed into a plain
/// shell instead, which needs no hook at all.
#[test]
fn an_unknown_shell_falls_back_to_typing() {
    let call = |shell| {
        startup_for(
            shell,
            Path::new("/tmp/l"),
            &home(),
            "claude:wt",
            "sess-1",
            "claude",
            "claude",
        )
    };
    assert!(call("/usr/bin/fish").is_none());
    assert!(call("/usr/bin/nu").is_none());
}

/// The stamp goes in ahead of the command, from inside the window: the
/// shell starts Claude on its own, so a stamp sent after `new-window`
/// returns can lose the race to Claude's first hook.
#[test]
fn the_startup_file_stamps_before_it_launches() {
    let tail = startup_tail("yolo:wt", "sess-1", "claude -n 'yolo:wt'");
    let stamp = tail.find("@claude_birth_name").expect("a stamp");
    let monitor = tail.find("set -m").expect("job control");
    let launch = tail.find("claude -n").expect("the command");
    assert!(
        stamp < monitor && monitor < launch,
        "out of order: {tail:?}"
    );
    assert!(tail.contains("@claude_birth_name 'yolo:wt'"));
}
