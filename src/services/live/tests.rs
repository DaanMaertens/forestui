use super::*;

fn write_beat(dir: &Path, sid: &str, pid: u32, pane: &str) {
    std::fs::write(
        dir.join(format!("{sid}.json")),
        format!(
            "{{\"session_id\":\"{sid}\",\"pid\":{pid},\"tmux_pane\":\"{pane}\",\"cwd\":\"/x\"}}"
        ),
    )
    .unwrap();
}

/// A heartbeat is only as good as its process: dead or reused pids are
/// swept off disk, not just skipped, so a crash cannot leave a session
/// reading as live forever.
#[test]
fn stale_heartbeats_are_swept_and_live_ones_kept() {
    let dir = tempfile::tempdir().unwrap();
    write_beat(dir.path(), "alive-1", 100, "%5");
    write_beat(dir.path(), "dead-1", 200, "");
    std::fs::write(dir.path().join("garbage.json"), "not json").unwrap();
    // A crash between write and rename leaves the temp sibling behind.
    std::fs::write(dir.path().join(".x.123.tmp"), "half a beat").unwrap();
    // A file whose name disagrees with its content is not trusted.
    std::fs::write(
        dir.path().join("mismatch.json"),
        "{\"session_id\":\"other\",\"pid\":100}",
    )
    .unwrap();

    let beats = read_heartbeats_in(dir.path(), |_| HashSet::from([100u32]));
    assert_eq!(beats.len(), 1);
    assert_eq!(beats[0].session_id, "alive-1");
    assert_eq!(beats[0].pid, 100);
    assert_eq!(beats[0].tmux_pane, "%5");

    assert!(!dir.path().join("dead-1.json").exists(), "dead pid swept");
    assert!(!dir.path().join("garbage.json").exists(), "garbage swept");
    assert!(
        !dir.path().join(".x.123.tmp").exists(),
        "a crashed write's temp file swept"
    );
    assert!(
        !dir.path().join("mismatch.json").exists(),
        "mismatched file swept"
    );
    assert!(dir.path().join("alive-1.json").exists(), "live one kept");
}

/// A missing directory is the common case (plugin not installed, nothing
/// ever ran) and must be silence, not an error.
#[test]
fn a_missing_live_dir_reads_as_no_heartbeats() {
    assert!(read_heartbeats_in(Path::new("/nowhere/forestui-live"), |_| HashSet::new()).is_empty());
}

/// The plugin's hook scripts installed into a throwaway `HOME`, with `ps`
/// and `tmux` faked on PATH: the pid walk needs an ancestor that claims to
/// be claude, and a test must never speak to a real tmux server.
#[cfg(unix)]
struct Hooks {
    home: tempfile::TempDir,
}

#[cfg(unix)]
impl Hooks {
    /// `None` when jq is missing — the scripts do nothing without it, so
    /// there is nothing to test.
    fn install() -> Option<Self> {
        use crate::test_support::write_executable;
        if std::process::Command::new("jq")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("jq not installed; skipping the script test");
            return None;
        }
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        write_executable(
            &bin.join("heartbeat.sh"),
            include_str!("../../../assets/claude-plugin/heartbeat.sh"),
        );
        write_executable(
            &bin.join("state.sh"),
            include_str!("../../../assets/claude-plugin/state.sh"),
        );
        // A fake ps: every ancestry query answers pid 4242, whose command is
        // claude — so the walk terminates on the first hop.
        write_executable(
            &bin.join("ps"),
            "#!/bin/sh\ncase \"$*\" in *ppid*) echo 4242;; *comm*) echo claude;; esac\n",
        );
        // A fake tmux that records its argv instead of reaching any server.
        write_executable(
            &bin.join("tmux"),
            &format!(
                "#!/bin/sh\necho \"$@\" >> {}\n",
                home.path().join("tmux.log").display()
            ),
        );
        Some(Self { home })
    }

    fn run(&self, script: &str, event: &str, input: &str, pane: Option<&str>) {
        let bin = self.home.path().join("bin");
        let mut cmd = std::process::Command::new(bin.join(script));
        cmd.arg(event)
            .env_remove("TMUX")
            .env("HOME", self.home.path())
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null());
        match pane {
            Some(pane) => cmd
                .env("TMUX_PANE", pane)
                .env("TMUX", "/tmp/tmux-1000/de\"fault,77,3"),
            None => cmd.env_remove("TMUX_PANE"),
        };
        let mut child = cmd.spawn().expect("the script runs");
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
    }

    fn live(&self) -> PathBuf {
        self.home.path().join(".config/forestui/live")
    }

    fn beat(&self, sid: &str) -> Option<Heartbeat> {
        let raw = std::fs::read_to_string(self.live().join(format!("{sid}.json"))).ok()?;
        Some(serde_json::from_str(&raw).expect("valid JSON"))
    }
}

/// Run the shipped `heartbeat.sh` for real. This pins the script's whole
/// observable contract — the file it writes, the sweep on SessionEnd, the
/// id sanitisation, and the window-stamp healing call.
#[cfg(unix)]
#[test]
fn the_heartbeat_script_writes_sweeps_and_heals() {
    let Some(hooks) = Hooks::install() else {
        return;
    };
    let run = |event: &str, input: &str, pane: Option<&str>| {
        hooks.run("heartbeat.sh", event, input, pane);
    };
    let live = hooks.live();

    // SessionStart writes the heartbeat, with the walked-to claude pid.
    run(
        "SessionStart",
        r#"{"session_id":"beat-1","cwd":"/w/x"}"#,
        None,
    );
    let parsed = hooks.beat("beat-1").expect("a heartbeat");
    assert_eq!(parsed.session_id, "beat-1");
    assert_eq!(parsed.pid, 4242);
    assert_eq!(parsed.cwd, "/w/x");
    assert_eq!(parsed.tmux_pane, "");
    assert_eq!(parsed.tmux_server, "");

    // Inside tmux, the stamp on the window is healed to this session.
    run(
        "UserPromptSubmit",
        r#"{"session_id":"beat-1","cwd":"/w/x"}"#,
        Some("%9"),
    );
    // It records which server the pane is on, quoted however the socket
    // path is spelled.
    let inside = hooks.beat("beat-1").expect("a heartbeat");
    assert_eq!(inside.tmux_pane, "%9");
    assert_eq!(inside.tmux_server, "/tmp/tmux-1000/de\"fault,77");
    let stamped = std::fs::read_to_string(hooks.home.path().join("tmux.log")).unwrap_or_default();
    assert!(
        stamped.contains("set-option -w -t %9 @claude_session_id beat-1"),
        "no healing call: {stamped:?}"
    );

    // A hostile id cannot write outside the live directory.
    run(
        "SessionStart",
        r#"{"session_id":"../../escape","cwd":"/w"}"#,
        None,
    );
    assert!(
        !hooks.home.path().join("escape.json").exists()
            && !hooks.home.path().join(".config/escape.json").exists(),
        "a traversal id escaped the live dir"
    );

    // SessionEnd sweeps the heartbeat away.
    run("SessionEnd", r#"{"session_id":"beat-1"}"#, None);
    assert!(!live.join("beat-1.json").exists(), "SessionEnd must sweep");
}

/// Hook payloads drive the state, through both scripts: the heartbeat
/// writes it on SessionStart and UserPromptSubmit, `state.sh` on
/// everything in between. The payloads are the shapes Claude Code sends.
#[cfg(unix)]
#[test]
fn hook_payloads_drive_the_session_state() {
    let Some(hooks) = Hooks::install() else {
        return;
    };
    let state = |event: &str, input: &str| {
        hooks.run("state.sh", event, input, None);
        hooks.beat("s-1").and_then(|beat| beat.state)
    };
    let beat = |event: &str, input: &str| {
        hooks.run("heartbeat.sh", event, input, None);
        hooks.beat("s-1").and_then(|beat| beat.state)
    };
    use SessionState::*;

    // Before any heartbeat there is no file to carry a state, and state.sh
    // does not invent one: it cannot validate a pid.
    assert_eq!(state("Stop", r#"{"session_id":"s-1"}"#), None);
    assert!(!hooks.live().join("s-1.json").exists());

    // A fresh or resumed session sits at its prompt.
    assert_eq!(
        beat(
            "SessionStart",
            r#"{"session_id":"s-1","cwd":"/w","source":"startup"}"#
        ),
        Some(Idle)
    );
    assert_eq!(
        beat("UserPromptSubmit", r#"{"session_id":"s-1","cwd":"/w"}"#),
        Some(Busy)
    );
    // Tool use while busy changes nothing.
    assert_eq!(
        state(
            "PostToolUse",
            r#"{"session_id":"s-1","tool_name":"Bash","tool_input":{},"tool_response":{}}"#
        ),
        Some(Busy)
    );
    // A permission prompt needs you; the tool running after it is answered
    // means Claude carried on.
    assert_eq!(
        state(
            "Notification",
            r#"{"session_id":"s-1","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"}"#
        ),
        Some(NeedsYou)
    );
    assert_eq!(
        state("PostToolUse", r#"{"session_id":"s-1","tool_name":"Bash"}"#),
        Some(Busy)
    );
    // A denied tool never runs, so its PostToolUse never comes; the next
    // tool Claude reaches for says it carried on all the same.
    state(
        "Notification",
        r#"{"session_id":"s-1","notification_type":"permission_prompt"}"#,
    );
    assert_eq!(
        state("PreToolUse", r#"{"session_id":"s-1","tool_name":"Read"}"#),
        Some(Busy)
    );
    // A question to the user needs you too, until it is answered.
    assert_eq!(
        state(
            "PreToolUse",
            r#"{"session_id":"s-1","tool_name":"AskUserQuestion"}"#
        ),
        Some(NeedsYou)
    );
    assert_eq!(
        state(
            "PostToolUse",
            r#"{"session_id":"s-1","tool_name":"AskUserQuestion"}"#
        ),
        Some(Busy)
    );
    // A notification this does not know leaves the state alone.
    assert_eq!(
        state(
            "Notification",
            r#"{"session_id":"s-1","notification_type":"auth_success","message":"ok"}"#
        ),
        Some(Busy)
    );
    // The turn ends.
    assert_eq!(state("Stop", r#"{"session_id":"s-1"}"#), Some(Idle));
    // Tool use after a stop (a background task finishing) does not wake it.
    assert_eq!(
        state("PostToolUse", r#"{"session_id":"s-1","tool_name":"Bash"}"#),
        Some(Idle)
    );
    // Esc skips Stop; the idle prompt a minute later corrects busy to idle.
    beat("UserPromptSubmit", r#"{"session_id":"s-1","cwd":"/w"}"#);
    assert_eq!(
        state(
            "Notification",
            r#"{"session_id":"s-1","notification_type":"idle_prompt","message":"Claude is waiting for your input"}"#
        ),
        Some(Idle)
    );
    // An untyped notification is not read by its wording.
    assert_eq!(
        state(
            "Notification",
            r#"{"session_id":"s-1","message":"Claude needs your permission to use Edit"}"#
        ),
        Some(Idle)
    );
    // Compaction restarts the session mid-turn: whatever it was doing, it
    // still is. The heartbeat keeps the state rather than resetting it.
    beat("UserPromptSubmit", r#"{"session_id":"s-1","cwd":"/w"}"#);
    assert_eq!(
        beat(
            "SessionStart",
            r#"{"session_id":"s-1","cwd":"/w","source":"compact"}"#
        ),
        Some(Busy)
    );
    // The heartbeat survives every state write intact.
    let kept = hooks.beat("s-1").unwrap();
    assert_eq!((kept.pid, kept.cwd.as_str()), (4242, "/w"));

    // A hostile id is stripped to the characters an id can hold, so it
    // cannot reach a file outside the live directory.
    hooks.run("state.sh", "Stop", r#"{"session_id":"../s-1"}"#, None);
    assert!(!hooks.home.path().join(".config/forestui/s-1.json").exists());
}

/// Where these tests' forestui runs: a server (socket and pid, as `$TMUX`
/// spells them) and tmux session `$0`.
const OUR_SERVER: &str = "/tmp/tmux-1000/default,77";

fn home() -> Option<Home> {
    Some(Home {
        server: OUR_SERVER.into(),
        session: "$0".into(),
        group: String::new(),
    })
}

/// A heartbeat written on forestui's own tmux server.
fn beat(sid: &str, pid: u32, pane: &str) -> Heartbeat {
    Heartbeat {
        session_id: sid.into(),
        pid,
        tmux_pane: pane.into(),
        tmux_server: if pane.is_empty() {
            String::new()
        } else {
            OUR_SERVER.into()
        },
        cwd: String::new(),
        state: None,
    }
}

fn window(sid: &str, running: bool) -> tmux::ClaudeWindow {
    tmux::ClaudeWindow {
        window_id: "@1".into(),
        window_name: format!("claude:{sid}"),
        running,
        session_id: sid.into(),
    }
}

fn pane_in(session_id: &str, session: &str, window_id: &str, window: &str) -> tmux::PaneWindow {
    tmux::PaneWindow {
        tmux_session: session_id.into(),
        session_name: session.into(),
        session_group: String::new(),
        session_windows: 3,
        window_id: window_id.into(),
        window_name: window.into(),
    }
}

/// The merge's whole contract: exited windows stay out (that session is
/// free), stamps outrank heartbeats for the same session, a heartbeat
/// pane on our own tmux session resolves to a switchable window, one on
/// another session of our server to a window that can be brought over,
/// and everything else is elsewhere.
#[test]
fn the_merge_ranks_stamps_resolves_panes_and_frees_exited_windows() {
    let windows = vec![window("in-window", true), window("exited", false)];
    let beats = vec![
        beat("in-window", 10, "%1"),  // covered by its stamp already
        beat("our-pane", 20, "%2"),   // resolves to our session
        beat("other-pane", 30, "%3"), // another tmux session
        beat("no-tmux", 40, ""),      // no pane at all
    ];
    let resolve = |pane: &str| match pane {
        "%2" => Some(pane_in("$0", "forest", "@7", "hand-made")),
        "%3" => Some(tmux::PaneWindow {
            session_windows: 1,
            ..pane_in("$9", "work", "@8", "foreign")
        }),
        _ => None,
    };

    let merged = merge(windows, beats, home(), resolve, &HashMap::new());
    let by_id: std::collections::HashMap<&str, &LivePlace> = merged
        .iter()
        .map(|s| (s.session_id.as_str(), &s.place))
        .collect();

    assert_eq!(merged.len(), 4, "the exited window must not appear");
    assert!(!by_id.contains_key("exited"), "exited sessions are free");
    assert!(matches!(
        by_id["in-window"],
        LivePlace::Window { window_name, .. } if window_name == "claude:in-window"
    ));
    assert!(matches!(
        by_id["our-pane"],
        LivePlace::Window { window_id, window_name } if window_id == "@7" && window_name == "hand-made"
    ));
    // Our server, another tmux session: a window forestui can bring over.
    assert_eq!(
        by_id["other-pane"],
        &LivePlace::OtherSession {
            window_id: "@8".into(),
            window_name: "foreign".into(),
            session_name: "work".into(),
            last_window: true,
        }
    );
    assert!(matches!(by_id["no-tmux"], LivePlace::Elsewhere { pid: 40 }));

    // Outside tmux entirely, every heartbeat is elsewhere.
    let merged = merge(
        Vec::new(),
        vec![beat("x", 5, "%2")],
        None,
        |_| None,
        &HashMap::new(),
    );
    assert!(matches!(merged[0].place, LivePlace::Elsewhere { pid: 5 }));
}

/// Pane ids are only unique per tmux server. A heartbeat from another
/// server whose pane id happens to name one of ours is elsewhere — and so
/// is one that recorded no server at all (an older plugin): resolving it
/// would be a guess. Neither is even looked up.
#[test]
fn a_pane_resolves_only_on_our_own_tmux_server() {
    let mut foreign = beat("foreign", 10, "%0");
    foreign.tmux_server = "/tmp/tmux-1000/other,88".into();
    let mut unrecorded = beat("unrecorded", 20, "%0");
    unrecorded.tmux_server = String::new();
    // Same socket, but a server that was restarted since: its panes are
    // numbered afresh, so the old `%0` is not today's.
    let mut restarted = beat("restarted", 30, "%0");
    restarted.tmux_server = "/tmp/tmux-1000/default,76".into();
    let looked_up = std::cell::RefCell::new(Vec::new());
    let merged = merge(
        Vec::new(),
        vec![foreign, unrecorded, restarted, beat("ours", 40, "%0")],
        home(),
        |pane: &str| {
            looked_up.borrow_mut().push(pane.to_string());
            Some(pane_in("$0", "forest", "@1", "forestui"))
        },
        &HashMap::new(),
    );
    let by_id: std::collections::HashMap<&str, &LivePlace> = merged
        .iter()
        .map(|s| (s.session_id.as_str(), &s.place))
        .collect();
    assert_eq!(by_id["foreign"], &LivePlace::Elsewhere { pid: 10 });
    assert_eq!(by_id["unrecorded"], &LivePlace::Elsewhere { pid: 20 });
    assert_eq!(by_id["restarted"], &LivePlace::Elsewhere { pid: 30 });
    assert!(matches!(by_id["ours"], LivePlace::Window { .. }));
    assert_eq!(
        looked_up.borrow().len(),
        1,
        "only our server's pane is resolved"
    );
}

/// Sessions in one group share their windows, and tmux names whichever
/// member it likes as a pane's session. A pane in forestui's own group is
/// therefore ours — switchable, never offered to be moved to where it
/// already is.
#[test]
fn a_pane_in_our_session_group_is_ours() {
    let mut home = home().unwrap();
    home.group = "forest".into();
    let peer = |pane: &str| {
        Some(tmux::PaneWindow {
            session_group: if pane == "%1" { "forest" } else { "" }.into(),
            ..pane_in("$4", "forest-1", "@3", "claude")
        })
    };
    let merged = merge(
        Vec::new(),
        vec![beat("grouped", 10, "%1"), beat("ungrouped", 20, "%2")],
        Some(home),
        peer,
        &HashMap::new(),
    );
    assert!(matches!(merged[0].place, LivePlace::Window { .. }));
    assert!(matches!(merged[1].place, LivePlace::OtherSession { .. }));
}

/// The states a heartbeat can carry, most urgent last — the sidebar row
/// picks the maximum, so the order is the urgency rule.
#[test]
fn session_states_parse_and_rank_by_urgency() {
    assert_eq!(SessionState::parse("busy"), Some(SessionState::Busy));
    assert_eq!(
        SessionState::parse("needs_you"),
        Some(SessionState::NeedsYou)
    );
    assert_eq!(SessionState::parse("idle"), Some(SessionState::Idle));
    assert_eq!(SessionState::parse("thinking"), None);
    assert!(SessionState::NeedsYou > SessionState::Busy);
    assert!(SessionState::Busy > SessionState::Idle);
    assert_eq!(SessionState::NeedsYou.label(), "needs you");
}

/// A heartbeat from a plugin older than the state hooks has no `state`, and
/// one from a newer plugin may carry a state this build does not know.
/// Neither may fail the parse — an unparseable heartbeat is swept as stale.
#[test]
fn a_heartbeat_without_a_known_state_still_loads() {
    let old: Heartbeat =
        serde_json::from_str(r#"{"session_id":"a","pid":1,"tmux_pane":"","cwd":"/x"}"#).unwrap();
    assert_eq!(old.state, None);
    let future: Heartbeat =
        serde_json::from_str(r#"{"session_id":"a","pid":1,"state":"compacting"}"#).unwrap();
    assert_eq!(future.state, None);
    let busy: Heartbeat =
        serde_json::from_str(r#"{"session_id":"a","pid":1,"state":"busy"}"#).unwrap();
    assert_eq!(busy.state, Some(SessionState::Busy));
}

/// A stamped window wins the session's place, but its heartbeat still says
/// what the session is doing and where it runs.
#[test]
fn the_merge_carries_state_and_cwd_from_the_heartbeat() {
    let mut stamped = beat("in-window", 10, "%1");
    stamped.state = Some(SessionState::NeedsYou);
    stamped.cwd = "/w/feature".into();
    let merged = merge(
        vec![window("in-window", true), window("no-beat", true)],
        vec![stamped],
        home(),
        |_| None,
        &HashMap::new(),
    );
    let by_id: std::collections::HashMap<&str, &LiveSession> =
        merged.iter().map(|s| (s.session_id.as_str(), s)).collect();
    assert_eq!(by_id["in-window"].state, Some(SessionState::NeedsYou));
    assert_eq!(by_id["in-window"].cwd.as_deref(), Some("/w/feature"));
    assert_eq!(by_id["no-beat"].state, None, "no heartbeat, no guess");
    assert_eq!(by_id["no-beat"].cwd, None);
}

/// The once-a-second read: only the named sessions, only their state, no
/// `ps` — and an id that would name a path outside the directory is never
/// opened.
#[test]
fn reading_states_reads_only_the_named_heartbeats() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.json"),
        r#"{"session_id":"a","pid":1,"state":"busy"}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("b.json"), r#"{"session_id":"b","pid":2}"#).unwrap();
    std::fs::write(
        dir.path().join("c.json"),
        r#"{"session_id":"c","pid":3,"state":"idle"}"#,
    )
    .unwrap();
    let ids = ["a", "b", "gone", "../c"].map(String::from);
    let states = read_states_in(dir.path(), &ids);
    assert_eq!(states.get("a"), Some(&SessionState::Busy));
    assert!(!states.contains_key("b"), "no state field, no state");
    assert!(!states.contains_key("gone"));
    assert!(!states.contains_key("../c"));
    assert!(!states.contains_key("c"), "not asked for");
}

fn live_in(sid: &str, cwd: &str, state: Option<SessionState>, ours: bool) -> LiveSession {
    LiveSession {
        session_id: sid.into(),
        place: if ours {
            LivePlace::Window {
                window_id: "@1".into(),
                window_name: sid.into(),
            }
        } else {
            LivePlace::Elsewhere { pid: 1 }
        },
        state,
        cwd: Some(cwd.into()),
    }
}

/// A worktree row's roll-up: only sessions in forestui's own windows, in
/// that worktree count — and the most urgent known state wins.
#[test]
fn a_worktree_rolls_up_the_most_urgent_state_of_its_own_windows() {
    use SessionState::*;
    let sessions = [
        live_in("a", "/f/repo/wt", Some(Idle), true),
        live_in("b", "/f/repo/wt/src", Some(NeedsYou), true),
        live_in("c", "/f/repo/wt", Some(Busy), true),
        // Elsewhere: another tmux session, not counted.
        live_in("d", "/f/repo/wt", Some(NeedsYou), false),
        // A sibling whose name merely starts with ours.
        live_in("e", "/f/repo/wt-two", Some(NeedsYou), true),
        // No state known: counted, but lends no colour.
        live_in("f", "/f/repo/wt", None, true),
    ];
    let activity = activity(sessions.iter(), Path::new("/f/repo/wt"));
    assert_eq!(
        activity,
        Some(Activity {
            state: NeedsYou,
            count: 4
        })
    );

    let unknown = [live_in("a", "/f/repo/wt", None, true)];
    assert_eq!(activity_of(&unknown, "/f/repo/wt"), None, "nothing to show");
    let only_elsewhere = [live_in("d", "/f/repo/wt", Some(Busy), false)];
    assert_eq!(activity_of(&only_elsewhere, "/f/repo/wt"), None);
    let one = [live_in("a", "/f/repo/wt", Some(Busy), true)];
    assert_eq!(
        activity_of(&one, "/f/repo/wt"),
        Some(Activity {
            state: Busy,
            count: 1
        })
    );
}

fn activity_of(sessions: &[LiveSession], path: &str) -> Option<Activity> {
    activity(sessions.iter(), Path::new(path))
}

/// ← in Claude Code hands the conversation to a background daemon under a
/// new id. The window still holds the old id and the old client is still
/// alive, while the daemon's heartbeat has no pane. One conversation, so
/// one live session: the successor, reachable where the predecessor was.
#[test]
fn a_handed_off_session_lives_on_as_its_successor_in_the_same_window() {
    let mut successor = beat("new-id", 20, "");
    successor.cwd = "/work/wt".into();
    successor.state = Some(SessionState::Busy);
    let continued = HashMap::from([("old-id".to_string(), "new-id".to_string())]);

    let merged = merge(
        vec![window("old-id", true)],
        vec![beat("old-id", 10, "%1"), successor],
        home(),
        |_| None,
        &continued,
    );

    assert_eq!(
        merged,
        vec![LiveSession {
            session_id: "new-id".into(),
            place: LivePlace::Window {
                window_id: "@1".into(),
                window_name: "claude:old-id".into(),
            },
            state: Some(SessionState::Busy),
            cwd: Some("/work/wt".into()),
        }]
    );
}

/// A successor that has ended leaves nothing live: the predecessor handed
/// the conversation away, so its own still-running client is not it.
#[test]
fn a_handed_off_session_whose_successor_ended_is_not_live() {
    let continued = HashMap::from([("old-id".to_string(), "new-id".to_string())]);
    let merged = merge(
        vec![window("old-id", true)],
        vec![beat("old-id", 10, "%1")],
        home(),
        |_| None,
        &continued,
    );
    assert_eq!(merged, Vec::new());
}

/// A conversation handed off twice follows the chain to the session that
/// has it now, and a chain that loops back on itself still ends.
#[test]
fn a_hand_off_chain_is_followed_to_its_end() {
    let continued = HashMap::from([
        ("a".to_string(), "b".to_string()),
        ("b".to_string(), "c".to_string()),
        ("x".to_string(), "y".to_string()),
        ("y".to_string(), "x".to_string()),
    ]);
    let merged = merge(
        vec![window("a", true), window("x", true)],
        vec![beat("b", 20, ""), beat("c", 30, ""), beat("y", 40, "")],
        home(),
        |_| None,
        &continued,
    );
    assert_eq!(
        merged.len(),
        1,
        "only the end of the chain is live: {merged:?}"
    );
    assert_eq!(merged[0].session_id, "c");
    assert!(
        matches!(&merged[0].place, LivePlace::Window { window_name, .. } if window_name == "claude:a")
    );
}
