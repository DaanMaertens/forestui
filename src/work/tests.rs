use super::*;
use crate::models::{IssueFilter, Naming, StartWorkSettings};

fn defaults() -> StartWorkSettings {
    StartWorkSettings::default()
}

fn default_labels() -> Vec<String> {
    defaults().feature_labels
}

#[test]
fn an_issue_is_named_issue_number_slug_on_a_fix_branch() {
    assert_eq!(
        issue_names(&defaults(), IssueKind::Issue, 42, "Fix login bug!"),
        WorktreeNames {
            worktree: "issue-42-fix-login-bug".into(),
            branch: "fix/issue-42-fix-login-bug".into(),
        }
    );
}

#[test]
fn a_feature_issue_is_named_feat_on_a_feat_branch() {
    assert_eq!(
        issue_names(&defaults(), IssueKind::Feature, 7, "Dark mode"),
        WorktreeNames {
            worktree: "feat-7-dark-mode".into(),
            branch: "feat/feat-7-dark-mode".into(),
        }
    );
}

#[test]
fn a_title_with_nothing_sluggable_names_by_number_alone() {
    assert_eq!(
        issue_names(&defaults(), IssueKind::Issue, 5, "!!! ???").worktree,
        "issue-5"
    );
    assert_eq!(
        issue_names(&defaults(), IssueKind::Issue, 5, "").branch,
        "fix/issue-5"
    );
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

/// A PR someone else opened is reviewed and reported on; one I opened
/// gets the self-review. GitHub logins ignore case, so the comparison does.
#[test]
fn the_review_kind_follows_authorship() {
    assert_eq!(review_kind("alice", Some("daan")), ReviewKind::Others);
    assert_eq!(review_kind("daan", Some("daan")), ReviewKind::Own);
    assert_eq!(
        review_kind("DaanMaertens", Some("daanmaertens")),
        ReviewKind::Own
    );
}

/// Without knowing who `gh` is logged in as, every PR is someone else's:
/// that review changes no code, so a wrong guess costs nothing.
#[test]
fn an_unknown_viewer_reviews_without_changing_code() {
    assert_eq!(review_kind("daan", None), ReviewKind::Others);
    assert_eq!(review_kind("", Some("")), ReviewKind::Others);
}

#[test]
fn a_pull_request_is_named_pr_number_slug() {
    assert_eq!(
        pull_request_names(&defaults(), 31, "Give the cache tests their own cache").worktree,
        "pr-31-give-the-cache-tests-their-own-cache"
    );
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

fn kinds(issues: &[GitHubIssue], labels: &[impl AsRef<str>]) -> Vec<IssueKind> {
    issues.iter().map(|i| issue_kind(i, labels)).collect()
}

/// A feature label makes an issue a feature, and wins over anything else
/// it carries: a `bug` + `enhancement` issue starts as a feature.
#[test]
fn a_feature_label_makes_an_issue_a_feature() {
    use IssueKind::{Feature, Issue};
    let issues = vec![
        labelled(1, &[]),
        labelled(2, &["enhancement"]),
        labelled(3, &["bug"]),
        labelled(4, &["bug", "enhancement"]),
        labelled(5, &["Enhancement"]),
    ];
    assert_eq!(
        kinds(&issues, &default_labels()),
        vec![Issue, Feature, Issue, Feature, Feature]
    );
}

/// The mode step offers the kinds of work, with no separate `feature`:
/// every issue is under `issue`.
#[test]
fn the_mode_step_lists_issue_session_pr_review_worktree() {
    let labels: Vec<_> = WorkMode::ALL.iter().map(|m| m.label()).collect();
    assert_eq!(labels, vec!["issue", "session", "pr-review", "worktree"]);
}

/// `issue` lists every issue, feature or not, in its own order.
#[test]
fn the_issue_list_holds_every_issue() {
    let issues = vec![
        labelled(1, &["bug"]),
        labelled(2, &["enhancement"]),
        labelled(3, &[]),
    ];
    let entries = issue_entries(&issues, IssueFilter::All, &defaults(), false, |_| None);
    assert_eq!(entry_numbers(&entries), vec![1, 2, 3]);
}

fn pull_request(review: ReviewKind) -> WorkItem {
    WorkItem::PullRequest {
        number: 31,
        title: "x".into(),
        author: "alice".into(),
        head_branch: "fix/x".into(),
        from_fork: false,
        review,
    }
}

#[test]
fn each_work_item_fills_its_own_template() {
    let issue = |kind| WorkItem::Issue {
        number: 7,
        title: "x".into(),
        kind,
    };
    assert_eq!(issue(IssueKind::Issue).template(), Template::Issue);
    assert_eq!(issue(IssueKind::Feature).template(), Template::Feature);
    let session = WorkItem::Session { prompt: "x".into() };
    assert_eq!(session.template(), Template::Session);
    assert_eq!(
        pull_request(ReviewKind::Others).template(),
        Template::PrReviewOthers
    );
    assert_eq!(
        pull_request(ReviewKind::Own).template(),
        Template::PrReviewOwn
    );
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

/// A session is named from its prompt's first words alone: there is no
/// number to put in front of them.
#[test]
fn a_session_is_named_from_its_first_words() {
    assert_eq!(
        session_names(
            &defaults(),
            "Try a dark theme for the sidebar and every modal"
        ),
        Some(WorktreeNames {
            worktree: "session-try-a-dark-theme-for-the".into(),
            branch: "session/session-try-a-dark-theme-for-the".into(),
        })
    );
    assert_eq!(
        session_names(&defaults(), "  Spike:   GraphQL?  ").map(|n| n.worktree),
        Some("session-spike-graphql".into())
    );
}

/// A prompt with nothing sluggable names nothing, rather than a bare
/// `session-` that every such prompt would share.
#[test]
fn a_prompt_with_nothing_sluggable_has_no_names() {
    assert_eq!(session_names(&defaults(), ""), None);
    assert_eq!(session_names(&defaults(), "!!! ???"), None);
}

#[test]
fn long_first_words_are_still_capped() {
    let names = session_names(&defaults(), &"abcdefghij ".repeat(6)).unwrap();
    assert!(names.worktree.len() <= "session-".len() + 40, "{names:?}");
    assert!(!names.worktree.ends_with('-'), "{names:?}");
}

#[test]
fn a_copy_takes_the_first_free_suffix_from_two() {
    let names = session_names(&defaults(), "Dark mode").unwrap();
    let copy = first_free_copy(&names, |_| false);
    assert_eq!(copy.worktree, "session-dark-mode-2");
    assert_eq!(copy.branch, "session/session-dark-mode-2");

    let copy = first_free_copy(&names, |n| n.worktree == "session-dark-mode-2");
    assert_eq!(copy.worktree, "session-dark-mode-3");
    assert_eq!(copy.branch, "session/session-dark-mode-3");
}

fn worktree(name: &str, branch: &str) -> crate::models::Worktree {
    crate::models::Worktree::new(name.into(), branch.into(), format!("/forest/{name}"))
}

/// Taken means a worktree already has the name or the branch: either one
/// would make git refuse the create.
#[test]
fn names_clash_on_the_worktree_name_or_the_branch() {
    let names = session_names(&defaults(), "Dark mode").unwrap();
    let by_name = [worktree("session-dark-mode", "elsewhere")];
    let by_branch = [worktree("renamed", "session/session-dark-mode")];
    let neither = [worktree(
        "session-dark-mode-2",
        "session/session-dark-mode-2",
    )];
    assert_eq!(
        clash(&names, &by_name).map(|w| w.name.as_str()),
        Some("session-dark-mode")
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
        WorkItem::Session {
            prompt: "Dark mode".into()
        }
        .link(),
        crate::models::WorkItemLink::Session {
            prompt: "Dark mode".into()
        }
    );
    assert_eq!(
        WorkItem::Issue {
            number: 7,
            title: "x".into(),
            kind: IssueKind::Feature,
        }
        .link(),
        crate::models::WorkItemLink::Issue { number: 7 }
    );
    assert_eq!(
        pull_request(ReviewKind::Own).link(),
        crate::models::WorkItemLink::PullRequest { number: 31 }
    );
}

fn open_pr(number: i64, title: &str, draft: bool) -> crate::models::GitHubPullRequest {
    serde_json::from_value(serde_json::json!({
        "number": number,
        "title": title,
        "author": {"login": "alice"},
        "isDraft": draft,
        "headRefName": "fix/x",
        "createdAt": "2026-01-01T00:00:00Z",
    }))
    .unwrap()
}

/// The `pr-review` list holds the PRs, searched by number and title, each
/// with the worktree it was started in.
#[test]
fn the_pr_review_list_holds_the_pull_requests() {
    let prs = vec![
        open_pr(30, "Fix the flaky test", true),
        open_pr(31, "Cache", false),
    ];
    let entries = pull_request_entries(&prs, |n| (n == 31).then(|| "pr-31-cache".to_string()));
    let rows: Vec<_> = entries
        .iter()
        .map(|e| match e {
            ItemEntry::PullRequest(entry) => (
                entry.search_text().to_string(),
                entry.pr.is_draft,
                entry.worktree.clone(),
            ),
            other => panic!("only PRs expected, found {other:?}"),
        })
        .collect();
    assert_eq!(
        rows,
        vec![
            ("#30 Fix the flaky test".to_string(), true, None),
            (
                "#31 Cache".to_string(),
                false,
                Some("pr-31-cache".to_string())
            ),
        ]
    );
}

/// The configured prefix and pattern name the worktree and branch, per kind.
#[test]
fn naming_follows_the_configured_prefix_and_pattern() {
    let mut settings = defaults();
    settings.issue = Naming {
        branch_prefix: "bug/".into(),
        name_pattern: "{number}_{slug}".into(),
    };
    settings.feature.name_pattern = "feature-{slug}-{number}".into();
    settings.pr_review.name_pattern = "review-{number}".into();
    assert_eq!(
        issue_names(&settings, IssueKind::Issue, 42, "Fix login bug"),
        WorktreeNames {
            worktree: "42_fix-login-bug".into(),
            branch: "bug/42_fix-login-bug".into(),
        }
    );
    assert_eq!(
        issue_names(&settings, IssueKind::Feature, 7, "Dark mode"),
        WorktreeNames {
            worktree: "feature-dark-mode-7".into(),
            branch: "feat/feature-dark-mode-7".into(),
        }
    );
    assert_eq!(
        pull_request_names(&settings, 31, "Anything").worktree,
        "review-31"
    );
}

/// A session has its own naming, and no number to fill in.
#[test]
fn a_session_follows_its_own_naming() {
    let mut settings = defaults();
    settings.session = Naming {
        branch_prefix: "spike/".into(),
        name_pattern: "try-{slug}".into(),
    };
    assert_eq!(
        session_names(&settings, "Dark mode"),
        Some(WorktreeNames {
            worktree: "try-dark-mode".into(),
            branch: "spike/try-dark-mode".into(),
        })
    );
    settings.session.name_pattern = "idea-{number}-{slug}".into();
    assert_eq!(
        session_names(&settings, "Dark mode").map(|n| n.worktree),
        Some("idea-dark-mode".into())
    );
}

/// A pattern whose fill leaves a separator dangling, or holds something a
/// directory name should not, still produces a clean name.
#[test]
fn a_pattern_fills_to_a_clean_name() {
    let mut settings = defaults();
    settings.issue.name_pattern = "{slug}--{number} / x?".into();
    assert_eq!(
        issue_names(&settings, IssueKind::Issue, 5, "").worktree,
        "5-x"
    );
    // A pattern with nothing to fill still names the worktree by number.
    settings.issue.name_pattern = "???".into();
    assert_eq!(
        issue_names(&settings, IssueKind::Issue, 5, "Title").worktree,
        "5"
    );
}

/// Changing the feature labels changes how an issue starts, and the labels
/// still match case-insensitively.
#[test]
fn the_configured_feature_labels_decide_the_kind() {
    use IssueKind::{Feature, Issue};
    let issues = vec![
        labelled(1, &["enhancement"]),
        labelled(2, &["Feature"]),
        labelled(3, &["type: feature", "bug"]),
        labelled(4, &[]),
    ];
    let labels = vec!["feature".to_string(), "type: feature".to_string()];
    assert_eq!(
        kinds(&issues, &labels),
        vec![Issue, Feature, Feature, Issue]
    );
    let none: Vec<String> = Vec::new();
    assert_eq!(kinds(&issues, &none), vec![Issue; 4]);
}

/// `labelled`, blocked by an open #`by` — and by a closed #99, which does
/// not count.
fn blocked(number: i64, labels: &[&str], by: &[i64]) -> GitHubIssue {
    let mut issue = labelled(number, labels);
    let mut nodes: Vec<_> = by
        .iter()
        .map(|n| serde_json::json!({"number": n, "state": "OPEN"}))
        .collect();
    nodes.push(serde_json::json!({"number": 99, "state": "CLOSED"}));
    issue.blocked_by = serde_json::from_value(serde_json::json!({ "nodes": nodes })).unwrap();
    issue
}

fn entry_numbers(entries: &[ItemEntry]) -> Vec<i64> {
    entries
        .iter()
        .filter_map(|e| match e {
            ItemEntry::Issue(entry) => Some(entry.issue.number),
            _ => None,
        })
        .collect()
}

#[test]
fn blocked_issues_are_listed_only_when_shown() {
    let issues = vec![
        labelled(1, &[]),
        blocked(2, &[], &[9]),
        blocked(3, &["enhancement"], &[9]),
        blocked(4, &[], &[]),
    ];
    let hidden = issue_entries(&issues, IssueFilter::All, &defaults(), false, |_| None);
    assert_eq!(
        entry_numbers(&hidden),
        vec![1, 4],
        "a closed blocker blocks nothing"
    );
    let shown = issue_entries(&issues, IssueFilter::All, &defaults(), true, |_| None);
    assert_eq!(entry_numbers(&shown), vec![1, 2, 3, 4]);
}

#[test]
fn the_hidden_count_is_of_the_blocked_issues_a_list_leaves_out() {
    let issues = vec![
        labelled(1, &[]),
        blocked(2, &[], &[9]),
        blocked(3, &[], &[9, 10]),
        blocked(4, &["enhancement"], &[9]),
    ];
    assert_eq!(
        hidden_blocked(&issues, IssueFilter::All, &defaults(), false),
        3
    );
    assert_eq!(
        hidden_blocked(&issues, IssueFilter::All, &defaults(), true),
        0,
        "nothing is hidden while shown"
    );
    assert_eq!(hidden_blocked_text(2).as_deref(), Some("2 blocked hidden"));
    assert_eq!(hidden_blocked_text(0), None);
}

#[test]
fn the_blocked_marker_names_the_open_blockers() {
    assert_eq!(
        blocked_marker(&blocked(2, &[], &[9, 10])).as_deref(),
        Some("blocked by #9, #10")
    );
    assert_eq!(blocked_marker(&blocked(2, &[], &[])), None);
    assert_eq!(blocked_marker(&labelled(2, &[])), None);
    // A long list stays short enough to sit beside the worktree marker.
    assert_eq!(
        blocked_marker(&blocked(2, &[], &[1, 2, 3, 4, 5])).as_deref(),
        Some("blocked by #1, #2, #3 +2")
    );
}

fn issue(number: i64) -> WorkItemLink {
    WorkItemLink::Issue { number }
}

fn pr(number: i64) -> WorkItemLink {
    WorkItemLink::PullRequest { number }
}

fn session() -> WorkItemLink {
    WorkItemLink::Session {
        prompt: "try a thing".to_string(),
    }
}

#[test]
fn an_issue_is_finished_once_closed() {
    assert_eq!(
        is_finished(&issue(1), Observed::State(ItemState::Open)),
        Some(false)
    );
    assert_eq!(
        is_finished(&issue(1), Observed::State(ItemState::Closed)),
        Some(true)
    );
}

#[test]
fn a_pull_request_is_finished_once_merged_or_closed() {
    assert_eq!(
        is_finished(&pr(2), Observed::State(ItemState::Open)),
        Some(false)
    );
    assert_eq!(
        is_finished(&pr(2), Observed::State(ItemState::Merged)),
        Some(true)
    );
    assert_eq!(
        is_finished(&pr(2), Observed::State(ItemState::Closed)),
        Some(true)
    );
}

#[test]
fn a_session_is_finished_once_its_own_commits_are_merged() {
    let merged = Observed::Branch {
        merged: true,
        has_commits: true,
    };
    assert_eq!(is_finished(&session(), merged), Some(true));
    let unmerged = Observed::Branch {
        merged: false,
        has_commits: true,
    };
    assert_eq!(is_finished(&session(), unmerged), Some(false));
}

/// A branch fresh off the default branch is trivially "merged" into it —
/// it has nothing of its own yet, which is the opposite of finished.
#[test]
fn a_session_branch_with_no_commits_of_its_own_is_not_finished() {
    let fresh = Observed::Branch {
        merged: true,
        has_commits: false,
    };
    assert_eq!(is_finished(&session(), fresh), Some(false));
}

#[test]
fn an_observation_of_the_wrong_kind_decides_nothing() {
    let branch = Observed::Branch {
        merged: true,
        has_commits: true,
    };
    assert_eq!(is_finished(&issue(1), branch), None);
    assert_eq!(is_finished(&pr(2), branch), None);
    assert_eq!(
        is_finished(&session(), Observed::State(ItemState::Closed)),
        None
    );
}

fn in_view(issues: &[GitHubIssue], filter: IssueFilter, settings: &StartWorkSettings) -> Vec<i64> {
    issues
        .iter()
        .filter(|issue| in_filter(issue, filter, settings))
        .map(|issue| issue.number)
        .collect()
}

/// Features carry a feature label, bugs a bug label, others neither; an
/// issue with both is under Features and under Bugs, and All is every issue.
#[test]
fn the_issue_filter_sorts_issues_by_their_labels() {
    let issues = vec![
        labelled(1, &[]),
        labelled(2, &["enhancement"]),
        labelled(3, &["bug"]),
        labelled(4, &["bug", "enhancement"]),
        labelled(5, &["documentation"]),
    ];
    let settings = defaults();
    assert_eq!(
        in_view(&issues, IssueFilter::All, &settings),
        vec![1, 2, 3, 4, 5]
    );
    assert_eq!(
        in_view(&issues, IssueFilter::Features, &settings),
        vec![2, 4]
    );
    assert_eq!(in_view(&issues, IssueFilter::Bugs, &settings), vec![3, 4]);
    assert_eq!(in_view(&issues, IssueFilter::Other, &settings), vec![1, 5]);
}

/// Labels are matched as GitHub matches them, ignoring case, and the bug
/// labels are the setting's, not a fixed `bug`.
#[test]
fn the_issue_filter_reads_the_label_settings_ignoring_case() {
    let issues = vec![
        labelled(1, &["Bug"]),
        labelled(2, &["kind/defect"]),
        labelled(3, &["KIND/FEATURE"]),
    ];
    assert_eq!(in_view(&issues, IssueFilter::Bugs, &defaults()), vec![1]);
    let settings = StartWorkSettings {
        feature_labels: vec!["kind/feature".into()],
        bug_labels: vec!["kind/defect".into()],
        ..defaults()
    };
    assert_eq!(in_view(&issues, IssueFilter::Bugs, &settings), vec![2]);
    assert_eq!(in_view(&issues, IssueFilter::Features, &settings), vec![3]);
    assert_eq!(in_view(&issues, IssueFilter::Other, &settings), vec![1]);
}

/// Enter cycles All → Features → Bugs → Other and round again.
#[test]
fn the_issue_filter_cycles_through_its_values() {
    let mut filter = IssueFilter::default();
    let mut seen = Vec::new();
    for _ in 0..5 {
        seen.push(filter.label());
        filter = filter.next();
    }
    assert_eq!(seen, vec!["All", "Features", "Bugs", "Other", "All"]);
}

/// The filter applies before blocked-hiding: the list holds the filter's
/// issues, and the hidden count is of the filter's blocked ones only.
#[test]
fn the_issue_filter_applies_before_blocked_hiding() {
    let issues = vec![
        labelled(1, &["bug"]),
        blocked(2, &["bug"], &[9]),
        blocked(3, &["enhancement"], &[9]),
        labelled(4, &[]),
    ];
    let settings = defaults();
    let entries = issue_entries(&issues, IssueFilter::Bugs, &settings, false, |_| None);
    assert_eq!(entry_numbers(&entries), vec![1]);
    assert_eq!(
        hidden_blocked(&issues, IssueFilter::Bugs, &settings, false),
        1
    );
    assert_eq!(
        hidden_blocked(&issues, IssueFilter::All, &settings, false),
        2
    );
    let entries = issue_entries(&issues, IssueFilter::Bugs, &settings, true, |_| None);
    assert_eq!(entry_numbers(&entries), vec![1, 2]);
}

/// A name suggestion's reply, named like a session: its first non-empty
/// line, slugged and capped, through the `session` naming.
#[test]
fn a_clean_suggestion_is_named_through_the_session_naming() {
    assert_eq!(
        suggested_session_names(&defaults(), "sidebar-scroll-perf\n"),
        Some(WorktreeNames {
            worktree: "session-sidebar-scroll-perf".into(),
            branch: "session/session-sidebar-scroll-perf".into(),
        })
    );
    let settings = StartWorkSettings {
        session: Naming {
            branch_prefix: "spike/".into(),
            name_pattern: "s-{slug}".into(),
        },
        ..defaults()
    };
    assert_eq!(
        suggested_session_names(&settings, "dark-mode"),
        Some(WorktreeNames {
            worktree: "s-dark-mode".into(),
            branch: "spike/s-dark-mode".into(),
        })
    );
}

/// Quotes, backticks, punctuation and capitals are cleaned off: a model
/// asked for a bare name does not always give one.
#[test]
fn a_suggestion_is_cleaned_to_directory_safe_characters() {
    for reply in [
        "\"sidebar-scroll-perf\"",
        "`sidebar-scroll-perf`",
        "Sidebar Scroll Perf.",
        "  **sidebar_scroll/perf**  ",
    ] {
        assert_eq!(
            suggested_session_names(&defaults(), reply).map(|n| n.worktree),
            Some("session-sidebar-scroll-perf".into()),
            "{reply:?}"
        );
    }
}

/// Only the first non-empty line is the name; whatever prose follows it is
/// not.
#[test]
fn a_suggestion_is_its_first_non_empty_line() {
    let reply = "\n\n  dark-mode-toggle\nThis name says what the session is about.\n";
    assert_eq!(
        suggested_session_names(&defaults(), reply).map(|n| n.worktree),
        Some("session-dark-mode-toggle".into())
    );
}

#[test]
fn a_long_suggestion_is_capped_like_a_title() {
    let names = suggested_session_names(&defaults(), &"abcdefghij-".repeat(8)).unwrap();
    assert!(names.worktree.len() <= "session-".len() + 40, "{names:?}");
    assert!(!names.worktree.ends_with('-'), "{names:?}");
}

/// A reply with nothing a name can be made of suggests nothing, and the
/// first-words names stay.
#[test]
fn a_suggestion_that_cleans_to_nothing_names_nothing() {
    for reply in ["", "  \n \n", "\"\"", "!!! ???\nsidebar-perf"] {
        assert_eq!(
            suggested_session_names(&defaults(), reply),
            None,
            "{reply:?}"
        );
    }
}
