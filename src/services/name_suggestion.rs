//! The name suggestion: a headless `claude -p` that reads a session prompt
//! and answers with a worktree name for it.
//!
//! Everything about it is best effort. A missing or too-old `claude`, no
//! network, a timeout or a non-zero exit all come back as `None`, and the
//! session keeps its first-words names without a word said: the suggestion
//! is an improvement on names that already work, never something to wait on
//! or to be told about.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::models::SessionNaming;

/// Ask Claude for a name for `instruction` — the `session-name` template
/// already filled with the session prompt — run in `cwd`, the repository,
/// so any tools `naming` allows see the code the session is about. The raw
/// reply; turning it into names is `work::suggested_session_names`.
pub async fn suggest(cwd: &Path, instruction: &str, naming: &SessionNaming) -> Option<String> {
    if cfg!(test) {
        // The suite must not depend on, spawn, or pay for whatever Claude
        // the developer has installed.
        return None;
    }
    let mut command = Command::new("claude");
    command
        .args(arguments(naming))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let run = async {
        let mut child = command.spawn().ok()?;
        // Through stdin, never as an argument: `--tools` takes every word
        // after it, so a positional prompt there would be read as tools.
        let mut stdin = child.stdin.take()?;
        stdin.write_all(instruction.as_bytes()).await.ok()?;
        drop(stdin);
        let output = child.wait_with_output().await.ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    tokio::time::timeout(Duration::from_secs(naming.timeout_secs), run)
        .await
        .ok()
        .flatten()
}

/// The command line after `claude`. The call is a throwaway, so it leaves
/// nothing behind: no transcript (which would show up as a session card in
/// the repository), and no hooks — forestui's own plugin would otherwise
/// report it as a live session for as long as it runs. MCP servers are not
/// started, since no tool may reach them unless allowed here.
fn arguments(naming: &SessionNaming) -> Vec<String> {
    let tools = naming.allowed_tools.join(",");
    let mut args = vec![
        "-p".to_string(),
        "--model".into(),
        naming.model.clone(),
        "--no-session-persistence".into(),
        "--strict-mcp-config".into(),
        "--disable-slash-commands".into(),
        "--settings".into(),
        r#"{"disableAllHooks":true}"#.into(),
        // `""` takes every built-in tool away.
        "--tools".into(),
        tools.clone(),
    ];
    // Print mode has nobody to ask, so a tool needing permission would be
    // refused; the ones the user allowed are allowed outright.
    if !tools.is_empty() {
        args.extend(["--allowedTools".into(), tools]);
    }
    args
}

#[cfg(test)]
mod tests;
