use super::*;
use crate::fuzzy::BranchEntry;
use ratatui::crossterm::event::{KeyEventKind, KeyEventState, KeyModifiers};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn branches(names: &[&str]) -> Vec<BranchEntry> {
    names.iter().map(|n| BranchEntry::new(n, &[])).collect()
}

fn match_texts(picker: &SearchPicker<BranchEntry>) -> Vec<&str> {
    picker.matches().map(|b| b.search_text()).collect()
}

fn selected_text(picker: &SearchPicker<BranchEntry>) -> Option<&str> {
    picker.selected().map(|b| b.search_text())
}

#[test]
fn typing_narrows_the_matches_and_selects_the_best() {
    let mut picker = SearchPicker::new(branches(&["main", "feat/login", "fix/bug"]), "");

    for c in "fea".chars() {
        assert!(picker.edit_query(key(KeyCode::Char(c))));
    }

    assert_eq!(picker.query().value(), "fea");
    assert_eq!(match_texts(&picker), vec!["feat/login"]);
    assert_eq!(selected_text(&picker), Some("feat/login"));
}

/// Up and Down are always used, even at either end of the list: the
/// dialog must not see them, or they would move its focus instead.
#[test]
fn up_and_down_move_the_selection_and_stop_at_the_ends() {
    let mut picker = SearchPicker::new(branches(&["main", "feat/login", "fix/bug"]), "");

    assert!(picker.move_selection(key(KeyCode::Down)));
    assert_eq!(selected_text(&picker), Some("feat/login"));

    for _ in 0..3 {
        assert!(picker.move_selection(key(KeyCode::Down)));
    }
    assert_eq!(selected_text(&picker), Some("fix/bug"));

    for _ in 0..3 {
        assert!(picker.move_selection(key(KeyCode::Up)));
    }
    assert_eq!(selected_text(&picker), Some("main"));

    assert!(!picker.move_selection(key(KeyCode::Enter)));
}

/// A click lands on a drawn row, but the list may have shrunk since the
/// frame was drawn: clamp to the last match, and do nothing on an empty
/// list rather than panic.
#[test]
fn a_clicked_row_is_selected_and_clamped_to_the_list() {
    let mut picker = SearchPicker::new(branches(&["main", "feat/login", "fix/bug"]), "");

    picker.select_row(1);
    assert_eq!(selected_text(&picker), Some("feat/login"));

    picker.select_row(99);
    assert_eq!(selected_text(&picker), Some("fix/bug"));

    for c in "zzz".chars() {
        picker.edit_query(key(KeyCode::Char(c)));
    }
    picker.select_row(0);
    assert_eq!(selected_text(&picker), None);
}

/// A dialog that writes the query itself (the worktree form commits a picked
/// branch into the field) gets matches for the new text, not stale ones.
#[test]
fn setting_the_query_matches_again() {
    let mut picker = SearchPicker::new(branches(&["main", "feat/login", "fix/bug"]), "");
    picker.select_row(2);

    picker.set_query("fix/bug");

    assert_eq!(picker.query().value(), "fix/bug");
    assert_eq!(match_texts(&picker), vec!["fix/bug"]);
    assert_eq!(selected_text(&picker), Some("fix/bug"));
}

/// The first match `wanted` accepts is highlighted; with none, the
/// selection stays where it was.
#[test]
fn select_where_highlights_the_first_accepted_match() {
    let mut picker = SearchPicker::new(branches(&["main", "feat/login", "fix/bug"]), "");

    assert!(picker.select_where(|b| b.search_text() == "fix/bug"));
    assert_eq!(selected_text(&picker), Some("fix/bug"));

    assert!(!picker.select_where(|b| b.search_text() == "gone"));
    assert_eq!(selected_text(&picker), Some("fix/bug"));
}
