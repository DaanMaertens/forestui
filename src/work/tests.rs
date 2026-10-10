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

/// A free-text feature is named from its first words alone: there is no
/// number to put in front of them.
#[test]
fn a_free_text_feature_is_named_from_its_first_words() {
    assert_eq!(
        free_text_names("Try a dark theme for the sidebar and every modal"),
        Some(WorktreeNames {
            worktree: "feat-try-a-dark-theme-for-the".into(),
            branch: "feat/feat-try-a-dark-theme-for-the".into(),
        })
    );
    assert_eq!(
        free_text_names("  Spike:   GraphQL?  ").map(|n| n.worktree),
        Some("feat-spike-graphql".into())
    );
}

/// A description with nothing sluggable names nothing, rather than a bare
/// `feat-` that every such description would share.
#[test]
fn a_description_with_nothing_sluggable_has_no_names() {
    assert_eq!(free_text_names(""), None);
    assert_eq!(free_text_names("!!! ???"), None);
}

#[test]
fn long_first_words_are_still_capped() {
    let names = free_text_names(&"abcdefghij ".repeat(6)).unwrap();
    assert!(names.worktree.len() <= "feat-".len() + 40, "{names:?}");
    assert!(!names.worktree.ends_with('-'), "{names:?}");
}

#[test]
fn a_copy_takes_the_first_free_suffix_from_two() {
    let names = free_text_names("Dark mode").unwrap();
    let copy = first_free_copy(&names, |_| false);
    assert_eq!(copy.worktree, "feat-dark-mode-2");
    assert_eq!(copy.branch, "feat/feat-dark-mode-2");

    let copy = first_free_copy(&names, |n| n.worktree == "feat-dark-mode-2");
    assert_eq!(copy.worktree, "feat-dark-mode-3");
    assert_eq!(copy.branch, "feat/feat-dark-mode-3");
}

fn worktree(name: &str, branch: &str) -> crate::models::Worktree {
    crate::models::Worktree::new(name.into(), branch.into(), format!("/forest/{name}"))
}

/// Taken means a worktree already has the name or the branch: either one
/// would make git refuse the create.
#[test]
fn names_clash_on_the_worktree_name_or_the_branch() {
    let names = free_text_names("Dark mode").unwrap();
    let by_name = [worktree("feat-dark-mode", "elsewhere")];
    let by_branch = [worktree("renamed", "feat/feat-dark-mode")];
    let neither = [worktree("feat-dark-mode-2", "feat/feat-dark-mode-2")];
    assert_eq!(
        clash(&names, &by_name).map(|w| w.name.as_str()),
        Some("feat-dark-mode")
    );
    assert_eq!(
        clash(&names, &by_branch).map(|w| w.name.as_str()),
        Some("renamed")
    );
    assert!(clash(&names, &neither).is_none());
}

#[test]
fn a_work_item_links_to_what_it_was_started_for() {
    assert_eq!(
        WorkItem::FreeText {
            description: "Dark mode".into()
        }
        .link(),
        crate::models::WorkItemLink::FreeText {
            description: "Dark mode".into()
        }
    );
    assert_eq!(
        WorkItem::Issue {
            number: 7,
            title: "x".into()
        }
        .link(),
        crate::models::WorkItemLink::Issue { number: 7 }
    );
}

/// The free-text entry is first, and the issues follow in their own order.
#[test]
fn the_feature_list_leads_with_free_text() {
    let issues = vec![labelled(2, &["enhancement"]), labelled(3, &["enhancement"])];
    let entries = item_entries(WorkMode::Feature, &issues, DEFAULT_FEATURE_LABELS, |_| None);
    assert!(matches!(entries[0], ItemEntry::FreeText));
    let numbers: Vec<_> = entries
        .iter()
        .filter_map(|e| match e {
            ItemEntry::Issue(entry) => Some(entry.issue.number),
            ItemEntry::FreeText => None,
        })
        .collect();
    assert_eq!(numbers, vec![2, 3]);

    let entries = item_entries(WorkMode::Issue, &issues, DEFAULT_FEATURE_LABELS, |_| None);
    assert!(entries.is_empty(), "no free text outside `feature`");
}
