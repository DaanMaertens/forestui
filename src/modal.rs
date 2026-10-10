//! Modal dialogs: their state, focus model, and key handling.
//!
//! Textual pushed modal *screens* and awaited their result. Here modals live on
//! an explicit stack (`Vec<Modal>`), because the settings flow genuinely nests
//! three deep: Settings → Custom Buttons → Edit Button. A child that closes with
//! a value hands it to its parent through [`Modal::receive_child`].

use crate::event::BranchList;
use crate::fuzzy::{self, BranchEntry, Searchable};
use crate::models::{
    CustomClaudeButton, GitHubIssue, GitHubPullRequest, MAX_BUTTON_LABEL_LENGTH,
    MAX_BUTTON_PREFIX_LENGTH, MAX_CLAUDE_COMMAND_LENGTH, Repository, Settings, WorkItemLink,
    derive_prefix, validate_button_label, validate_button_prefix, validate_claude_command,
};
use crate::picker::SearchPicker;
use crate::prompt;
use crate::ui::widgets::{TextInput, is_plain_press, text_input_claims};
use crate::util;
use crate::work::{self, ItemEntry, WorkItem, WorkMode, WorktreeNames};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

pub const EDITORS: [(&str, &str); 10] = [
    ("VS Code", "code"),
    ("Cursor", "cursor"),
    ("Neovim (tmux)", "nvim"),
    ("Vim (tmux)", "vim"),
    ("Helix (tmux)", "hx"),
    ("Emacs TUI (tmux)", "emacs -nw"),
    ("PyCharm", "pycharm"),
    ("Sublime Text", "subl"),
    ("Nano (tmux)", "nano"),
    ("Micro (tmux)", "micro"),
];

/// What the app must do after a modal handled a key.
#[derive(Debug)]
pub enum ModalOutcome {
    /// Nothing to do; the modal keeps the focus.
    None,
    /// Dismiss the top modal without a result.
    Close,
    /// Open a nested modal.
    Push(Box<Modal>),
    /// Dismiss the top modal and act on its result.
    Submit(ModalResult),
    /// Run a side effect; the modal stays open.
    Effect(ModalEffect),
}

#[derive(Debug, Clone)]
pub enum ModalEffect {
    /// `git fetch` in the given repository, then refresh the branch list.
    Fetch(String),
    /// Install, upgrade, overwrite or remove the Claude Code integration. The
    /// dialog stays open and re-reads its status, so the result of the action
    /// is visible where the action was taken.
    ClaudePlugin(IntegrationAction),
    /// Start work picked an issue in a work mode: load the branches and open
    /// the confirm screen for it on top of the picker.
    ConfirmIssue(Box<GitHubIssue>, WorkMode),
    /// Start work picked a PR to review, its review kind already settled:
    /// open the confirm screen for it on top of the picker.
    ConfirmPullRequest(Box<WorkItem>),
    /// Start work was given a free-text feature: open the confirm screen for
    /// it — asking first if its names are taken, unless this is the answer
    /// to that question (`copy`), which takes the next free suffix instead.
    ConfirmFreeText { description: String, copy: bool },
    /// The clash question's "open existing": go to that worktree.
    OpenWorktree(Uuid),
    /// Open the confirm screen's prompt in an editor; the text comes back as
    /// [`crate::event::AppEvent::PromptEdited`].
    EditPrompt(String),
}

/// What the integration dialog can do, given what is currently on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationAction {
    Install,
    /// An older forestui installed it; upgrading is not overwriting anyone's
    /// work, so it is offered plainly rather than as a warning.
    Upgrade,
    Reinstall,
    /// The shipped files were edited by hand. Only ever reached deliberately.
    Overwrite,
    Uninstall,
    Close,
}

impl IntegrationAction {
    pub fn label(self) -> &'static str {
        match self {
            IntegrationAction::Install => "Install",
            IntegrationAction::Upgrade => "Upgrade",
            IntegrationAction::Reinstall => "Reinstall",
            IntegrationAction::Overwrite => "Overwrite my edits",
            IntegrationAction::Uninstall => "Uninstall",
            IntegrationAction::Close => "Close",
        }
    }

    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            IntegrationAction::Uninstall | IntegrationAction::Overwrite
        )
    }
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeleteWorktree(Uuid),
    /// Second-step confirm: the worktree was seen to hold uncommitted work.
    ForceDeleteWorktree(Uuid),
    RemoveRepository(Uuid),
    /// Permanently delete a Claude session's transcript. No undo — the dialog
    /// says so, and that is the design: a trash can nobody empties is clutter.
    DeleteClaudeSession {
        path: String,
        session_id: String,
        title: String,
    },
    /// The duplicate-open guard's offer: the session is already open in a tmux
    /// window, so jump there instead of starting a second Claude on the same
    /// transcript and letting the two halves diverge.
    SwitchToWindow {
        window_id: String,
        window_name: String,
    },
    /// The guard's other shape: the session is running somewhere forestui
    /// cannot jump to (another tmux session, or no tmux at all — found via
    /// the plugin's heartbeat). Confirming resumes here anyway, eyes open:
    /// two Claudes on one transcript diverge.
    ForceResume {
        path: String,
        session_id: String,
        yolo: bool,
        custom_button: Option<usize>,
        seed: Option<String>,
    },
}

impl ConfirmAction {
    /// Label on the confirm button. Derived from the action so a construction
    /// site cannot pair a discard-your-work action with a bland "Delete"
    /// button.
    pub fn confirm_label(&self) -> &'static str {
        match self {
            ConfirmAction::ForceDeleteWorktree(_) => "Delete anyway",
            ConfirmAction::DeleteWorktree(_)
            | ConfirmAction::RemoveRepository(_)
            | ConfirmAction::DeleteClaudeSession { .. } => "Delete",
            ConfirmAction::SwitchToWindow { .. } => "Switch",
            ConfirmAction::ForceResume { .. } => "Resume anyway",
        }
    }

    /// Whether confirming destroys something. Drives the button's colour: a
    /// "Switch" painted destructive-red would warn against the safe choice,
    /// while "Resume anyway" — forking a running conversation — earns it.
    pub fn is_destructive(&self) -> bool {
        !matches!(self, ConfirmAction::SwitchToWindow { .. })
    }
}

#[derive(Debug, Clone)]
pub enum ModalResult {
    RepositoryAdded {
        path: String,
    },
    WorktreeCreated {
        repo_id: Uuid,
        name: String,
        branch: String,
        new_branch: bool,
    },
    WorktreeForWorkItem {
        repo_id: Uuid,
        /// What the worktree records it was started for.
        link: WorkItemLink,
        name: String,
        branch: String,
        pull_first: bool,
        base_branch: Option<String>,
        /// The prompt as the confirm screen showed it.
        first_prompt: String,
        /// Start Claude with the built-in YOLO flag.
        yolo: bool,
    },
    SettingsSaved(Box<Settings>),
    CustomButtonsSaved(Vec<CustomClaudeButton>),
    CustomButtonSaved(Box<CustomClaudeButton>),
    /// The theme picker committed a slug; the Settings modal underneath
    /// carries it into the settings it will save. `&'static` because every
    /// slug lives in `theme::THEMES` — a non-theme value is unrepresentable.
    ThemeChosen(&'static str),
    Confirmed(ConfirmAction),
    /// The rename-session dialog committed a name. `live_window_id` decides
    /// the mechanism: a live session is renamed through its tmux window (the
    /// sync plugin carries it into the session), a stopped one by appending a
    /// `custom-title` record to its transcript.
    SessionRenamed {
        path: String,
        session_id: String,
        name: String,
        live_window_id: Option<String>,
    },
}

#[derive(Debug)]
pub enum Modal {
    AddRepository(AddRepositoryModal),
    AddWorktree(Box<AddWorktreeModal>),
    StartWork(Box<StartWorkModal>),
    StartWorkConfirm(Box<StartWorkConfirmModal>),
    Settings(Box<SettingsModal>),
    CustomButtons(CustomButtonsModal),
    EditButton(Box<EditButtonModal>),
    ThemePicker(ThemePickerModal),
    ClaudeIntegration(ClaudeIntegrationModal),
    Confirm(ConfirmModal),
    RenameSession(RenameSessionModal),
}

impl Modal {
    pub fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        // Every modal below matches letter shortcuts on `KeyCode::Char` alone,
        // so without this `Ctrl+D` deleted a custom button and `Ctrl+Y`
        // answered a confirmation. Only the combinations a focused text field
        // claims get through — see `widgets::is_plain_press`.
        if matches!(key.code, KeyCode::Char(_)) && !is_plain_press(key) && !text_input_claims(key) {
            return ModalOutcome::None;
        }

        match self {
            Modal::AddRepository(m) => m.handle_key(key),
            Modal::AddWorktree(m) => m.handle_key(key),
            Modal::StartWork(m) => m.handle_key(key),
            Modal::StartWorkConfirm(m) => m.handle_key(key),
            Modal::Settings(m) => m.handle_key(key),
            Modal::CustomButtons(m) => m.handle_key(key),
            Modal::EditButton(m) => m.handle_key(key),
            Modal::ThemePicker(m) => m.handle_key(key),
            Modal::ClaudeIntegration(m) => m.handle_key(key),
            Modal::Confirm(m) => m.handle_key(key),
            Modal::RenameSession(m) => m.handle_key(key),
        }
    }

    /// Deliver a nested modal's result to its parent.
    pub fn receive_child(&mut self, result: &ModalResult) {
        match (self, result) {
            (Modal::Settings(parent), ModalResult::CustomButtonsSaved(buttons)) => {
                parent.custom_buttons = buttons.clone();
            }
            (Modal::Settings(parent), ModalResult::ThemeChosen(slug)) => {
                parent.theme_slug = slug;
            }
            (Modal::CustomButtons(parent), ModalResult::CustomButtonSaved(button)) => {
                parent.apply_edit((**button).clone());
            }
            _ => {}
        }
    }

    /// Move this modal's focus to `index`, as a click on that control does.
    ///
    /// The list-style modals do not have a focus ring, so the index means the
    /// selected row for Custom Buttons and the choice for Confirm.
    pub fn set_focus(&mut self, index: usize) {
        match self {
            Modal::AddRepository(m) => m.focus = index.min(AddRepositoryModal::FIELDS - 1),
            Modal::AddWorktree(m) => m.focus = index.min(m.field_count() - 1),
            Modal::StartWork(m) => m.set_focus(index),
            Modal::StartWorkConfirm(m) => m.set_focus(index),
            Modal::Settings(m) => m.focus = index.min(SettingsModal::FIELDS - 1),
            Modal::EditButton(m) => m.focus = index.min(EditButtonModal::FIELDS - 1),
            Modal::ThemePicker(m) => m.select(index),
            Modal::ClaudeIntegration(m) => m.set_focus(index),
            Modal::CustomButtons(m) => {
                if !m.buttons.is_empty() {
                    m.selected = index.min(m.buttons.len() - 1);
                }
            }
            // Confirm has two choices rather than a ring: 0 Cancel, 1 Delete.
            Modal::Confirm(m) => m.confirm_focused = index == 1,
            Modal::RenameSession(m) => m.focus = index.min(RenameSessionModal::FIELDS - 1),
        }
    }

    /// Select a row in whichever list this modal shows, for a click on it.
    pub fn set_row(&mut self, row: usize) {
        match self {
            Modal::AddWorktree(m) => {
                m.picker.select_row(row);
                // Picking a row commits it, the way Enter does from the
                // keyboard: highlighting alone left Create disabled and a
                // mouse-only user with no way to choose a branch.
                m.pick_selected();
            }
            Modal::CustomButtons(m) if !m.buttons.is_empty() => {
                m.selected = row.min(m.buttons.len() - 1);
            }
            Modal::ThemePicker(m) => m.select(row),
            Modal::StartWork(m) => m.set_row(row),
            _ => {}
        }
    }

    /// Advance any spinner this modal owns. Returns whether something visible
    /// moved, so the caller knows the frame needs repainting.
    pub fn tick(&mut self) -> bool {
        if let Modal::StartWorkConfirm(m) = self
            && m.is_fetching
        {
            m.spinner_index = (m.spinner_index + 1) % SPINNER.len();
            return true;
        }
        false
    }
}

pub const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

fn is_escape(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc
}

/// Cycle a focus index with Tab / Shift+Tab / Up / Down.
fn cycle_focus(focus: &mut usize, len: usize, key: KeyEvent) -> bool {
    if len == 0 {
        return false;
    }
    match key.code {
        KeyCode::Tab | KeyCode::Down => {
            *focus = (*focus + 1) % len;
            true
        }
        KeyCode::BackTab | KeyCode::Up => {
            *focus = if *focus == 0 { len - 1 } else { *focus - 1 };
            true
        }
        _ => false,
    }
}

/// Apply an editing key to a text input. Returns true when the key was consumed.
fn edit_input(input: &mut TextInput, key: KeyEvent) -> bool {
    input.apply_edit_key(key)
}

// ---------------------------------------------------------------- Add repository

#[derive(Debug)]
pub struct AddRepositoryModal {
    pub path: TextInput,
    /// One of the `FOCUS_*` constants; see [`AddRepositoryModal`].
    pub focus: usize,
}

impl Default for AddRepositoryModal {
    fn default() -> Self {
        Self::new()
    }
}

impl AddRepositoryModal {
    /// The focus ring, in the order `ui/modals.rs` draws it. These are the
    /// single definition of the contract between the two files: the renderer
    /// records a click region against the same constant `handle_key` matches on.
    pub const FOCUS_PATH: usize = 0;
    pub const FOCUS_ADD: usize = 1;
    pub const FOCUS_CANCEL: usize = 2;
    pub const FIELDS: usize = Self::FOCUS_CANCEL + 1;

    pub fn new() -> Self {
        Self {
            path: TextInput::new("").with_placeholder("Enter path or paste from clipboard..."),
            focus: Self::FOCUS_PATH,
        }
    }

    /// Validation message shown under the input, and whether the path is usable.
    pub fn status(&self) -> (String, bool) {
        let raw = self.path.value();
        if raw.is_empty() {
            return (String::new(), false);
        }
        let path = util::expanduser(raw);
        if !path.exists() {
            return ("Path does not exist".into(), false);
        }
        if !path.is_dir() {
            return ("Path is not a directory".into(), false);
        }
        if !path.join(".git").exists() {
            return ("Not a git repository".into(), false);
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        (format!("Repository: {name}"), true)
    }

    fn submit(&self) -> ModalOutcome {
        let (_, valid) = self.status();
        if !valid {
            return ModalOutcome::None;
        }
        let path = util::expanduser(self.path.value())
            .to_string_lossy()
            .to_string();
        ModalOutcome::Submit(ModalResult::RepositoryAdded { path })
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }
        // While editing the path, arrows move the cursor rather than the focus.
        if self.focus == Self::FOCUS_PATH && edit_input(&mut self.path, key) {
            return ModalOutcome::None;
        }
        if cycle_focus(&mut self.focus, Self::FIELDS, key) {
            return ModalOutcome::None;
        }
        match (self.focus, key.code) {
            (Self::FOCUS_PATH | Self::FOCUS_ADD, KeyCode::Enter) => self.submit(),
            (Self::FOCUS_CANCEL, KeyCode::Enter) => ModalOutcome::Close,
            _ => ModalOutcome::None,
        }
    }
}

// ------------------------------------------------------------------ Add worktree

#[derive(Debug)]
pub struct AddWorktreeModal {
    pub repo_id: Uuid,
    pub repo_name: String,
    pub forest_dir: PathBuf,
    pub branch_prefix: String,
    pub name: TextInput,
    pub branch: TextInput,
    /// The existing-branch search. Its query is the chosen branch.
    pub picker: SearchPicker<BranchEntry>,
    pub new_branch: bool,
    pub error: String,
    /// One of the `FOCUS_*` constants, or [`AddWorktreeModal::create_index`] /
    /// [`AddWorktreeModal::cancel_index`], whose positions depend on the mode.
    pub focus: usize,
}

impl AddWorktreeModal {
    /// The fixed head of the focus ring. Create and Cancel follow, but their
    /// indices move with the mode — the results list only exists while an
    /// existing branch is being picked — so they are methods, not constants.
    pub const FOCUS_NAME: usize = 0;
    pub const FOCUS_MODE: usize = 1;
    pub const FOCUS_BRANCH: usize = 2;
    pub const FOCUS_RESULTS: usize = 3;

    pub fn new(
        repo: &Repository,
        branches: Vec<String>,
        remotes: Vec<String>,
        forest_dir: PathBuf,
        branch_prefix: String,
    ) -> Self {
        let entries = branches
            .iter()
            .map(|b| BranchEntry::new(b, &remotes))
            .collect();
        Self {
            repo_id: repo.id,
            repo_name: repo.name.clone(),
            forest_dir,
            branch_prefix: branch_prefix.clone(),
            name: TextInput::new("").with_placeholder("my-feature"),
            branch: TextInput::new("").with_placeholder(format!("{branch_prefix}my-feature")),
            picker: SearchPicker::new(entries, "Start typing to search branches..."),
            new_branch: true,
            error: String::new(),
            focus: Self::FOCUS_NAME,
        }
    }

    pub fn field_count(&self) -> usize {
        // The fixed head, then Create and Cancel; the results list sits between
        // them only while an existing branch is being picked.
        let head = Self::FOCUS_BRANCH + 1;
        head + usize::from(!self.new_branch) + 2
    }

    pub fn create_index(&self) -> usize {
        self.field_count() - 2
    }

    pub fn cancel_index(&self) -> usize {
        self.field_count() - 1
    }

    /// Index of the results list, which only exists in existing-branch mode.
    fn results_index(&self) -> Option<usize> {
        if self.new_branch {
            None
        } else {
            Some(Self::FOCUS_RESULTS)
        }
    }

    /// Commit the highlighted branch into the query, which is what
    /// `selected_branch` and `can_create` read.
    fn pick_selected(&mut self) {
        if let Some(branch) = self.picker.selected().map(|b| b.search_text().to_string()) {
            self.picker.set_query(&branch);
        }
    }

    /// Sanitised worktree name: alphanumerics, `-` and `_` only.
    pub fn sanitized_name(&self) -> String {
        self.name
            .value()
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .collect()
    }

    pub fn path_preview(&self) -> Option<PathBuf> {
        let name = self.sanitized_name();
        if name.is_empty() {
            return None;
        }
        Some(self.forest_dir.join(&self.repo_name).join(name))
    }

    pub fn selected_branch(&self) -> String {
        if self.new_branch {
            self.branch.value().to_string()
        } else {
            self.picker.query().value().to_string()
        }
    }

    /// The Create control is disabled when an existing branch is not real.
    pub fn can_create(&self) -> bool {
        if self.new_branch {
            return true;
        }
        self.branch_exists(&self.selected_branch())
    }

    fn branch_exists(&self, name: &str) -> bool {
        self.picker.entries().any(|b| b.search_text() == name)
    }

    fn set_mode(&mut self, new_branch: bool) {
        self.new_branch = new_branch;
        self.focus = self.focus.min(self.field_count() - 1);
        self.error.clear();
    }

    fn submit(&mut self) -> ModalOutcome {
        let name = self.sanitized_name();
        if name.is_empty() {
            self.error = "Worktree name is required".into();
            return ModalOutcome::None;
        }
        let branch = self.selected_branch();
        if branch.is_empty() {
            self.error = "Branch name is required".into();
            return ModalOutcome::None;
        }
        if !self.new_branch && !self.branch_exists(&branch) {
            self.error = format!("Branch '{branch}' does not exist");
            return ModalOutcome::None;
        }
        if let Some(path) = self.path_preview()
            && path.exists()
        {
            self.error = "Worktree path already exists".into();
            return ModalOutcome::None;
        }
        ModalOutcome::Submit(ModalResult::WorktreeCreated {
            repo_id: self.repo_id,
            name,
            branch,
            new_branch: self.new_branch,
        })
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }

        // Text editing takes precedence while a field has focus.
        if self.focus == Self::FOCUS_NAME && edit_input(&mut self.name, key) {
            self.error.clear();
            // A new branch name tracks the worktree name.
            if self.new_branch {
                self.branch
                    .set_value(format!("{}{}", self.branch_prefix, self.sanitized_name()));
            }
            return ModalOutcome::None;
        }
        if self.focus == Self::FOCUS_BRANCH {
            let used = if self.new_branch {
                edit_input(&mut self.branch, key)
            } else {
                self.picker.edit_query(key)
            };
            if used {
                self.error.clear();
                return ModalOutcome::None;
            }
        }

        // The results list owns Up/Down and Enter when it has focus.
        if self.results_index() == Some(self.focus) {
            if self.picker.move_selection(key) {
                return ModalOutcome::None;
            }
            if key.code == KeyCode::Enter {
                self.pick_selected();
                return ModalOutcome::None;
            }
        }

        if self.focus == Self::FOCUS_MODE {
            match key.code {
                KeyCode::Left => {
                    self.set_mode(true);
                    return ModalOutcome::None;
                }
                KeyCode::Right => {
                    self.set_mode(false);
                    return ModalOutcome::None;
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let next = !self.new_branch;
                    self.set_mode(next);
                    return ModalOutcome::None;
                }
                _ => {}
            }
        }

        let field_count = self.field_count();
        if cycle_focus(&mut self.focus, field_count, key) {
            return ModalOutcome::None;
        }

        if key.code == KeyCode::Enter {
            if self.focus == self.cancel_index() {
                return ModalOutcome::Close;
            }
            if self.focus == self.create_index()
                || self.focus == Self::FOCUS_NAME
                || self.focus == Self::FOCUS_BRANCH
            {
                if !self.can_create() {
                    self.error = format!("Branch '{}' does not exist", self.selected_branch());
                    return ModalOutcome::None;
                }
                return self.submit();
            }
        }
        ModalOutcome::None
    }
}

// -------------------------------------------------------------------- Start work

/// Which step of Start work is on screen. The confirm screen is a modal of
/// its own, pushed on top, so Esc there lands back on the step before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartWorkStep {
    Mode,
    Item,
    /// Typing a free-text feature's description.
    Describe,
    /// The description's names are taken: open that worktree, make a
    /// numbered copy, or go back and edit.
    Clash,
}

/// The worktree a free-text feature's names collided with.
#[derive(Debug, Clone)]
pub struct Clash {
    pub worktree_id: Uuid,
    pub worktree_name: String,
    /// The worktree name a copy would take, so the dialog can say what it makes.
    pub copy_name: String,
}

/// Start work's steps before the confirm screen: choose a work mode, then a
/// work item — or describe a free-text feature.
#[derive(Debug)]
pub struct StartWorkModal {
    pub repo_id: Uuid,
    pub repo_name: String,
    /// The repository's checkout: the issues are the ones loaded for it.
    pub repo_path: String,
    pub step: StartWorkStep,
    /// The highlighted mode on the mode step, and the chosen one after it.
    pub mode: WorkMode,
    /// Whether `gh` resolves a GitHub repository for the checkout. `None`
    /// until the probe answers, and treated as yes meanwhile: nearly every
    /// repository has one, and greying its modes out for the length of a
    /// `gh` call would flash on every open.
    pub github: Option<bool>,
    /// Every loaded issue; the picker holds the ones `mode` lists.
    issues: Option<Vec<GitHubIssue>>,
    /// The PRs `pr-review` lists, once `gh` has answered.
    pull_requests: Option<Vec<GitHubPullRequest>>,
    /// The login `gh` is authenticated as, which a PR's author is compared
    /// with to settle its review kind.
    viewer: Option<String>,
    /// The rows `mode` lists. `None` while its items are still loading.
    pub picker: Option<SearchPicker<ItemEntry>>,
    /// A free-text feature's description, kept across Back so an edit after
    /// a clash starts from what was typed.
    pub description: TextInput,
    /// Set on the clash step.
    pub clash: Option<Clash>,
    /// Work item → the worktree already started for it, as the flow opened.
    /// Only marks the list: picking an item checks the live state.
    linked: HashMap<WorkItemLink, String>,
    /// One of the `FOCUS_*` constants; see [`StartWorkModal::ring`].
    pub focus: usize,
}

impl StartWorkModal {
    /// The mode list on the mode step; the query and its results on the item
    /// step, which share one focus stop so typing and Up/Down both work
    /// without tabbing between them; the description on the describe step.
    pub const FOCUS_LIST: usize = 0;
    /// Only on the item and describe steps.
    pub const FOCUS_BACK: usize = 1;
    pub const FOCUS_CANCEL: usize = 2;
    /// The clash step's three answers.
    pub const FOCUS_OPEN: usize = 3;
    pub const FOCUS_COPY: usize = 4;
    pub const FOCUS_EDIT: usize = 5;

    pub fn new(repo: &Repository, issues: Option<Vec<GitHubIssue>>) -> Self {
        let mut modal = Self {
            repo_id: repo.id,
            repo_name: repo.name.clone(),
            repo_path: repo.source_path.clone(),
            step: StartWorkStep::Mode,
            mode: WorkMode::Issue,
            github: None,
            issues: None,
            pull_requests: None,
            viewer: None,
            picker: None,
            description: TextInput::new("").with_placeholder("Describe the feature..."),
            clash: None,
            linked: repo
                .worktrees
                .iter()
                .filter_map(|w| Some((w.work_item.clone()?, w.name.clone())))
                .collect(),
            focus: Self::FOCUS_LIST,
        };
        if let Some(issues) = issues {
            modal.set_issues(issues);
        }
        modal
    }

    pub fn has_github(&self) -> bool {
        self.github != Some(false)
    }

    /// Whether `mode` can be picked here: the repository is on GitHub, or
    /// the mode needs nothing from it — only `feature` does, through its
    /// free-text entry.
    pub fn mode_enabled(&self, mode: WorkMode) -> bool {
        self.has_github() || mode == WorkMode::Feature
    }

    /// Take the probe's answer. A repository found not to be on GitHub moves
    /// off any mode it can no longer offer — even mid-list, if the answer
    /// was slower than the user — and drops the issues from its list.
    pub fn set_github(&mut self, is_github: bool) {
        self.github = Some(is_github);
        // A yes is what the flow assumed all along: nothing on screen
        // changes, and rebuilding the list would drop a half-typed query.
        if is_github {
            return;
        }
        if !self.mode_enabled(self.mode) {
            self.mode = WorkMode::Feature;
        }
        self.picker = None;
        self.fill_picker();
    }

    /// Where Esc and Back go from the describe step: the list the free-text
    /// entry was picked from, or the modes when there was no list to show.
    fn before_describe(&self) -> StartWorkStep {
        if self.has_github() {
            StartWorkStep::Item
        } else {
            StartWorkStep::Mode
        }
    }

    /// Ask what to do about a taken slug.
    pub fn ask_clash(&mut self, clash: Clash) {
        self.clash = Some(clash);
        self.go_to(StartWorkStep::Clash);
        self.focus = Self::FOCUS_OPEN;
    }

    /// The description as it would be recorded: trimmed, so stray spaces do
    /// not make two links for the same words.
    fn typed_description(&self) -> String {
        self.description.value().trim().to_string()
    }

    /// Fill the item list once the issues have loaded. Issues already loaded
    /// are kept: replacing the list under a half-typed query would move the
    /// selection out from under the user.
    pub fn set_issues(&mut self, issues: Vec<GitHubIssue>) {
        if self.issues.is_none() {
            self.issues = Some(issues);
            self.fill_picker();
        }
    }

    /// Fill `pr-review`'s list once the PRs have loaded; kept, like the
    /// issues, once there.
    pub fn set_pull_requests(&mut self, prs: Vec<GitHubPullRequest>, viewer: Option<String>) {
        if self.pull_requests.is_none() {
            self.pull_requests = Some(prs);
            self.viewer = viewer;
            if self.mode == WorkMode::PrReview {
                self.fill_picker();
            }
        }
    }

    /// Choose a mode, listing its items. Re-choosing the current one keeps
    /// the list, query and all.
    fn set_mode(&mut self, mode: WorkMode) {
        if mode != self.mode {
            self.mode = mode;
            self.picker = None;
            self.fill_picker();
        }
    }

    fn fill_picker(&mut self) {
        let entries = if self.mode == WorkMode::PrReview {
            let Some(prs) = self.pull_requests.as_deref() else {
                return;
            };
            work::pull_request_entries(prs, |number| {
                self.linked
                    .get(&WorkItemLink::PullRequest { number })
                    .cloned()
            })
        } else {
            // Without GitHub there are no issues to wait for: the list is
            // the free-text entry alone.
            let issues = match (self.has_github(), self.issues.as_deref()) {
                (false, _) => &[][..],
                (true, Some(issues)) => issues,
                (true, None) => return,
            };
            work::item_entries(self.mode, issues, work::DEFAULT_FEATURE_LABELS, |number| {
                self.linked.get(&WorkItemLink::Issue { number }).cloned()
            })
        };
        self.picker = Some(SearchPicker::new(
            entries,
            "Type a number or part of a title...",
        ));
    }

    /// The focus stops on the current step, in Tab order.
    pub fn ring(&self) -> &'static [usize] {
        match self.step {
            StartWorkStep::Mode => &[Self::FOCUS_LIST, Self::FOCUS_CANCEL],
            StartWorkStep::Item | StartWorkStep::Describe => {
                &[Self::FOCUS_LIST, Self::FOCUS_BACK, Self::FOCUS_CANCEL]
            }
            StartWorkStep::Clash => &[Self::FOCUS_OPEN, Self::FOCUS_COPY, Self::FOCUS_EDIT],
        }
    }

    pub fn set_focus(&mut self, index: usize) {
        if self.ring().contains(&index) {
            self.focus = index;
        }
    }

    /// Select a clicked row: a mode on the mode step, an item on the item step.
    pub fn set_row(&mut self, row: usize) {
        match self.step {
            StartWorkStep::Mode => {
                if let Some(mode) = WorkMode::ALL.get(row).copied()
                    && self.mode_enabled(mode)
                {
                    self.set_mode(mode);
                }
            }
            StartWorkStep::Item => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.select_row(row);
                }
            }
            StartWorkStep::Describe | StartWorkStep::Clash => {}
        }
    }

    fn go_to(&mut self, step: StartWorkStep) {
        self.step = step;
        self.focus = Self::FOCUS_LIST;
    }

    /// Move the mode highlight, skipping the modes that cannot be picked.
    fn step_mode(&mut self, delta: isize) {
        let modes = WorkMode::ALL;
        let Some(mut position) = modes.iter().position(|m| *m == self.mode) else {
            return;
        };
        loop {
            let next = position as isize + delta;
            if next < 0 || next as usize >= modes.len() {
                return;
            }
            position = next as usize;
            if self.mode_enabled(modes[position]) {
                self.set_mode(modes[position]);
                return;
            }
        }
    }

    fn cycle(&mut self, key: KeyEvent) -> bool {
        let ring = self.ring();
        let Some(mut position) = ring.iter().position(|f| *f == self.focus) else {
            self.focus = Self::FOCUS_LIST;
            return true;
        };
        if !cycle_focus(&mut position, ring.len(), key) {
            return false;
        }
        self.focus = ring[position];
        true
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        // Esc steps back rather than throwing the whole flow away; on the
        // first step there is nothing behind it, so it closes.
        if is_escape(key) {
            return match self.step {
                StartWorkStep::Mode => ModalOutcome::Close,
                StartWorkStep::Item | StartWorkStep::Describe | StartWorkStep::Clash => {
                    self.go_back();
                    ModalOutcome::None
                }
            };
        }

        if self.focus == Self::FOCUS_LIST {
            match self.step {
                StartWorkStep::Mode => match key.code {
                    KeyCode::Up => {
                        self.step_mode(-1);
                        return ModalOutcome::None;
                    }
                    KeyCode::Down => {
                        self.step_mode(1);
                        return ModalOutcome::None;
                    }
                    KeyCode::Enter if self.mode_enabled(self.mode) => {
                        // Without GitHub the list would hold the free-text
                        // entry and nothing else; picking it is a step
                        // with nothing to decide.
                        let next = if self.has_github() {
                            StartWorkStep::Item
                        } else {
                            StartWorkStep::Describe
                        };
                        self.go_to(next);
                        return ModalOutcome::None;
                    }
                    _ => {}
                },
                StartWorkStep::Item => {
                    if let Some(picker) = self.picker.as_mut() {
                        if picker.edit_query(key) || picker.move_selection(key) {
                            return ModalOutcome::None;
                        }
                        if key.code == KeyCode::Enter {
                            return match picker.selected() {
                                Some(ItemEntry::Issue(entry)) => {
                                    ModalOutcome::Effect(ModalEffect::ConfirmIssue(
                                        Box::new(entry.issue.clone()),
                                        self.mode,
                                    ))
                                }
                                Some(ItemEntry::PullRequest(entry)) => {
                                    let item = WorkItem::from_pull_request(
                                        &entry.pr,
                                        self.viewer.as_deref(),
                                    );
                                    ModalOutcome::Effect(ModalEffect::ConfirmPullRequest(Box::new(
                                        item,
                                    )))
                                }
                                Some(ItemEntry::FreeText) => {
                                    self.go_to(StartWorkStep::Describe);
                                    ModalOutcome::None
                                }
                                None => ModalOutcome::None,
                            };
                        }
                    }
                }
                StartWorkStep::Describe => {
                    if edit_input(&mut self.description, key) {
                        return ModalOutcome::None;
                    }
                    if key.code == KeyCode::Enter {
                        // Words with nothing to name a worktree after go
                        // nowhere; the field stays up to say so by not moving.
                        let description = self.typed_description();
                        if work::free_text_names(&description).is_none() {
                            return ModalOutcome::None;
                        }
                        return ModalOutcome::Effect(ModalEffect::ConfirmFreeText {
                            description,
                            copy: false,
                        });
                    }
                }
                StartWorkStep::Clash => {}
            }
        }

        if self.cycle(key) {
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            (Self::FOCUS_BACK, KeyCode::Enter) | (Self::FOCUS_EDIT, KeyCode::Enter) => {
                self.go_back();
                ModalOutcome::None
            }
            (Self::FOCUS_CANCEL, KeyCode::Enter) => ModalOutcome::Close,
            (Self::FOCUS_OPEN, KeyCode::Enter) => match &self.clash {
                Some(clash) => ModalOutcome::Effect(ModalEffect::OpenWorktree(clash.worktree_id)),
                None => ModalOutcome::None,
            },
            (Self::FOCUS_COPY, KeyCode::Enter) => {
                ModalOutcome::Effect(ModalEffect::ConfirmFreeText {
                    description: self.typed_description(),
                    copy: true,
                })
            }
            _ => ModalOutcome::None,
        }
    }

    /// One step back: the clash question back to the description it was
    /// asked about, the description back to where it was picked from, the
    /// list back to the modes.
    fn go_back(&mut self) {
        let previous = match self.step {
            StartWorkStep::Mode | StartWorkStep::Item => StartWorkStep::Mode,
            StartWorkStep::Describe => self.before_describe(),
            StartWorkStep::Clash => StartWorkStep::Describe,
        };
        self.clash = None;
        self.go_to(previous);
    }
}

/// Start work's confirm screen: the worktree a work item will start in, the
/// base it branches from, and the prompt Claude will be started with.
///
/// Also opened directly by an issue card's Start work, with nothing beneath it.
#[derive(Debug)]
pub struct StartWorkConfirmModal {
    pub repo_id: Uuid,
    pub repo_name: String,
    pub repo_path: String,
    /// Decides the names, and for an issue, the prompt template.
    pub mode: WorkMode,
    /// The issue, PR or free-text feature the worktree is for.
    pub item: WorkItem,
    pub branches: BranchList,
    pub forest_dir: PathBuf,
    pub name: TextInput,
    pub branch: TextInput,
    pub base_branch: TextInput,
    pub pull_first: bool,
    /// Launch with the built-in YOLO flag. Off on every open: trusting one
    /// task must not become the default for the next.
    pub yolo: bool,
    /// The prompt as an editor left it, in place of the filled-in template.
    /// Held here and nowhere else, so the edit applies to this launch only.
    pub edited_prompt: Option<String>,
    /// An editor is open on the prompt; a second one would race the first.
    pub is_editing: bool,
    pub is_fetching: bool,
    pub spinner_index: usize,
    /// Opened from Start work's item step, which is still underneath: the
    /// dismiss button goes back to it rather than cancelling.
    pub has_parent: bool,
    /// One of the `FOCUS_*` constants; see [`StartWorkConfirmModal`].
    pub focus: usize,
}

impl StartWorkConfirmModal {
    /// The focus ring, in the order `ui/modals.rs` draws it.
    pub const FOCUS_NAME: usize = 0;
    pub const FOCUS_BRANCH: usize = 1;
    pub const FOCUS_BASE: usize = 2;
    pub const FOCUS_FETCH: usize = 3;
    pub const FOCUS_PULL: usize = 4;
    pub const FOCUS_EDIT: usize = 5;
    pub const FOCUS_YOLO: usize = 6;
    pub const FOCUS_LAUNCH: usize = 7;
    pub const FOCUS_CANCEL: usize = 8;

    /// The focus stops, in Tab order. A PR review has no branch to name and
    /// no base to branch from — `gh pr checkout` decides both — so its
    /// screen draws none of those controls and the ring skips them.
    pub fn ring(&self) -> &'static [usize] {
        if self.is_review() {
            &[
                Self::FOCUS_NAME,
                Self::FOCUS_EDIT,
                Self::FOCUS_YOLO,
                Self::FOCUS_LAUNCH,
                Self::FOCUS_CANCEL,
            ]
        } else {
            &[
                Self::FOCUS_NAME,
                Self::FOCUS_BRANCH,
                Self::FOCUS_BASE,
                Self::FOCUS_FETCH,
                Self::FOCUS_PULL,
                Self::FOCUS_EDIT,
                Self::FOCUS_YOLO,
                Self::FOCUS_LAUNCH,
                Self::FOCUS_CANCEL,
            ]
        }
    }

    /// Whether this is a PR review, which checks out the PR's own branch.
    pub fn is_review(&self) -> bool {
        matches!(self.item, WorkItem::PullRequest { .. })
    }

    /// Focus a clicked control, if this screen has it.
    pub fn set_focus(&mut self, index: usize) {
        if self.ring().contains(&index) {
            self.focus = index;
        }
    }

    /// `names` are passed in rather than derived from `item`: a free-text
    /// feature's may carry a suffix the clash question chose.
    pub fn new(
        repo: &Repository,
        mode: WorkMode,
        item: WorkItem,
        names: WorktreeNames,
        branches: BranchList,
        forest_dir: PathBuf,
        has_parent: bool,
    ) -> Self {
        let base = default_base_branch(&branches);
        Self {
            repo_id: repo.id,
            repo_name: repo.name.clone(),
            repo_path: repo.source_path.clone(),
            mode,
            item,
            branches,
            forest_dir,
            name: TextInput::new(names.worktree).with_placeholder("worktree-name"),
            branch: TextInput::new(names.branch).with_placeholder("branch-name"),
            base_branch: TextInput::new(base).with_placeholder("origin/main"),
            pull_first: true,
            yolo: false,
            edited_prompt: None,
            is_editing: false,
            is_fetching: false,
            spinner_index: 0,
            has_parent,
            focus: Self::FOCUS_NAME,
        }
    }

    pub fn path_preview(&self) -> PathBuf {
        self.forest_dir
            .join(&self.repo_name)
            .join(self.name.value())
    }

    /// The first prompt: an edit if there is one, else the template filled in
    /// from the fields as they stand now — so what the screen shows is
    /// exactly what Claude is started with.
    ///
    /// An edit is taken as final; renaming the worktree afterwards does not
    /// rewrite text the user wrote by hand.
    pub fn prompt(&self) -> String {
        if let Some(edited) = &self.edited_prompt {
            return edited.clone();
        }
        // A free-text feature has no number and no title of its own; its
        // description stands in for the title and is the only `{body}`.
        let (number, title, body) = match &self.item {
            WorkItem::Issue { number, title } | WorkItem::PullRequest { number, title, .. } => {
                (Some(*number), title.as_str(), None)
            }
            WorkItem::FreeText { description } => {
                (None, description.as_str(), Some(description.as_str()))
            }
        };
        prompt::fill(
            self.item.template(self.mode),
            &prompt::TemplateVars {
                number,
                title,
                branch: self.branch.value(),
                worktree: self.name.value(),
                repo: &self.repo_name,
                body,
            },
        )
    }

    /// The inline suggestion shown after the typed base branch.
    pub fn base_suggestion(&self) -> Option<String> {
        let value = self.base_branch.value();
        if value.is_empty() {
            return None;
        }
        fuzzy::fuzzy_match_branches(value, &self.branches.branches, &self.branches.remotes, 1)
            .into_iter()
            .next()
            .map(|(b, _)| b)
    }

    /// Mid-edit counts as not ready: launching then would start Claude on
    /// the prompt the editor is about to replace.
    pub fn can_launch(&self) -> bool {
        if self.is_editing || self.name.is_empty() {
            return false;
        }
        if self.is_review() {
            return true;
        }
        let base = self.base_branch.value();
        !self.branch.is_empty()
            && (base.is_empty() || self.branches.branches.iter().any(|b| b == base))
    }

    /// Apply a refreshed branch list after a fetch.
    pub fn update_branches(&mut self, branches: BranchList) {
        self.is_fetching = false;
        self.branches = branches;
        let base = self.base_branch.value().to_string();
        if base.is_empty() || !self.branches.branches.contains(&base) {
            self.base_branch
                .set_value(default_base_branch(&self.branches));
        }
    }

    pub fn fetch_failed(&mut self) {
        self.is_fetching = false;
    }

    /// Take what the editor left in the prompt file. Returns false for a file
    /// emptied out, which reads as "never mind" — the way git treats an empty
    /// commit message — rather than as a launch with no prompt at all.
    pub fn finish_edit(&mut self, text: &str) -> bool {
        self.is_editing = false;
        // Editors end a file with a newline the prompt never had.
        let text = text.trim_end();
        if text.trim().is_empty() {
            return false;
        }
        // Saved untouched: the template keeps following the name fields
        // instead of freezing at the names it was opened with.
        if text != self.prompt().trim_end() {
            self.edited_prompt = Some(text.to_string());
        }
        true
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }

        let editable = match self.focus {
            Self::FOCUS_NAME => Some(&mut self.name),
            Self::FOCUS_BRANCH => Some(&mut self.branch),
            Self::FOCUS_BASE => Some(&mut self.base_branch),
            _ => None,
        };
        if let Some(input) = editable
            && edit_input(input, key)
        {
            return ModalOutcome::None;
        }

        if self.focus == Self::FOCUS_BASE
            && key.code == KeyCode::Right
            && let Some(suggestion) = self.base_suggestion()
        {
            self.base_branch.set_value(suggestion);
            return ModalOutcome::None;
        }

        let ring = self.ring();
        if let Some(mut position) = ring.iter().position(|f| *f == self.focus)
            && cycle_focus(&mut position, ring.len(), key)
        {
            self.focus = ring[position];
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            (Self::FOCUS_FETCH, KeyCode::Enter) => {
                if self.is_fetching {
                    return ModalOutcome::None;
                }
                self.is_fetching = true;
                self.spinner_index = 0;
                ModalOutcome::Effect(ModalEffect::Fetch(self.repo_path.clone()))
            }
            (Self::FOCUS_PULL, KeyCode::Enter) | (Self::FOCUS_PULL, KeyCode::Char(' ')) => {
                self.pull_first = !self.pull_first;
                ModalOutcome::None
            }
            (Self::FOCUS_EDIT, KeyCode::Enter) => {
                if self.is_editing {
                    return ModalOutcome::None;
                }
                self.is_editing = true;
                ModalOutcome::Effect(ModalEffect::EditPrompt(self.prompt()))
            }
            (Self::FOCUS_YOLO, KeyCode::Enter) | (Self::FOCUS_YOLO, KeyCode::Char(' ')) => {
                self.yolo = !self.yolo;
                ModalOutcome::None
            }
            (Self::FOCUS_CANCEL, KeyCode::Enter) => ModalOutcome::Close,
            (_, KeyCode::Enter) => {
                if !self.can_launch() {
                    return ModalOutcome::None;
                }
                // A review branches from nothing: `gh pr checkout` fetches
                // the PR's head itself.
                let base = self.base_branch.value().to_string();
                let review = self.is_review();
                ModalOutcome::Submit(ModalResult::WorktreeForWorkItem {
                    repo_id: self.repo_id,
                    link: self.item.link(),
                    name: self.name.value().to_string(),
                    branch: self.branch.value().to_string(),
                    pull_first: self.pull_first && !review,
                    base_branch: (!base.is_empty() && !review).then_some(base),
                    first_prompt: self.prompt(),
                    yolo: self.yolo,
                })
            }
            _ => ModalOutcome::None,
        }
    }
}

/// What new work branches from: the remote's default branch when it records
/// one, then `<remote>/<current>`, then the local current branch, then the
/// first branch in the list.
pub fn default_base_branch(list: &BranchList) -> String {
    if let Some(remote_default) = &list.remote_default
        && list.branches.contains(remote_default)
    {
        return remote_default.clone();
    }
    let current = &list.current_branch;
    for remote in &list.remotes {
        let candidate = format!("{remote}/{current}");
        if list.branches.contains(&candidate) {
            return candidate;
        }
    }
    if list.branches.iter().any(|b| b == current) {
        return current.to_string();
    }
    list.branches.first().cloned().unwrap_or_default()
}

// ---------------------------------------------------------------------- Settings

#[derive(Debug)]
pub struct SettingsModal {
    pub editor_index: usize,
    /// Slug of the theme this dialog will save. The *active* theme may run
    /// ahead of it while the picker previews; Save persists this, Cancel
    /// restores the theme the dialog opened with.
    pub theme_slug: &'static str,
    /// What was active when the dialog opened, for the Cancel path.
    opened_with_theme: &'static str,
    pub branch_prefix: TextInput,
    pub custom_buttons: Vec<CustomClaudeButton>,
    /// Start Claude sessions with `--remote-control`.
    pub remote_control: bool,
    /// Snapshot, not a live read. The renderer runs on every frame, and asking
    /// on each one meant three file reads and three digests per repaint — I/O
    /// in a draw path, which stutters visibly on a slow or networked home.
    /// Refreshed when this dialog opens and whenever the integration dialog it
    /// opens is dismissed.
    pub integration_status: crate::services::claude_plugin::Status,
    /// One of the `FOCUS_*` constants; see [`SettingsModal`].
    pub focus: usize,
}

impl SettingsModal {
    /// The focus ring, in the order `ui/modals.rs` draws it.
    pub const FOCUS_EDITOR: usize = 0;
    pub const FOCUS_PREFIX: usize = 1;
    pub const FOCUS_THEME: usize = 2;
    pub const FOCUS_MANAGE: usize = 3;
    pub const FOCUS_INTEGRATION: usize = 4;
    pub const FOCUS_REMOTE_CONTROL: usize = 5;
    pub const FOCUS_SAVE: usize = 6;
    pub const FOCUS_CANCEL: usize = 7;
    pub const FIELDS: usize = Self::FOCUS_CANCEL + 1;

    pub fn new(settings: &Settings) -> Self {
        // Resolve an unknown slug to the default once, here, so the dialog
        // and the picker only ever see valid themes.
        let theme_slug = crate::theme::by_slug(&settings.theme_name)
            .unwrap_or(&crate::theme::THEMES[0])
            .slug;
        Self {
            editor_index: EDITORS
                .iter()
                .position(|(_, cmd)| *cmd == settings.default_editor)
                .unwrap_or(0),
            opened_with_theme: theme_slug,
            theme_slug,
            branch_prefix: TextInput::new(settings.branch_prefix.clone()).with_placeholder("feat/"),
            custom_buttons: settings.custom_buttons.clone(),
            remote_control: settings.remote_control,
            integration_status: crate::services::claude_plugin::status(),
            focus: Self::FOCUS_EDITOR,
        }
    }

    pub fn buttons_summary(&self) -> String {
        match self.custom_buttons.len() {
            0 => "No custom buttons configured".into(),
            1 => "1 custom button configured".into(),
            n => format!("{n} custom buttons configured"),
        }
    }

    fn to_settings(&self) -> Settings {
        Settings {
            default_editor: EDITORS[self.editor_index].1.to_string(),
            branch_prefix: self.branch_prefix.value().to_string(),
            theme_name: self.theme_slug.to_string(),
            custom_buttons: self.custom_buttons.clone(),
            remote_control: self.remote_control,
        }
    }

    /// Closing without saving abandons any theme the picker applied — the
    /// live preview must not outlive the dialog it was previewed in.
    fn close_without_saving(&self) -> ModalOutcome {
        crate::theme::set_active(self.opened_with_theme);
        ModalOutcome::Close
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return self.close_without_saving();
        }
        if self.focus == Self::FOCUS_PREFIX && edit_input(&mut self.branch_prefix, key) {
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            (Self::FOCUS_EDITOR, KeyCode::Left) => {
                self.editor_index = wrap_dec(self.editor_index, EDITORS.len());
                return ModalOutcome::None;
            }
            (Self::FOCUS_EDITOR, KeyCode::Right) => {
                self.editor_index = (self.editor_index + 1) % EDITORS.len();
                return ModalOutcome::None;
            }
            _ => {}
        }

        if cycle_focus(&mut self.focus, Self::FIELDS, key) {
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            // Enter toggles rather than saves: it is what a click sends, and a
            // click on a checkbox that closed the dialog would be a surprise.
            (Self::FOCUS_REMOTE_CONTROL, KeyCode::Enter | KeyCode::Char(' ')) => {
                self.remote_control = !self.remote_control;
                ModalOutcome::None
            }
            (Self::FOCUS_THEME, KeyCode::Enter) => {
                ModalOutcome::Push(Box::new(Modal::ThemePicker(ThemePickerModal::new())))
            }
            (Self::FOCUS_INTEGRATION, KeyCode::Enter) => ModalOutcome::Push(Box::new(
                Modal::ClaudeIntegration(ClaudeIntegrationModal::new()),
            )),
            (Self::FOCUS_MANAGE, KeyCode::Enter) => ModalOutcome::Push(Box::new(
                Modal::CustomButtons(CustomButtonsModal::new(self.custom_buttons.clone())),
            )),
            (Self::FOCUS_CANCEL, KeyCode::Enter) => self.close_without_saving(),
            (_, KeyCode::Enter) => {
                ModalOutcome::Submit(ModalResult::SettingsSaved(Box::new(self.to_settings())))
            }
            _ => ModalOutcome::None,
        }
    }
}

// ------------------------------------------------------------------ Theme picker

/// Scrollable list over [`crate::theme::THEMES`]. Moving the highlight applies
/// the candidate theme to the whole app immediately — the panes behind the
/// dialog are the preview — and Esc puts back what was active when the picker
/// opened. Enter commits the slug to the Settings dialog underneath, which
/// still has to be saved to persist it.
#[derive(Debug)]
pub struct ThemePickerModal {
    /// Highlighted row, an index into [`crate::theme::THEMES`].
    pub index: usize,
    /// Active slug when the picker opened, restored on Esc.
    opened_with: &'static str,
}

impl ThemePickerModal {
    pub fn new() -> Self {
        let opened_with = crate::theme::active().slug;
        Self {
            index: crate::theme::THEMES
                .iter()
                .position(|t| t.slug == opened_with)
                .unwrap_or(0),
            opened_with,
        }
    }

    /// Move the highlight and apply the candidate, clamped to the list.
    pub fn select(&mut self, index: usize) {
        self.index = index.min(crate::theme::THEMES.len() - 1);
        crate::theme::set_active(crate::theme::THEMES[self.index].slug);
    }

    /// One step in either direction, for the wheel: unlike the arrow keys it
    /// does not wrap, so scrolling to an end stops there the way every
    /// scrolled list does.
    pub fn step(&mut self, delta: isize) {
        let stepped = self.index.saturating_add_signed(delta);
        self.select(stepped);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            crate::theme::set_active(self.opened_with);
            return ModalOutcome::Close;
        }
        let len = crate::theme::THEMES.len();
        match key.code {
            KeyCode::Up | KeyCode::BackTab => {
                self.select(wrap_dec(self.index, len));
                ModalOutcome::None
            }
            KeyCode::Down | KeyCode::Tab => {
                self.select((self.index + 1) % len);
                ModalOutcome::None
            }
            KeyCode::Home => {
                self.select(0);
                ModalOutcome::None
            }
            KeyCode::End => {
                self.select(len - 1);
                ModalOutcome::None
            }
            KeyCode::Enter => ModalOutcome::Submit(ModalResult::ThemeChosen(
                crate::theme::THEMES[self.index].slug,
            )),
            _ => ModalOutcome::None,
        }
    }
}

impl Default for ThemePickerModal {
    fn default() -> Self {
        Self::new()
    }
}

fn wrap_dec(index: usize, len: usize) -> usize {
    if index == 0 { len - 1 } else { index - 1 }
}

// ---------------------------------------------------------------- Custom buttons

#[derive(Debug)]
pub struct CustomButtonsModal {
    pub buttons: Vec<CustomClaudeButton>,
    pub selected: usize,
    /// Index being edited, so the child's result lands in the right slot.
    pub editing: Option<usize>,
}

impl CustomButtonsModal {
    pub fn new(buttons: Vec<CustomClaudeButton>) -> Self {
        Self {
            buttons,
            selected: 0,
            editing: None,
        }
    }

    fn apply_edit(&mut self, button: CustomClaudeButton) {
        match self.editing.take() {
            Some(index) if index < self.buttons.len() => self.buttons[index] = button,
            _ => self.buttons.push(button),
        }
    }

    fn swap(&mut self, index: usize, delta: isize) {
        let target = index as isize + delta;
        if target < 0 || target as usize >= self.buttons.len() {
            return;
        }
        let target = target as usize;
        self.buttons.swap(index, target);
        self.selected = target;
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }
        let len = self.buttons.len();
        match key.code {
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down if len > 0 => {
                self.selected = (self.selected + 1).min(len - 1);
            }
            KeyCode::Char('a') => {
                self.editing = None;
                return ModalOutcome::Push(Box::new(Modal::EditButton(Box::new(
                    EditButtonModal::new(None, &self.buttons, None),
                ))));
            }
            KeyCode::Enter | KeyCode::Char('e') if self.selected < len => {
                self.editing = Some(self.selected);
                return ModalOutcome::Push(Box::new(Modal::EditButton(Box::new(
                    EditButtonModal::new(
                        Some(self.buttons[self.selected].clone()),
                        &self.buttons,
                        Some(self.selected),
                    ),
                ))));
            }
            KeyCode::Char('d') | KeyCode::Delete if self.selected < len => {
                self.buttons.remove(self.selected);
                self.selected = self.selected.min(self.buttons.len().saturating_sub(1));
            }
            KeyCode::Char('K') => self.swap(self.selected, -1),
            KeyCode::Char('J') => self.swap(self.selected, 1),
            KeyCode::Char('s') => {
                return ModalOutcome::Submit(ModalResult::CustomButtonsSaved(self.buttons.clone()));
            }
            _ => {}
        }
        ModalOutcome::None
    }
}

// ------------------------------------------------------------------- Edit button

#[derive(Debug)]
pub struct EditButtonModal {
    pub label: TextInput,
    pub prefix: TextInput,
    pub command: TextInput,
    pub is_edit: bool,
    pub other_labels: Vec<String>,
    pub other_prefixes: Vec<String>,
    /// Whether the prefix still auto-follows the label.
    pub follows: bool,
    pub error: String,
    /// One of the `FOCUS_*` constants; see [`EditButtonModal`].
    pub focus: usize,
}

impl EditButtonModal {
    /// The focus ring, in the order `ui/modals.rs` draws it.
    pub const FOCUS_LABEL: usize = 0;
    pub const FOCUS_PREFIX: usize = 1;
    pub const FOCUS_COMMAND: usize = 2;
    pub const FOCUS_SAVE: usize = 3;
    pub const FOCUS_CANCEL: usize = 4;
    pub const FIELDS: usize = Self::FOCUS_CANCEL + 1;

    pub fn new(
        existing: Option<CustomClaudeButton>,
        all: &[CustomClaudeButton],
        editing_index: Option<usize>,
    ) -> Self {
        let others: Vec<&CustomClaudeButton> = all
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != editing_index)
            .map(|(_, b)| b)
            .collect();

        let follows = existing
            .as_ref()
            .map(|b| b.prefix == derive_prefix(&b.label))
            .unwrap_or(true);

        Self {
            label: TextInput::new(
                existing
                    .as_ref()
                    .map(|b| b.label.clone())
                    .unwrap_or_default(),
            )
            .with_placeholder("e.g., YoloDisc")
            .with_max_length(MAX_BUTTON_LABEL_LENGTH),
            prefix: TextInput::new(
                existing
                    .as_ref()
                    .map(|b| b.prefix.clone())
                    .unwrap_or_default(),
            )
            .with_placeholder("e.g., yolodisc")
            .with_max_length(MAX_BUTTON_PREFIX_LENGTH),
            command: TextInput::new(
                existing
                    .as_ref()
                    .map(|b| b.command.clone())
                    .unwrap_or_default(),
            )
            .with_placeholder("e.g., claude --dangerously-skip-permissions")
            .with_max_length(MAX_CLAUDE_COMMAND_LENGTH),
            is_edit: existing.is_some(),
            other_labels: others.iter().map(|b| b.label.clone()).collect(),
            other_prefixes: others.iter().map(|b| b.prefix.clone()).collect(),
            follows,
            error: String::new(),
            focus: Self::FOCUS_LABEL,
        }
    }

    pub fn title(&self) -> &'static str {
        if self.is_edit {
            "Edit Button"
        } else {
            "Add Button"
        }
    }

    fn save(&mut self) -> ModalOutcome {
        let label = self.label.value().trim().to_string();
        let prefix = self.prefix.value().trim().to_string();
        let command = self.command.value().trim().to_string();

        let command_error = if command.is_empty() {
            Some("Command cannot be empty".to_string())
        } else {
            validate_claude_command(&command)
        };

        // Report the first failing rule, in the order the fields are shown.
        let first_error = [
            validate_button_label(&label),
            validate_button_prefix(&prefix),
            command_error,
        ]
        .into_iter()
        .flatten()
        .next();
        if let Some(error) = first_error {
            self.error = error;
            return ModalOutcome::None;
        }

        if self.other_labels.contains(&label) {
            self.error = "Another button already uses this label".into();
            return ModalOutcome::None;
        }
        if self.other_prefixes.contains(&prefix) {
            self.error = "Another button already uses this prefix".into();
            return ModalOutcome::None;
        }

        self.error.clear();
        ModalOutcome::Submit(ModalResult::CustomButtonSaved(Box::new(
            CustomClaudeButton {
                label,
                prefix,
                command,
            },
        )))
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }

        match self.focus {
            Self::FOCUS_LABEL if edit_input(&mut self.label, key) => {
                if self.follows {
                    self.prefix.set_value(derive_prefix(self.label.value()));
                }
                return ModalOutcome::None;
            }
            Self::FOCUS_PREFIX if edit_input(&mut self.prefix, key) => {
                self.follows = self.prefix.value() == derive_prefix(self.label.value());
                return ModalOutcome::None;
            }
            Self::FOCUS_COMMAND if edit_input(&mut self.command, key) => return ModalOutcome::None,
            _ => {}
        }

        if cycle_focus(&mut self.focus, Self::FIELDS, key) {
            return ModalOutcome::None;
        }

        match (self.focus, key.code) {
            (Self::FOCUS_CANCEL, KeyCode::Enter) => ModalOutcome::Close,
            (_, KeyCode::Enter) => self.save(),
            _ => ModalOutcome::None,
        }
    }
}

/// Install and remove the Claude Code integration.
///
/// Everything it can do is derived from what is on disk right now, so the
/// dialog cannot offer an upgrade that is not available or an uninstall of
/// something that was never installed. The action list is built once and both
/// the key handler and the renderer walk it, so what is drawn is what fires.
#[derive(Debug, Clone)]
pub struct ClaudeIntegrationModal {
    pub status: crate::services::claude_plugin::Status,
    /// What the last action did, shown under the status.
    pub message: Option<String>,
    /// Index into [`Self::actions`].
    pub focus: usize,
}

impl Default for ClaudeIntegrationModal {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeIntegrationModal {
    pub fn new() -> Self {
        Self {
            status: crate::services::claude_plugin::status(),
            message: None,
            focus: 0,
        }
    }

    pub fn actions(&self) -> Vec<IntegrationAction> {
        use crate::services::claude_plugin::Status;
        let mut actions = vec![match self.status {
            Status::NotInstalled => IntegrationAction::Install,
            Status::Installed => IntegrationAction::Reinstall,
            Status::Outdated { .. } => IntegrationAction::Upgrade,
            Status::Drifted(_) => IntegrationAction::Overwrite,
        }];
        if !matches!(self.status, Status::NotInstalled) {
            actions.push(IntegrationAction::Uninstall);
        }
        actions.push(IntegrationAction::Close);
        actions
    }

    /// Re-read the status after an action, and keep the focus in range: the
    /// action list shrinks by one the moment the plugin is uninstalled.
    pub fn refresh(&mut self, message: impl Into<String>) {
        self.status = crate::services::claude_plugin::status();
        self.message = Some(message.into());
        self.focus = self.focus.min(self.actions().len().saturating_sub(1));
    }

    pub fn set_focus(&mut self, index: usize) {
        if index < self.actions().len() {
            self.focus = index;
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        let count = self.actions().len();
        match key.code {
            KeyCode::Esc => ModalOutcome::Close,
            KeyCode::Left | KeyCode::Up | KeyCode::BackTab => {
                self.focus = (self.focus + count - 1) % count;
                ModalOutcome::None
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Tab => {
                self.focus = (self.focus + 1) % count;
                ModalOutcome::None
            }
            KeyCode::Enter => match self.actions()[self.focus] {
                IntegrationAction::Close => ModalOutcome::Close,
                action => ModalOutcome::Effect(ModalEffect::ClaudePlugin(action)),
            },
            _ => ModalOutcome::None,
        }
    }
}

// --------------------------------------------------------------- Rename session

/// The longest name worth having: it becomes a tmux tab and a prompt-box
/// title, both of which truncate far sooner than this.
pub const MAX_SESSION_NAME_LENGTH: usize = 60;

/// Rename a Claude session, live or stopped.
///
/// One dialog for both cases; only the submit differs, and the dialog says
/// which mechanism will run so the user is never surprised by a tab renaming
/// itself (live) or not (stopped).
#[derive(Debug)]
pub struct RenameSessionModal {
    pub path: String,
    pub session_id: String,
    pub name: TextInput,
    /// The tmux window holding this session right now, if any. Some means the
    /// rename goes through the window; the sync plugin does the rest.
    pub live_window: Option<(String, String)>,
    pub error: String,
    /// One of the `FOCUS_*` constants; see [`RenameSessionModal`].
    pub focus: usize,
}

impl RenameSessionModal {
    /// The focus ring, in the order `ui/modals.rs` draws it.
    pub const FOCUS_NAME: usize = 0;
    pub const FOCUS_SAVE: usize = 1;
    pub const FOCUS_CANCEL: usize = 2;
    pub const FIELDS: usize = Self::FOCUS_CANCEL + 1;

    pub fn new(
        path: String,
        session_id: String,
        current_name: String,
        live_window: Option<(String, String)>,
    ) -> Self {
        // The seed can be an unnamed session's first-prompt title, which runs
        // to 100 characters; the field's cap only gates *typed* input, so an
        // over-long seed is clipped here or Save would accept what typing
        // could never produce.
        let current_name: String = current_name.chars().take(MAX_SESSION_NAME_LENGTH).collect();
        Self {
            path,
            session_id,
            name: TextInput::new(current_name)
                .with_placeholder("session name")
                .with_max_length(MAX_SESSION_NAME_LENGTH),
            live_window,
            error: String::new(),
            focus: Self::FOCUS_NAME,
        }
    }

    fn save(&mut self) -> ModalOutcome {
        let name = self.name.value().trim().to_string();
        if name.is_empty() {
            self.error = "Name cannot be empty".into();
            return ModalOutcome::None;
        }
        // The name becomes a tmux window name, which reaches a shell via the
        // quoting in `services/tmux.rs` — but a control character would end a
        // startup-file line, so it is refused at the door like button labels.
        if name.chars().any(char::is_control) {
            self.error = "Name cannot contain control characters".into();
            return ModalOutcome::None;
        }
        ModalOutcome::Submit(ModalResult::SessionRenamed {
            path: self.path.clone(),
            session_id: self.session_id.clone(),
            name,
            live_window_id: self.live_window.as_ref().map(|(id, _)| id.clone()),
        })
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }
        if self.focus == Self::FOCUS_NAME && edit_input(&mut self.name, key) {
            self.error.clear();
            return ModalOutcome::None;
        }
        if cycle_focus(&mut self.focus, Self::FIELDS, key) {
            return ModalOutcome::None;
        }
        match (self.focus, key.code) {
            (Self::FOCUS_CANCEL, KeyCode::Enter) => ModalOutcome::Close,
            (_, KeyCode::Enter) => self.save(),
            _ => ModalOutcome::None,
        }
    }
}

// ----------------------------------------------------------------------- Confirm

#[derive(Debug)]
pub struct ConfirmModal {
    pub title: String,
    pub message: String,
    pub action: ConfirmAction,
    /// `true` when the destructive choice is highlighted.
    pub confirm_focused: bool,
    /// Submit keys are ignored until this instant. Set on confirms pushed by a
    /// *background event*: a keystroke queued before the modal appeared (a `y`
    /// aimed at ClaudeYolo, letters typed into the modal underneath) must not
    /// confirm a dialog the user never saw. Cancel always works.
    armed_at: Option<std::time::Instant>,
}

/// Long enough to outlive any keystroke already in the event queue, far too
/// short for a human deliberately confirming to notice.
const CONFIRM_ARM_DELAY: std::time::Duration = std::time::Duration::from_millis(400);

impl ConfirmModal {
    pub fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        action: ConfirmAction,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            action,
            confirm_focused: false,
            armed_at: None,
        }
    }

    /// For confirms pushed by a background event rather than a user action.
    pub fn with_arm_delay(mut self) -> Self {
        self.armed_at = Some(std::time::Instant::now() + CONFIRM_ARM_DELAY);
        self
    }

    #[cfg(test)]
    pub fn disarm(&mut self) {
        self.armed_at = None;
    }

    fn submit_armed(&self) -> bool {
        self.armed_at
            .is_none_or(|at| std::time::Instant::now() >= at)
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalOutcome {
        if is_escape(key) {
            return ModalOutcome::Close;
        }
        match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                self.confirm_focused = !self.confirm_focused;
                ModalOutcome::None
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if self.submit_armed() {
                    ModalOutcome::Submit(ModalResult::Confirmed(self.action.clone()))
                } else {
                    ModalOutcome::None
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => ModalOutcome::Close,
            KeyCode::Enter => {
                if self.confirm_focused && self.submit_armed() {
                    ModalOutcome::Submit(ModalResult::Confirmed(self.action.clone()))
                } else if !self.confirm_focused {
                    ModalOutcome::Close
                } else {
                    ModalOutcome::None
                }
            }
            _ => ModalOutcome::None,
        }
    }
}

#[cfg(test)]
mod tests;
