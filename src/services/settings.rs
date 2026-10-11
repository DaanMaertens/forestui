//! User preferences and the runtime forest path.
//!
//! Preferences live in `~/.config/forestui/settings.json` (global). The forest
//! path is a CLI argument, not a setting, so multiple forests stay independent.

use crate::models::Settings;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

static FOREST_PATH: OnceLock<RwLock<PathBuf>> = OnceLock::new();

fn forest_cell() -> &'static RwLock<PathBuf> {
    FOREST_PATH.get_or_init(|| RwLock::new(default_forest_path()))
}

pub fn default_forest_path() -> PathBuf {
    crate::util::home_dir().join("forest")
}

/// Set the runtime forest path. `None` restores the default `~/forest`.
pub fn set_forest_path(path: Option<&str>) {
    let resolved = match path {
        None => default_forest_path(),
        Some(p) => crate::util::expand_and_resolve(p),
    };
    if let Ok(mut guard) = forest_cell().write() {
        *guard = resolved;
    }
}

pub fn get_forest_path() -> PathBuf {
    forest_cell()
        .read()
        .map(|p| p.clone())
        .unwrap_or_else(|_| default_forest_path())
}

pub fn settings_path() -> PathBuf {
    // A test that drives the app through a saving action must never reach
    // the developer's own settings file. One file per test thread, since
    // tests run side by side; the process id keeps a later run from reading
    // a file an earlier one left behind.
    #[cfg(test)]
    return std::env::temp_dir().join(format!(
        "forestui-test-settings-{}-{:?}.json",
        std::process::id(),
        std::thread::current().id()
    ));
    #[cfg(not(test))]
    crate::util::home_dir()
        .join(".config")
        .join("forestui")
        .join("settings.json")
}

/// Where the global prompt templates live: `prompts/` beside the settings
/// file, one Markdown file per template.
pub fn prompts_dir() -> PathBuf {
    // Beside the test settings file too, so a test that opens a template
    // writes into its own temp dir rather than the developer's config.
    #[cfg(test)]
    return std::env::temp_dir().join(format!(
        "forestui-test-prompts-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    #[cfg(not(test))]
    crate::util::home_dir()
        .join(".config")
        .join("forestui")
        .join("prompts")
}

/// Where repository settings live: `repos/<owner>/<name>/` beside the
/// settings file. Never inside a repository — see [`crate::repo_settings`].
pub fn repos_dir() -> PathBuf {
    #[cfg(test)]
    return std::env::temp_dir().join(format!(
        "forestui-test-repos-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    #[cfg(not(test))]
    crate::util::home_dir()
        .join(".config")
        .join("forestui")
        .join("repos")
}

/// Load settings, falling back to defaults for a missing or unreadable file.
pub fn load_settings() -> Settings {
    load_settings_from(&settings_path())
}

pub fn load_settings_from(path: &Path) -> Settings {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Settings::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> std::io::Result<()> {
    save_settings_to(&settings_path(), settings)
}

pub fn save_settings_to(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    crate::util::write_atomically(path, &json)
}

#[cfg(test)]
mod tests;
