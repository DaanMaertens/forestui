//! Start work's pure rules: the work modes, how a work item names its
//! worktree and branch, and the entry a work item is searched by.

use crate::fuzzy::Searchable;
use crate::models::GitHubIssue;
use crate::prompt;

/// The longest slug a title contributes to a name, so a long title cannot
/// produce a directory or branch name nobody wants to type.
const SLUG_LIMIT: usize = 40;

/// The labels that make an issue a feature, until they become a setting.
pub const DEFAULT_FEATURE_LABELS: &[&str] = &["enhancement"];

/// The kind of work about to start, which decides what the item list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkMode {
    Issue,
    Feature,
    PrReview,
}

impl WorkMode {
    /// Every mode, in the order the mode step lists them.
    pub const ALL: [WorkMode; 3] = [WorkMode::Issue, WorkMode::Feature, WorkMode::PrReview];

    pub fn label(self) -> &'static str {
        match self {
            WorkMode::Issue => "issue",
            WorkMode::Feature => "feature",
            WorkMode::PrReview => "pr-review",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            WorkMode::Issue => "Fix a bug or chore from an open issue",
            WorkMode::Feature => "Build a feature from a labelled issue",
            WorkMode::PrReview => "Review a pull request",
        }
    }

    /// Whether the mode can be picked yet. Listing the others greyed out
    /// shows where the flow is going without offering what does not exist.
    pub fn is_enabled(self) -> bool {
        matches!(self, WorkMode::Issue | WorkMode::Feature)
    }

    /// The built-in template the mode's first prompt is filled from.
    /// `pr-review` cannot be picked yet and has no template of its own.
    pub fn template(self) -> &'static str {
        match self {
            WorkMode::Issue | WorkMode::PrReview => prompt::ISSUE_TEMPLATE,
            WorkMode::Feature => prompt::FEATURE_TEMPLATE,
        }
    }
}

/// Whether an issue carries one of the feature labels. GitHub treats label
/// names case-insensitively, so `Enhancement` counts as `enhancement`.
pub fn is_feature(issue: &GitHubIssue, feature_labels: &[&str]) -> bool {
    issue.labels.iter().any(|label| {
        feature_labels
            .iter()
            .any(|wanted| label.name.eq_ignore_ascii_case(wanted))
    })
}

/// The mode an issue belongs to: `feature` if it carries a feature label —
/// which wins over anything else it carries, so a `bug` + `enhancement`
/// issue is a feature — and `issue` otherwise. An issue card starts in it
/// directly, and [`issues_for`] lists by it, so the two never disagree.
pub fn mode_for(issue: &GitHubIssue, feature_labels: &[&str]) -> WorkMode {
    if is_feature(issue, feature_labels) {
        WorkMode::Feature
    } else {
        WorkMode::Issue
    }
}

/// The issues a mode lists. Every issue lands in exactly one of `issue` and
/// `feature`; `pr-review` lists no issues.
pub fn issues_for(
    mode: WorkMode,
    issues: &[GitHubIssue],
    feature_labels: &[&str],
) -> Vec<GitHubIssue> {
    issues
        .iter()
        .filter(|issue| mode_for(issue, feature_labels) == mode)
        .cloned()
        .collect()
}

/// The worktree and branch a work item starts in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeNames {
    pub worktree: String,
    pub branch: String,
}

/// Name the worktree and branch for a numbered work item.
///
/// The worktree carries the mode, the number and a slug of the title, so the
/// sidebar shows what kind of work it holds; the branch is the worktree name
/// under the mode's conventional prefix. A `pr-review` worktree checks out
/// the PR's own branch, so only its worktree name is used.
pub fn names(mode: WorkMode, number: i64, title: &str) -> WorktreeNames {
    let (prefix, branch_prefix) = match mode {
        WorkMode::Issue => ("issue", "fix/"),
        WorkMode::Feature => ("feat", "feat/"),
        WorkMode::PrReview => ("pr", ""),
    };
    let slug = title_slug(title);
    let worktree = if slug.is_empty() {
        format!("{prefix}-{number}")
    } else {
        format!("{prefix}-{number}-{slug}")
    };
    WorktreeNames {
        branch: format!("{branch_prefix}{worktree}"),
        worktree,
    }
}

/// A title as a name fragment: lowercase ASCII letters and digits, every other
/// run of characters collapsed to one `-`, capped at [`SLUG_LIMIT`].
pub fn title_slug(title: &str) -> String {
    let slug = crate::util::slugify(title);
    let capped: String = slug.chars().take(SLUG_LIMIT).collect();
    capped.trim_matches('-').to_string()
}

/// An issue as the item picker searches it: by number and title together, so
/// typing either finds it.
#[derive(Debug, Clone)]
pub struct IssueEntry {
    pub issue: GitHubIssue,
    /// The name of the worktree already started for this issue, if any. Shown
    /// as a marker only; it is not searched.
    pub worktree: Option<String>,
    text: String,
}

impl IssueEntry {
    pub fn new(issue: GitHubIssue, worktree: Option<String>) -> Self {
        let text = format!("#{} {}", issue.number, issue.title);
        Self {
            issue,
            worktree,
            text,
        }
    }
}

impl Searchable for IssueEntry {
    fn search_text(&self) -> &str {
        &self.text
    }
}

#[cfg(test)]
mod tests;
