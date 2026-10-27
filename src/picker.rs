//! A searchable list: a query field, the entries matching it, and a selected
//! row. Shared by every dialog that picks one entry from a long list.

use crate::fuzzy::{self, Searchable};
use crate::ui::widgets::TextInput;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

/// The most entries a picker lists for any one query, unless it is made
/// [`SearchPicker::unlimited`].
const MAX_RESULTS: usize = 50;

#[derive(Debug)]
pub struct SearchPicker<T> {
    items: Vec<T>,
    query: TextInput,
    /// Positions into `items`, best first. Recomputed whenever the query
    /// changes, so it can never disagree with it.
    matches: Vec<usize>,
    /// Position into `matches`, not into `items`.
    selected_index: usize,
    /// The most entries `matches` holds.
    limit: usize,
}

impl<T: Searchable> SearchPicker<T> {
    pub fn new(items: Vec<T>, placeholder: &str) -> Self {
        let mut picker = Self {
            items,
            query: TextInput::new("").with_placeholder(placeholder),
            matches: Vec::new(),
            selected_index: 0,
            limit: MAX_RESULTS,
        };
        picker.refresh();
        picker
    }

    /// List every matching entry rather than the best [`MAX_RESULTS`], for a
    /// list meant to be browsed to its end, not only searched.
    pub fn unlimited(mut self) -> Self {
        self.limit = usize::MAX;
        self.refresh();
        self
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
                self.step(-1);
                true
            }
            KeyCode::Down => {
                self.step(1);
                true
            }
            _ => false,
        }
    }

    /// Move the selection `delta` rows, stopping at either end.
    pub fn step(&mut self, delta: isize) {
        let last = self.matches.len().saturating_sub(1);
        self.selected_index = self.selected_index.saturating_add_signed(delta).min(last);
    }

    /// Select a clicked row. The click was resolved against the last drawn
    /// frame, so the list may have shrunk since: clamp, and ignore a click
    /// on an empty list.
    pub fn select_row(&mut self, row: usize) {
        if let Some(last) = self.matches.len().checked_sub(1) {
            self.selected_index = row.min(last);
        }
    }

    /// Highlight the first match `wanted` accepts, e.g. to keep the user's
    /// place across a reload of the list. Whether one did; with none, the
    /// selection stays put.
    pub fn select_where(&mut self, wanted: impl Fn(&T) -> bool) -> bool {
        let Some(row) = self.matches.iter().position(|&i| wanted(&self.items[i])) else {
            return false;
        };
        self.selected_index = row;
        true
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

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// Position of the highlighted entry among `matches`, for drawing.
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Every entry, matching or not: what an empty query lists.
    pub fn entry_count(&self) -> usize {
        self.items.len()
    }

    /// Every entry, matching or not, in the order given.
    pub fn entries(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }

    /// The highlighted entry, or `None` when nothing matches.
    pub fn selected(&self) -> Option<&T> {
        let position = self.matches.get(self.selected_index)?;
        Some(&self.items[*position])
    }

    /// Match again and put the selection back on the best match.
    fn refresh(&mut self) {
        self.matches = fuzzy::fuzzy_match(self.query.value(), &self.items, self.limit)
            .into_iter()
            .map(|(i, _score)| i)
            .collect();
        self.selected_index = 0;
    }
}

#[cfg(test)]
mod tests;
