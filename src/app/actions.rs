//! Actions and the flows behind them: running a control, folding modal
//! results into state, and the git/tmux work they spawn.
//!
//! Anything that may take time runs in a `tokio::spawn` with a cloned
//! `EventTx`; results come back as events, and only the main loop ever
//! writes the state file.

use super::{Action, App, Field, PendingJump, PendingParent, PendingStart};
use crate::event::Severity;
use crate::event::{AppEvent, BranchList, BranchTarget, ClaudeStart, EventTx};
use crate::modal::{BackKeyAnswer, IntegrationAction, ModalOutcome, ParentLaunch, ParentSource};
use crate::modal::{
    Clash, ConfirmAction, ConfirmModal, Modal, ModalEffect, ModalResult, PromptTemplatesModal,
    StartWorkConfirmModal, StartWorkModal, WorktreeForm,
};
use crate::models::{
    CustomClaudeButton, GitHubIssue, IssueFilter, Repository, WorkItemLink, Worktree,
};
use crate::services::live::LivePlace;
use crate::services::{claude_session, git, github, settings as settings_service, tmux};
use crate::util::Noun;
use crate::work::{self, WorkItem};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// A worktree created to start work in: the work item it records, and the
/// first message Claude is started with once it exists.
struct WorkStart {
    link: WorkItemLink,
    first_prompt: Option<String>,
    /// Start that Claude with the built-in YOLO flag.
    yolo: bool,
}

/// Create the worktree `name` on `branch` at `worktree_path`, recording
/// where it stemmed from for the detail header and the finished check.
async fn new_worktree(
    source_path: &str,
    worktree_path: &Path,
    name: String,
    branch: String,
    new_branch: bool,
    base_branch: Option<String>,
) -> git::GitResult<Worktree> {
    let (base, base_ref) = if let Some(base) = base_branch.clone() {
        let reference = git::get_ref(source_path, &base).await;
        (Some(base), reference)
    } else if new_branch {
        let current = git::get_current_branch(source_path).await.ok();
        (current, git::get_ref(source_path, "HEAD").await)
    } else {
        (
            Some(branch.clone()),
            git::get_ref(source_path, &branch).await,
        )
    };

    let branch = git::create_worktree(
        source_path,
        worktree_path,
        &branch,
        new_branch,
        base_branch.as_deref(),
    )
    .await?;

    let mut worktree = Worktree::new(name, branch, worktree_path.to_string_lossy().to_string());
    worktree.base_branch = base;
    worktree.created_from_ref = base_ref;
    Ok(worktree)
}

/// Create a worktree for reviewing PR `number`, on the PR's own branch, and
/// return that branch.
///
/// The worktree starts detached and `gh pr checkout` runs inside it: `gh`
/// knows where the head lives, a fork included, and names the local branch.
/// `expected` is only the fallback should git not say which branch that was.
/// A checkout that fails takes the half-made worktree away with it, so a
/// retry does not trip over the directory.
async fn create_review_worktree(
    source_path: &str,
    worktree_path: &Path,
    number: i64,
    expected: &str,
) -> Result<String, String> {
    git::create_detached_worktree(source_path, worktree_path)
        .await
        .map_err(|error| error.to_string())?;
    let path = worktree_path.to_string_lossy().to_string();
    if let Err(error) = github::checkout_pull_request(&path, number).await {
        let _ = git::force_remove_worktree(source_path, &path, git::Force::Changes).await;
        return Err(error);
    }
    Ok(match git::get_current_branch(&path).await {
        Ok(branch) if branch != "HEAD" => branch,
        _ => expected.to_string(),
    })
}

/// Gather the branch and remote lists and deliver them to `target`.
///
/// The one implementation behind both the initial modal open and the
/// refetch-after-fetch inside it — what a branch load returns must not depend
/// on which of the two paths asked for it.
async fn load_branches(tx: EventTx, path: String, repo_id: Uuid, target: BranchTarget) {
    let branches = git::list_branches(&path).await.unwrap_or_default();
    let remotes = git::list_remotes(&path).await.unwrap_or_default();
    let remote_default = git::remote_default_branch(&path, &remotes).await;
    let current_branch = git::get_current_branch(&path)
        .await
        .unwrap_or_else(|_| "main".to_string());
    tx.send(AppEvent::Branches {
        repo_id,
        list: BranchList {
            branches,
            remotes,
            remote_default,
            current_branch,
        },
        target,
    });
}

impl App {
    // ------------------------------------------------------------------- modals

    pub(super) fn handle_modal_key(&mut self, key: ratatui::crossterm::event::KeyEvent) {
        let Some(modal) = self.modals.last_mut() else {
            return;
        };
        match modal.handle_key(key) {
            ModalOutcome::None => {}
            ModalOutcome::Close => {
                let closed = self.modals.pop();
                // An issue picked just before closing must not open its
                // confirm screen over nothing once the branches arrive.
                if matches!(closed, Some(Modal::StartWork(_))) {
                    self.pending_start = None;
                }
                // The integration dialog installs and removes the plugin, so
                // the Settings section that opened it has to re-read the status
                // it snapshotted — including on Esc, which reports nothing.
                if matches!(closed, Some(Modal::ClaudeIntegration(_)))
                    && let Some(Modal::Settings(parent)) = self.modals.last_mut()
                {
                    parent.integration_status = crate::services::claude_plugin::status();
                }
            }
            ModalOutcome::Push(child) => self.modals.push(*child),
            ModalOutcome::Effect(effect) => self.run_modal_effect(effect),
            ModalOutcome::Submit(result) => {
                self.modals.pop();
                if let Some(parent) = self.modals.last_mut() {
                    parent.receive_child(&result);
                }
                self.apply_modal_result(result);
            }
        }
    }

    /// Install, upgrade, overwrite or remove the plugin, then leave the dialog
    /// showing what happened.
    ///
    /// Synchronous on purpose: this writes three small files inside the user's
    /// own config directory, and a dialog that reported "Installed" from a
    /// background task could just as easily report it before the write landed.
    fn run_claude_plugin_action(&mut self, action: IntegrationAction) {
        use crate::services::claude_plugin;

        let outcome = match action {
            IntegrationAction::Install | IntegrationAction::Upgrade => {
                claude_plugin::install(false)
                    .map(|()| "Sessions started from now on are named after their tab.".to_string())
            }
            // Reinstall writes the same bytes back; overwrite is the only path
            // that discards a hand edit, and it is only reachable from a dialog
            // whose button says so.
            IntegrationAction::Reinstall => {
                claude_plugin::install(false).map(|()| "Shipped files rewritten.".to_string())
            }
            IntegrationAction::Overwrite => {
                claude_plugin::install(true).map(|()| "Local edits replaced.".to_string())
            }
            IntegrationAction::Uninstall => claude_plugin::uninstall()
                .map(|()| "Removed. Sessions already running keep their names.".to_string()),
            IntegrationAction::Close => return,
        };

        let message = match outcome {
            Ok(done) => {
                self.notify(done.clone(), Severity::Information);
                done
            }
            Err(error) => {
                self.notify(error.clone(), Severity::Error);
                error
            }
        };
        if let Some(Modal::ClaudeIntegration(modal)) = self.modals.last_mut() {
            modal.refresh(message);
        }
    }

    /// Show or hide blocked issues in Start work, and remember the choice.
    /// Saved here because the main loop is the one writer of the settings
    /// file; only a failure is worth a toast, since the list changing is the
    /// confirmation.
    fn set_show_blocked_issues(&mut self, show: bool) {
        if self.settings.show_blocked_issues == show {
            return;
        }
        self.settings.show_blocked_issues = show;
        if let Err(error) = settings_service::save_settings(&self.settings) {
            self.notify(format!("Could not save settings: {error}"), Severity::Error);
        }
    }

    /// Remember Start work's issue filter, saved like "Show blocked".
    fn set_issue_filter(&mut self, filter: IssueFilter) {
        if self.settings.issue_filter == filter {
            return;
        }
        self.settings.issue_filter = filter;
        if let Err(error) = settings_service::save_settings(&self.settings) {
            self.notify(format!("Could not save settings: {error}"), Severity::Error);
        }
    }

    /// Save the answer to the back key offer and act on it. A new key goes
    /// through the same check, so it can bring the offer back for itself.
    fn answer_back_key(&mut self, answer: BackKeyAnswer) {
        let (key, accepted) = match answer {
            BackKeyAnswer::Key(key) => (Some(key), None),
            BackKeyAnswer::UseAnyway { key, binding } => (Some(key), Some(binding)),
            BackKeyAnswer::Off => (None, None),
        };
        self.settings.back_key = key;
        self.settings.back_key_accepted = accepted;
        if let Err(error) = settings_service::save_settings(&self.settings) {
            self.notify(format!("Could not save settings: {error}"), Severity::Error);
        }
        self.apply_back_key();
    }

    fn run_modal_effect(&mut self, effect: ModalEffect) {
        match effect {
            ModalEffect::ClaudePlugin(action) => self.run_claude_plugin_action(action),
            ModalEffect::ConfirmIssue(issue) => {
                // The repository the flow was opened for, not whatever the
                // selection says now.
                let Some(Modal::StartWork(flow)) = self.modals.last() else {
                    return;
                };
                let repo_id = flow.repo_id;
                let parent = issue
                    .open_parent()
                    .and_then(|parent| flow.loaded_issue(parent.number))
                    .cloned();
                self.start_work_on_issue(repo_id, *issue, parent);
            }
            ModalEffect::LoadWorktreeBranches => {
                let Some(Modal::StartWork(flow)) = self.modals.last() else {
                    return;
                };
                let repo_id = flow.repo_id;
                self.load_branches_then(repo_id, BranchTarget::StartWorkWorktree);
            }
            ModalEffect::ConfirmPullRequest(item) => {
                let Some(Modal::StartWork(flow)) = self.modals.last() else {
                    return;
                };
                let repo_id = flow.repo_id;
                self.start_work_on_pull_request(repo_id, *item);
            }
            ModalEffect::ConfirmSession { prompt, names } => {
                let Some(Modal::StartWork(flow)) = self.modals.last() else {
                    return;
                };
                let repo_id = flow.repo_id;
                self.start_session(repo_id, prompt, names);
            }
            ModalEffect::LaunchSession => self.launch_session(),
            ModalEffect::OpenWorktree(worktree_id) => {
                let Some(Modal::StartWork(flow)) = self.modals.last() else {
                    return;
                };
                let repo_id = flow.repo_id;
                // Chosen by the user, so there is nothing to tell them.
                self.jump_to_worktree(repo_id, worktree_id, None);
            }
            ModalEffect::EditPrompt(text) => {
                let Some(Modal::StartWorkConfirm(confirm)) = self.modals.last() else {
                    return;
                };
                let (repo_id, link) = (confirm.repo_id, confirm.item.link());
                let editor = self.prompt_editor();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        crate::prompt::edit_via_file(&text, |file| {
                            tmux::edit_in_popup(&editor, file)
                        })
                    })
                    .await
                    .unwrap_or_else(|_| Err("the editor task panicked".to_string()));
                    tx.send(AppEvent::PromptEdited {
                        repo_id,
                        link,
                        result,
                    });
                });
            }
            ModalEffect::ShowBlockedIssues(show) => self.set_show_blocked_issues(show),
            ModalEffect::IssueFilter(filter) => self.set_issue_filter(filter),
            ModalEffect::ListScope(path) => self.list_for_flow(&path),
            ModalEffect::EditTemplate(template) => self.edit_template(template),
            ModalEffect::EditRepositoryFile(key, file) => self.edit_repository_file(key, file),
            ModalEffect::RepositoryStartWorkSettings(key) => {
                self.open_repository_start_work_settings(key)
            }
            ModalEffect::Fetch(repo_path) => {
                let repo_id = self.state.selection.repository_id.unwrap_or_default();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    if let Err(error) = git::fetch(&repo_path).await {
                        tx.send(AppEvent::FetchFailed(error.to_string()));
                        return;
                    }
                    load_branches(tx, repo_path, repo_id, BranchTarget::RefetchOpenModal).await;
                });
            }
        }
    }

    fn apply_modal_result(&mut self, result: ModalResult) {
        match result {
            ModalResult::RepositoryAdded { path } => self.add_repository(path),
            ModalResult::WorktreeCreated {
                repo_id,
                name,
                branch,
                new_branch,
                base_branch,
            } => self.create_worktree(repo_id, name, branch, new_branch, base_branch, false, None),
            ModalResult::WorktreeForWorkItem {
                repo_id,
                link,
                name,
                branch,
                pull_first,
                base_branch,
                first_prompt,
                yolo,
                parent,
            } => {
                // Launching ends the whole flow: the item step left beneath
                // the confirm screen has nothing more to offer.
                self.modals.retain(|m| !matches!(m, Modal::StartWork(_)));
                let start = WorkStart {
                    link,
                    first_prompt,
                    yolo,
                };
                if let Some(parent) = parent {
                    let names = work::WorktreeNames {
                        worktree: name,
                        branch,
                    };
                    self.create_sub_issue_worktree(repo_id, names, parent, pull_first, start);
                    return;
                }
                self.create_worktree(
                    repo_id,
                    name,
                    branch,
                    true,
                    base_branch,
                    pull_first,
                    Some(start),
                );
            }
            ModalResult::SettingsSaved(settings) => {
                let back_key_moved = settings.back_key != self.settings.back_key
                    || settings.back_key_accepted != self.settings.back_key_accepted;
                self.settings = *settings;
                // On every path that reaches Save the picker has already made
                // active == theme_name (commit applies, Esc reverts), so this
                // is not correction but ownership: the fold applies the state
                // it was handed, rather than trusting a dialog to have left
                // the global in the right place.
                crate::theme::set_active(&self.settings.theme_name);
                if let Err(error) = settings_service::save_settings(&self.settings) {
                    self.notify(format!("Could not save settings: {error}"), Severity::Error);
                } else {
                    self.notify("Settings saved", Severity::Information);
                }
                self.detail_index = 0;
                // The sidebar derives each row's branch suffix from
                // `branch_prefix`, so a changed prefix has to rebuild the rows
                // or they keep showing the old elision until something else does.
                self.rebuild_rows();
                if back_key_moved {
                    self.apply_back_key();
                }
            }
            ModalResult::StartWorkSettingsSaved(start_work) => {
                self.save_global_start_work(*start_work)
            }
            ModalResult::RepositoryStartWorkSaved { key, changes } => {
                self.save_repository_start_work(key, changes)
            }
            ModalResult::BackKeyAnswered(answer) => self.answer_back_key(answer),
            // The picker already applied the theme live and `receive_child`
            // carried the slug into the Settings dialog; persisting waits for
            // Save.
            ModalResult::CustomButtonsSaved(_)
            | ModalResult::CustomButtonSaved(_)
            | ModalResult::ThemeChosen(_)
            | ModalResult::BackKeyPicked(_) => {}
            ModalResult::Confirmed(action) => self.apply_confirmed(action),
            ModalResult::SessionRenamed {
                path,
                session_id,
                name,
                live_window_id,
            } => self.apply_session_rename(path, session_id, name, live_window_id),
        }
    }

    /// Run a committed session rename, live or stopped.
    fn apply_session_rename(
        &mut self,
        path: String,
        session_id: String,
        name: String,
        live_window_id: Option<String>,
    ) {
        match live_window_id {
            // Live: rename the tmux window and let the sync plugin adopt the
            // name into the session on its next hook — the same path a manual
            // tab rename takes, and the only safe one while Claude holds the
            // transcript open.
            Some(window_id) => {
                let tx = self.tx.clone();
                let shown = name.clone();
                tokio::spawn(async move {
                    let renamed = tokio::task::spawn_blocking(move || {
                        let renamed = tmux::rename_window_by_id(&window_id, &name);
                        // The badge on screen names the window; refresh it in
                        // the same breath so it does not spend the next sweep
                        // interval telling the user the name they just changed.
                        (renamed, crate::services::live::snapshot())
                    })
                    .await
                    .ok();
                    match renamed {
                        Some((true, sessions)) => {
                            tx.info(format!(
                                "Window renamed to '{shown}' — the session follows on its next prompt"
                            ));
                            tx.send(AppEvent::LiveSessions { sessions });
                        }
                        _ => tx.error("Could not rename the window"),
                    }
                });
            }
            // Stopped: append the same record `/rename` writes, then re-read
            // the transcript so the card shows the new name in the same fold
            // that announces it.
            None => {
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let (read_path, read_id, rename_name) =
                        (path.clone(), session_id.clone(), name.clone());
                    let result = tokio::task::spawn_blocking(move || {
                        claude_session::rename_session(&read_path, &read_id, &rename_name)
                    })
                    .await
                    .unwrap_or_else(|_| Err("the rename task panicked".to_string()));
                    let session = if result.is_ok() {
                        let (p, id) = (path.clone(), session_id.clone());
                        tokio::task::spawn_blocking(move || claude_session::refresh_one(&p, &id))
                            .await
                            .ok()
                            .flatten()
                    } else {
                        None
                    };
                    tx.send(AppEvent::SessionRenamed {
                        path,
                        session_id,
                        session: Box::new(session),
                        result,
                    });
                });
            }
        }
    }

    fn apply_confirmed(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::RemoveRepository(repo_id) => {
                self.state.remove_repository(repo_id);
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
            }
            ConfirmAction::DeleteWorktree(worktree_id) => {
                self.spawn_worktree_removal(worktree_id, None);
            }
            ConfirmAction::ForceDeleteWorktree(worktree_id, force) => {
                self.spawn_worktree_removal(worktree_id, Some(force));
            }
            ConfirmAction::PruneFinished(worktree_ids) => self.spawn_prune(&worktree_ids),
            ConfirmAction::DeleteClaudeSession {
                path,
                session_id,
                title,
            } => {
                let tx = self.tx.clone();
                let (task_path, task_id) = (path.clone(), session_id.clone());
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        // The guard that disabled Del read a map up to a sweep
                        // old; a session resumed since would lose its
                        // transcript out from under a running Claude. Ask
                        // again at the moment of truth — tmux for a *running*
                        // window (an exited one holds nothing), and the
                        // heartbeats for a claude running anywhere else.
                        if let Some(window) = tmux::list_claude_windows()
                            .into_iter()
                            .find(|window| window.running && window.session_id == task_id)
                        {
                            return Err(format!("it is open in window '{}'", window.window_name));
                        }
                        if crate::services::live::heartbeat_alive(&task_id) {
                            return Err("it is running elsewhere".to_string());
                        }
                        claude_session::delete_session(&task_path, &task_id)
                    })
                    .await
                    .unwrap_or_else(|_| Err("the delete task panicked".to_string()));
                    tx.send(AppEvent::SessionDeleted {
                        path,
                        session_id,
                        title,
                        result,
                    });
                });
            }
            ConfirmAction::BringWindow {
                window_id,
                window_name,
                session_id,
                ..
            } => self.bring_window(&window_id, &window_name, &session_id),
            ConfirmAction::ForceResume {
                path,
                session_id,
                yolo,
                custom_button,
                seed,
            } => {
                // Confirmed with the fork warning on screen; the guard is
                // deliberately not consulted again.
                let custom = custom_button.and_then(|index| self.custom_button(index));
                self.start_claude(&path, Some(&session_id), yolo, custom, seed, None);
            }
        }
    }

    /// Ask before pruning, naming every worktree that would go.
    fn action_prune_finished(&mut self) {
        let Some(repo_id) = self.state.selection.repository_id else {
            return;
        };
        let finished = self.finished_worktrees(repo_id);
        if finished.is_empty() {
            return;
        }
        let names: Vec<String> = finished
            .iter()
            .map(|(_, name)| format!("  {}", crate::util::truncate(name, 40)))
            .collect();
        let message = format!(
            "Remove these finished worktrees and their local branches?\n{}\nAny with uncommitted or unpushed work is skipped.",
            names.join("\n")
        );
        self.modals.push(Modal::Confirm(ConfirmModal::new(
            "Prune Finished Worktrees",
            message,
            ConfirmAction::PruneFinished(finished.into_iter().map(|(id, _)| id).collect()),
        )));
    }

    /// Prune `worktree_ids` in one background task. Each removal starts
    /// through `WorktreeSync` like any other, and the task hands every
    /// ticket back in one result, so the user hears about the whole prune
    /// once. A worktree with a change already running is not touched.
    fn spawn_prune(&mut self, worktree_ids: &[Uuid]) {
        let mut jobs = Vec::new();
        let mut busy = Vec::new();
        let mut repo_path = None;
        for &worktree_id in worktree_ids {
            let Some((repo, worktree)) = self.state.find_worktree(worktree_id) else {
                continue;
            };
            let (repo_id, name) = (repo.id, worktree.name.clone());
            let (path, branch) = (worktree.path.clone(), worktree.branch.clone());
            repo_path.get_or_insert_with(|| repo.source_path.clone());
            match self.worktree_sync.begin_removal(repo_id, worktree_id) {
                Some(change) => jobs.push((change, name, path, branch)),
                None => busy.push(name),
            }
        }
        let Some(repo_path) = repo_path.filter(|_| !jobs.is_empty()) else {
            // Nothing to run, but the user still hears why.
            if !busy.is_empty() {
                self.on_worktrees_pruned(Vec::new(), busy);
            }
            return;
        };
        self.notify(
            format!(
                "Pruning {}…",
                Noun::new("worktree", "worktrees").count(jobs.len())
            ),
            Severity::Information,
        );
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut removals = Vec::with_capacity(jobs.len());
            for (change, name, path, branch) in jobs {
                let outcome = crate::prune::prune_worktree(&repo_path, &path, &branch).await;
                removals.push((change, name, outcome));
            }
            tx.send(AppEvent::WorktreesPruned { removals, busy });
        });
    }

    pub(super) fn on_branches(&mut self, repo_id: Uuid, list: BranchList, target: BranchTarget) {
        match target {
            BranchTarget::RefetchOpenModal => {
                if let Some(Modal::StartWorkConfirm(modal)) = self.modals.last_mut() {
                    modal.update_branches(list);
                }
            }
            BranchTarget::StartWorkWorktree => {
                let Some(repo) = self.state.find_repository(repo_id) else {
                    return;
                };
                let configured_base = self.base_branch_for(&repo.source_path);
                // The flow it was loaded for, still open; otherwise the user
                // moved on while the branches loaded.
                if let Some(Modal::StartWork(flow)) = self.modals.last_mut()
                    && flow.repo_id == repo_id
                {
                    flow.set_worktree_form(WorktreeForm::new(
                        repo,
                        list,
                        settings_service::get_forest_path(),
                        self.settings.branch_prefix.clone(),
                        configured_base,
                    ));
                }
            }
            BranchTarget::StartWorkConfirm => {
                let Some(start) = self.pending_start.take() else {
                    return;
                };
                self.open_confirm(repo_id, start, list);
            }
        }
    }

    /// Put Start work's confirm screen up for `start`.
    fn open_confirm(&mut self, repo_id: Uuid, start: PendingStart, list: BranchList) {
        let Some(repo) = self.state.find_repository(repo_id) else {
            return;
        };
        // Opened over Start work's item step. Anything else on top means the
        // user moved on while the branches loaded.
        if !matches!(self.modals.last(), Some(Modal::StartWork(_))) {
            return;
        }
        let layers = self.prompt_layers_for(&repo.source_path);
        let template = crate::prompt::resolve(
            &layers.iter().map(PathBuf::as_path).collect::<Vec<_>>(),
            start.item.template(),
        );
        let mut modal = StartWorkConfirmModal::new(
            repo,
            start.item,
            start.names,
            template.text,
            list,
            settings_service::get_forest_path(),
            self.base_branch_for(&repo.source_path),
        );
        let mut warnings: Vec<String> = template.warning.into_iter().collect();
        if let Some(parent) = start.parent {
            let start_work = self.start_work_for(&repo.source_path);
            let (kind, title) = match &parent.issue {
                Some(issue) => (
                    work::issue_kind(issue, &start_work.feature_labels),
                    issue.title.as_str(),
                ),
                None => (work::IssueKind::Issue, parent.title.as_str()),
            };
            let names = work::issue_names(&start_work, kind, parent.number, title);
            let worktree = work::parent_worktree(&repo.worktrees, parent.number, names);
            let template = crate::prompt::resolve(
                &layers.iter().map(PathBuf::as_path).collect::<Vec<_>>(),
                crate::prompt::Template::SubIssue,
            );
            warnings.extend(template.warning);
            modal.set_parent(parent.number, worktree, template.text);
        }
        self.modals.push(Modal::StartWorkConfirm(Box::new(modal)));
        for warning in warnings {
            self.notify(warning, Severity::Warning);
        }
        self.suggest_session_name();
    }

    pub(super) fn switch_to_window(&mut self, window_id: &str, window_name: &str) {
        if self.select_live_window(window_id, window_name) {
            self.notify(format!("Switched to {window_name}"), Severity::Information);
        }
    }

    /// Select a window the live scan found, saying nothing on success. A
    /// failure is still worth a warning: the switch the user asked for did
    /// not happen.
    fn select_live_window(&mut self, window_id: &str, window_name: &str) -> bool {
        if tmux::select_window(window_id) {
            return true;
        }
        // The window closed between the scan and the click; the next sweep
        // drops its badge.
        self.notify(format!("Window {window_name} is gone"), Severity::Warning);
        self.scan_live_sessions();
        false
    }

    /// Move a session's window over from another tmux session and switch to
    /// it. The dialog was built from a scan, so the window is checked again
    /// first: it must still hold that session, by the `@claude_session_id`
    /// stamp the plugin's heartbeat put on it — the same stamp that lets the
    /// live scan find it here afterwards, like a window forestui opened. It
    /// gets no birth name: forestui did not open it, so title sync and the
    /// back key leave it alone, as they do any hand-made window.
    fn bring_window(&mut self, window_id: &str, window_name: &str, session_id: &str) {
        // Either way the badge is now out of date: it moved here, or it was
        // closed, moved or reused since the scan.
        if tmux::window_session_id(window_id).as_deref() == Some(session_id)
            && tmux::move_window_here(window_id)
            && tmux::select_window(window_id)
        {
            self.notify(format!("Brought {window_name} here"), Severity::Information);
        } else {
            self.notify(
                format!("Could not bring window {window_name} here"),
                Severity::Warning,
            );
        }
        self.scan_live_sessions();
    }

    /// Start work on a picked issue: the confirm screen once the branches
    /// load, or — one work item, one worktree — a jump to the worktree it was
    /// already started in. Checked against the state now, not the marker the
    /// list drew at open.
    ///
    /// Its kind — feature or issue, and so its naming and template — is
    /// settled here, by the feature labels in effect for the repository.
    ///
    /// A sub-issue whose parent can be branched from carries the parent
    /// along. `loaded_parent` is the parent as the flow's list holds it, if
    /// it does; a parent with no worktree that the list lacks is asked of
    /// `gh` while the branches load, for the names its worktree would take.
    fn start_work_on_issue(
        &mut self,
        repo_id: Uuid,
        issue: GitHubIssue,
        loaded_parent: Option<GitHubIssue>,
    ) {
        let link = WorkItemLink::Issue {
            number: issue.number,
        };
        if let Some(worktree) = self.state.linked_worktree(repo_id, &link) {
            let worktree_id = worktree.id;
            let notice = format!(
                "Issue #{} is already in worktree '{}'",
                issue.number, worktree.name
            );
            self.jump_to_worktree(repo_id, worktree_id, Some(notice));
            return;
        }
        let Some(repo) = self.state.find_repository(repo_id) else {
            return;
        };
        let start_work = self.start_work_for(&repo.source_path);
        let kind = work::issue_kind(&issue, &start_work.feature_labels);
        let parent = issue.open_parent().map(|parent| PendingParent {
            number: parent.number,
            title: parent.title.clone(),
            issue: loaded_parent,
        });
        let fetch_parent = parent
            .as_ref()
            .filter(|parent| parent.issue.is_none())
            .map(|parent| parent.number)
            .filter(|&number| {
                let link = WorkItemLink::Issue { number };
                self.state.linked_worktree(repo_id, &link).is_none()
            });
        let path = repo.source_path.clone();
        self.pending_start = Some(PendingStart {
            names: work::issue_names(&start_work, kind, issue.number, &issue.title),
            item: WorkItem::from_issue(&issue, kind),
            parent,
        });
        let Some(number) = fetch_parent else {
            self.load_branches_then(repo_id, BranchTarget::StartWorkConfirm);
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let issue = github::issue(&path, number).await.map(Box::new);
            tx.send(AppEvent::ParentIssueLoaded { number, issue });
            load_branches(tx, path, repo_id, BranchTarget::StartWorkConfirm).await;
        });
    }

    /// Start work on a picked PR: the confirm screen at once — a review has
    /// no base branch, so there is nothing to load first — or a jump to the
    /// worktree that already holds it. That is the one it was started in
    /// (one work item, one worktree), or one with its branch checked out:
    /// git checks a branch out in one worktree only, so `gh pr checkout`
    /// would fail there anyway. A fork's branch names are its own, so a
    /// fork's PR is never matched by one.
    fn start_work_on_pull_request(&mut self, repo_id: Uuid, item: WorkItem) {
        let WorkItem::PullRequest {
            number,
            title,
            head_branch,
            from_fork,
            ..
        } = &item
        else {
            return;
        };
        let (number, link) = (*number, item.link());
        if let Some(worktree) = self.state.linked_worktree(repo_id, &link) {
            let worktree_id = worktree.id;
            let notice = format!("PR #{number} is already in worktree '{}'", worktree.name);
            self.jump_to_worktree(repo_id, worktree_id, Some(notice));
            return;
        }
        if !from_fork && let Some(worktree) = self.state.worktree_on_branch(repo_id, head_branch) {
            let worktree_id = worktree.id;
            let notice = format!(
                "PR #{number}'s branch '{head_branch}' is already checked out in '{}'",
                worktree.name
            );
            self.jump_to_worktree(repo_id, worktree_id, Some(notice));
            return;
        }
        let Some(repo) = self.state.find_repository(repo_id) else {
            return;
        };
        let start_work = self.start_work_for(&repo.source_path);
        let names = work::WorktreeNames {
            branch: head_branch.clone(),
            ..work::pull_request_names(&start_work, number, title)
        };
        let start = PendingStart {
            item,
            names,
            parent: None,
        };
        self.open_confirm(repo_id, start, BranchList::default());
    }

    /// Start work on a session prompt: the confirm screen once the branches
    /// load, named from the prompt's first words until a suggestion lands.
    /// Its link is never looked up — a prompt is not an identity, and the
    /// same words may well mean new work. What can collide is the names,
    /// and those are checked on Launch, once they are final.
    fn start_session(&mut self, repo_id: Uuid, prompt: String, names: work::WorktreeNames) {
        if self.state.find_repository(repo_id).is_none() {
            return;
        }
        self.pending_start = Some(PendingStart {
            item: WorkItem::Session { prompt },
            names,
            parent: None,
        });
        self.load_branches_then(repo_id, BranchTarget::StartWorkConfirm);
    }

    /// Launch a session's confirm screen under the names it shows — typed,
    /// first words or suggested — unless a worktree already has one of them:
    /// then the screen steps aside for the clash question, which can put it
    /// back.
    fn launch_session(&mut self) {
        let Some(Modal::StartWorkConfirm(confirm)) = self.modals.last() else {
            return;
        };
        let Some(repo) = self.state.find_repository(confirm.repo_id) else {
            return;
        };
        let names = confirm.names();
        let clash = work::clash(&names, &repo.worktrees).map(|existing| Clash {
            worktree_id: existing.id,
            worktree_name: existing.name.clone(),
            copy: work::first_free_copy(&names, |n| work::clash(n, &repo.worktrees).is_some()),
        });
        let Some(Modal::StartWorkConfirm(confirm)) = self.modals.pop() else {
            return;
        };
        let Some(clash) = clash else {
            self.apply_modal_result(confirm.launch());
            return;
        };
        match self.modals.last_mut() {
            Some(Modal::StartWork(flow)) => flow.ask_clash(clash, confirm),
            // Always opened over Start work; without it there is no
            // question to ask, so the screen stays.
            _ => self.modals.push(Modal::StartWorkConfirm(confirm)),
        }
    }

    /// The name suggestion a session prompt typed in the checkout at
    /// `path` gets: the `session-name` template, filled with the prompt, under
    /// the repository's naming-call settings. `None` when there is nothing
    /// to ask — the call is turned off, or the prompt is blank.
    pub(super) fn session_name_call(&self, path: &str, prompt: &str) -> Option<NameCall> {
        let naming = self.start_work_for(path).session_naming;
        if !naming.enabled || prompt.trim().is_empty() {
            return None;
        }
        let layers = self.prompt_layers_for(path);
        let template = crate::prompt::resolve(
            &layers.iter().map(PathBuf::as_path).collect::<Vec<_>>(),
            crate::prompt::Template::SessionName,
        );
        let instruction = crate::prompt::fill(
            &template.text,
            &crate::prompt::TemplateVars {
                body: Some(prompt),
                ..Default::default()
            },
        );
        Some(NameCall {
            cwd: PathBuf::from(path),
            instruction,
            naming,
            warning: template.warning,
        })
    }

    /// Ask for a name for the session on the confirm screen on top, if its
    /// settings call for one. The answer comes back as
    /// [`AppEvent::SessionNameSuggested`], keyed to this screen.
    fn suggest_session_name(&mut self) {
        let Some(Modal::StartWorkConfirm(confirm)) = self.modals.last() else {
            return;
        };
        let WorkItem::Session { prompt } = &confirm.item else {
            return;
        };
        let Some(call) = self.session_name_call(&confirm.repo_path, prompt) else {
            return;
        };
        let request = Uuid::new_v4();
        if let Some(Modal::StartWorkConfirm(confirm)) = self.modals.last_mut() {
            confirm.name_request = Some(request);
        }
        if let Some(warning) = call.warning {
            self.notify(warning, Severity::Warning);
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let reply = crate::services::name_suggestion::suggest(
                &call.cwd,
                &call.instruction,
                &call.naming,
            )
            .await;
            tx.send(AppEvent::SessionNameSuggested { request, reply });
        });
    }

    /// Go to a worktree the work was already started in, closing Start work.
    ///
    /// Selecting it loads its sessions and rescans the live map; once both
    /// have answered, [`Self::finish_jump`] switches to a running Claude there.
    /// With none running the worktree is only selected — which session to
    /// resume is the user's call, and relaunching would resend the prompt.
    fn jump_to_worktree(&mut self, repo_id: Uuid, worktree_id: Uuid, notice: Option<String>) {
        let Some((_, worktree)) = self.state.find_worktree(worktree_id) else {
            return;
        };
        let (path, archived) = (worktree.path.clone(), worktree.is_archived);
        self.modals.retain(|m| !matches!(m, Modal::StartWork(_)));
        // Reveal the row, as the create fold does, or the cursor would point
        // at nothing.
        self.collapsed.remove(&repo_id);
        if archived {
            self.state.show_archived = true;
        }
        self.state.select_worktree(repo_id, worktree_id);
        self.rebuild_rows();
        self.sync_sidebar_index();
        self.reload_detail();
        self.pending_jump = Some(PendingJump {
            path,
            sessions: false,
            live: false,
        });
        if let Some(notice) = notice {
            self.notify(notice, Severity::Information);
        }
    }

    /// Complete a jump once the worktree's sessions and the live map have
    /// both come back: switch to the most recent session running in one of
    /// our windows. One running elsewhere cannot be switched to, so it is
    /// only pointed out.
    pub(super) fn finish_jump(&mut self) {
        let Some(jump) = self.pending_jump.take_if(|j| j.sessions && j.live) else {
            return;
        };
        // `reload_detail` already drops a jump on any selection change; this
        // only keeps a late answer from acting on a pane it does not show.
        if jump.path != self.meta.path {
            return;
        }
        let mut window: Option<(chrono::DateTime<chrono::Utc>, String, String)> = None;
        let mut elsewhere: Option<String> = None;
        for session in self.sessions.iter().flatten() {
            match self.live_sessions.get(&session.id).map(|live| &live.place) {
                Some(LivePlace::Window {
                    window_id,
                    window_name,
                }) => {
                    if window
                        .as_ref()
                        .is_none_or(|(newest, _, _)| session.last_timestamp > *newest)
                    {
                        window = Some((
                            session.last_timestamp,
                            window_id.clone(),
                            window_name.clone(),
                        ));
                    }
                }
                Some(LivePlace::Elsewhere { pid }) => {
                    elsewhere.get_or_insert(format!(
                        "Its Claude session is running outside this tmux session (pid {pid})"
                    ));
                }
                // Bringing a window over is the guard's offer, made from the
                // session card; a jump only points at it.
                Some(LivePlace::OtherSession { session_name, .. }) => {
                    elsewhere.get_or_insert(format!(
                        "Its Claude session is running in tmux session '{session_name}' \
                         — resume it from its card to bring it here"
                    ));
                }
                None => {}
            }
        }
        if let Some((_, window_id, window_name)) = window {
            self.switch_to_window(&window_id, &window_name);
        } else if let Some(notice) = elsewhere {
            self.notify(notice, Severity::Information);
        }
    }

    // ------------------------------------------------------------------ actions

    fn load_branches_then(&mut self, repo_id: Uuid, target: BranchTarget) {
        let Some(repo) = self.state.find_repository(repo_id) else {
            return;
        };
        let path = repo.source_path.clone();
        let tx = self.tx.clone();
        tokio::spawn(load_branches(tx, path, repo_id, target));
    }

    /// Open Start work for the selected repository — from its own row or from
    /// one of its worktrees.
    ///
    /// The issues are the list the repository pane loads, for the scope the
    /// settings choose. With a warm cache the picker is ready at once;
    /// otherwise it fills when the fetch folds in, so the mode step never
    /// waits on `gh`. Either way `gh` is asked again: a fresh cache entry
    /// can still miss an issue assigned since, and the answer replaces the
    /// list in place (#119).
    pub(super) fn action_start_work(&mut self) {
        let Some(repo) = self.state.selected_repository() else {
            self.notify("Select a repository first", Severity::Warning);
            return;
        };
        let path = repo.source_path.clone();
        // The settings as last known, re-read so an edit since counts; the
        // probe below corrects them if the repository turns out different.
        self.reread_repository_settings(&path);
        let Some(repo) = self.state.selected_repository() else {
            return;
        };
        let start_work = self.start_work_for(&path);
        let scope = start_work.scope;
        let cached = github::peek_issues(&path, scope);
        let modal = StartWorkModal::new(
            repo,
            cached.map(|(issues, _)| issues),
            start_work,
            self.settings.show_blocked_issues,
            self.settings.issue_filter,
        );
        self.modals.push(Modal::StartWork(Box::new(modal)));
        // Nobody waits on the answer: the modes stay usable until it says
        // otherwise, and the PRs are fetched once it is in.
        self.probe_github(path.clone(), true);
        self.fetch_issues_scoped(path, scope, true);
    }

    /// Open the selected repository's settings chooser: its settings file or
    /// one of its templates. Without a key there is nothing to open; the
    /// control is drawn disabled then, and the pane says why.
    pub(super) fn action_repository_settings(&mut self) {
        if let Some(repo) = self.state.selected_repository()
            && let Some(Some(key)) = self.repo_key(&repo.source_path)
        {
            let picker = PromptTemplatesModal::for_repository(key.clone());
            self.modals.push(Modal::PromptTemplates(picker));
        }
    }

    pub(super) fn action_toggle_archive(&mut self) {
        let Some(worktree_id) = self.state.selection.worktree_id else {
            return;
        };
        let archived = self
            .state
            .find_worktree(worktree_id)
            .map(|(_, w)| w.is_archived)
            .unwrap_or(false);
        self.state.set_archived(worktree_id, !archived);
        self.rebuild_rows();
        self.sync_sidebar_index();
        self.reload_detail();
    }

    /// Mark or unmark the selected worktree finished. Only the mark moves:
    /// the check's last answer stays, so the next sweep finding the same
    /// thing leaves the user's choice alone.
    pub(super) fn action_toggle_finished(&mut self) {
        let Some(worktree_id) = self.state.selection.worktree_id else {
            return;
        };
        let Some(finished) = self
            .state
            .find_worktree(worktree_id)
            .map(|(_, w)| w.finished)
        else {
            return;
        };
        self.state.set_finished(worktree_id, !finished);
        self.rebuild_rows();
    }

    pub(super) fn action_delete(&mut self) {
        if let Some((repo, worktree)) = self.state.selected_worktree() {
            let name = worktree.name.clone();
            let id = worktree.id;
            let kept = if self.base_branch_for(&repo.source_path).is_some() {
                "The default and base branches, or one with unpushed commits, are kept."
            } else {
                "The default branch, or one with unpushed commits, is kept."
            };
            // A removal running already shows on the disabled Delete control;
            // a rename running is invisible, so it gets a word instead of a
            // confirm whose answer would be refused.
            if self.worktree_sync.is_removing(id) {
                return;
            }
            if self.worktree_sync.change_running(id) {
                self.notify(
                    format!("'{name}' is still being renamed"),
                    Severity::Information,
                );
                return;
            }
            // A detached worktree ("HEAD") has no branch of its own to go.
            let message = match worktree.branch.as_str() {
                "" | "HEAD" => format!("Permanently delete '{name}'?"),
                // One line each, capped: the dialog clips rather than wraps.
                branch => format!(
                    "Permanently delete '{}'\nand its local branch '{}'?\n{kept}",
                    crate::util::truncate(&name, 50),
                    crate::util::truncate(branch, 50),
                ),
            };
            self.modals.push(Modal::Confirm(ConfirmModal::new(
                "Delete Worktree",
                message,
                ConfirmAction::DeleteWorktree(id),
            )));
        } else if let Some(repo) = self.state.selected_repository() {
            let name = repo.name.clone();
            let id = repo.id;
            self.modals.push(Modal::Confirm(ConfirmModal::new(
                "Remove Repository",
                format!("Remove '{name}' from forestui?"),
                ConfirmAction::RemoveRepository(id),
            )));
        }
    }

    fn custom_button(&self, index: usize) -> Option<CustomClaudeButton> {
        self.settings.custom_buttons.get(index).cloned()
    }

    fn session_id(&self, index: usize) -> Option<String> {
        self.sessions
            .as_ref()
            .and_then(|s| s.get(index))
            .map(|s| s.id.clone())
    }

    fn session_at(&self, index: usize) -> Option<&crate::models::ClaudeSession> {
        self.sessions.as_ref().and_then(|s| s.get(index))
    }

    /// The duplicate-open guard. A resume aimed at a session that is already
    /// running would start a second Claude on the same transcript — two
    /// conversations diverging from one history, exactly the mess the
    /// liveness tracking exists to prevent. A session in one of our windows
    /// is switched to; one running somewhere unreachable (seen via its
    /// heartbeat) gets an eyes-open "resume anyway". Returns true when the
    /// guard took the action over.
    ///
    /// Only a *running* session guards — the live map holds nothing else. A
    /// window whose Claude exited leaves the session free to resume anywhere,
    /// deliberately: the shell prompt left behind holds nothing.
    pub(super) fn guard_open_session(
        &mut self,
        index: usize,
        path: &str,
        yolo: bool,
        custom_button: Option<usize>,
    ) -> bool {
        let Some(session) = self.session_at(index) else {
            return false;
        };
        let Some(live) = self.live_sessions.get(&session.id) else {
            return false;
        };
        match &live.place {
            // Going to the conversation is what Resume means, so there is
            // nothing to ask and nothing to announce: the switch is the
            // feedback.
            crate::services::live::LivePlace::Window {
                window_id,
                window_name,
            } => {
                // Owned copies end the borrow of the live map, which the
                // select's failure path rescans.
                let (window_id, window_name) = (window_id.clone(), window_name.clone());
                self.select_live_window(&window_id, &window_name);
            }
            crate::services::live::LivePlace::OtherSession {
                window_id,
                window_name,
                session_name,
                last_window,
            } => {
                let closes = if *last_window {
                    format!(
                        "\nIt is that session's only window, so tmux session '{session_name}' will close."
                    )
                } else {
                    String::new()
                };
                let action = ConfirmAction::BringWindow {
                    window_id: window_id.clone(),
                    window_name: window_name.clone(),
                    session_id: session.id.clone(),
                    closes_session: *last_window,
                };
                self.modals.push(Modal::Confirm(ConfirmModal::new(
                    "Session In Another tmux Session",
                    format!(
                        "This session is running in window '{window_name}' of tmux session \
                         '{session_name}'.\nMove that window here and switch to it?{closes}"
                    ),
                    action,
                )));
            }
            crate::services::live::LivePlace::Elsewhere { pid } => {
                let seed = self.session_window_name(index);
                self.modals.push(Modal::Confirm(ConfirmModal::new(
                    "Session Running Elsewhere",
                    format!(
                        "This session is running outside this tmux session (pid {pid}).\n\
                         Resuming here would run two Claudes on one conversation."
                    ),
                    ConfirmAction::ForceResume {
                        path: path.to_string(),
                        session_id: session.id.clone(),
                        yolo,
                        custom_button,
                        seed,
                    },
                )));
            }
        }
        true
    }

    /// The name the user gave this session, if any. Resuming a named session
    /// names its window after the session, so the tab says which conversation
    /// it holds rather than repeating the worktree already selected on screen.
    fn session_window_name(&self, index: usize) -> Option<String> {
        self.sessions
            .as_ref()
            .and_then(|s| s.get(index))
            .and_then(|s| s.custom_title.clone())
    }

    pub fn run_action(&mut self, action: Action) {
        let Some(path) = self.state.selected_path() else {
            return;
        };

        // Opening a window in a directory that is gone silently lands the shell
        // in $HOME and lies about where it is, so refuse instead.
        let needs_directory = matches!(
            action,
            Action::Sync
                | Action::Editor
                | Action::Terminal
                | Action::Files
                | Action::ClaudeNew
                | Action::ClaudeYolo
                | Action::ClaudeCustom(_)
                | Action::ResumeSession(_)
                | Action::ResumeYolo(_)
                | Action::ResumeCustom { .. }
        );
        if needs_directory && !crate::util::path_exists(&path) {
            self.notify(
                format!("Directory no longer exists: {path}"),
                Severity::Error,
            );
            return;
        }

        match action {
            Action::Sync => self.sync(path),
            Action::StartWork => self.action_start_work(),
            Action::RepositorySettings => self.action_repository_settings(),
            Action::Editor => self.open_in_editor(&path),
            Action::Terminal => self.open_in_terminal(&path),
            Action::Files => self.open_in_file_manager(&path),
            Action::ClaudeNew => self.start_claude(&path, None, false, None, None, None),
            Action::ClaudeYolo => self.start_claude(&path, None, true, None, None, None),
            Action::ClaudeCustom(index) => {
                if let Some(button) = self.custom_button(index) {
                    self.start_claude(&path, None, false, Some(button), None, None);
                }
            }
            Action::ResumeSession(index) => {
                if !self.guard_open_session(index, &path, false, None)
                    && let Some(id) = self.session_id(index)
                {
                    let seed = self.session_window_name(index);
                    self.start_claude(&path, Some(&id), false, None, seed, None);
                }
            }
            Action::ResumeYolo(index) => {
                if !self.guard_open_session(index, &path, true, None)
                    && let Some(id) = self.session_id(index)
                {
                    let seed = self.session_window_name(index);
                    self.start_claude(&path, Some(&id), true, None, seed, None);
                }
            }
            Action::ResumeCustom { button, session } => {
                if !self.guard_open_session(session, &path, false, Some(button))
                    && let (Some(id), Some(button)) =
                        (self.session_id(session), self.custom_button(button))
                {
                    let seed = self.session_window_name(session);
                    self.start_claude(&path, Some(&id), false, Some(button), seed, None);
                }
            }
            Action::RenameSession(index) => self.action_rename_session(index, &path),
            Action::TogglePinSession(index) => self.action_toggle_pin(index, &path),
            Action::DeleteSession(index) => self.action_delete_session(index, &path),
            Action::PruneFinished => self.action_prune_finished(),
            Action::RemoveRepository | Action::Delete => self.action_delete(),
            Action::Archive | Action::Unarchive => self.action_toggle_archive(),
            Action::ToggleFinished => self.action_toggle_finished(),
        }
    }

    fn action_rename_session(&mut self, index: usize, path: &str) {
        let Some(session) = self.session_at(index) else {
            return;
        };
        let id = session.id.clone();
        // A session running somewhere unreachable can be renamed by neither
        // mechanism: no window to rename, and its Claude is mid-write on the
        // transcript an append would race. One in another tmux session has a
        // window, but renaming it would rearrange that session behind the
        // user's back — bringing it here is the asked-first way in.
        let live_window = match self.live_sessions.get(&id).map(|live| &live.place) {
            Some(crate::services::live::LivePlace::Elsewhere { .. }) => {
                self.notify(
                    "Session is running elsewhere — rename it there with /rename",
                    Severity::Warning,
                );
                return;
            }
            Some(crate::services::live::LivePlace::OtherSession { session_name, .. }) => {
                self.notify(
                    format!(
                        "Session is running in tmux session '{session_name}' — \
                         rename it there with /rename, or resume it to bring it here"
                    ),
                    Severity::Warning,
                );
                return;
            }
            Some(crate::services::live::LivePlace::Window {
                window_id,
                window_name,
            }) => Some((window_id.clone(), window_name.clone())),
            None => None,
        };
        // The dialog opens on the name the card shows, cursor at the end —
        // the way tmux's own rename prompt continues the existing name. For a
        // live session the window's current name is the truest form (the two
        // are one string by design); a stopped one falls back through the
        // chosen title to whatever the card is displaying, so the field is
        // never empty on a session that visibly has a name.
        let current = live_window
            .as_ref()
            .map(|(_, name)| name.clone())
            .or_else(|| session.custom_title.clone())
            .unwrap_or_else(|| session.title.clone());
        self.modals
            .push(Modal::RenameSession(crate::modal::RenameSessionModal::new(
                path.to_string(),
                id,
                current,
                live_window,
            )));
    }

    fn action_toggle_pin(&mut self, index: usize, path: &str) {
        let Some(id) = self.session_id(index) else {
            return;
        };
        self.state.toggle_pin(path, &id);
        self.sort_visible_sessions();
        self.report_save_error();
    }

    fn action_delete_session(&mut self, index: usize, path: &str) {
        let Some(session) = self.session_at(index) else {
            return;
        };
        // The control is drawn disabled for a live session, but the drawn
        // snapshot can be a sweep older than the map — refuse here too.
        if let Some(live) = self.live_sessions.get(&session.id) {
            let place = match &live.place {
                crate::services::live::LivePlace::Window { window_name, .. } => {
                    format!("open in window '{window_name}'")
                }
                crate::services::live::LivePlace::OtherSession {
                    window_name,
                    session_name,
                    ..
                } => format!("open in window '{window_name}' of tmux session '{session_name}'"),
                crate::services::live::LivePlace::Elsewhere { pid } => {
                    format!("running elsewhere (pid {pid})")
                }
            };
            self.notify(
                format!("Session is {place} — close it first"),
                Severity::Warning,
            );
            return;
        }
        let title = crate::util::truncate(&session.title, 40);
        let msgs = Noun::new("message", "messages").count(session.message_count);
        self.modals.push(Modal::Confirm(ConfirmModal::new(
            "Delete Session",
            format!("Permanently delete '{title}' ({msgs})?\nThere is no undo."),
            ConfirmAction::DeleteClaudeSession {
                path: path.to_string(),
                session_id: session.id.clone(),
                title: session.title.clone(),
            },
        )));
    }

    fn sync(&mut self, path: String) {
        self.notify("Syncing...", Severity::Information);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match git::pull(&path).await {
                Ok(()) => {
                    tx.info("Sync complete");
                    tx.send(AppEvent::ReloadDetail);
                }
                Err(error) => tx.error(format!("Sync failed: {error}")),
            }
        });
    }

    /// Open a global prompt template in the default editor, the same way a
    /// worktree is opened, writing it from its default first so the editor
    /// starts on the text in use. Nothing is read back: the file is the
    /// template, and the next confirm screen reads it afresh.
    fn edit_template(&mut self, template: crate::prompt::Template) {
        match crate::prompt::ensure_file(&settings_service::prompts_dir(), template, &[]) {
            Ok(path) => self.open_in_editor_as(
                &format!("prompts/{}", template.file_name()),
                &path.to_string_lossy(),
            ),
            Err(error) => self.notify(
                format!(
                    "Could not create the {} template: {error}",
                    template.label()
                ),
                Severity::Error,
            ),
        }
    }

    /// The editor a prompt is edited in: the one the shell names, as git
    /// picks it, then forestui's own if it is a terminal editor, then `vi`.
    /// The popup ends when the editor exits, so a GUI editor that returns at
    /// once would close it before the user typed anything.
    fn prompt_editor(&self) -> String {
        ["VISUAL", "EDITOR"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|editor| !editor.trim().is_empty())
            .or_else(|| {
                let own = &self.settings.default_editor;
                tmux::is_tui_editor(own).then(|| own.clone())
            })
            .unwrap_or_else(|| "vi".to_string())
    }

    fn open_in_editor(&mut self, path: &str) {
        let name = self.state.tmux_window_name(path);
        self.open_in_editor_as(&name, path);
    }

    /// Open `path` — a directory or a single file — in the default editor:
    /// a terminal editor in an `edit:<name>` window, anything else detached.
    pub(super) fn open_in_editor_as(&mut self, name: &str, path: &str) {
        let editor = self.settings.default_editor.clone();

        if tmux::is_inside_tmux()
            && tmux::is_tui_editor(&editor)
            && tmux::create_editor_window(name, path, &editor)
        {
            self.notify(
                format!("Opened {editor} in edit:{name}"),
                Severity::Information,
            );
            return;
        }

        // GUI editor, or not inside tmux: spawn it detached. Never from a
        // unit test, which has no tmux and would start a real editor.
        if cfg!(test) {
            return;
        }
        let mut parts = editor.split_whitespace();
        let Some(program) = parts.next() else {
            return;
        };
        let args: Vec<&str> = parts.collect();
        match std::process::Command::new(program)
            .args(&args)
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => self.notify(format!("Opened in {program}"), Severity::Information),
            Err(_) => self.notify(format!("Editor '{editor}' not found"), Severity::Error),
        }
    }

    fn open_in_terminal(&mut self, path: &str) {
        let name = self.state.tmux_window_name(path);
        if tmux::create_shell_window(&name, path) {
            self.notify(
                format!("Opened terminal in term:{name}"),
                Severity::Information,
            );
        } else {
            self.notify("Failed to create terminal window", Severity::Error);
        }
    }

    fn open_in_file_manager(&mut self, path: &str) {
        let name = self.state.tmux_window_name(path);
        if tmux::create_mc_window(&name, path) {
            self.notify(format!("Opened mc in files:{name}"), Severity::Information);
        } else {
            self.notify("Failed to create mc window", Severity::Error);
        }
    }

    pub(super) fn start_claude(
        &mut self,
        path: &str,
        resume_session_id: Option<&str>,
        yolo: bool,
        custom: Option<CustomClaudeButton>,
        seed_name: Option<String>,
        first_prompt: Option<&str>,
    ) {
        let opening_name = tmux::opening_window_name(
            seed_name.as_deref(),
            &self.state.tmux_window_name(path),
            yolo,
            custom.as_ref().map(|b| b.prefix.as_str()),
        );
        let window = tmux::create_claude_window(
            &opening_name,
            path,
            tmux::ClaudeLaunch {
                resume_session_id,
                yolo,
                custom_command: custom.as_ref().map(|b| b.command.as_str()),
                custom_prefix: custom.as_ref().map(|b| b.prefix.as_str()),
                remote_control: self.settings.remote_control
                    && crate::services::claude_cli::remote_control_supported(),
                first_prompt,
            },
        );

        match window {
            Some(window_name) => {
                let verb = if resume_session_id.is_some() {
                    "Resuming"
                } else {
                    "Started"
                };
                let label = match (&custom, yolo) {
                    (Some(button), _) => format!(" ({})", button.label),
                    (None, true) => " (YOLO)".to_string(),
                    (None, false) => String::new(),
                };
                self.notify(
                    format!("{verb} Claude{label} in {window_name}"),
                    Severity::Information,
                );
            }
            None => self.notify("Failed to create Claude window", Severity::Error),
        }
    }

    pub(super) fn submit_rename(&mut self, field: Field) {
        let Some((repo, worktree)) = self.state.selected_worktree() else {
            return;
        };
        let repo_path = repo.source_path.clone();
        let repo_id = repo.id;
        let worktree_id = worktree.id;
        let old_name = worktree.name.clone();
        let old_branch = worktree.branch.clone();
        let old_path = PathBuf::from(&worktree.path);

        match field {
            Field::WorktreeName => {
                let new_name = self.name_input.value().to_string();
                if new_name.is_empty() || new_name == old_name {
                    return;
                }
                let new_path = old_path
                    .parent()
                    .map(|p| p.join(&new_name))
                    .unwrap_or_else(|| PathBuf::from(&new_name));

                // Everything that touches the filesystem — the existence
                // probe, the directory rename, the session-history migration,
                // the git repair — runs off the loop; a slow disk must not
                // freeze the UI mid-keystroke. The result comes back as an
                // event and only the main loop touches state.
                //
                // While it runs, a worktree scan must not act on what a
                // listing says about this entry: after `git worktree repair`
                // the listing shows the new path while the config still holds
                // the old one, which reads as "removed" to the reconcile.
                // Every exit below releases the protection by event.
                let Some(change) = self.worktree_sync.begin_rename(repo_id, worktree_id) else {
                    self.refuse_busy(&old_name);
                    return;
                };
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    if tokio::fs::try_exists(&new_path).await.unwrap_or(false) {
                        tx.error("Path already exists");
                        tx.send(AppEvent::WorktreeRenameAborted { change });
                        return;
                    }
                    if let Err(error) = tokio::fs::rename(&old_path, &new_path).await {
                        tx.error(format!("Rename failed: {error}"));
                        tx.send(AppEvent::WorktreeRenameAborted { change });
                        return;
                    }
                    let (migrate_old, migrate_new) = (old_path.clone(), new_path.clone());
                    let _ = tokio::task::spawn_blocking(move || {
                        claude_session::migrate_sessions(&migrate_old, &migrate_new);
                    })
                    .await;
                    // The directory has moved either way; a failed repair is
                    // reported but must not strand state on the old path.
                    if let Err(error) = git::repair_worktree(&repo_path, &new_path).await {
                        tx.error(format!("Rename failed: {error}"));
                    }
                    tx.send(AppEvent::WorktreeRenamed {
                        change,
                        name: new_name,
                        path: new_path.to_string_lossy().to_string(),
                    });
                });
            }
            Field::BranchName => {
                let new_branch = self.branch_input.value().to_string();
                if new_branch.is_empty() || new_branch == old_branch {
                    return;
                }
                let worktree_path = worktree.path.clone();
                // Reconcile-protected like the directory rename: a listing
                // captured mid-rename must not "correct" the branch either
                // direction while git is still deciding.
                let Some(change) = self.worktree_sync.begin_rename(repo_id, worktree_id) else {
                    self.refuse_busy(&old_name);
                    return;
                };
                let tx = self.tx.clone();
                let requested = new_branch.clone();
                tokio::spawn(async move {
                    match git::rename_branch(&worktree_path, &old_branch, &requested).await {
                        // Folded on success only. Writing the name up front left
                        // the config — and the sidebar — permanently showing a
                        // branch git had refused to create.
                        Ok(()) => tx.send(AppEvent::WorktreeBranchRenamed {
                            change,
                            branch: requested,
                        }),
                        Err(error) => {
                            tx.error(format!("Branch rename failed: {error}"));
                            tx.send(AppEvent::WorktreeRenameAborted { change });
                        }
                    }
                });
            }
        }
    }

    /// A worktree takes one change at a time. A refusal says why: otherwise
    /// Enter in a rename field, or "y" on a delete confirm, would look dead.
    pub(super) fn refuse_busy(&mut self, name: &str) {
        self.notify(
            format!("'{name}' is still being changed; try again when it finishes"),
            Severity::Information,
        );
    }

    fn add_repository(&mut self, path: String) {
        let name = Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let repo = Repository::new(name, path);
        let repo_id = repo.id;
        self.state.add_repository(repo);
        self.state.select_repository(repo_id);
        self.rebuild_rows();
        self.sync_sidebar_index();
        self.reload_detail();
        // Existing worktrees appear via the same reconcile scan that keeps
        // them fresh afterwards — there is no one-shot import to opt into.
        self.scan_worktrees(repo_id);
    }

    #[allow(clippy::too_many_arguments)]
    fn create_worktree(
        &mut self,
        repo_id: Uuid,
        name: String,
        branch: String,
        new_branch: bool,
        base_branch: Option<String>,
        pull_first: bool,
        start: Option<WorkStart>,
    ) {
        let Some(repo) = self.state.find_repository(repo_id) else {
            return;
        };
        let source_path = repo.source_path.clone();
        let worktree_path = settings_service::get_forest_path()
            .join(&repo.name)
            .join(&name);
        // Holds back this repository's scans until the fold: one landing
        // mid-create would adopt the new tree as found outside forestui.
        let create = self.worktree_sync.begin_create(repo_id);
        let tx = self.tx.clone();
        let review = match start.as_ref().map(|start| &start.link) {
            Some(WorkItemLink::PullRequest { number }) => Some(*number),
            _ => None,
        };

        if let Some(number) = review {
            tokio::spawn(async move {
                let branch =
                    match create_review_worktree(&source_path, &worktree_path, number, &branch)
                        .await
                    {
                        Ok(branch) => branch,
                        Err(error) => {
                            tx.error(format!("Failed to create worktree: {error}"));
                            tx.send(AppEvent::WorktreeAddFailed { create });
                            return;
                        }
                    };
                let mut worktree =
                    Worktree::new(name, branch, worktree_path.to_string_lossy().to_string());
                let claude = start.map(|start| {
                    worktree.work_item = Some(start.link);
                    ClaudeStart {
                        first_prompt: start.first_prompt,
                        yolo: start.yolo,
                    }
                });
                tx.send(AppEvent::WorktreeAdded {
                    create,
                    worktree: Box::new(worktree),
                    claude,
                });
            });
            return;
        }

        tokio::spawn(async move {
            if pull_first {
                tx.info("Pulling repo...");
                let _ = git::pull(&source_path).await;
            }

            let mut worktree = match new_worktree(
                &source_path,
                &worktree_path,
                name,
                branch,
                new_branch,
                base_branch,
            )
            .await
            {
                Ok(worktree) => worktree,
                Err(error) => {
                    tx.error(format!("Failed to create worktree: {error}"));
                    tx.send(AppEvent::WorktreeAddFailed { create });
                    return;
                }
            };
            let claude = start.map(|start| {
                worktree.work_item = Some(start.link);
                ClaudeStart {
                    first_prompt: start.first_prompt,
                    yolo: start.yolo,
                }
            });

            tx.send(AppEvent::WorktreeAdded {
                create,
                worktree: Box::new(worktree),
                claude,
            });
        });
    }
}

impl App {
    /// Launch a sub-issue's worktree on a branch from its parent issue's
    /// local branch tip — worktrees share refs, so nothing has to be pushed.
    /// A parent with no worktree gets one first, with no Claude in it; one
    /// that has a worktree is fast-forwarded first when pulling first, and
    /// never changed beyond that. Anything in the way stops the launch
    /// before the sub-issue's worktree is made.
    fn create_sub_issue_worktree(
        &mut self,
        repo_id: Uuid,
        names: work::WorktreeNames,
        parent: ParentLaunch,
        pull_first: bool,
        start: WorkStart,
    ) {
        let Some(repo) = self.state.find_repository(repo_id) else {
            return;
        };
        let source_path = repo.source_path.clone();
        let repo_dir = settings_service::get_forest_path().join(&repo.name);
        let worktree_path = repo_dir.join(&names.worktree);
        let number = parent.number;
        if let ParentSource::Create {
            names: parent_names,
            ..
        } = &parent.source
            && let Some(taken) = work::clash(parent_names, &repo.worktrees)
        {
            let message = format!(
                "Cannot create #{number}'s worktree '{}': '{}' already has its name or branch",
                parent_names.worktree, taken.name
            );
            self.notify(message, Severity::Error);
            return;
        }
        let parent_create = matches!(parent.source, ParentSource::Create { .. })
            .then(|| self.worktree_sync.begin_create(repo_id));
        let create = self.worktree_sync.begin_create(repo_id);
        let tx = self.tx.clone();

        tokio::spawn(async move {
            // Both tickets go back on every exit before the sub-issue's
            // worktree exists.
            let abort = |tx: &EventTx, parent_create, create, error: String| {
                tx.error(error);
                if let Some(create) = parent_create {
                    tx.send(AppEvent::WorktreeAddFailed { create });
                }
                tx.send(AppEvent::WorktreeAddFailed { create });
            };
            match parent.source {
                ParentSource::Create {
                    names: parent_names,
                    base_branch,
                } => {
                    let parent_path = repo_dir.join(&parent_names.worktree);
                    let local = format!("refs/heads/{}", parent_names.branch);
                    if git::get_ref(&source_path, &local).await.is_some() {
                        let error = format!(
                            "Cannot create #{number}'s worktree: branch '{}' already exists",
                            parent_names.branch
                        );
                        return abort(&tx, parent_create, create, error);
                    }
                    if tokio::fs::try_exists(&parent_path).await.unwrap_or(true) {
                        let error = format!(
                            "Cannot create #{number}'s worktree: {} already exists",
                            parent_path.display()
                        );
                        return abort(&tx, parent_create, create, error);
                    }
                    if pull_first {
                        tx.info("Pulling repo...");
                        let _ = git::pull(&source_path).await;
                    }
                    let mut worktree = match new_worktree(
                        &source_path,
                        &parent_path,
                        parent_names.worktree,
                        parent_names.branch,
                        true,
                        base_branch,
                    )
                    .await
                    {
                        Ok(worktree) => worktree,
                        Err(error) => {
                            let error = format!("Failed to create #{number}'s worktree: {error}");
                            return abort(&tx, parent_create, create, error);
                        }
                    };
                    worktree.work_item = Some(WorkItemLink::Issue { number });
                    if let Some(parent_create) = parent_create {
                        tx.send(AppEvent::WorktreeAdded {
                            create: parent_create,
                            worktree: Box::new(worktree),
                            claude: None,
                        });
                    }
                }
                ParentSource::Existing { path } => {
                    if pull_first && git::has_remote_tracking(&path).await.unwrap_or(false) {
                        tx.info(format!("Pulling #{number}'s worktree..."));
                        if let Err(error) = git::pull_fast_forward(&path).await {
                            let error = format!(
                                "Could not fast-forward #{number}'s worktree, so nothing was \
                                 created: {error}"
                            );
                            return abort(&tx, parent_create, create, error);
                        }
                    }
                }
            }

            let mut worktree = match new_worktree(
                &source_path,
                &worktree_path,
                names.worktree,
                names.branch,
                true,
                Some(parent.branch),
            )
            .await
            {
                Ok(worktree) => worktree,
                Err(error) => {
                    tx.error(format!("Failed to create worktree: {error}"));
                    tx.send(AppEvent::WorktreeAddFailed { create });
                    return;
                }
            };
            worktree.work_item = Some(start.link);
            worktree.parent_issue = Some(number);
            tx.send(AppEvent::WorktreeAdded {
                create,
                worktree: Box::new(worktree),
                claude: Some(ClaudeStart {
                    first_prompt: start.first_prompt,
                    yolo: start.yolo,
                }),
            });
        });
    }
}

/// One name suggestion, ready to run: see [`App::session_name_call`].
#[derive(Debug)]
pub(super) struct NameCall {
    pub cwd: PathBuf,
    pub instruction: String,
    pub naming: crate::models::SessionNaming,
    /// Why the `session-name` template in use is not the user's file.
    pub warning: Option<String>,
}
