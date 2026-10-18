//! Repository settings as the app holds them: the GitHub repository each
//! checkout resolves to, and the overrides read from its settings directory.
//!
//! The key comes from `gh`, so it is learned in the background — on
//! selecting a repository that has none yet, and on every Start work open —
//! and folded in here. The overrides are a small local file, re-read on selection, on
//! Start work and on focus return, so an edit made in the editor counts the
//! next time any of those happens.

use super::App;
use crate::event::{AppEvent, Severity};
use crate::modal::{Modal, StartWorkSettingsModal};
use crate::models::{PartialStartWork, Scope, StartWorkSettings};
use crate::repo_settings::{self, ConfigFile, PROMPTS_DIR, RepoKey};
use crate::services::{github, settings as settings_service};
use std::path::PathBuf;

/// What is known about one checkout's repository settings.
#[derive(Debug, Default)]
pub(super) struct RepoLookup {
    /// `None` once `gh` resolved no GitHub repository for the checkout:
    /// there is no key, so the global settings apply.
    pub key: Option<RepoKey>,
    pub overrides: PartialStartWork,
    /// The warning last shown for the settings file, so a broken file is
    /// pointed out once rather than on every selection.
    warned: Option<String>,
}

impl App {
    /// The repository settings key for a checkout: `None` while `gh` has not
    /// answered, `Some(None)` when it found no GitHub repository.
    pub fn repo_key(&self, path: &str) -> Option<Option<&RepoKey>> {
        self.repos.get(path).map(|lookup| lookup.key.as_ref())
    }

    /// The Start work settings in effect for a checkout: the global ones,
    /// with its repository's laid over them once the key is known.
    pub fn start_work_for(&self, path: &str) -> StartWorkSettings {
        match self.repos.get(path) {
            Some(lookup) => repo_settings::effective(&self.settings.start_work, &lookup.overrides),
            None => self.settings.start_work.clone(),
        }
    }

    /// The base branch a checkout's repository settings name, if any. Only
    /// a repository's file has one; there is no global value to fall back to.
    pub fn base_branch_for(&self, path: &str) -> Option<String> {
        self.repos
            .get(path)
            .and_then(|lookup| lookup.overrides.base_branch())
            .map(str::to_string)
    }

    /// Where a checkout's prompt templates are looked for, first to last:
    /// its repository's, then the global ones. The built-in default comes
    /// after both.
    pub(super) fn prompt_layers_for(&self, path: &str) -> Vec<PathBuf> {
        let repository = self
            .repo_key(path)
            .flatten()
            .map(|key| key.dir(&settings_service::repos_dir()).join(PROMPTS_DIR));
        repository
            .into_iter()
            .chain([settings_service::prompts_dir()])
            .collect()
    }

    /// Ask `gh` which GitHub repository `path` resolves to. Start work asks
    /// on every open — a remote can be added or `gh repo set-default` run
    /// between two opens — and a selection only until the first answer.
    pub(super) fn probe_github(&mut self, path: String, for_start_work: bool) {
        if !for_start_work && !self.repo_probes.insert(path.clone()) {
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let repo = github::get_repo_info(&path)
                .await
                .and_then(|(owner, name)| RepoKey::new(&owner, &name));
            tx.send(AppEvent::GitHubChecked {
                path,
                repo,
                for_start_work,
            });
        });
    }

    pub(super) fn on_github_checked(
        &mut self,
        path: String,
        repo: Option<RepoKey>,
        for_start_work: bool,
    ) {
        self.repo_probes.remove(&path);
        let before = self.start_work_for(&path);
        let lookup = self.repos.entry(path.clone()).or_default();
        // `gh` answers no for a failure too — offline, logged out, rate
        // limited — and a remote is far less likely to have gone than `gh`
        // to have failed, so a no never drops a key already known: the
        // repository keeps its own settings.
        if lookup.key != repo && !(lookup.key.is_some() && repo.is_none()) {
            *lookup = RepoLookup {
                key: repo.clone(),
                ..RepoLookup::default()
            };
        }
        self.load_overrides(&path);
        for flow in self.start_work_flows(&path) {
            flow.set_github(repo.is_some());
        }
        self.settings_changed(&path, &before);
        // Only a GitHub repository has PRs to ask for, and only for the
        // scope now known to be in effect. Fetched on every open: a review
        // requested a minute ago belongs in the list.
        if for_start_work && repo.is_some() {
            self.fetch_pull_requests(&path);
        }
    }

    /// The scope an open Start work for `path` lists, if one is open.
    fn flow_scope(&mut self, path: &str) -> Option<Scope> {
        self.start_work_flows(path).next().map(|flow| flow.scope)
    }

    /// Fetch `pr-review`'s list for an open Start work, for the scope it
    /// lists.
    pub(super) fn fetch_pull_requests(&mut self, path: &str) {
        let Some(scope) = self.flow_scope(path) else {
            return;
        };
        let tx = self.tx.clone();
        let path = path.to_string();
        tokio::spawn(async move {
            let result = github::list_pull_requests(&path, scope).await;
            tx.send(AppEvent::PullRequests {
                path,
                scope,
                result,
            });
        });
    }

    /// Fill an open Start work's lists for the scope it now lists: the
    /// cached issues at once, refreshed behind when stale, and the PRs —
    /// which are never cached — from `gh`.
    pub(super) fn list_for_flow(&mut self, path: &str) {
        let Some(scope) = self.flow_scope(path) else {
            return;
        };
        let cached = github::peek_issues(path, scope);
        if let Some((issues, _)) = &cached {
            for flow in self.start_work_flows(path) {
                flow.set_issues(scope, issues.clone());
            }
        }
        if !matches!(cached, Some((_, true))) {
            self.fetch_issues_scoped(path.to_string(), scope);
        }
        if matches!(self.repo_key(path), Some(Some(_))) {
            self.fetch_pull_requests(path);
        }
    }

    /// Re-read a checkout's repository settings, and follow whatever that
    /// changed.
    pub(super) fn reread_repository_settings(&mut self, path: &str) {
        let before = self.start_work_for(path);
        self.load_overrides(path);
        self.settings_changed(path, &before);
    }

    /// Read the overrides for a checkout whose key is known.
    fn load_overrides(&mut self, path: &str) {
        let Some(lookup) = self.repos.get_mut(path) else {
            return;
        };
        let Some(key) = &lookup.key else {
            return;
        };
        let loaded = repo_settings::load(&key.dir(&settings_service::repos_dir()));
        lookup.overrides = loaded.overrides;
        if loaded.warning == lookup.warned {
            return;
        }
        lookup.warned = loaded.warning.clone();
        if let Some(warning) = loaded.warning {
            self.notify(warning, Severity::Warning);
        }
    }

    /// Hand a checkout's settings, if they changed from `before`, to an open
    /// Start work; and, if they changed whose issues are listed, re-list
    /// them for the flow and re-warm the cache for the selected repository.
    fn settings_changed(&mut self, path: &str, before: &StartWorkSettings) {
        let after = self.start_work_for(path);
        if after == *before {
            return;
        }
        let mut flow_open = false;
        for flow in self.start_work_flows(path) {
            flow.set_start_work(after.clone());
            flow_open = true;
        }
        if after.scope == before.scope {
            return;
        }
        let selected = self.state.selection.is_repository()
            && self
                .state
                .selected_repository()
                .map(|r| r.source_path.as_str())
                == Some(path);
        if selected {
            self.prefetch_issues(path);
        }
        // The flow dropped both lists, fetched for the old scope.
        if flow_open {
            self.list_for_flow(path);
        }
    }

    /// Open a repository's settings file or one of its templates in the
    /// editor, created from the global version on first use. The file lives
    /// in forestui's config dir, never in the checkout, so opening it changes
    /// nothing `git status` can see.
    pub(super) fn edit_repository_file(&mut self, key: RepoKey, file: ConfigFile) {
        let dir = key.dir(&settings_service::repos_dir());
        match repo_settings::ensure_file(
            &dir,
            file,
            &settings_service::prompts_dir(),
            &self.settings.start_work,
        ) {
            Ok(path) => self.open_in_editor_as(
                &format!("{key}/{}", file.relative_path()),
                &path.to_string_lossy(),
            ),
            Err(error) => self.notify(
                format!("Could not create {key}'s {}: {error}", file.relative_path()),
                Severity::Error,
            ),
        }
    }

    /// Open the Start Work Settings dialog on a repository's overrides, as
    /// its settings file holds them now. A small local file, read here for
    /// the same reason the overrides are re-read on selection.
    pub(super) fn open_repository_start_work_settings(&mut self, key: RepoKey) {
        let file = repo_settings::read_file(&key.dir(&settings_service::repos_dir()));
        let modal = StartWorkSettingsModal::repository(key, &self.settings.start_work, file);
        self.modals.push(Modal::StartWorkSettings(Box::new(modal)));
    }

    /// Replace the global Start work settings with what the dialog saved. A
    /// repository's overrides are laid over them as before, so they keep
    /// winning in their repository.
    pub(super) fn save_global_start_work(&mut self, start_work: StartWorkSettings) {
        let paths: Vec<String> = self.repos.keys().cloned().collect();
        let before: Vec<StartWorkSettings> =
            paths.iter().map(|path| self.start_work_for(path)).collect();
        self.settings.start_work = start_work;
        match settings_service::save_settings(&self.settings) {
            Ok(()) => self.notify("Start work settings saved", Severity::Information),
            Err(error) => self.notify(format!("Could not save settings: {error}"), Severity::Error),
        }
        for (path, before) in paths.iter().zip(before) {
            self.settings_changed(path, &before);
        }
    }

    /// Write a repository's changed overrides into its settings file, then
    /// re-read them for every checkout of that repository, so the next
    /// Start work there uses them.
    pub(super) fn save_repository_start_work(
        &mut self,
        key: RepoKey,
        changes: Vec<repo_settings::OverrideChange>,
    ) {
        let dir = key.dir(&settings_service::repos_dir());
        if let Err(error) = repo_settings::save_overrides(&dir, &changes) {
            self.notify(
                format!("Could not save {key}'s settings: {error}"),
                Severity::Error,
            );
            return;
        }
        self.notify(
            format!("Saved {key}'s Start work settings"),
            Severity::Information,
        );
        let paths: Vec<String> = self
            .repos
            .iter()
            .filter(|(_, lookup)| lookup.key.as_ref() == Some(&key))
            .map(|(path, _)| path.clone())
            .collect();
        for path in paths {
            self.reread_repository_settings(&path);
        }
    }
}
