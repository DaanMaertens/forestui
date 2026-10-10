//! A searchable list: a query field, the entries matching it, and a selected
//! row. Shared by every dialog that picks one entry from a long list.

use crate::fuzzy::{self, Searchable};
use crate::ui::widgets::TextInput;
use crate::util;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

#[derive(Debug)]
pub struct SearchPicker<T> {
    items: Vec<T>,
    query: TextInput,
    /// Positions into `items`, best first. Recomputed whenever the query
    /// changes, so it can never disagree with it.
    matches: Vec<usize>,
    /// Position into `matches`, not into `items`.
    selected_index: usize,
}

impl<T: Searchable> SearchPicker<T> {
    pub fn new(items: Vec<T>, placeholder: &str) -> Self {
        let mut picker = Self {
            items,
            query: TextInput::new("").with_placeholder(placeholder),
            matches: Vec::new(),
            selected_index: 0,
        };
        picker.refresh();
        picker
    }

    /// Apply a text-editing key to the query. Returns whether it was used.
    pub fn edit_query(&mut self, key: KeyEvent) -> bool {
        let used = self.query.apply_edit_key(key);
        if used {
            self.refresh();
        }
        used
    }

    /// Move the selection with Up/Down, stopping at either end. Both keys are
    /// used even when the selection cannot move, so they never fall through
    /// to the dialog's focus ring.
    pub fn move_selection(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Up => {
                self.selected_index = self.selected_index.saturating_sub(1);
                true
            }
            KeyCode::Down => {
                let last = self.matches.len().saturating_sub(1);
                self.selected_index = (self.selected_index + 1).min(last);
                true
            }
            _ => false,
        }
    }

    /// Select a clicked row. The click was resolved against the last drawn
    /// frame, so the list may have shrunk since: clamp, and ignore a click
    /// on an empty list.
    pub fn select_row(&mut self, row: usize) {
        if let Some(last) = self.matches.len().checked_sub(1) {
            self.selected_index = row.min(last);
        }
    }

    /// Replace the query text, e.g. with an entry the dialog has picked.
    pub fn set_query(&mut self, text: &str) {
        self.query.set_value(text);
        self.refresh();
    }

    pub fn query(&self) -> &TextInput {
        &self.query
    }

    /// The entries matching the query, best first.
    pub fn matches(&self) -> impl Iterator<Item = &T> {
        self.matches.iter().map(|&i| &self.items[i])
    }

    /// Position of the highlighted entry among `matches`, for drawing.
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Every entry, matching or not: what an empty query lists.
    pub fn entry_count(&self) -> usize {
        self.items.len()
    }

    /// The highlighted entry, or `None` when nothing matches.
    pub fn selected(&self) -> Option<&T> {
        let position = self.matches.get(self.selected_index)?;
        Some(&self.items[*position])
    }

    /// Match again and put the selection back on the best match.
    fn refresh(&mut self) {
        self.matches =
            fuzzy::fuzzy_match(self.query.value(), &self.items, util::MAX_DROPDOWN_RESULTS)
                .into_iter()
                .map(|(i, _score)| i)
                .collect();
        self.selected_index = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzzy::{BranchEntry, Searchable};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

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

    /// A dialog that writes the query itself (Add Worktree commits a picked
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
}
