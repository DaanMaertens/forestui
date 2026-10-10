use super::*;
use uuid::Uuid;

#[test]
fn worktree_rows_show_a_branch_that_differs() {
    let row = SidebarRow::Worktree {
        repo_id: Uuid::new_v4(),
        id: Uuid::new_v4(),
        name: "wt-two".into(),
        branch: Some("release-2".into()),
        is_last: true,
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
    };
    let rendered = format!("{:?}", row_to_item(&row, false));
    assert!(rendered.contains("wt-two"));
    assert!(!rendered.contains('['));
}
