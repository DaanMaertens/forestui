//! Repository settings: one repository's overrides of the global Start work
//! settings and prompt templates.
//!
//! They live under `~/.config/forestui/repos/<owner>/<name>/`, never inside
//! the repository: a checkout's `git status` and its remote stay untouched,
//! and every worktree of the repository shares one copy. The key is the
//! GitHub repository `gh` resolves for the checkout, so a repository without
//! one has no settings of its own and uses the global ones.

use crate::models::{PartialStartWork, StartWorkSettings};
use crate::prompt::Template;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// The settings file in a repository's directory. Shaped like the global
/// `settings.json`, so a section copied from one works in the other; only
/// `start_work` is read from it.
pub const SETTINGS_FILE: &str = "settings.json";

/// A repository's templates, in a `prompts` directory as the global ones are.
pub const PROMPTS_DIR: &str = "prompts";

/// The GitHub `owner/name` a checkout resolves to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoKey {
    owner: String,
    name: String,
}

impl RepoKey {
    /// `None` for a part that is not a single plain path component. `gh`
    /// only ever reports GitHub's own names, but these become directories,
    /// and nothing read from outside may reach out of the `repos` tree.
    pub fn new(owner: &str, name: &str) -> Option<Self> {
        let plain =
            |part: &str| !matches!(part, "" | "." | "..") && !part.contains(['/', '\\', '\0']);
        (plain(owner) && plain(name)).then(|| Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    /// This repository's directory under `root`, the `repos` directory.
    pub fn dir(&self, root: &Path) -> PathBuf {
        root.join(&self.owner).join(&self.name)
    }
}

impl fmt::Display for RepoKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// One file a settings picker opens: the repository's settings file, or one
/// of the prompt templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFile {
    Settings,
    Template(Template),
}

impl ConfigFile {
    /// Where the file sits, relative to its settings directory.
    pub fn relative_path(self) -> String {
        match self {
            ConfigFile::Settings => SETTINGS_FILE.to_string(),
            ConfigFile::Template(template) => format!("{PROMPTS_DIR}/{}", template.file_name()),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ConfigFile::Settings => "Start work settings",
            ConfigFile::Template(template) => template.label(),
        }
    }
}

/// What a repository's settings file holds as forestui reads it.
#[derive(Default, Deserialize)]
#[serde(default)]
struct RepoSettingsFile {
    start_work: PartialStartWork,
}

/// The same shape as written, pre-filled from the global settings.
#[derive(Serialize)]
struct PrefilledFile<'a> {
    start_work: &'a StartWorkSettings,
}

/// A repository's overrides, and why there are none if its file was there
/// but could not be used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Loaded {
    pub overrides: PartialStartWork,
    pub warning: Option<String>,
}

/// The overrides in the repository directory `dir`.
///
/// No file is the normal case: nothing is overridden. A file that does not
/// parse overrides nothing either — a typo must not stop Start work — but
/// says so, because the user wrote it and expects it to count.
pub fn load(dir: &Path) -> Loaded {
    let path = dir.join(SETTINGS_FILE);
    let why = match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<RepoSettingsFile>(&raw) {
            Ok(file) => {
                return Loaded {
                    overrides: file.start_work,
                    warning: None,
                };
            }
            Err(e) => format!("does not parse ({e})"),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::default(),
        Err(e) => format!("could not be read ({e})"),
    };
    Loaded {
        overrides: PartialStartWork::default(),
        warning: Some(format!(
            "Repository settings {} {why}; using the global settings",
            path.display()
        )),
    }
}

/// The Start work settings in effect for a repository: the global values
/// with whatever the repository sets laid over them.
pub fn effective(global: &StartWorkSettings, repository: &PartialStartWork) -> StartWorkSettings {
    global.overlaid(repository)
}

/// The settings file in `dir`, written first from `global` if it is not
/// there yet, so the editor opens on the values in effect. Pre-filled, a
/// file overrides every key; deleting a key hands it back to the global
/// setting. A file that exists is never touched.
pub fn ensure_settings_file(dir: &Path, global: &StartWorkSettings) -> std::io::Result<PathBuf> {
    let path = dir.join(SETTINGS_FILE);
    if !path.exists() {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(&PrefilledFile { start_work: global })
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::util::write_atomically(&path, &format!("{json}\n"))?;
    }
    Ok(path)
}

/// `file` in the repository directory `dir`, created from the global
/// version on first use: the global settings, or the template as the global
/// prompts directory resolves it.
pub fn ensure_file(
    dir: &Path,
    file: ConfigFile,
    global_prompts: &Path,
    global: &StartWorkSettings,
) -> std::io::Result<PathBuf> {
    match file {
        ConfigFile::Settings => ensure_settings_file(dir, global),
        ConfigFile::Template(template) => {
            crate::prompt::ensure_file(&dir.join(PROMPTS_DIR), template, &[global_prompts])
        }
    }
}

#[cfg(test)]
mod tests;
