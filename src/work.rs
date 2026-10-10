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

/// The issues a mode lists. Every issue lands in exactly one of `issue` and
/// `feature`: a feature label wins over anything else it carries, so a
/// `bug` + `enhancement` issue is a feature. `pr-review` lists no issues.
pub fn issues_for(
    mode: WorkMode,
    issues: &[GitHubIssue],
    feature_labels: &[&str],
) -> Vec<GitHubIssue> {
    let want_feature = match mode {
        WorkMode::Issue => false,
        WorkMode::Feature => true,
        WorkMode::PrReview => return Vec::new(),
    };
    issues
        .iter()
        .filter(|issue| is_feature(issue, feature_labels) == want_feature)
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
    text: String,
}

impl IssueEntry {
    pub fn new(issue: GitHubIssue) -> Self {
        let text = format!("#{} {}", issue.number, issue.title);
        Self { issue, text }
    }
}

impl Searchable for IssueEntry {
    fn search_text(&self) -> &str {
        &self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issue_is_named_issue_number_slug_on_a_fix_branch() {
        assert_eq!(
            names(WorkMode::Issue, 42, "Fix login bug!"),
            WorktreeNames {
                worktree: "issue-42-fix-login-bug".into(),
                branch: "fix/issue-42-fix-login-bug".into(),
            }
        );
    }

    #[test]
    fn a_feature_issue_is_named_feat_on_a_feat_branch() {
        assert_eq!(
            names(WorkMode::Feature, 7, "Dark mode"),
            WorktreeNames {
                worktree: "feat-7-dark-mode".into(),
                branch: "feat/feat-7-dark-mode".into(),
            }
        );
    }

    #[test]
    fn a_title_with_nothing_sluggable_names_by_number_alone() {
        assert_eq!(names(WorkMode::Issue, 5, "!!! ???").worktree, "issue-5");
        assert_eq!(names(WorkMode::Issue, 5, "").branch, "fix/issue-5");
    }

    /// The cap applies to the slug, and a cut that lands on a separator does
    /// not leave a dangling dash.
    #[test]
    fn a_long_title_is_capped_without_a_trailing_dash() {
        let title = format!("{} tail", "a".repeat(39));
        let slug = title_slug(&title);
        assert_eq!(slug, "a".repeat(39));
        assert_eq!(title_slug(&"word ".repeat(20)).chars().count(), 39);
    }

    #[test]
    fn non_ascii_and_punctuation_collapse_to_single_dashes() {
        assert_eq!(title_slug("Café: crash -- on   save"), "caf-crash-on-save");
    }

    #[test]
    fn issue_and_feature_are_enabled_pr_review_not_yet() {
        let enabled: Vec<_> = WorkMode::ALL
            .into_iter()
            .filter(|m| m.is_enabled())
            .collect();
        assert_eq!(enabled, vec![WorkMode::Issue, WorkMode::Feature]);
    }

    fn labelled(number: i64, labels: &[&str]) -> GitHubIssue {
        serde_json::from_value(serde_json::json!({
            "number": number,
            "title": format!("issue {number}"),
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "labels": labels.iter().map(|l| serde_json::json!({"name": l})).collect::<Vec<_>>(),
        }))
        .unwrap()
    }

    fn numbers(issues: &[GitHubIssue]) -> Vec<i64> {
        issues.iter().map(|i| i.number).collect()
    }

    #[test]
    fn feature_labelled_issues_split_from_the_rest() {
        let issues = vec![
            labelled(1, &[]),
            labelled(2, &["enhancement"]),
            labelled(3, &["bug"]),
            labelled(4, &["bug", "enhancement"]),
            labelled(5, &["Enhancement"]),
        ];
        let labels = DEFAULT_FEATURE_LABELS;
        assert_eq!(
            numbers(&issues_for(WorkMode::Issue, &issues, labels)),
            vec![1, 3]
        );
        assert_eq!(
            numbers(&issues_for(WorkMode::Feature, &issues, labels)),
            vec![2, 4, 5]
        );
        assert!(issues_for(WorkMode::PrReview, &issues, labels).is_empty());
    }

    /// Whatever the labels, the two lists partition the issues: none is
    /// dropped and none is listed twice.
    #[test]
    fn every_issue_is_in_exactly_one_list() {
        let issues = vec![
            labelled(1, &["feature"]),
            labelled(2, &["ui", "feature"]),
            labelled(3, &["enhancement"]),
        ];
        let labels = ["feature", "ui"];
        let mut both = numbers(&issues_for(WorkMode::Issue, &issues, &labels));
        both.extend(numbers(&issues_for(WorkMode::Feature, &issues, &labels)));
        both.sort();
        assert_eq!(both, vec![1, 2, 3]);
        assert_eq!(
            numbers(&issues_for(WorkMode::Feature, &issues, &labels)),
            vec![1, 2]
        );
    }

    #[test]
    fn each_mode_fills_its_own_template() {
        assert_eq!(WorkMode::Issue.template(), prompt::ISSUE_TEMPLATE);
        assert_eq!(WorkMode::Feature.template(), prompt::FEATURE_TEMPLATE);
    }

    #[test]
    fn an_issue_entry_is_searched_by_number_and_title() {
        let issue: GitHubIssue = serde_json::from_str(
            r#"{"number":42,"title":"Fix login bug","createdAt":"2026-01-01T00:00:00Z",
                "updatedAt":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(IssueEntry::new(issue).search_text(), "#42 Fix login bug");
    }
}
