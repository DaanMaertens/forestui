//! Installing the Claude Code plugin that names a session after its tmux window.
//!
//! The plugin is a directory forestui owns end to end: a manifest, a hooks
//! declaration and one shell script. Claude Code discovers plugin directories
//! under `<config>/skills/` on its own, so installing writes nothing into the
//! user's `settings.json` and cannot disturb hooks they configured themselves —
//! plugin hooks and settings hooks both run.
//!
//! Enabling and disabling is Claude's own business (`claude plugin
//! enable|disable`), which is why nothing here edits `enabledPlugins`.

use crate::util;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const PLUGIN_NAME: &str = "forestui-integration";

/// The files that make up the plugin, relative to its directory. Shipped inside
/// the binary so an install is never a download and never a partial checkout.
const ASSETS: &[(&str, &str)] = &[
    (
        ".claude-plugin/plugin.json",
        include_str!("../../assets/claude-plugin/plugin.json"),
    ),
    (
        "hooks/hooks.json",
        include_str!("../../assets/claude-plugin/hooks.json"),
    ),
    (
        "hooks/tmux-title.sh",
        include_str!("../../assets/claude-plugin/tmux-title.sh"),
    ),
    (
        "hooks/heartbeat.sh",
        include_str!("../../assets/claude-plugin/heartbeat.sh"),
    ),
];

/// The files that have to be executable — Claude runs them as commands.
const EXECUTABLES: &[&str] = &["hooks/tmux-title.sh", "hooks/heartbeat.sh"];

/// Bumped whenever a shipped file changes, so an install by an older forestui
/// is recognised as old rather than reported as the user having edited it.
const SHIPPED_VERSION: &str = "5";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    NotInstalled,
    /// Every shipped file is present and byte-identical to this build's copy.
    Installed,
    /// An older forestui installed this. Upgrading is not overwriting the
    /// user's work, so it needs no confirmation — which is the whole reason
    /// this is a separate state from drift.
    Outdated {
        installed: String,
    },
    /// Installed at this build's version, but these files differ from what it
    /// ships. That is a hand edit, and it is never overwritten unasked.
    Drifted(Vec<String>),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::NotInstalled => "Not installed".to_string(),
            Status::Installed => "Installed".to_string(),
            Status::Outdated { installed } => {
                format!("Installed at version {installed}, this build ships {SHIPPED_VERSION}")
            }
            Status::Drifted(files) => format!("Installed, {} file(s) modified", files.len()),
        }
    }
}

/// The startup nag for an install an older forestui wrote, and only that.
///
/// `NotInstalled` is a choice the user never made and `Drifted` is their own
/// edit — neither is forestui's to warn about. Outdated is different: the
/// user opted in, and a newer build ships hooks their install lacks (the
/// liveness heartbeat arrived in version 4), so silence would look like the
/// feature not working.
pub fn upgrade_notice(status: &Status) -> Option<String> {
    match status {
        Status::Outdated { installed } => Some(format!(
            "Claude integration is outdated (v{installed}, this build ships v{SHIPPED_VERSION}) — \
             upgrade it in Settings > Claude Code Integration"
        )),
        _ => None,
    }
}

/// Where Claude Code keeps its configuration. `CLAUDE_CONFIG_DIR` wins so that
/// installing lands wherever Claude will actually read from.
pub fn claude_config_dir() -> PathBuf {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(dir) if !dir.trim().is_empty() => util::expanduser(dir.trim()),
        _ => util::home_dir().join(".claude"),
    }
}

pub fn plugin_dir() -> PathBuf {
    claude_config_dir().join("skills").join(PLUGIN_NAME)
}

pub fn status() -> Status {
    status_in(&plugin_dir())
}

/// Directory-scoped form of [`status`], for testing.
pub fn status_in(dir: &Path) -> Status {
    if !dir.join(".claude-plugin/plugin.json").exists() {
        return Status::NotInstalled;
    }
    if installed_version(dir).as_deref() != Some(SHIPPED_VERSION) {
        return Status::Outdated {
            installed: installed_version(dir).unwrap_or_else(|| "unknown".to_string()),
        };
    }

    let drifted: Vec<String> = ASSETS
        .iter()
        .filter(|(rel, shipped)| {
            std::fs::read_to_string(dir.join(rel))
                .map(|found| digest(&found) != digest(shipped))
                .unwrap_or(true)
        })
        .map(|(rel, _)| (*rel).to_string())
        .collect();

    if drifted.is_empty() {
        Status::Installed
    } else {
        Status::Drifted(drifted)
    }
}

fn installed_version(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join(".claude-plugin/plugin.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(parsed.get("version")?.as_str()?.to_string())
}

/// The paths an install touches, for showing the user before it happens.
pub fn planned_paths() -> Vec<PathBuf> {
    let dir = plugin_dir();
    ASSETS.iter().map(|(rel, _)| dir.join(rel)).collect()
}

/// The shipped files, relative to the plugin directory. Shown under the
/// directory rather than as absolute paths: three absolute paths that differ
/// only in their last component are three identical lines once a dialog has
/// truncated them.
pub fn asset_names() -> Vec<&'static str> {
    ASSETS.iter().map(|(rel, _)| *rel).collect()
}

pub fn install(overwrite_drift: bool) -> Result<(), String> {
    install_in(&plugin_dir(), overwrite_drift)
}

/// Directory-scoped form of [`install`], for testing.
pub fn install_in(dir: &Path, overwrite_drift: bool) -> Result<(), String> {
    if let Status::Drifted(files) = status_in(dir)
        && !overwrite_drift
    {
        return Err(format!(
            "{} was modified after forestui installed it ({}). Re-install to overwrite those edits.",
            dir.display(),
            files.join(", ")
        ));
    }

    for (rel, contents) in ASSETS {
        let target = dir.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        util::write_atomically(&target, contents)
            .map_err(|e| format!("could not write {}: {e}", target.display()))?;
    }
    for rel in EXECUTABLES {
        make_executable(&dir.join(rel))?;
    }
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    uninstall_in(&plugin_dir())
}

/// Directory-scoped form of [`uninstall`], for testing.
///
/// Refuses to remove a directory that does not look like this plugin, so a
/// mis-set `CLAUDE_CONFIG_DIR` cannot turn an uninstall into a recursive
/// delete of something else.
pub fn uninstall_in(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    let manifest = std::fs::read_to_string(dir.join(".claude-plugin/plugin.json"))
        .map_err(|_| format!("{} is not a forestui plugin directory", dir.display()))?;
    // Parsed, not substring-matched: this guards a recursive delete, and
    // "forestui-integration-fork" contains our name without being ours.
    let named_ours = serde_json::from_str::<serde_json::Value>(&manifest)
        .ok()
        .and_then(|v| v.get("name")?.as_str().map(str::to_string))
        .is_some_and(|name| name == PLUGIN_NAME);
    if !named_ours {
        return Err(format!(
            "{} holds a different plugin; refusing to remove it",
            dir.display()
        ));
    }
    std::fs::remove_dir_all(dir).map_err(|e| format!("could not remove {}: {e}", dir.display()))
}

/// The hook reads the session's own name with `jq`. Without it the sync does
/// nothing at all rather than half of it, so this is worth saying out loud
/// wherever the plugin is installed.
pub fn jq_available() -> bool {
    std::process::Command::new("jq")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn digest(contents: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    hasher.finalize().into()
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| format!("could not stat {}: {e}", path.display()))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
        .map_err(|e| format!("could not make {} executable: {e}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests;
