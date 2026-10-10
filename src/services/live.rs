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
use std::collections::HashSet;
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

/// One live Claude session and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    pub session_id: String,
    pub place: LivePlace,
}

/// A session's heartbeat, as `heartbeat.sh` writes it from inside the session.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Heartbeat {
    pub session_id: String,
    pub pid: u32,
    #[serde(default)]
    pub tmux_pane: String,
    #[serde(default)]
    pub cwd: String,
}

/// Where the plugin writes heartbeats. Must match `heartbeat.sh` verbatim.
pub fn live_dir() -> PathBuf {
    crate::util::home_dir()
        .join(".config")
        .join("forestui")
        .join("live")
}

/// The full liveness picture, both sources merged. One `list-windows`, one
/// directory read, one `ps` — run off the UI loop like every other scan.
pub fn snapshot() -> Vec<LiveSession> {
    merge(
        tmux::list_claude_windows(),
        read_heartbeats(),
        tmux::current_session(),
        tmux::pane_window,
    )
}

/// Pure form of [`snapshot`], for testing: `resolve` stands in for asking
/// tmux which window a heartbeat's pane belongs to.
pub fn merge(
    windows: Vec<tmux::ClaudeWindow>,
    beats: Vec<Heartbeat>,
    ours: Option<String>,
    resolve: impl Fn(&str) -> Option<tmux::PaneWindow>,
) -> Vec<LiveSession> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<LiveSession> = Vec::new();

    // Stamped windows where Claude is the pane's foreground process. A window
    // whose Claude exited deliberately stays out: that session is free again.
    for window in windows {
        if window.running && seen.insert(window.session_id.clone()) {
            out.push(LiveSession {
                session_id: window.session_id,
                place: LivePlace::Window {
                    window_id: window.window_id,
                    window_name: window.window_name,
                },
            });
        }
    }

    // Heartbeats cover everything else. A recorded pane that resolves onto
    // our own tmux session is still switchable; anything else is elsewhere.
    // (A suspended Claude — Ctrl-Z — is alive by pid and so counts as live
    // here, which is right: resuming it somewhere else would fork the
    // conversation a `fg` can still continue.)
    for beat in read_heartbeats_merge(beats, &mut seen) {
        let place = match (&ours, beat.tmux_pane.is_empty()) {
            (Some(ours), false) => match resolve(&beat.tmux_pane) {
                Some(pane) if &pane.tmux_session == ours => LivePlace::Window {
                    window_id: pane.window_id,
                    window_name: pane.window_name,
                },
                _ => LivePlace::Elsewhere { pid: beat.pid },
            },
            _ => LivePlace::Elsewhere { pid: beat.pid },
        };
        out.push(LiveSession {
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
