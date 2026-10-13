//! Where each Claude session is running right now.
//!
//! Two sources, merged into one answer:
//!
//! - **Window stamps** — the `@claude_session_id` option on windows forestui
//!   opened (and on any window the plugin's heartbeat hook healed). Cheap,
//!   and it names a window forestui can jump to.
//! - **Heartbeats** — files the plugin writes from inside every session on
//!   the machine, wherever it was launched: a hand-made tmux window, a
//!   terminal with no tmux at all. They carry the claude process id, which is
//!   what separates a live session from a file a crash left behind.
//!
//! A session found by stamp wins (it is switchable); a heartbeat fills in the
//! rest, resolving its recorded pane back to a window when that pane lives on
//! forestui's own tmux session, and falling back to "elsewhere" otherwise.

use crate::services::tmux;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Where a live session can be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LivePlace {
    /// A window on forestui's own tmux session — the guard can offer to
    /// switch there.
    Window {
        window_id: String,
        window_name: String,
    },
    /// Running somewhere forestui cannot jump to: another tmux session, or no
    /// tmux at all. The pid is shown so the user can find it.
    Elsewhere { pid: u32 },
}

/// What a live session is doing, as the plugin's hooks last recorded it.
///
/// Declared least urgent first: the derived order *is* the urgency rule, so a
/// worktree with several sessions shows the maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SessionState {
    /// Waiting for the next prompt — a turn ended, or Claude says so after a
    /// minute of no input.
    Idle,
    /// Working on a turn.
    Busy,
    /// Blocked on the user: a permission prompt or a question.
    NeedsYou,
}

impl SessionState {
    /// The heartbeat's spelling. Anything else is unknown, never a guess.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "idle" => Some(Self::Idle),
            "busy" => Some(Self::Busy),
            "needs_you" => Some(Self::NeedsYou),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Busy => "busy",
            Self::NeedsYou => "needs you",
        }
    }
}

/// One live Claude session and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    pub session_id: String,
    pub place: LivePlace,
    /// From the session's heartbeat; `None` without one, or from a plugin
    /// older than the state hooks.
    pub state: Option<SessionState>,
    /// The directory the session runs in, from its heartbeat — what ties a
    /// session to a worktree's sidebar row.
    pub cwd: Option<String>,
}

/// What a worktree's sidebar row shows of the sessions running in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Activity {
    /// The most urgent state among them.
    pub state: SessionState,
    /// How many there are; the row prints a count only from two.
    pub count: usize,
}

/// Roll the sessions running under `worktree` up into one row's worth.
///
/// Only sessions in windows of forestui's own tmux session count — the row
/// is about what a keypress away needs you, and a session elsewhere is not.
/// A session with no known state (an older plugin) still counts towards the
/// number but lends the row no colour; with no known state at all the row
/// is `None` and looks as it always did.
pub fn activity<'a>(
    sessions: impl IntoIterator<Item = &'a LiveSession>,
    worktree: &Path,
) -> Option<Activity> {
    let counted: Vec<&LiveSession> = sessions
        .into_iter()
        .filter(|session| matches!(session.place, LivePlace::Window { .. }))
        .filter(|session| {
            // A session started in a subdirectory still belongs to the
            // worktree; `Path::starts_with` compares whole components, so
            // `wt-two` is not inside `wt`.
            session
                .cwd
                .as_deref()
                .is_some_and(|cwd| Path::new(cwd).starts_with(worktree))
        })
        .collect();
    Some(Activity {
        state: counted.iter().filter_map(|session| session.state).max()?,
        count: counted.len(),
    })
}

/// A session's heartbeat, as `heartbeat.sh` writes it from inside the session.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Heartbeat {
    pub session_id: String,
    pub pid: u32,
    #[serde(default)]
    pub tmux_pane: String,
    /// The tmux server `tmux_pane` belongs to, as [`server_of`] spells it.
    /// Pane ids are only unique per server, so a pane means nothing without
    /// it. Empty outside tmux, and in a heartbeat from a plugin older than
    /// version 7 — which is then never resolved to a window.
    #[serde(default)]
    pub tmux_server: String,
    #[serde(default)]
    pub cwd: String,
    /// Written by `state.sh` and on `UserPromptSubmit`. Read leniently: a
    /// heartbeat that fails to parse is swept as stale, so a state this
    /// build does not know must not cost the session its liveness.
    #[serde(default, deserialize_with = "lenient_state")]
    pub state: Option<SessionState>,
}

impl Heartbeat {
    /// The recorded directory, if the hook had one to record.
    fn cwd(&self) -> Option<String> {
        (!self.cwd.is_empty()).then(|| self.cwd.clone())
    }
}

fn lenient_state<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SessionState>, D::Error> {
    let raw: Option<serde_json::Value> = serde::Deserialize::deserialize(deserializer)?;
    Ok(raw
        .as_ref()
        .and_then(serde_json::Value::as_str)
        .and_then(SessionState::parse))
}

/// Where the plugin writes heartbeats. Must match `heartbeat.sh` verbatim.
pub fn live_dir() -> PathBuf {
    crate::util::home_dir()
        .join(".config")
        .join("forestui")
        .join("live")
}

/// Where forestui itself runs: the tmux server and the session on it that
/// its windows live in. A heartbeat pane is only ours when both match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Home {
    pub server: String,
    pub session: String,
}

/// A tmux server's identity from a `$TMUX` value, `<socket>,<pid>,<index>`:
/// the socket path and the server's pid. The socket alone is not enough — a
/// server restarted on the same socket numbers its panes afresh, so an old
/// `%3` would name a new, unrelated pane. `heartbeat.sh` strips the same
/// last field, so the two compare as plain strings.
pub fn server_of(tmux_env: &str) -> Option<&str> {
    tmux_env
        .rsplit_once(',')
        .map(|(server, _index)| server)
        .filter(|server| server.contains(','))
}

/// The full liveness picture, both sources merged. One `list-windows`, one
/// directory read, one `ps` — run off the UI loop like every other scan.
pub fn snapshot() -> Vec<LiveSession> {
    let home = tmux::current_session().and_then(|session| {
        let env = std::env::var("TMUX").ok()?;
        Some(Home {
            server: server_of(&env)?.to_string(),
            session,
        })
    });
    merge(
        tmux::list_claude_windows(),
        read_heartbeats(),
        home,
        tmux::pane_window,
    )
}

/// Pure form of [`snapshot`], for testing: `resolve` stands in for asking
/// tmux which window a heartbeat's pane belongs to.
pub fn merge(
    windows: Vec<tmux::ClaudeWindow>,
    beats: Vec<Heartbeat>,
    home: Option<Home>,
    resolve: impl Fn(&str) -> Option<tmux::PaneWindow>,
) -> Vec<LiveSession> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<LiveSession> = Vec::new();
    // A stamped window outranks its heartbeat for the place, but only the
    // heartbeat knows what the session is doing and where it runs.
    let by_heartbeat: HashMap<&str, &Heartbeat> = beats
        .iter()
        .map(|beat| (beat.session_id.as_str(), beat))
        .collect();

    // Stamped windows where Claude is the pane's foreground process. A window
    // whose Claude exited deliberately stays out: that session is free again.
    for window in windows {
        if window.running && seen.insert(window.session_id.clone()) {
            let beat = by_heartbeat.get(window.session_id.as_str());
            let state = beat.and_then(|beat| beat.state);
            let cwd = beat.and_then(|beat| beat.cwd());
            out.push(LiveSession {
                session_id: window.session_id,
                place: LivePlace::Window {
                    window_id: window.window_id,
                    window_name: window.window_name,
                },
                state,
                cwd,
            });
        }
    }

    // Heartbeats cover everything else. A recorded pane that resolves onto
    // our own tmux session is still switchable; anything else is elsewhere.
    // A pane from another server is not even looked up: `resolve` asks *our*
    // server, where the same id can name an unrelated pane.
    // (A suspended Claude — Ctrl-Z — is alive by pid and so counts as live
    // here, which is right: resuming it somewhere else would fork the
    // conversation a `fg` can still continue.)
    for beat in read_heartbeats_merge(beats, &mut seen) {
        let place = match &home {
            Some(home) if !beat.tmux_pane.is_empty() && beat.tmux_server == home.server => {
                match resolve(&beat.tmux_pane) {
                    Some(pane) if pane.tmux_session == home.session => LivePlace::Window {
                        window_id: pane.window_id,
                        window_name: pane.window_name,
                    },
                    _ => LivePlace::Elsewhere { pid: beat.pid },
                }
            }
            _ => LivePlace::Elsewhere { pid: beat.pid },
        };
        out.push(LiveSession {
            cwd: beat.cwd(),
            state: beat.state,
            session_id: beat.session_id,
            place,
        });
    }
    out
}

/// The heartbeats not already accounted for by a stamped window.
fn read_heartbeats_merge(beats: Vec<Heartbeat>, seen: &mut HashSet<String>) -> Vec<Heartbeat> {
    beats
        .into_iter()
        .filter(|beat| seen.insert(beat.session_id.clone()))
        .collect()
}

/// Whether this session id has a heartbeat naming a live claude — the
/// delete task's moment-of-truth recheck, alongside the tmux one.
pub fn heartbeat_alive(session_id: &str) -> bool {
    read_heartbeats()
        .iter()
        .any(|beat| beat.session_id == session_id)
}

/// What each of these sessions is doing, read straight from their heartbeat
/// files. No `ps`, no tmux: this runs once a second, and the full scan has
/// already decided which sessions are live — this only refreshes the state
/// of the ones it found. A session absent from the answer has no state.
pub fn read_states(ids: &[String]) -> HashMap<String, SessionState> {
    read_states_in(&live_dir(), ids)
}

/// Directory-scoped form of [`read_states`], for testing.
pub fn read_states_in(dir: &Path, ids: &[String]) -> HashMap<String, SessionState> {
    ids.iter()
        // The id names a file; one that could step outside the directory is
        // not a session id, whatever produced it.
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .filter_map(|id| {
            let raw = std::fs::read_to_string(dir.join(format!("{id}.json"))).ok()?;
            let beat: Heartbeat = serde_json::from_str(&raw).ok()?;
            Some((id.clone(), beat.state?))
        })
        .collect()
}

/// Every heartbeat whose process is still a running claude. Stale files —
/// a crash skipped `SessionEnd`, or the pid was reused by something else —
/// are swept here, so the directory never accumulates lies.
pub fn read_heartbeats() -> Vec<Heartbeat> {
    read_heartbeats_in(&live_dir(), alive_claude_pids)
}

/// Directory-scoped form of [`read_heartbeats`], for testing. `alive` answers
/// which of the recorded pids currently run claude.
pub fn read_heartbeats_in(dir: &Path, alive: impl Fn(&[u32]) -> HashSet<u32>) -> Vec<Heartbeat> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut beats: Vec<(PathBuf, Heartbeat)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            // The plugin writes via a `.tmp` sibling and renames; a crash
            // between the two leaves the sibling forever. The directory is
            // wholly ours, so anything that is not a heartbeat is litter.
            if path.extension().and_then(|e| e.to_str()) == Some("tmp") {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        let Some(beat) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Heartbeat>(&raw).ok())
        else {
            // Unparseable is stale by definition; the plugin writes atomically,
            // so this is a leftover of something else, not a half-write.
            let _ = std::fs::remove_file(&path);
            continue;
        };
        // The filename is the session id; a mismatch is a copied or tampered
        // file and is not trusted either way.
        if path.file_stem().and_then(|s| s.to_str()) != Some(beat.session_id.as_str()) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        beats.push((path, beat));
    }

    let pids: Vec<u32> = beats.iter().map(|(_, b)| b.pid).collect();
    let alive = alive(&pids);
    beats
        .into_iter()
        .filter_map(|(path, beat)| {
            if alive.contains(&beat.pid) {
                Some(beat)
            } else {
                let _ = std::fs::remove_file(&path);
                None
            }
        })
        .collect()
}

/// Which of these pids are running claude right now. One `ps` for the lot;
/// a reused pid whose command is something else does not count — a heartbeat
/// must never outlive the session it describes.
fn alive_claude_pids(pids: &[u32]) -> HashSet<u32> {
    if pids.is_empty() {
        return HashSet::new();
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(output) = std::process::Command::new("ps")
        .args(["-o", "pid=,comm=", "-p", &list])
        .output()
    else {
        return HashSet::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid: u32 = parts.next()?.parse().ok()?;
            // macOS ps prints comm as the executable's full path; Linux the
            // bare name. Compared by basename so both answer alike.
            let comm = parts.next()?.rsplit('/').next()?;
            (comm.starts_with("claude") || comm.starts_with("node")).then_some(pid)
        })
        .collect()
}

#[cfg(test)]
mod tests;
