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
    /// The parent issue a sub-issue's worktree was branched from, whose
    /// branch is `base_branch`. Kept beside the link rather than in it, so
    /// the link still names the work item alone. Its work is done once its
    /// pull request into that branch is merged.
    #[serde(default)]
    pub parent_issue: Option<i64>,
    /// Its work is done, so Prune removes it. Set by the user or by the
    /// finished check, whichever changed it last — see
    /// [`Worktree::fold_finished_check`].
    #[serde(default)]
    pub finished: bool,
    /// The finished check's last answer, `None` before it has given one.
    /// Kept so the check writes the mark only when its answer changes and a
    /// hand-set mark survives the sweep re-asking the same question.
    #[serde(default)]
    pub last_finished_check: Option<bool>,
}

/// The record on a worktree of which work item it was started for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkItemLink {
    Issue {
        number: i64,
    },
    PullRequest {
        number: i64,
    },
    /// A session prompt. Recorded under the free-text feature's kind and
    /// field names, which it replaced, so worktrees started before keep
    /// their links and their finished checks.
    #[serde(rename = "free_text")]
    Session {
        #[serde(rename = "description")]
        prompt: String,
    },
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
            parent_issue: None,
            finished: false,
            last_finished_check: None,
        }
    }

    /// Fold one verdict of the finished check into the saved mark: a new
    /// answer sets the mark, the answer it gave last time leaves it alone,
    /// so whichever of the user and the check changed it last wins. Returns
    /// whether anything changed.
    pub fn fold_finished_check(&mut self, finished: bool) -> bool {
        if self.last_finished_check == Some(finished) {
            return false;
        }
        self.last_finished_check = Some(finished);
        self.finished = finished;
        true
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
    /// The session this conversation was handed off to (Claude Code's
    /// `continued-in` record), if it was. A superseded session gets no card:
    /// the successor carries the conversation on.
    pub continued_in: Option<String>,
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
    /// List blocked issues in Start work's picker, remembered across runs.
    /// Off by default, which is also what a file without the key loads as.
    #[serde(default)]
    pub show_blocked_issues: bool,
    /// Which issues Start work's `issue` list shows, remembered across runs
    /// like "Show blocked". A value this build does not know — written by a
    /// newer one — loads as All rather than taking the file down with it.
    #[serde(default, deserialize_with = "lenient_issue_filter")]
    pub issue_filter: IssueFilter,
    /// The back key: the tmux key, after the prefix, that takes the viewer
    /// from a Claude window forestui opened back to forestui. `None` is off —
    /// written as `null`, so a file without the key still loads as the
    /// default, prefix + ←.
    #[serde(default = "default_back_key")]
    pub back_key: Option<String>,
    /// The user's own binding for `back_key` that they said to take anyway,
    /// as the command tmux printed for it. The offer stays answered while the
    /// key is bound to exactly this; a different binding is a new conflict.
    #[serde(default)]
    pub back_key_accepted: Option<String>,
}

fn default_back_key() -> Option<String> {
    Some(crate::services::tmux::back_key::DEFAULT_KEY.to_string())
}

fn lenient_issue_filter<'de, D>(deserializer: D) -> Result<IssueFilter, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// Which open issues the `issue` work mode lists, by their labels: all of
/// them, the features, the bugs, or the others — neither. An issue with a
/// feature and a bug label is among the features and among the bugs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IssueFilter {
    #[default]
    All,
    Features,
    Bugs,
    Other,
}

impl IssueFilter {
    /// The value Start work's filter button moves to next.
    pub fn next(self) -> Self {
        match self {
            IssueFilter::All => IssueFilter::Features,
            IssueFilter::Features => IssueFilter::Bugs,
            IssueFilter::Bugs => IssueFilter::Other,
            IssueFilter::Other => IssueFilter::All,
        }
    }

    /// What the filter button reads.
    pub fn label(self) -> &'static str {
        match self {
            IssueFilter::All => "All",
            IssueFilter::Features => "Features",
            IssueFilter::Bugs => "Bugs",
            IssueFilter::Other => "Other",
        }
    }
}

fn lenient_start_work<'de, D>(deserializer: D) -> Result<StartWorkSettings, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// Whose issues and PRs Start work lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Issues assigned to or opened by me; PRs with a review requested from
    /// me or opened by me.
    Mine,
    /// Every open issue or PR.
    All,
}

impl Scope {
    /// The other scope: what Start work's "Not just mine" flips to.
    pub fn toggled(self) -> Self {
        match self {
            Scope::Mine => Scope::All,
            Scope::All => Scope::Mine,
        }
    }
}

/// How one work mode names its worktree and branch.
///
/// `name_pattern` is the worktree name, with `{number}` and `{slug}` (the
/// title's, or a session prompt's first words) filled in; the branch is
/// `branch_prefix` followed by `branch_pattern`, filled the same way. The
/// two patterns are separate because the worktree name carries the kind for
/// the sidebar, and on the branch the prefix already says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Naming {
    pub branch_prefix: String,
    pub name_pattern: String,
    pub branch_pattern: String,
}

impl Naming {
    fn new(branch_prefix: &str, name_pattern: &str, branch_pattern: &str) -> Self {
        Self {
            branch_prefix: branch_prefix.into(),
            name_pattern: name_pattern.into(),
            branch_pattern: branch_pattern.into(),
        }
    }

    /// `self` with whatever `partial` sets laid over it, so a mode written
    /// with only a prefix keeps its pattern, and the other way round.
    fn overlaid(&self, partial: &PartialNaming) -> Self {
        Self {
            branch_prefix: partial
                .branch_prefix
                .clone()
                .unwrap_or_else(|| self.branch_prefix.clone()),
            name_pattern: partial
                .name_pattern
                .clone()
                .unwrap_or_else(|| self.name_pattern.clone()),
            branch_pattern: partial
                .branch_pattern
                .clone()
                .unwrap_or_else(|| self.branch_pattern.clone()),
        }
    }
}

/// A `pr-review` worktree checks out the PR's own branch, so the mode names
/// only the worktree and has no branch prefix to set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrReviewNaming {
    pub name_pattern: String,
}

/// The name suggestion: a headless Claude that reads a session prompt and
/// proposes the worktree and branch name for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionNaming {
    /// Off means no call is made: a session keeps its first-words names.
    pub enabled: bool,
    /// Claude's `--model`, an alias or a full model name.
    pub model: String,
    /// How long the call may run before the first-words names are final.
    pub timeout_secs: u64,
    /// The built-in tools the call may use. None by default, so it can read
    /// only the prompt it is given.
    pub allowed_tools: Vec<String>,
}

/// Start work's global settings. Every field is defaulted to the behaviour
/// that shipped before it was a setting, so a file without them changes
/// nothing.
///
/// Read through [`PartialStartWork`], so the defaults live in one place —
/// the `Default` impl — and a key the file leaves out, down to one half of
/// a mode's naming, keeps its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "PartialStartWork")]
pub struct StartWorkSettings {
    pub scope: Scope,
    /// An issue carrying any of these starts as a feature, with the `feature`
    /// naming and template; every other issue starts as an issue. Both are
    /// listed under the `issue` work mode. Matched case-insensitively, as
    /// GitHub does.
    pub feature_labels: Vec<String>,
    /// An issue carrying any of these is a bug. It only decides where the
    /// issue appears in the issue filter, never how it starts.
    pub bug_labels: Vec<String>,
    pub issue: Naming,
    pub feature: Naming,
    pub session: Naming,
    pub pr_review: PrReviewNaming,
    pub session_naming: SessionNaming,
}

impl Default for StartWorkSettings {
    fn default() -> Self {
        Self {
            scope: Scope::Mine,
            feature_labels: vec!["enhancement".into()],
            bug_labels: vec!["bug".into()],
            // `fix/` does not say `issue`, so an issue branch keeps it.
            issue: Naming::new("fix/", "issue-{number}-{slug}", "issue-{number}-{slug}"),
            feature: Naming::new("feat/", "feat-{number}-{slug}", "{number}-{slug}"),
            session: Naming::new("session/", "session-{slug}", "{slug}"),
            pr_review: PrReviewNaming {
                name_pattern: "pr-{number}-{slug}".into(),
            },
            session_naming: SessionNaming {
                enabled: true,
                model: "haiku".into(),
                timeout_secs: 20,
                allowed_tools: Vec::new(),
            },
        }
    }
}

impl StartWorkSettings {
    /// `self` with whatever `partial` sets laid over it. A key the partial
    /// leaves out, down to one half of a mode's naming, keeps `self`'s value.
    ///
    /// The one merge rule for both layers: a global file over the defaults,
    /// and a repository's settings over the global ones.
    pub fn overlaid(&self, partial: &PartialStartWork) -> Self {
        Self {
            scope: partial.scope.unwrap_or(self.scope),
            feature_labels: partial
                .feature_labels
                .clone()
                .unwrap_or_else(|| self.feature_labels.clone()),
            bug_labels: partial
                .bug_labels
                .clone()
                .unwrap_or_else(|| self.bug_labels.clone()),
            issue: self.issue.overlaid(&partial.issue),
            feature: self.feature.overlaid(&partial.feature),
            session: self.session.overlaid(&partial.session),
            pr_review: PrReviewNaming {
                name_pattern: partial
                    .pr_review
                    .name_pattern
                    .clone()
                    .unwrap_or_else(|| self.pr_review.name_pattern.clone()),
            },
            session_naming: self.session_naming.overlaid(&partial.session_naming),
        }
    }
}

/// The `start_work` section as a file holds it: any key may be missing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PartialStartWork {
    pub scope: Option<Scope>,
    pub feature_labels: Option<Vec<String>>,
    pub bug_labels: Option<Vec<String>>,
    pub issue: PartialNaming,
    pub feature: PartialNaming,
    pub session: PartialNaming,
    pub pr_review: PartialPrReviewNaming,
    pub session_naming: PartialSessionNaming,
    /// What new work stems from in this repository, in place of the
    /// remote's default branch. Only a repository's file has it: the
    /// remote's default is the only sensible global one, and that is
    /// already the fallback. Not in [`StartWorkSettings`] for that reason.
    pub base_branch: Option<String>,
}

impl PartialStartWork {
    /// The base branch this repository sets, if any. An empty value is
    /// none, as `null` is.
    pub fn base_branch(&self) -> Option<&str> {
        self.base_branch
            .as_deref()
            .map(str::trim)
            .filter(|branch| !branch.is_empty())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PartialNaming {
    pub branch_prefix: Option<String>,
    pub name_pattern: Option<String>,
    pub branch_pattern: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PartialPrReviewNaming {
    pub name_pattern: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct PartialSessionNaming {
    pub enabled: Option<bool>,
    pub model: Option<String>,
    pub timeout_secs: Option<u64>,
    pub allowed_tools: Option<Vec<String>>,
}

impl SessionNaming {
    fn overlaid(&self, partial: &PartialSessionNaming) -> Self {
        Self {
            enabled: partial.enabled.unwrap_or(self.enabled),
            model: partial.model.clone().unwrap_or_else(|| self.model.clone()),
            timeout_secs: partial.timeout_secs.unwrap_or(self.timeout_secs),
            allowed_tools: partial
                .allowed_tools
                .clone()
                .unwrap_or_else(|| self.allowed_tools.clone()),
        }
    }
}

impl From<PartialStartWork> for StartWorkSettings {
    fn from(partial: PartialStartWork) -> Self {
        Self::default().overlaid(&partial)
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
            show_blocked_issues: false,
            issue_filter: IssueFilter::All,
            back_key: default_back_key(),
            back_key_accepted: None,
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
    #[serde(default)]
    pub labels: Vec<GitHubLabel>,
    /// GitHub's native "blocked by" list. `None` from a `gh` too old to
    /// know the field, which leaves the issue unblocked.
    #[serde(default)]
    pub blocked_by: Option<BlockedBy>,
    /// The issue's page, which says which repository it is in: a parent is
    /// only branched from when its URL names the same one.
    #[serde(default)]
    pub url: String,
    /// The issue this one is a sub-issue of. `None` from a `gh` too old to
    /// know the field, and for an answer that does not parse: a garbled
    /// parent must not take the whole list down.
    #[serde(default, deserialize_with = "lenient_parent")]
    pub parent: Option<ParentIssue>,
}

/// The parent issue `gh issue list --json parent` returns.
#[derive(Debug, Clone, Deserialize)]
pub struct ParentIssue {
    #[serde(default)]
    pub number: i64,
    #[serde(default)]
    pub title: String,
    /// `OPEN` or `CLOSED`.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub url: String,
}

fn lenient_parent<'de, D>(deserializer: D) -> Result<Option<ParentIssue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

/// The `https://github.com/<owner>/<name>` an issue URL belongs to, `None`
/// for one that is not an issue URL.
fn issue_repository(url: &str) -> Option<&str> {
    url.rsplit_once("/issues/").map(|(repo, _)| repo)
}

/// The `blockedBy` connection `gh issue list --json blockedBy` returns.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BlockedBy {
    #[serde(default)]
    pub nodes: Vec<Blocker>,
}

/// One issue another one is blocked by.
#[derive(Debug, Clone, Deserialize)]
pub struct Blocker {
    #[serde(default)]
    pub number: i64,
    /// `OPEN` or `CLOSED`.
    #[serde(default)]
    pub state: String,
}

/// The login of whoever opened an issue or pull request.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GitHubUser {
    #[serde(default)]
    pub login: String,
}

/// Where an issue or pull request stands on GitHub, as `gh … view --json
/// state` reports it. An issue is never `Merged`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ItemState {
    Open,
    Closed,
    Merged,
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
    /// The issues still blocking this one. A closed blocker has been dealt
    /// with, so it no longer counts.
    fn open_blocker_nodes(&self) -> impl Iterator<Item = &Blocker> {
        self.blocked_by
            .iter()
            .flat_map(|blocked_by| &blocked_by.nodes)
            .filter(|blocker| blocker.state.eq_ignore_ascii_case("open"))
    }

    /// The numbers of the issues still blocking this one.
    pub fn open_blockers(&self) -> Vec<i64> {
        self.open_blocker_nodes()
            .map(|blocker| blocker.number)
            .collect()
    }

    /// Whether at least one open issue blocks this one.
    pub fn is_blocked(&self) -> bool {
        self.open_blocker_nodes().next().is_some()
    }

    /// The parent as a reference from this issue: `#12` in the same
    /// repository, `owner/name#12` in another.
    pub fn parent_reference(&self) -> Option<String> {
        let parent = self.parent.as_ref()?;
        let theirs = issue_repository(&parent.url);
        if theirs.is_some() && theirs == issue_repository(&self.url) {
            return Some(format!("#{}", parent.number));
        }
        let repo = theirs
            .and_then(|repo| repo.strip_prefix("https://github.com/"))
            .unwrap_or_default();
        Some(format!("{repo}#{}", parent.number))
    }

    /// The parent a sub-issue can be started from: an open one in this
    /// issue's own repository. A closed parent's work has landed, and one
    /// elsewhere has no branch here to stem from.
    pub fn open_parent(&self) -> Option<&ParentIssue> {
        let parent = self.parent.as_ref()?;
        let here = issue_repository(&self.url)?;
        (parent.state.eq_ignore_ascii_case("open") && issue_repository(&parent.url) == Some(here))
            .then_some(parent)
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
