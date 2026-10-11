//! Data models for forestui.
//!
//! The on-disk JSON has to load in the release before this one and the release
//! after it — forestui updates itself, so users run the newest release or the
//! one before, and both may touch the same files. New fields are additive and
//! defaulted; an older build ignores keys it does not know.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_CLAUDE_COMMAND_LENGTH: usize = 200;
pub const MAX_BUTTON_LABEL_LENGTH: usize = 20;
pub const MAX_BUTTON_PREFIX_LENGTH: usize = 20;

/// Derive a tmux-safe window prefix from a button label.
///
/// Lowercase, keep `[a-z0-9_-]`, collapse other runs to `-`, strip leading and
/// trailing `-`, truncate to [`MAX_BUTTON_PREFIX_LENGTH`].
pub fn derive_prefix(label: &str) -> String {
    let lowered = label.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_dash = false;
    for ch in lowered.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-' {
            if pending_dash {
                out.push('-');
                pending_dash = false;
            }
            out.push(ch);
        } else if !out.is_empty() {
            pending_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    trimmed.chars().take(MAX_BUTTON_PREFIX_LENGTH).collect()
}

fn has_control_chars(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '\n' | '\r' | '\t' | '\0'))
}

pub fn validate_button_label(label: &str) -> Option<String> {
    if label.is_empty() {
        return Some("Label cannot be empty".into());
    }
    if label.chars().count() > MAX_BUTTON_LABEL_LENGTH {
        return Some(format!(
            "Label too long (max {MAX_BUTTON_LABEL_LENGTH} characters)"
        ));
    }
    if has_control_chars(label) {
        return Some("Label cannot contain control characters".into());
    }
    None
}

pub fn validate_button_prefix(prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return Some("Prefix cannot be empty".into());
    }
    if prefix.chars().count() > MAX_BUTTON_PREFIX_LENGTH {
        return Some(format!(
            "Prefix too long (max {MAX_BUTTON_PREFIX_LENGTH} characters)"
        ));
    }
    let valid = prefix
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !valid {
        return Some("Prefix must be lowercase letters, digits, '-' or '_'".into());
    }
    None
}

/// Validate a custom Claude command. An empty command is valid here (it clears
/// the value); callers that require a command check for emptiness themselves.
pub fn validate_claude_command(command: &str) -> Option<String> {
    if command.is_empty() {
        return None;
    }
    if command.chars().count() > MAX_CLAUDE_COMMAND_LENGTH {
        return Some(format!(
            "Command too long (max {MAX_CLAUDE_COMMAND_LENGTH} characters)"
        ));
    }
    if has_control_chars(command) {
        return Some("Command cannot contain newlines or control characters".into());
    }
    None
}

/// A user-configured custom Claude command button.
///
/// `label` is shown on the button. `prefix` is the tmux window prefix
/// (`"yolodisc"` produces `yolodisc:<name>`). `command` runs as-is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomClaudeButton {
    pub label: String,
    pub prefix: String,
    pub command: String,
}

impl CustomClaudeButton {
    /// Whether this button's command enables the permissions bypass.
    pub fn is_yolo_style(&self) -> bool {
        self.command.contains("--dangerously-skip-permissions")
    }
}

fn default_true() -> bool {
    true
}

fn now() -> DateTime<Utc> {
    Utc::now()
}

/// Represents a Git worktree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worktree {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    pub branch: String,
    pub path: String,
    #[serde(default)]
    pub is_archived: bool,
    #[serde(default)]
    pub sort_order: Option<i64>,
    #[serde(default = "now")]
    pub last_modified: DateTime<Utc>,
    /// Branch this worktree was created from (e.g. `origin/main`).
    #[serde(default)]
    pub base_branch: Option<String>,
    /// Git commit ref at the time the worktree was created.
    #[serde(default)]
    pub created_from_ref: Option<String>,
    /// The work item this worktree was started for. Not derived from the
    /// name or branch, so renaming either keeps it.
    #[serde(default, deserialize_with = "lenient_work_item")]
    pub work_item: Option<WorkItemLink>,
}

/// The record on a worktree of which work item it was started for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkItemLink {
    Issue { number: i64 },
    PullRequest { number: i64 },
    FreeText { description: String },
}

/// A link this build cannot read loads as no link.
///
/// The tag is open-ended: a later release may add a kind, and the release
/// before it still has to load the file. A strict enum would fail the whole
/// config on that one field — and a config that does not parse loads as
/// empty state.
fn lenient_work_item<'de, D>(deserializer: D) -> Result<Option<WorkItemLink>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

impl Worktree {
    pub fn new(name: String, branch: String, path: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            branch,
            path,
            is_archived: false,
            sort_order: None,
            last_modified: Utc::now(),
            base_branch: None,
            created_from_ref: None,
            work_item: None,
        }
    }
}

/// Represents a Git repository with its worktrees.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    pub source_path: String,
    #[serde(default)]
    pub worktrees: Vec<Worktree>,
}

impl Repository {
    pub fn new(name: String, source_path: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            source_path,
            worktrees: Vec::new(),
        }
    }

    /// Active (non-archived) worktrees sorted by explicit order, then recency.
    pub fn active_worktrees(&self) -> Vec<&Worktree> {
        let mut active: Vec<&Worktree> = self.worktrees.iter().filter(|w| !w.is_archived).collect();
        active.sort_by(|a, b| {
            let ao = a.sort_order.unwrap_or(i64::MAX);
            let bo = b.sort_order.unwrap_or(i64::MAX);
            ao.cmp(&bo)
                .then_with(|| b.last_modified.cmp(&a.last_modified))
        });
        active
    }

    /// Archived worktrees sorted by recency.
    pub fn archived_worktrees(&self) -> Vec<&Worktree> {
        let mut archived: Vec<&Worktree> =
            self.worktrees.iter().filter(|w| w.is_archived).collect();
        archived.sort_by_key(|w| std::cmp::Reverse(w.last_modified));
        archived
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    User,
    Claude,
}

impl Speaker {
    pub fn label(self) -> &'static str {
        match self {
            Speaker::User => "user:",
            Speaker::Claude => "claude:",
        }
    }
}

/// One side speaking, for the preview on a session card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurn {
    pub speaker: Speaker,
    pub text: String,
}

/// A Claude Code session discovered on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSession {
    pub id: String,
    /// What to show in the list: the name Claude records, else the first prompt.
    pub title: String,
    /// A name a human chose via `/rename`, if any. Only this is eligible to
    /// seed a tmux window name — an ai-title is a sentence and the first-prompt
    /// fallback is 100 characters of prose; neither is a tab name.
    pub custom_title: Option<String>,
    /// The last two turns, oldest first. What identifies a conversation is the
    /// exchange it stopped on, not either half alone.
    pub recent_turns: Vec<SessionTurn>,
    pub last_timestamp: DateTime<Utc>,
    pub message_count: usize,
    /// The branch the conversation last saw (`gitBranch` on its records).
    /// Recognition data: a conversation is remembered by where it happened.
    pub git_branch: Option<String>,
    /// Token totals summed over every assistant record's `usage`. Kept as the
    /// API's own four categories because they are priced differently; the
    /// display and the cost estimate both derive from these.
    pub tokens: TokenUsage,
    /// The model of the last assistant record, for the cost estimate.
    pub model: Option<String>,
}

/// Token counts as the API reports them, summed across a transcript.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub output: u64,
}

impl TokenUsage {
    /// Everything the model read, whatever the price tier.
    pub fn total_in(&self) -> u64 {
        self.input + self.cache_write + self.cache_read
    }

    pub fn is_zero(&self) -> bool {
        self.total_in() == 0 && self.output == 0
    }
}

impl ClaudeSession {
    pub fn relative_time(&self) -> String {
        crate::util::naturaltime(self.last_timestamp)
    }
}

fn default_editor() -> String {
    "vim".into()
}

fn default_branch_prefix() -> String {
    "feat/".into()
}

fn default_theme_name() -> String {
    "forest-dark".into()
}

/// Application settings, persisted to `~/.config/forestui/settings.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_editor")]
    pub default_editor: String,
    #[serde(default = "default_branch_prefix")]
    pub branch_prefix: String,
    /// Slug of the named theme (`theme::THEMES`). Unknown slugs resolve to
    /// the default at activation.
    ///
    /// Not `theme`: files written before the theme picker carry an inert
    /// `"theme": "system"`, which is ignored on load and dropped on the next
    /// save. Reading that key for the palette is what once reset every launch
    /// to the default theme.
    #[serde(default = "default_theme_name")]
    pub theme_name: String,
    #[serde(default)]
    pub custom_buttons: Vec<CustomClaudeButton>,
    /// Start every Claude session forestui opens with `--remote-control`, so
    /// it can be picked up from the phone without remembering to `/rc` first.
    ///
    /// On by default, and a file without the key loads with it *on*: an
    /// existing install picks it up on update without anyone opening
    /// Settings. Only an explicit `false` — the user unticking it — turns it
    /// off.
    #[serde(default = "default_true")]
    pub remote_control: bool,
    /// How Start work lists and names work. A hand-edited section that does
    /// not parse loads as the defaults rather than taking the whole file —
    /// and the user's theme and buttons — down with it.
    #[serde(default, deserialize_with = "lenient_start_work")]
    pub start_work: StartWorkSettings,
}

fn lenient_start_work<'de, D>(deserializer: D) -> Result<StartWorkSettings, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// Whose issues and PRs Start work lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Issues assigned to or opened by me; PRs with a review requested from
    /// me or opened by me.
    #[default]
    Mine,
    /// Every open issue or PR.
    All,
}

/// How one work mode names its worktree and branch.
///
/// `name_pattern` is the worktree name, with `{number}` and `{slug}` (the
/// title's, or a free-text description's first words) filled in; the branch
/// is `branch_prefix` followed by that name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Naming {
    pub branch_prefix: String,
    pub name_pattern: String,
}

impl Naming {
    fn new(branch_prefix: &str, name_pattern: &str) -> Self {
        Self {
            branch_prefix: branch_prefix.into(),
            name_pattern: name_pattern.into(),
        }
    }

    /// `self` with whatever `partial` sets laid over it, so a mode written
    /// with only a prefix keeps its default pattern, and the other way round.
    fn overlaid(self, partial: PartialNaming) -> Self {
        Self {
            branch_prefix: partial.branch_prefix.unwrap_or(self.branch_prefix),
            name_pattern: partial.name_pattern.unwrap_or(self.name_pattern),
        }
    }
}

#[derive(Deserialize)]
struct PartialNaming {
    #[serde(default)]
    branch_prefix: Option<String>,
    #[serde(default)]
    name_pattern: Option<String>,
}

fn issue_naming() -> Naming {
    Naming::new("fix/", "issue-{number}-{slug}")
}

fn feature_naming() -> Naming {
    Naming::new("feat/", "feat-{number}-{slug}")
}

fn free_feature_naming() -> Naming {
    Naming::new("feat/", "feat-{slug}")
}

fn partial_issue_naming<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Naming, D::Error> {
    Ok(issue_naming().overlaid(PartialNaming::deserialize(d)?))
}

fn partial_feature_naming<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Naming, D::Error> {
    Ok(feature_naming().overlaid(PartialNaming::deserialize(d)?))
}

fn partial_free_feature_naming<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Naming, D::Error> {
    Ok(free_feature_naming().overlaid(PartialNaming::deserialize(d)?))
}

/// A `pr-review` worktree checks out the PR's own branch, so the mode names
/// only the worktree and has no branch prefix to set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrReviewNaming {
    #[serde(default = "default_pr_review_pattern")]
    pub name_pattern: String,
}

fn default_pr_review_pattern() -> String {
    "pr-{number}-{slug}".into()
}

impl Default for PrReviewNaming {
    fn default() -> Self {
        Self {
            name_pattern: default_pr_review_pattern(),
        }
    }
}

fn default_feature_labels() -> Vec<String> {
    vec!["enhancement".into()]
}

/// Start work's global settings. Every field is defaulted to the behaviour
/// that shipped before it was a setting, so a file without them changes
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartWorkSettings {
    #[serde(default)]
    pub scope: Scope,
    /// An issue carrying any of these is a `feature`; every other open issue
    /// is an `issue`. Matched case-insensitively, as GitHub does.
    #[serde(default = "default_feature_labels")]
    pub feature_labels: Vec<String>,
    #[serde(default = "issue_naming", deserialize_with = "partial_issue_naming")]
    pub issue: Naming,
    #[serde(
        default = "feature_naming",
        deserialize_with = "partial_feature_naming"
    )]
    pub feature: Naming,
    #[serde(
        default = "free_feature_naming",
        deserialize_with = "partial_free_feature_naming"
    )]
    pub free_feature: Naming,
    #[serde(default)]
    pub pr_review: PrReviewNaming,
}

impl Default for StartWorkSettings {
    fn default() -> Self {
        Self {
            scope: Scope::default(),
            feature_labels: default_feature_labels(),
            issue: issue_naming(),
            feature: feature_naming(),
            free_feature: free_feature_naming(),
            pr_review: PrReviewNaming::default(),
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_editor: default_editor(),
            branch_prefix: default_branch_prefix(),
            theme_name: default_theme_name(),
            custom_buttons: Vec::new(),
            remote_control: true,
            start_work: StartWorkSettings::default(),
        }
    }
}

/// The current sidebar selection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    pub repository_id: Option<Uuid>,
    pub worktree_id: Option<Uuid>,
}

impl Selection {
    pub fn is_repository(&self) -> bool {
        self.repository_id.is_some() && self.worktree_id.is_none()
    }

    pub fn is_worktree(&self) -> bool {
        self.worktree_id.is_some()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GitHubLabel {
    pub name: String,
}

/// A GitHub issue as returned by `gh issue list --json ...`.
///
/// Only the fields the UI shows are modelled; serde ignores the rest of the
/// payload, so the `--json` request stays unchanged.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubIssue {
    #[serde(default)]
    pub number: i64,
    #[serde(default)]
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub labels: Vec<GitHubLabel>,
}

/// The login of whoever opened an issue or pull request.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GitHubUser {
    #[serde(default)]
    pub login: String,
}

/// An open pull request as returned by `gh pr list --json ...`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubPullRequest {
    #[serde(default)]
    pub number: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: GitHubUser,
    #[serde(default)]
    pub is_draft: bool,
    /// The branch the PR is opened from — on the fork, for a PR from one.
    #[serde(default)]
    pub head_ref_name: String,
    /// Opened from a fork, whose branch names say nothing about this
    /// repository's.
    #[serde(default)]
    pub is_cross_repository: bool,
    pub created_at: DateTime<Utc>,
}

impl GitHubIssue {
    pub fn relative_time(&self) -> String {
        crate::util::naturaltime(self.updated_at)
    }
}

/// Serializable form of the per-forest state file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppStateData {
    #[serde(default)]
    pub repositories: Vec<Repository>,
    /// Pinned Claude sessions, keyed by the path whose pane shows them (a
    /// worktree's path or a repository's source path) — session ids in pin
    /// order, which is the order they are shown in.
    #[serde(default)]
    pub pinned_sessions: std::collections::HashMap<String, Vec<String>>,
}

#[cfg(test)]
mod tests;
