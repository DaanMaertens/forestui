use super::*;
use crate::services::live::{Activity, SessionState};
use uuid::Uuid;

#[test]
fn worktree_rows_show_a_branch_that_differs() {
    let row = SidebarRow::Worktree {
        repo_id: Uuid::new_v4(),
        id: Uuid::new_v4(),
        name: "wt-two".into(),
        branch: Some("release-2".into()),
        is_last: true,
        finished: false,
        activity: None,
    };
    let rendered = format!("{:?}", row_to_item(&row, false));
    // The Textual build lost this to console-markup parsing; assert it survives.
    assert!(rendered.contains("release-2"));
    assert!(rendered.contains("wt-two"));
}

/// The common case: the branch is the name with the prefix, and repeating it
/// only costs the columns the name needs.
#[test]
fn worktree_rows_omit_a_branch_that_only_repeats_the_name() {
    let row = SidebarRow::Worktree {
        repo_id: Uuid::new_v4(),
        id: Uuid::new_v4(),
        name: "wt-two".into(),
        branch: None,
        is_last: true,
        finished: false,
        activity: None,
    };
    let rendered = row_line(&row).to_string();
    assert!(rendered.contains("wt-two"));
    assert!(!rendered.contains('['));
}

#[test]
fn a_finished_worktree_row_leads_with_a_badge() {
    let row = SidebarRow::Worktree {
        repo_id: Uuid::new_v4(),
        id: Uuid::new_v4(),
        name: "wt-two".into(),
        branch: Some("release-2".into()),
        is_last: true,
        finished: true,
        activity: None,
    };
    assert_eq!(row_line(&row).to_string(), "└─ ✓ wt-two [release-2]");
}

fn a_worktree_row(finished: bool, activity: Option<Activity>) -> SidebarRow {
    SidebarRow::Worktree {
        repo_id: Uuid::new_v4(),
        id: Uuid::new_v4(),
        name: "wt-two".into(),
        branch: Some("release-2".into()),
        is_last: false,
        finished,
        activity,
    }
}

fn span_style(line: &Line, text: &str) -> Style {
    line.spans
        .iter()
        .find(|span| span.content == text)
        .unwrap_or_else(|| panic!("no span {text:?} in {line:?}"))
        .style
}

/// The name wears its most urgent session's colour; one session needs no
/// count.
#[test]
fn a_worktree_name_takes_its_sessions_state_colour() {
    let _theme = crate::theme::test_lock();
    for state in [
        SessionState::Idle,
        SessionState::Busy,
        SessionState::NeedsYou,
    ] {
        let line = row_line(&a_worktree_row(false, Some(Activity { state, count: 1 })));
        assert_eq!(line.to_string(), "├─ wt-two [release-2]");
        assert_eq!(span_style(&line, "wt-two"), theme::session_state(state));
    }
    // No counted session: exactly as before.
    let line = row_line(&a_worktree_row(false, None));
    assert_eq!(span_style(&line, "wt-two"), theme::primary());
}

/// From two sessions a count leads the name, after the ✓ — and on a
/// finished worktree the count keeps the state colour while the name keeps
/// the finished one.
#[test]
fn several_sessions_lead_the_name_with_a_count() {
    let _theme = crate::theme::test_lock();
    let busy = Some(Activity {
        state: SessionState::Busy,
        count: 3,
    });
    let line = row_line(&a_worktree_row(false, busy));
    assert_eq!(line.to_string(), "├─ {3} wt-two [release-2]");
    assert_eq!(
        span_style(&line, "{3} "),
        theme::session_state(SessionState::Busy)
    );

    let needs_you = Some(Activity {
        state: SessionState::NeedsYou,
        count: 2,
    });
    let line = row_line(&a_worktree_row(true, needs_you));
    assert_eq!(line.to_string(), "├─ ✓ {2} wt-two [release-2]");
    assert_eq!(
        span_style(&line, "{2} "),
        theme::session_state(SessionState::NeedsYou)
    );
    assert_eq!(span_style(&line, "wt-two"), theme::finished());
}
