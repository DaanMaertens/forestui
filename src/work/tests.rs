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

/// An issue picked without the mode step starts in the list it would
/// have been picked from.
#[test]
fn an_issue_starts_in_the_mode_that_lists_it() {
    let issues = vec![labelled(1, &["bug"]), labelled(2, &["bug", "Enhancement"])];
    for issue in &issues {
        let mode = mode_for(issue, DEFAULT_FEATURE_LABELS);
        let listed = issues_for(mode, &issues, DEFAULT_FEATURE_LABELS);
        assert!(
            numbers(&listed).contains(&issue.number),
            "#{}",
            issue.number
        );
    }
    assert_eq!(
        mode_for(&issues[0], DEFAULT_FEATURE_LABELS),
        WorkMode::Issue
    );
    assert_eq!(
        mode_for(&issues[1], DEFAULT_FEATURE_LABELS),
        WorkMode::Feature
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
    assert_eq!(
        IssueEntry::new(issue, Some("issue-42-x".into())).search_text(),
        "#42 Fix login bug",
        "the linked worktree is a marker, not search text"
    );
}
