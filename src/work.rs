//! Start work's pure rules: the work modes, how a work item names its
//! worktree and branch, and the entry a work item is searched by.

use crate::fuzzy::Searchable;
use crate::models::{GitHubIssue, WorkItemLink, Worktree};
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

/// How many words of a free-text description its names are made from. A
/// description is a sentence or more, not a title, and a name built from
/// all of it would be cut off mid-word by the cap anyway.
const FREE_TEXT_WORDS: usize = 6;

/// Name the worktree and branch for a free-text feature: `feat-<slug>` on
/// `feat/`, from the description's first words. `None` when they hold nothing
/// a name can be made of, so two such descriptions never share a bare `feat-`.
pub fn free_text_names(description: &str) -> Option<WorktreeNames> {
    let first_words: Vec<&str> = description
        .split_whitespace()
        .take(FREE_TEXT_WORDS)
        .collect();
    let slug = title_slug(&first_words.join(" "));
    if slug.is_empty() {
        return None;
    }
    let worktree = format!("feat-{slug}");
    Some(WorktreeNames {
        branch: format!("feat/{worktree}"),
        worktree,
    })
}

/// `names` with `-<n>` added to both, for the first `n` from 2 that `taken`
/// does not refuse.
pub fn first_free_copy(
    names: &WorktreeNames,
    taken: impl Fn(&WorktreeNames) -> bool,
) -> WorktreeNames {
    (2..)
        .map(|n| WorktreeNames {
            worktree: format!("{}-{n}", names.worktree),
            branch: format!("{}-{n}", names.branch),
        })
        .find(|copy| !taken(copy))
        .unwrap_or_else(|| names.clone())
}

/// The worktree that already holds `names`' worktree name or branch — either
/// one would make the create fail.
pub fn clash<'a>(names: &WorktreeNames, worktrees: &'a [Worktree]) -> Option<&'a Worktree> {
    worktrees
        .iter()
        .find(|w| w.name == names.worktree || w.branch == names.branch)
}

/// What a worktree is started for: an issue, or a feature described in the
/// user's own words that exists nowhere but here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkItem {
    Issue { number: i64, title: String },
    FreeText { description: String },
}

impl WorkItem {
    pub fn from_issue(issue: &GitHubIssue) -> Self {
        WorkItem::Issue {
            number: issue.number,
            title: issue.title.clone(),
        }
    }

    /// The record the worktree keeps of it.
    pub fn link(&self) -> WorkItemLink {
        match self {
            WorkItem::Issue { number, .. } => WorkItemLink::Issue { number: *number },
            WorkItem::FreeText { description } => WorkItemLink::FreeText {
                description: description.clone(),
            },
        }
    }
}

/// The label of the free-text entry, which is searched like any other row.
pub const FREE_TEXT_LABEL: &str = "✎ Free-text feature…";

/// One row of the item picker.
#[derive(Debug, Clone)]
pub enum ItemEntry {
    /// Describe a feature that has no issue.
    FreeText,
    Issue(IssueEntry),
}

impl Searchable for ItemEntry {
    fn search_text(&self) -> &str {
        match self {
            ItemEntry::FreeText => FREE_TEXT_LABEL,
            ItemEntry::Issue(entry) => entry.search_text(),
        }
    }
}

/// The rows a mode's picker lists: the issues it lists, each with the
/// worktree `linked` says it was started in, behind the free-text entry in
/// `feature`.
pub fn item_entries(
    mode: WorkMode,
    issues: &[GitHubIssue],
    feature_labels: &[&str],
    linked: impl Fn(i64) -> Option<String>,
) -> Vec<ItemEntry> {
    let free_text = (mode == WorkMode::Feature).then_some(ItemEntry::FreeText);
    free_text
        .into_iter()
        .chain(
            issues_for(mode, issues, feature_labels)
                .into_iter()
                .map(|issue| {
                    let worktree = linked(issue.number);
                    ItemEntry::Issue(IssueEntry::new(issue, worktree))
                }),
        )
        .collect()
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
