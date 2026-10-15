//! Modal overlays: one centred box per open dialog.
//!
//! `src/modal.rs` owns the state and the keys; this file draws, and records
//! where every control landed so a click can be routed back to it. Controls are
//! laid out in focus-index order so the picture and the key handling cannot
//! drift apart — both sides name the same `FOCUS_*` constants on the modal
//! struct rather than repeating a number, so there is one place to change.

use crate::app::{App, Direction, HitTarget, ModalClick};
use crate::fuzzy::{self, Searchable};
use crate::modal::{
    AddRepositoryModal, BackKeyModal, BackKeyStep, ClaudeIntegrationModal, ConfirmModal,
    CustomButtonsModal, EDITORS, EditButtonModal, Modal, PromptTemplatesModal, RenameSessionModal,
    SPINNER, SettingsModal, StartWorkConfirmModal, StartWorkModal, StartWorkStep, ThemePickerModal,
    WorktreeForm,
};
use crate::models::{IssueFilter, Scope};
use crate::picker::SearchPicker;
use crate::services::tmux::back_key;
use crate::theme;
use crate::ui::widgets::{self, BUTTON_HEIGHT, TextInput};
use crate::util::{self, Noun};
use crate::work::{self, ItemEntry, WorkItem, WorkMode};
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Width of a normal modal — Textual's `.modal-container { width: 80 }`.
const WIDTH: u16 = 80;
/// Width of the two wide modals — Textual's `.modal-wide { width: 140 }`.
const WIDE: u16 = 140;
/// Branch-dropdown rows kept on screen — Textual's `OptionList { max-height: 10 }`.
const DROPDOWN_ROWS: usize = 10;
/// Rows the custom-button list may occupy before it starts scrolling.
const BUTTON_LIST_ROWS: usize = 15;
/// Rows one custom-button block takes: label, prefix, command.
const BUTTON_BLOCK: usize = 3;
/// Space between two controls sharing a row. Textual gave every button
/// `margin: 0 1` and then collapsed the pair, so neighbours sit one cell apart.
const GAP: u16 = 1;
/// Rows the box spends on chrome before any content: two borders, the two
/// `padding: 1 2` rows, the title, and `.modal-title { margin-bottom: 1 }`.
const CHROME: u16 = 6;

/// Where the controls of one modal landed: a rectangle and the focus index a
/// click inside it selects. The indices are the modal's own `FOCUS_*` constants,
/// which `Modal::set_focus` and `handle_key` also use — anything else would
/// focus one control and act on another.
pub type Hits = Vec<(Rect, usize, ModalClick)>;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    // Only the top of the stack is drawn: Settings → Custom Buttons → Edit
    // Button nests, and a half-covered parent behind the child is just noise.
    // Only the top is clickable either, for the same reason.
    let hovered = match app.hovered {
        Some(HitTarget::ModalControl { index, .. }) => Some(index),
        _ => None,
    };
    let hits = match app.modals.last() {
        Some(modal) => HOVERED.with(|cell| {
            cell.set(hovered);
            let hits = render_modal(frame, modal, area);
            cell.set(None);
            hits
        }),
        None => return,
    };
    for (rect, index, click) in hits {
        app.push_hit(rect, HitTarget::ModalControl { index, click });
    }
}

/// Draw one modal and report where its controls landed. Split out from [`draw`]
/// so tests can render a modal without standing up a whole [`App`].
pub fn render_modal(frame: &mut Frame, modal: &Modal, area: Rect) -> Hits {
    let mut hits = Hits::new();
    match modal {
        Modal::AddRepository(m) => add_repository(frame, m, area, &mut hits),
        Modal::StartWork(m) => start_work(frame, m, area, &mut hits),
        Modal::StartWorkConfirm(m) => start_work_confirm(frame, m, area, &mut hits),
        Modal::Settings(m) => settings(frame, m, area, &mut hits),
        Modal::CustomButtons(m) => custom_buttons(frame, m, area, &mut hits),
        Modal::EditButton(m) => edit_button(frame, m, area, &mut hits),
        Modal::ThemePicker(m) => theme_picker(frame, m, area, &mut hits),
        Modal::PromptTemplates(m) => prompt_templates(frame, m, area, &mut hits),
        Modal::ClaudeIntegration(m) => claude_integration(frame, m, area, &mut hits),
        Modal::Confirm(m) => confirm(frame, m, area, &mut hits),
        Modal::RenameSession(m) => rename_session(frame, m, area, &mut hits),
        Modal::BackKey(m) => back_key(frame, m, area, &mut hits),
    }
    hits
}

// ------------------------------------------------------------------- Layout

/// A top-down cursor over a modal's inner area.
///
/// Modals are a vertical stack of one- and three-row pieces, so a running `y`
/// beats a `Layout` with a constraint per element — and it puts the overflow
/// clamp in one place: once the box is full, the rest simply does not draw.
struct Column {
    area: Rect,
    y: u16,
}

impl Column {
    fn new(area: Rect) -> Self {
        Self { area, y: area.y }
    }

    fn remaining(&self) -> u16 {
        (self.area.y + self.area.height).saturating_sub(self.y)
    }

    /// Reserve `height` rows, or `None` once they no longer fit.
    fn take(&mut self, height: u16) -> Option<Rect> {
        if height == 0 || self.remaining() < height {
            return None;
        }
        let rect = Rect {
            x: self.area.x,
            y: self.y,
            width: self.area.width,
            height,
        };
        self.y += height;
        Some(rect)
    }

    fn gap(&mut self) {
        let _ = self.take(1);
    }

    fn line(&mut self, frame: &mut Frame, line: Line) {
        if let Some(rect) = self.take(1) {
            frame.render_widget(Paragraph::new(line), rect);
        }
    }

    /// A line whose whole row carries `style` — used for list selection, where
    /// the highlight has to run to the edge of the box. Every list is pickable,
    /// so the whole row is the click target for `index`; `click` says whether
    /// it only selects the row or picks it.
    fn row(
        &mut self,
        frame: &mut Frame,
        hits: &mut Hits,
        line: Line,
        style: Style,
        index: usize,
        click: ModalClick,
    ) {
        if let Some(rect) = self.take(1) {
            frame.render_widget(Paragraph::new(line).style(style), rect);
            hits.push((rect, index, click));
        }
    }

    fn text(&mut self, frame: &mut Frame, text: impl Into<String>, style: Style) {
        self.line(frame, Line::from(Span::styled(text.into(), style)));
    }

    /// Text inputs draw their own border, so they are all-or-nothing.
    fn input(
        &mut self,
        frame: &mut Frame,
        hits: &mut Hits,
        input: &TextInput,
        focus: usize,
        index: usize,
    ) {
        if let Some(rect) = self.take(3) {
            input.render(frame, rect, focus == index);
            // Focus only: activating here would submit the whole dialog.
            hits.push((rect, index, ModalClick::Focus));
        }
    }

    /// Draw a row of controls left to right, recording where each one landed.
    /// `centre` groups them in the middle of the box, the way Textual's
    /// `.modal-buttons { align: center middle }` did with the dialog's buttons.
    fn controls(
        &mut self,
        frame: &mut Frame,
        hits: &mut Hits,
        controls: Vec<Control>,
        centre: bool,
    ) {
        // Boxing the buttons made every dialog several rows taller, and a
        // dialog is capped at the height of the terminal. Rather than let the
        // button row fall off the bottom — leaving a dialog that can only be
        // answered from the keyboard, with nothing on screen saying so — pin it
        // to the last three rows. The body above it loses those rows instead,
        // which is at least visible.
        let rect = match self.take(BUTTON_HEIGHT) {
            Some(rect) => rect,
            None => {
                let y = (self.area.y + self.area.height).saturating_sub(BUTTON_HEIGHT);
                self.y = self.area.y + self.area.height;
                let pinned = Rect {
                    x: self.area.x,
                    y,
                    width: self.area.width,
                    height: BUTTON_HEIGHT.min(self.area.height),
                };
                // Every row that overflows lands on this same strip, so an
                // earlier one is painted over by whatever comes after it.
                // Its hit regions have to go with it: a control nobody can see
                // must not answer a click, and the blank margin either side of
                // a centred button row is exactly where those stale regions
                // would still be sitting.
                hits.retain(|(recorded, _, _)| !recorded.intersects(pinned));
                pinned
            }
        };
        let total = controls
            .iter()
            .map(|(_, width, _, _)| width.saturating_add(GAP))
            .sum::<u16>()
            .saturating_sub(GAP);
        let indent = if centre {
            rect.width.saturating_sub(total) / 2
        } else {
            0
        };
        let right = rect.x + rect.width;
        let mut rows: [Vec<Span<'static>>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let mut x = rect.x.saturating_add(indent);
        for row in &mut rows {
            row.push(Span::raw(" ".repeat(indent as usize)));
        }
        for (control, width, index, click) in controls {
            // A control the box is too narrow to show must not be clickable.
            let visible = width.min(right.saturating_sub(x));
            if visible > 0 {
                hits.push((
                    Rect {
                        x,
                        y: rect.y,
                        width: visible,
                        height: BUTTON_HEIGHT,
                    },
                    index,
                    click,
                ));
            }
            for (row, spans) in rows.iter_mut().zip(control) {
                row.extend(spans);
                row.push(Span::raw(" ".repeat(GAP as usize)));
            }
            x = x.saturating_add(width).saturating_add(GAP);
        }
        frame.render_widget(Paragraph::new(rows.map(Line::from).to_vec()), rect);
    }
}

/// Draw the dialog box and return a column over its content.
///
/// Textual gave `.modal-container` `padding: 1 2` and put the title in a label
/// on the first content row — never in the border — followed by the blank row
/// `.modal-title { margin-bottom: 1 }` left behind. The leading space is the
/// one the Python labels carried literally (`Label(" Add Repository")`).
fn dialog(frame: &mut Frame, rect: Rect, title: &str, style: Style) -> Column {
    // `.modal-container { background: $bg-elevated; border: solid $border }` —
    // the box is raised off the page and its border is the resting colour, not
    // the focus accent. A dialog is always the focused thing, so an accent
    // border says nothing and competes with the button that does have one.
    let inner = widgets::framed(frame, rect, "").inner(Margin::new(2, 1));
    let mut column = Column::new(inner);
    column.text(frame, format!(" {title}"), style);
    column.gap();
    column
}

// ------------------------------------------------------------------ Controls

/// One piece of a control row: its three rows of spans, how wide it renders,
/// the focus index a click selects, and what that click does. Every piece is
/// the same height so a row of them lines up.
type Control = ([Vec<Span<'static>>; 3], u16, usize, ModalClick);

/// A button, drawn as the bordered box Textual drew.
fn control(label: &str, focused: bool, variant: theme::Variant, index: usize) -> Control {
    boxed(label, focused, variant, true, index)
}

/// The greyed-out form of [`control`], for a button Textual would `disable`.
/// Built from the same box so neither the layout nor the click region moves
/// when the button becomes enabled.
fn disabled(label: &str, index: usize) -> Control {
    boxed(label, false, theme::Variant::Normal, false, index)
}

thread_local! {
    /// Focus index the pointer is over, for the duration of one `draw`.
    ///
    /// Every control already resolves its own `focused` from its index, and
    /// hover is the same shape. Carrying it here rather than through all
    /// nineteen call sites keeps them readable; it is set and cleared inside a
    /// single synchronous `draw`, so nothing observes it between frames.
    static HOVERED: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// The shared body of [`control`] and [`disabled`]: the box, styled the way
/// `ui/detail.rs` styles its controls, so both panes read as one app.
fn boxed(
    label: &str,
    focused: bool,
    variant: theme::Variant,
    enabled: bool,
    index: usize,
) -> Control {
    // A disabled control must not light up under the pointer: an affordance
    // promising a click it will not honour is worse than none.
    let hovered = enabled && HOVERED.with(|cell| cell.get()) == Some(index);
    let fill = theme::action_bg(variant, hovered);
    let border = theme::action_border(focused, variant, hovered);
    // A control that cannot run keeps its box, so neither the layout nor the
    // hit region shifts; only the label goes grey — except under the cursor,
    // which has to stay legible.
    let text = if enabled || focused {
        theme::action(focused, variant, hovered)
    } else {
        theme::muted().bg(fill)
    };
    (
        widgets::button_box(label, border.bg(fill), text),
        widgets::button_box_width(label),
        index,
        ModalClick::Activate,
    )
}

fn checkbox(label: &str, checked: bool, focused: bool, index: usize) -> Control {
    let mark = if checked { 'x' } else { ' ' };
    control(
        &format!("[{mark}] {label}"),
        focused,
        theme::Variant::Normal,
        index,
    )
}

/// A value Left/Right cycles through — the stand-in for Textual's `Select`.
fn cycle(value: &str, focused: bool, index: usize) -> Control {
    let (spans, width, index, _) = control(
        &format!("◂ {value} ▸"),
        focused,
        theme::Variant::Normal,
        index,
    );
    (spans, width, index, ModalClick::Cycle(Direction::Next))
}

/// One segment of a two-option row, which selects itself rather than toggling
/// the row. `direction` is the key that lands on this option: the pair reads as
/// Left for the first and Right for the second, both idempotent.
fn option(label: &str, active: bool, index: usize, direction: Direction) -> Control {
    let (spans, width, index, _) = control(label, active, theme::Variant::Normal, index);
    (spans, width, index, ModalClick::Cycle(direction))
}

/// A bare arrow beside a control, standing for the key that drives it. Clicking
/// one does what that key does, so it carries the control's index too. It is one
/// cell wide on all three rows, so the buttons after it stay aligned.
fn arrow(glyph: char, direction: Direction, focused: bool, index: usize) -> Control {
    let style = if focused {
        theme::accent()
    } else {
        theme::muted()
    };
    // The arrows stand for Left/Right, so clicking one cycles the value.
    (
        [
            vec![Span::raw(" ")],
            vec![Span::styled(glyph.to_string(), style)],
            vec![Span::raw(" ")],
        ],
        1,
        index,
        ModalClick::Cycle(direction),
    )
}

// ----------------------------------------------------------- Add repository

fn add_repository(frame: &mut Frame, modal: &AddRepositoryModal, area: Rect, hits: &mut Hits) {
    // Label, input, status, gap, buttons. No import checkbox: existing
    // worktrees are adopted by the reconcile scan, not a one-shot choice.
    let rect = widgets::centered_rect(WIDTH, CHROME + 9, area);
    let mut column = dialog(frame, rect, "Add Repository", theme::title());

    column.line(frame, widgets::section("Repository Path"));
    column.input(
        frame,
        hits,
        &modal.path,
        modal.focus,
        AddRepositoryModal::FOCUS_PATH,
    );

    let (status, valid) = modal.status();
    let style = if !valid && !status.is_empty() {
        theme::destructive()
    } else {
        theme::secondary()
    };
    column.text(frame, status, style);
    column.gap();
    column.controls(
        frame,
        hits,
        vec![
            control(
                "Add Repository",
                modal.focus == AddRepositoryModal::FOCUS_ADD,
                theme::Variant::Primary,
                AddRepositoryModal::FOCUS_ADD,
            ),
            control(
                "Cancel",
                modal.focus == AddRepositoryModal::FOCUS_CANCEL,
                theme::Variant::Normal,
                AddRepositoryModal::FOCUS_CANCEL,
            ),
        ],
        true,
    );
}

/// A list's place in its dialog's focus ring. The list is one focus stop, so
/// every row carries the same `index`.
#[derive(Clone, Copy)]
struct ListFocus {
    index: usize,
    focused: bool,
}

impl ListFocus {
    /// The list at `index`, while the dialog's focus is on `current`.
    fn of(index: usize, current: usize) -> Self {
        Self {
            index,
            focused: index == current,
        }
    }
}

/// A picker's visible rows, scrolled to keep the selection in view. `click`
/// says what clicking a row does, and `line` draws one entry given the query,
/// so each list decides how a row behaves and looks.
#[allow(clippy::too_many_arguments)]
fn picker_results<T: Searchable>(
    frame: &mut Frame,
    column: &mut Column,
    hits: &mut Hits,
    picker: &SearchPicker<T>,
    visible: usize,
    focus: ListFocus,
    click: fn(usize) -> ModalClick,
    line: impl Fn(&T, &str, Style) -> Line<'static>,
) {
    let query = picker.query().value();
    let selected_index = picker.selected_index();
    let first = selected_index.saturating_sub(visible.saturating_sub(1));
    for (offset, entry) in picker.matches().skip(first).take(visible).enumerate() {
        let selected = first + offset == selected_index;
        let style = match (selected, focus.focused) {
            (true, true) => theme::cursor(),
            (true, false) => theme::cursor_unfocused(),
            (false, _) => theme::primary(),
        };
        column.row(
            frame,
            hits,
            line(entry, query, style),
            style,
            focus.index,
            click(first + offset),
        );
    }
}

/// A picker's count line, worded as `branch_search.py` worded it except that
/// the noun agrees with the count, where Python said "1 branches".
fn count_line<T: Searchable>(picker: &SearchPicker<T>, noun: Noun) -> String {
    count_text(
        is_filtered(picker),
        picker.match_count(),
        picker.entry_count(),
        noun,
    )
}

fn is_filtered<T: Searchable>(picker: &SearchPicker<T>) -> bool {
    !picker.query().value().trim().is_empty()
}

fn count_text(filtered: bool, shown: usize, total: usize, noun: Noun) -> String {
    if !filtered {
        // "N of M" reads as a count of M, so the noun agrees with M.
        let noun = noun.of(total);
        if shown < total {
            format!("{shown} of {total} {noun}")
        } else {
            format!("{total} {noun}")
        }
    } else if shown == 0 {
        "No matches".to_string()
    } else {
        Noun::new("match", "matches").count(shown)
    }
}

/// One dropdown row with the literal query run highlighted, as Rich did.
fn branch_line(branch: &str, query: &str, style: Style) -> Line<'static> {
    let mut spans = vec![Span::styled(" ", style)];
    // The offsets come from a lowercased copy, so they can land mid-character
    // for exotic casing; fall back to a plain row rather than panicking.
    match fuzzy::highlight_range(query, branch)
        .filter(|(start, end)| branch.is_char_boundary(*start) && branch.is_char_boundary(*end))
    {
        Some((start, end)) => {
            spans.push(Span::styled(branch[..start].to_string(), style));
            spans.push(Span::styled(
                branch[start..end].to_string(),
                style.add_modifier(Modifier::BOLD | Modifier::REVERSED),
            ));
            spans.push(Span::styled(branch[end..].to_string(), style));
        }
        None => spans.push(Span::styled(branch.to_string(), style)),
    }
    Line::from(spans)
}

// --------------------------------------------------------------- Start work

/// Height of Start work's item step, kept the same whatever the list holds
/// so the box does not jump as the query narrows it.
const START_WORK_HEIGHT: u16 = CHROME + 11 + DROPDOWN_ROWS as u16;

fn start_work(frame: &mut Frame, modal: &StartWorkModal, area: Rect, hits: &mut Hits) {
    match modal.step {
        StartWorkStep::Mode => start_work_mode(frame, modal, area, hits),
        StartWorkStep::Item => start_work_item(frame, modal, area, hits),
        StartWorkStep::Prompt => start_work_prompt(frame, modal, area, hits),
        StartWorkStep::Clash => start_work_clash(frame, modal, area, hits),
        StartWorkStep::Worktree => start_work_worktree(frame, modal, area, hits),
    }
}

/// The `worktree` mode's step: the form once the branches have loaded, a
/// loading line with Back and Cancel until then.
fn start_work_worktree(frame: &mut Frame, modal: &StartWorkModal, area: Rect, hits: &mut Hits) {
    let title = format!("Start Work: {}", WorkMode::Worktree.label());
    let Some(form) = modal.worktree.as_deref() else {
        // Repo, gap, loading, gap, buttons.
        let rect = widgets::centered_rect(WIDTH, CHROME + 4 + BUTTON_HEIGHT, area);
        let mut column = dialog(frame, rect, &title, theme::title());
        column.text(frame, format!("in {}", modal.repo_name), theme::secondary());
        column.gap();
        column.text(frame, " Loading branches...", theme::muted());
        column.gap();
        column.controls(frame, hits, back_and_cancel(modal).to_vec(), true);
        return;
    };
    worktree_form(frame, form, &title, area, hits);
}

fn worktree_form(
    frame: &mut Frame,
    modal: &WorktreeForm,
    title: &str,
    area: Rect,
    hits: &mut Hits,
) {
    // The dropdown only exists in existing-branch mode: a count line plus rows.
    let dropdown = if modal.new_branch {
        0
    } else {
        modal.picker.match_count().min(DROPDOWN_ROWS) + 1
    };
    // A new branch's base: label, input, hint.
    let base = if modal.new_branch { 5 } else { 0 };
    // Repo, label, input, preview, label, toggle, input, error, gap, buttons.
    let rect = widgets::centered_rect(WIDTH, CHROME + 18 + base + dropdown as u16, area);
    let mut column = dialog(frame, rect, title, theme::title());

    column.text(frame, format!("in {}", modal.repo_name), theme::secondary());
    column.line(frame, widgets::section("Worktree Name"));
    column.input(
        frame,
        hits,
        &modal.name,
        modal.focus,
        WorktreeForm::FOCUS_NAME,
    );

    let preview = modal
        .path_preview()
        .map(|path| format!(" {}", path.display()))
        .unwrap_or_default();
    column.text(frame, preview, theme::muted());

    column.line(frame, widgets::section("Branch"));
    // Left/Right switch modes here, so the arrows are literal: they light up
    // only while the toggle holds the focus. Each option *selects* itself
    // rather than toggling the row — Left is "New Branch" and Right is
    // "Existing" whichever is current, so clicking the option that is already
    // active does nothing instead of switching away from it.
    let mode = WorktreeForm::FOCUS_MODE;
    let toggle_focused = modal.focus == mode;
    column.controls(
        frame,
        hits,
        vec![
            arrow('◂', Direction::Previous, toggle_focused, mode),
            option("New Branch", modal.new_branch, mode, Direction::Previous),
            option("Existing", !modal.new_branch, mode, Direction::Next),
            arrow('▸', Direction::Next, toggle_focused, mode),
        ],
        false,
    );

    // One input slot either way; only which TextInput fills it depends on
    // the mode.
    let branch_input = if modal.new_branch {
        &modal.branch
    } else {
        modal.picker.query()
    };
    column.input(
        frame,
        hits,
        branch_input,
        modal.focus,
        WorktreeForm::FOCUS_BRANCH,
    );
    if modal.new_branch {
        column.line(frame, widgets::section("Base Branch"));
        column.input(
            frame,
            hits,
            &modal.base_branch,
            modal.focus,
            WorktreeForm::FOCUS_BASE,
        );
        let (hint, style) = base_hint(
            modal.base_suggestion(),
            modal.base_branch.value(),
            modal.missing_base(),
        );
        column.text(frame, hint, style);
    } else {
        column.text(
            frame,
            count_line(&modal.picker, Noun::new("branch", "branches")),
            theme::muted(),
        );

        // Leave room for the error line, the gap and the buttons below the list.
        let space = column.remaining().saturating_sub(2 + BUTTON_HEIGHT) as usize;
        picker_results(
            frame,
            &mut column,
            hits,
            &modal.picker,
            DROPDOWN_ROWS.min(space),
            ListFocus::of(WorktreeForm::FOCUS_RESULTS, modal.focus),
            ModalClick::Row,
            |branch, query, style| branch_line(branch.search_text(), query, style),
        );
    }

    column.text(frame, modal.error.clone(), theme::destructive());
    column.gap();

    // Create, Back and Cancel close the ring, so their indices follow the mode.
    let create = modal.create_index();
    let back = modal.back_index();
    let cancel = modal.cancel_index();
    let create_control = if modal.can_create() {
        control(
            "Create Worktree",
            modal.focus == create,
            theme::Variant::Primary,
            create,
        )
    } else {
        disabled("Create Worktree", create)
    };
    column.controls(
        frame,
        hits,
        vec![
            create_control,
            control("Back", modal.focus == back, theme::Variant::Normal, back),
            control(
                "Cancel",
                modal.focus == cancel,
                theme::Variant::Normal,
                cancel,
            ),
        ],
        true,
    );
}

/// The Back and Cancel that close every Start work step after the first.
fn back_and_cancel(modal: &StartWorkModal) -> [Control; 2] {
    [
        control(
            "Back",
            modal.focus == StartWorkModal::FOCUS_BACK,
            theme::Variant::Normal,
            StartWorkModal::FOCUS_BACK,
        ),
        control(
            "Cancel",
            modal.focus == StartWorkModal::FOCUS_CANCEL,
            theme::Variant::Normal,
            StartWorkModal::FOCUS_CANCEL,
        ),
    ]
}

fn start_work_mode(frame: &mut Frame, modal: &StartWorkModal, area: Rect, hits: &mut Hits) {
    // Repo, label, a row per mode, gap, hint, buttons.
    let rows = WorkMode::ALL.len() as u16;
    let rect = widgets::centered_rect(WIDTH, CHROME + 7 + rows, area);
    let mut column = dialog(frame, rect, "Start Work", theme::title());

    column.text(frame, format!("in {}", modal.repo_name), theme::secondary());
    column.line(frame, widgets::section("Work Mode"));
    let focused = modal.focus == StartWorkModal::FOCUS_LIST;
    for (row, mode) in WorkMode::ALL.into_iter().enumerate() {
        let enabled = modal.mode_enabled(mode);
        let style = if !enabled {
            theme::muted()
        } else if mode == modal.mode {
            if focused {
                theme::cursor()
            } else {
                theme::cursor_unfocused()
            }
        } else {
            theme::primary()
        };
        let mut text = format!(" {:<11}{}", mode.label(), mode.description());
        if !enabled {
            text.push_str("  (needs GitHub)");
        }
        column.row(
            frame,
            hits,
            Line::from(Span::styled(text, style)),
            style,
            StartWorkModal::FOCUS_LIST,
            ModalClick::Pick(row),
        );
    }
    column.gap();
    column.text(frame, "Enter chooses · Esc closes", theme::muted());
    column.controls(
        frame,
        hits,
        vec![control(
            "Cancel",
            modal.focus == StartWorkModal::FOCUS_CANCEL,
            theme::Variant::Normal,
            StartWorkModal::FOCUS_CANCEL,
        )],
        true,
    );
}

/// What the `issue` list says it holds under `filter`: the line it shows
/// when empty, and the noun it counts with. A list narrowed by its labels
/// says so, so a missing issue is read as filtered out rather than absent.
fn issue_words(filter: IssueFilter) -> (&'static str, Noun) {
    match filter {
        IssueFilter::All => ("No open issues", Noun::new("issue", "issues")),
        IssueFilter::Features => (
            "No open feature issues",
            Noun::new("feature issue", "feature issues"),
        ),
        IssueFilter::Bugs => ("No open bug issues", Noun::new("bug issue", "bug issues")),
        IssueFilter::Other => (
            "No other open issues",
            Noun::new("other issue", "other issues"),
        ),
    }
}

fn start_work_item(frame: &mut Frame, modal: &StartWorkModal, area: Rect, hits: &mut Hits) {
    // The issue filter takes a row of its own: beside the checkboxes, Back
    // and Cancel it would not fit the dialog's width.
    let filter_row = modal.offers_issue_toggles();
    let controls_height = BUTTON_HEIGHT * if filter_row { 2 } else { 1 };
    let rect = widgets::centered_rect(
        WIDTH,
        START_WORK_HEIGHT - BUTTON_HEIGHT + controls_height,
        area,
    );
    let title = format!("Start Work: {}", modal.mode.label());
    let mut column = dialog(frame, rect, &title, theme::title());

    column.text(frame, format!("in {}", modal.repo_name), theme::secondary());
    let (section, loading, empty, noun, items) = match modal.mode {
        // `session` has a prompt and `worktree` a form, not an item list,
        // so neither lands here.
        WorkMode::Issue | WorkMode::Session | WorkMode::Worktree => {
            let (empty, noun) = issue_words(modal.filter);
            ("Issue", " Loading issues...", empty, noun, "issues")
        }
        WorkMode::PrReview => (
            "Pull Request",
            " Loading pull requests...",
            "No open PRs",
            Noun::new("PR", "PRs"),
            "PRs",
        ),
    };
    // The empty line names the scope it searched, so "nothing for me" is
    // not read as "nothing at all".
    let mine = match (modal.scope, modal.mode) {
        (Scope::All, _) => "",
        (Scope::Mine, WorkMode::PrReview) => " awaiting your review or opened by you",
        (Scope::Mine, _) => " assigned to or opened by you",
    };
    // A list that could not be fetched is not an empty one: it says why
    // instead, so a failure never reads as "you have none".
    let (empty, empty_style) = match modal.list_error() {
        // The whole fetch failed, whatever the filter: "issues", not
        // "feature issues".
        Some(error) => (error.message(items), theme::warning()),
        _ => (format!("{empty}{mine}"), theme::muted()),
    };
    column.line(frame, widgets::section(section));
    match &modal.picker {
        None => {
            column.gap();
            column.text(frame, loading, theme::muted());
            column.gap();
        }
        Some(picker) => {
            column.input(
                frame,
                hits,
                picker.query(),
                modal.focus,
                StartWorkModal::FOCUS_LIST,
            );
            let (mut count, count_style) = if picker.entry_count() == 0 {
                (empty, empty_style)
            } else {
                (count_line(picker, noun), theme::muted())
            };
            // Said beside the count, so an issue the list leaves out is
            // accounted for rather than missing.
            if let Some(hidden) = work::hidden_blocked_text(modal.hidden_blocked()) {
                count.push_str(&format!(" · {hidden}"));
            }
            column.text(frame, count, count_style);
            // Leave room for the gap, the hint and the buttons below the list.
            let space = column.remaining().saturating_sub(2 + controls_height) as usize;
            picker_results(
                frame,
                &mut column,
                hits,
                picker,
                DROPDOWN_ROWS.min(space),
                ListFocus::of(StartWorkModal::FOCUS_LIST, modal.focus),
                ModalClick::Pick,
                |entry, query, style| {
                    let mut line = branch_line(entry.search_text(), query, style);
                    // Markers take accent or muted on a plain row; the
                    // highlighted row keeps its own colours, where either on
                    // the accent-dark cursor would fade.
                    let marker = |plain: Style| if style.bg.is_some() { style } else { plain };
                    if let ItemEntry::PullRequest(entry) = entry
                        && entry.pr.is_draft
                    {
                        line.spans
                            .push(Span::styled("  draft", marker(theme::muted())));
                    }
                    if let ItemEntry::Issue(entry) = entry
                        && let Some(blocked) = work::blocked_marker(&entry.issue)
                    {
                        line.spans.push(Span::styled(
                            format!("  {blocked}"),
                            marker(theme::warning()),
                        ));
                    }
                    // Picking a marked item jumps to its worktree rather than
                    // creating one, so the row says where it will land.
                    if let Some(worktree) = entry.worktree() {
                        line.spans.push(Span::styled(
                            format!("  → {worktree}"),
                            marker(theme::accent()),
                        ));
                    }
                    line
                },
            );
        }
    }
    // The hint and the buttons sit at the bottom however long the list is.
    let bottom = 1 + controls_height;
    while column.remaining() > bottom {
        column.gap();
    }
    column.text(frame, "Enter picks · Esc goes back", theme::muted());
    if filter_row {
        column.controls(
            frame,
            hits,
            vec![control(
                &format!("Filter: {}", modal.filter.label()),
                modal.focus == StartWorkModal::FOCUS_FILTER,
                theme::Variant::Normal,
                StartWorkModal::FOCUS_FILTER,
            )],
            true,
        );
    }
    let mut controls = Vec::new();
    if modal.offers_issue_toggles() {
        controls.push(checkbox(
            "Show blocked",
            modal.show_blocked,
            modal.focus == StartWorkModal::FOCUS_BLOCKED,
            StartWorkModal::FOCUS_BLOCKED,
        ));
    }
    if modal.offers_scope_toggle() {
        controls.push(checkbox(
            "Not just mine",
            modal.scope == Scope::All,
            modal.focus == StartWorkModal::FOCUS_SCOPE,
            StartWorkModal::FOCUS_SCOPE,
        ));
    }
    controls.extend(back_and_cancel(modal));
    column.controls(frame, hits, controls, true);
}

const SESSION_TITLE: &str = "Start Work: Session";

fn start_work_prompt(frame: &mut Frame, modal: &StartWorkModal, area: Rect, hits: &mut Hits) {
    // Repo, label, input, names, local note, gap, hint, buttons.
    let rect = widgets::centered_rect(WIDTH, CHROME + 12, area);
    let mut column = dialog(frame, rect, SESSION_TITLE, theme::title());

    column.text(frame, format!("in {}", modal.repo_name), theme::secondary());
    column.line(frame, widgets::section("Prompt"));
    column.input(
        frame,
        hits,
        &modal.prompt,
        modal.focus,
        StartWorkModal::FOCUS_LIST,
    );
    // What Enter would create, so the names are no surprise on the confirm
    // screen — and why Enter does nothing while there are none.
    let names = match work::session_names(&modal.start_work, modal.prompt.value()) {
        Some(names) => format!("Worktree: {}", names.worktree),
        None => "Type a few words to name the worktree after".to_string(),
    };
    column.text(frame, names, theme::muted());
    column.text(
        frame,
        "Stays local: nothing is created on GitHub.",
        theme::muted(),
    );
    column.gap();
    column.text(frame, "Enter continues · Esc goes back", theme::muted());
    column.controls(frame, hits, back_and_cancel(modal).to_vec(), true);
}

fn start_work_clash(frame: &mut Frame, modal: &StartWorkModal, area: Rect, hits: &mut Hits) {
    let Some(clash) = &modal.clash else {
        return;
    };
    // Repo, question, two lines of choices explained, gap, buttons.
    let rect = widgets::centered_rect(WIDTH, CHROME + 9, area);
    let mut column = dialog(frame, rect, SESSION_TITLE, theme::title());

    column.text(frame, format!("in {}", modal.repo_name), theme::secondary());
    column.text(
        frame,
        format!("A worktree named '{}' already exists.", clash.worktree_name),
        theme::primary(),
    );
    column.gap();
    column.text(
        frame,
        format!("Open it, create {} instead,", clash.copy.worktree),
        theme::muted(),
    );
    column.text(frame, "or go back and edit the name.", theme::muted());
    column.controls(
        frame,
        hits,
        vec![
            control(
                "Open existing",
                modal.focus == StartWorkModal::FOCUS_OPEN,
                theme::Variant::Primary,
                StartWorkModal::FOCUS_OPEN,
            ),
            control(
                "Create copy",
                modal.focus == StartWorkModal::FOCUS_COPY,
                theme::Variant::Normal,
                StartWorkModal::FOCUS_COPY,
            ),
            control(
                "Edit name",
                modal.focus == StartWorkModal::FOCUS_EDIT,
                theme::Variant::Normal,
                StartWorkModal::FOCUS_EDIT,
            ),
        ],
        true,
    );
}

/// The fewest rows the prompt preview keeps on a short terminal, where the
/// rest of the screen is already over budget.
const MIN_PROMPT_ROWS: u16 = 3;
/// Everything on the confirm screen but the prompt's own lines: issue, three
/// labelled fields with a hint each, fetch and pull on one row, the prompt's
/// label, gap, buttons.
const CONFIRM_BODY: u16 = CHROME + 23;
/// The same for a PR review: the PR, the review kind with its line of
/// explanation, the name field and its path, the branch `gh pr checkout`
/// will check out, the prompt's label, gap, buttons.
const REVIEW_CONFIRM_BODY: u16 = CHROME + 16;

fn start_work_confirm(
    frame: &mut Frame,
    modal: &StartWorkConfirmModal,
    area: Rect,
    hits: &mut Hits,
) {
    // Inside the border and padding, less the one-space indent of each line.
    let width = WIDTH.saturating_sub(7) as usize;
    // As much of the prompt as the terminal has room for, so a tall one shows
    // it whole; a longer prompt ends in `…`. The 95% is `centered_rect`'s cap.
    let body = if modal.is_review() {
        REVIEW_CONFIRM_BODY
    } else {
        CONFIRM_BODY
    };
    let room = (area.height.saturating_mul(95) / 100)
        .saturating_sub(body)
        .max(MIN_PROMPT_ROWS) as usize;
    let mut prompt: Vec<String> = modal
        .prompt()
        .lines()
        .flat_map(|line| {
            let wrapped = super::wrap_words(line, width);
            // A blank line separates paragraphs; wrapping drops it.
            if wrapped.is_empty() {
                vec![String::new()]
            } else {
                wrapped
            }
        })
        .collect();
    if prompt.len() > room {
        prompt.truncate(room);
        if let Some(last) = prompt.last_mut() {
            *last = last.chars().take(width.saturating_sub(2)).collect();
            last.push_str(" …");
        }
    }

    let rect = widgets::centered_rect(WIDTH, body + prompt.len() as u16, area);
    let (title, subtitle) = match &modal.item {
        WorkItem::Issue {
            number,
            title,
            kind,
        } => (
            format!("Start Work: {} #{number}", kind.label()),
            title.to_string(),
        ),
        WorkItem::PullRequest {
            number,
            title,
            author,
            ..
        } => (
            format!("Start Work: PR #{number}"),
            format!("{title}  (by @{author})"),
        ),
        WorkItem::Session { prompt } => (SESSION_TITLE.to_string(), prompt.clone()),
    };
    let mut column = dialog(frame, rect, &title, theme::title());

    column.text(frame, subtitle, theme::muted());

    // Settled by who opened the PR, so it is stated, not offered.
    if let WorkItem::PullRequest { review, .. } = &modal.item {
        column.line(frame, widgets::section("Review Kind"));
        column.text(frame, format!(" {}", review.label()), theme::primary());
        column.text(frame, format!(" {}", review.description()), theme::muted());
    }

    // On the header rather than a row of its own, so the screen keeps its
    // height when the suggestion lands.
    let header = if modal.name_request.is_some() {
        let spinner = SPINNER[modal.spinner_index % SPINNER.len()];
        format!("Worktree Name  {spinner} suggesting a name…")
    } else {
        "Worktree Name".to_string()
    };
    column.line(frame, widgets::section(&header));
    column.input(
        frame,
        hits,
        &modal.name,
        modal.focus,
        StartWorkConfirmModal::FOCUS_NAME,
    );
    column.text(
        frame,
        format!("Path: {}", modal.path_preview().display()),
        theme::muted(),
    );

    if modal.is_review() {
        // `gh pr checkout` names the branch, so there is nothing to edit.
        column.line(frame, widgets::section("Branch"));
        column.text(
            frame,
            format!(" {}  (gh pr checkout)", modal.branch.value()),
            theme::secondary(),
        );
    } else {
        branch_fields(frame, &mut column, modal, hits);
    }

    // Marked, because an edited prompt no longer follows the name fields.
    let label = if modal.edited_prompt.is_some() {
        "Prompt (edited)"
    } else {
        "Prompt"
    };
    column.line(frame, widgets::section(label));
    for line in prompt {
        column.text(frame, format!(" {line}"), theme::secondary());
    }
    let edit = if modal.is_editing {
        disabled("Editing…", StartWorkConfirmModal::FOCUS_EDIT)
    } else {
        control(
            "Edit prompt",
            modal.focus == StartWorkConfirmModal::FOCUS_EDIT,
            theme::Variant::Normal,
            StartWorkConfirmModal::FOCUS_EDIT,
        )
    };
    column.gap();

    let launch = if modal.can_launch() {
        control(
            "Launch",
            modal.focus == StartWorkConfirmModal::FOCUS_LAUNCH,
            theme::Variant::Primary,
            StartWorkConfirmModal::FOCUS_LAUNCH,
        )
    } else {
        disabled("Launch", StartWorkConfirmModal::FOCUS_LAUNCH)
    };
    column.controls(
        frame,
        hits,
        // Edit and YOLO share the button row: a row of their own would cost
        // the prompt above three of its lines.
        vec![
            edit,
            checkbox(
                "YOLO",
                modal.yolo,
                modal.focus == StartWorkConfirmModal::FOCUS_YOLO,
                StartWorkConfirmModal::FOCUS_YOLO,
            ),
            launch,
            // Always over Start work's item step, so dismissing goes back.
            control(
                "Back",
                modal.focus == StartWorkConfirmModal::FOCUS_CANCEL,
                theme::Variant::Normal,
                StartWorkConfirmModal::FOCUS_CANCEL,
            ),
        ],
        true,
    );
}

/// The row under a Base Branch input. The completion the Textual suggester
/// used to ghost inside the input comes first — Right accepts it, so say
/// so. With none to offer, a base branch the repository settings name but
/// the branch list lacks is pointed out, since the field then holds a
/// fallback. The row is always reserved, so the controls below do not jump
/// as the user types.
fn base_hint(suggestion: Option<String>, value: &str, missing: Option<&str>) -> (String, Style) {
    match (suggestion, missing) {
        (Some(suggestion), _) if suggestion != value => {
            (format!(" {suggestion}  (→ accepts)"), theme::muted())
        }
        (_, Some(missing)) => (
            format!(" Repository base branch '{missing}' not found"),
            theme::warning(),
        ),
        _ => (String::new(), theme::muted()),
    }
}

/// The branch fields of an issue or feature: the branch to create, the base
/// it branches from with its completion hint, and Fetch beside the pull
/// checkbox. A PR review has none of them.
fn branch_fields(
    frame: &mut Frame,
    column: &mut Column,
    modal: &StartWorkConfirmModal,
    hits: &mut Hits,
) {
    column.line(frame, widgets::section("Branch Name"));
    column.input(
        frame,
        hits,
        &modal.branch,
        modal.focus,
        StartWorkConfirmModal::FOCUS_BRANCH,
    );

    column.line(frame, widgets::section("Base Branch"));
    column.input(
        frame,
        hits,
        &modal.base_branch,
        modal.focus,
        StartWorkConfirmModal::FOCUS_BASE,
    );
    let (hint, style) = base_hint(
        modal.base_suggestion(),
        modal.base_branch.value(),
        modal.missing_base(),
    );
    column.text(frame, hint, style);

    let fetch = if modal.is_fetching {
        SPINNER[modal.spinner_index % SPINNER.len()].to_string()
    } else {
        "Fetch".to_string()
    };
    column.controls(
        frame,
        hits,
        vec![
            control(
                &fetch,
                modal.focus == StartWorkConfirmModal::FOCUS_FETCH,
                theme::Variant::Normal,
                StartWorkConfirmModal::FOCUS_FETCH,
            ),
            checkbox(
                "Pull repo before creating",
                modal.pull_first,
                modal.focus == StartWorkConfirmModal::FOCUS_PULL,
                StartWorkConfirmModal::FOCUS_PULL,
            ),
        ],
        false,
    );
}

// ------------------------------------------------------------------ Settings

fn settings(frame: &mut Frame, modal: &SettingsModal, area: Rect, hits: &mut Hits) {
    // Three labelled controls, the custom-button, template and integration
    // sections, then the gap and the buttons. 36 rows: still inside a 40-row
    // terminal's 95%, which is the size this dialog must fit whole.
    let rect = widgets::centered_rect(WIDE, CHROME + 30, area);
    let mut column = dialog(frame, rect, "Settings", theme::title());

    let editor = EDITORS
        .get(modal.editor_index)
        .map_or("", |(name, _)| *name);
    column.line(frame, widgets::section("DEFAULT EDITOR"));
    column.controls(
        frame,
        hits,
        vec![cycle(
            editor,
            modal.focus == SettingsModal::FOCUS_EDITOR,
            SettingsModal::FOCUS_EDITOR,
        )],
        false,
    );

    column.line(frame, widgets::section("BRANCH PREFIX"));
    column.input(
        frame,
        hits,
        &modal.branch_prefix,
        modal.focus,
        SettingsModal::FOCUS_PREFIX,
    );

    let theme_name = crate::theme::by_slug(modal.theme_slug).map_or("", |t| t.name);
    column.line(frame, widgets::section("THEME"));
    column.controls(
        frame,
        hits,
        vec![control(
            &format!("{theme_name}…"),
            modal.focus == SettingsModal::FOCUS_THEME,
            theme::Variant::Normal,
            SettingsModal::FOCUS_THEME,
        )],
        false,
    );

    column.line(frame, widgets::section("CUSTOM CLAUDE BUTTONS"));
    column.text(frame, modal.buttons_summary(), theme::muted());
    column.controls(
        frame,
        hits,
        vec![control(
            "Manage Custom Buttons...",
            modal.focus == SettingsModal::FOCUS_MANAGE,
            theme::Variant::Normal,
            SettingsModal::FOCUS_MANAGE,
        )],
        false,
    );
    // The back key shares the templates' row for the same reason the Remote
    // Control checkbox shares the integration's: a section of its own would
    // push the dialog past a 40-row terminal.
    let templates = "Edit Prompt Templates...";
    let indent = (widgets::button_box_width(templates) + GAP) as usize;
    column.line(
        frame,
        Line::from(vec![
            Span::styled(
                format!("{:<indent$}", "PROMPT TEMPLATES"),
                theme::section_header(),
            ),
            Span::styled("BACK KEY TO FORESTUI", theme::section_header()),
        ]),
    );
    column.controls(
        frame,
        hits,
        vec![
            control(
                templates,
                modal.focus == SettingsModal::FOCUS_TEMPLATES,
                theme::Variant::Normal,
                SettingsModal::FOCUS_TEMPLATES,
            ),
            checkbox(
                &format!("prefix {}", back_key::glyph(&modal.back_key)),
                modal.back_key_on,
                modal.focus == SettingsModal::FOCUS_BACK_KEY,
                SettingsModal::FOCUS_BACK_KEY,
            ),
            control(
                "Change Key...",
                modal.focus == SettingsModal::FOCUS_BACK_KEY_CHANGE,
                theme::Variant::Normal,
                SettingsModal::FOCUS_BACK_KEY_CHANGE,
            ),
        ],
        false,
    );
    column.gap();
    column.line(frame, widgets::section("CLAUDE CODE INTEGRATION"));
    column.text(frame, modal.integration_status.label(), theme::muted());
    // The Remote Control checkbox shares this row rather than taking a
    // section of its own: five more rows pushed the dialog past a 40-row
    // terminal's 95%, painting its lower controls over one another.
    column.controls(
        frame,
        hits,
        vec![
            control(
                "Manage Integration...",
                modal.focus == SettingsModal::FOCUS_INTEGRATION,
                theme::Variant::Normal,
                SettingsModal::FOCUS_INTEGRATION,
            ),
            checkbox(
                "Start sessions with Remote Control",
                modal.remote_control,
                modal.focus == SettingsModal::FOCUS_REMOTE_CONTROL,
                SettingsModal::FOCUS_REMOTE_CONTROL,
            ),
        ],
        false,
    );
    column.gap();
    column.controls(
        frame,
        hits,
        vec![
            control(
                "Save",
                modal.focus == SettingsModal::FOCUS_SAVE,
                theme::Variant::Primary,
                SettingsModal::FOCUS_SAVE,
            ),
            control(
                "Cancel",
                modal.focus == SettingsModal::FOCUS_CANCEL,
                theme::Variant::Normal,
                SettingsModal::FOCUS_CANCEL,
            ),
        ],
        true,
    );
}

// ------------------------------------------------------------ Custom buttons

fn custom_buttons(frame: &mut Frame, modal: &CustomButtonsModal, area: Rect, hits: &mut Hits) {
    let list_rows = if modal.buttons.is_empty() {
        1
    } else {
        (modal.buttons.len() * BUTTON_BLOCK).min(BUTTON_LIST_ROWS)
    };
    // Intro, gap, the list, gap, help footer.
    let rect = widgets::centered_rect(WIDE, CHROME + list_rows as u16 + 4, area);
    let mut column = dialog(frame, rect, "Custom Claude Buttons", theme::title());

    column.text(
        frame,
        "Order here matches display order in the Claude section.",
        theme::muted(),
    );
    column.gap();

    if modal.buttons.is_empty() {
        column.text(frame, "No buttons yet. Press a to add.", theme::muted());
    } else {
        // Leave the gap and the help footer their rows before filling the list.
        let space = column.remaining().saturating_sub(2) as usize;
        let visible = (space / BUTTON_BLOCK).max(1);
        let first = modal.selected.saturating_sub(visible.saturating_sub(1));
        for (offset, entry) in modal.buttons.iter().skip(first).take(visible).enumerate() {
            let index = first + offset;
            let selected = index == modal.selected;
            let style = if selected {
                theme::cursor()
            } else {
                theme::primary()
            };
            let marker = if selected { "▸" } else { " " };
            let yolo = if entry.is_yolo_style() { " (YOLO)" } else { "" };
            column.row(
                frame,
                hits,
                Line::from(Span::styled(
                    format!("{marker} {}{yolo}", entry.label),
                    style,
                )),
                style,
                index,
                ModalClick::Row(index),
            );
            // The whole three-row block picks the entry, so its description
            // rows are click targets for the same index.
            column.row(
                frame,
                hits,
                Line::from(format!("   prefix: {}", entry.prefix)),
                theme::muted(),
                index,
                ModalClick::Row(index),
            );
            column.row(
                frame,
                hits,
                Line::from(format!("   $ {}", entry.command)),
                theme::muted(),
                index,
                ModalClick::Row(index),
            );
        }
    }

    column.gap();
    column.text(
        frame,
        "a add · e/Enter edit · d delete · K/J move · s save · Esc cancel",
        theme::muted(),
    );
}

// -------------------------------------------------------------- Edit button

fn edit_button(frame: &mut Frame, modal: &EditButtonModal, area: Rect, hits: &mut Hits) {
    // Three labelled fields with a hint each, the error line, gap, buttons.
    let rect = widgets::centered_rect(WIDTH, CHROME + 20, area);
    let mut column = dialog(frame, rect, modal.title(), theme::title());

    column.line(frame, widgets::section("LABEL"));
    column.input(
        frame,
        hits,
        &modal.label,
        modal.focus,
        EditButtonModal::FOCUS_LABEL,
    );
    column.text(
        frame,
        "Shown on the button, next to Claude and YOLO",
        theme::muted(),
    );

    column.line(frame, widgets::section("TMUX PREFIX"));
    column.input(
        frame,
        hits,
        &modal.prefix,
        modal.focus,
        EditButtonModal::FOCUS_PREFIX,
    );
    column.text(
        frame,
        "Window prefix: <prefix>:<worktree>. Auto-derived from label until you edit it.",
        theme::muted(),
    );

    column.line(frame, widgets::section("COMMAND"));
    column.input(
        frame,
        hits,
        &modal.command,
        modal.focus,
        EditButtonModal::FOCUS_COMMAND,
    );
    column.text(
        frame,
        "Run as-is, with the window name (or resume id) appended.",
        theme::muted(),
    );

    column.text(frame, modal.error.clone(), theme::destructive());
    column.gap();
    column.controls(
        frame,
        hits,
        vec![
            control(
                "Save",
                modal.focus == EditButtonModal::FOCUS_SAVE,
                theme::Variant::Primary,
                EditButtonModal::FOCUS_SAVE,
            ),
            control(
                "Cancel",
                modal.focus == EditButtonModal::FOCUS_CANCEL,
                theme::Variant::Normal,
                EditButtonModal::FOCUS_CANCEL,
            ),
        ],
        true,
    );
}

// -------------------------------------------------------------- Theme picker

/// Theme rows kept on screen at once; the list scrolls around the highlight.
const THEME_ROWS: usize = 14;

fn theme_picker(frame: &mut Frame, modal: &ThemePickerModal, area: Rect, hits: &mut Hits) {
    let themes = crate::theme::THEMES;
    // Deliberately narrow: the app behind the dialog is the preview, so the
    // dialog covers as little of it as a readable list allows.
    let rect = widgets::centered_rect(44, CHROME + THEME_ROWS.min(themes.len()) as u16 + 2, area);
    let mut column = dialog(frame, rect, "Theme", theme::title());

    // The window is sized from the rows that actually fit — `centered_rect`
    // clamps the dialog on a short terminal, and a window computed from the
    // ideal height would scroll the highlighted row right out of the box.
    let rows = (column.remaining().saturating_sub(2) as usize)
        .min(THEME_ROWS)
        .min(themes.len())
        .max(1);

    // Keep the highlight centred while the window stays inside the list.
    let first = modal
        .index
        .saturating_sub(rows / 2)
        .min(themes.len().saturating_sub(rows));

    for (row, candidate) in themes.iter().enumerate().skip(first).take(rows) {
        let Some(line_rect) = column.take(1) else {
            break;
        };
        let selected = row == modal.index;
        let row_style = if selected {
            theme::cursor()
        } else {
            theme::secondary()
        };
        // The swatch shows each candidate's accent without applying it, so the
        // list itself hints at what a row would look like before it is chosen.
        let swatch_style = row_style.fg(candidate.accent);
        let name = format!(
            "{:<width$}",
            candidate.name,
            width = (line_rect.width as usize).saturating_sub(5)
        );
        let line = Line::from(vec![
            Span::styled(if selected { " ▸ " } else { "   " }.to_string(), row_style),
            Span::styled("●", swatch_style),
            Span::styled(" ", row_style),
            Span::styled(name, row_style),
        ]);
        frame.render_widget(Paragraph::new(line), line_rect);
        // A click selects the row — which previews it — and commits, the way
        // Enter does from the keyboard.
        hits.push((line_rect, row, ModalClick::Activate));
    }
    column.gap();
    column.text(
        frame,
        "↑/↓ preview · Enter apply · Esc revert",
        theme::muted(),
    );
}

// ------------------------------------------------------- Prompt templates

fn prompt_templates(frame: &mut Frame, modal: &PromptTemplatesModal, area: Rect, hits: &mut Hits) {
    let rows = modal.rows();
    // Where the files are, a gap, the list, a gap, and the hint.
    let rect = widgets::centered_rect(68, CHROME + rows.len() as u16 + 4, area);
    let (title, dir) = match &modal.repo {
        Some(key) => (
            "Repository Settings",
            key.dir(&crate::services::settings::repos_dir()),
        ),
        None => ("Prompt Templates", crate::services::settings::prompts_dir()),
    };
    let mut column = dialog(frame, rect, title, theme::title());
    let dir = match dir.strip_prefix(crate::util::home_dir()) {
        Ok(relative) => format!("~/{}", relative.display()),
        Err(_) => dir.display().to_string(),
    };
    column.text(frame, format!("Files in {dir}"), theme::muted());
    column.gap();

    for (row, file) in rows.iter().enumerate() {
        let Some(line_rect) = column.take(1) else {
            break;
        };
        let selected = row == modal.index;
        let style = if selected {
            theme::cursor()
        } else {
            theme::secondary()
        };
        // A repository's templates sit in `prompts/`; the global list is
        // already inside it.
        let file_name = match (&modal.repo, file) {
            (None, crate::repo_settings::ConfigFile::Template(template)) => {
                template.file_name().to_string()
            }
            _ => file.relative_path(),
        };
        let label = format!(
            "{:<width$}",
            file.label(),
            // One column spare, so the file name never touches the border.
            width = (line_rect.width as usize).saturating_sub(4 + file_name.len())
        );
        let line = Line::from(vec![
            Span::styled(if selected { " ▸ " } else { "   " }.to_string(), style),
            Span::styled(label, style),
            Span::styled(file_name, style.patch(theme::muted())),
        ]);
        frame.render_widget(Paragraph::new(line), line_rect);
        hits.push((line_rect, row, ModalClick::Activate));
    }
    column.gap();
    column.text(
        frame,
        match modal.repo {
            Some(_) => "↑/↓ choose · Enter edit, from the global one if new · Esc back",
            None => "↑/↓ choose · Enter edit, from the default if new · Esc back",
        },
        theme::muted(),
    );
}

// -------------------------------------------------- Claude Code integration

fn claude_integration(
    frame: &mut Frame,
    modal: &ClaudeIntegrationModal,
    area: Rect,
    hits: &mut Hits,
) {
    use crate::services::claude_plugin::{self, Status};

    let names = claude_plugin::asset_names();
    let drifted = match &modal.status {
        Status::Drifted(files) => files.len(),
        _ => 0,
    };
    let height = CHROME
        + 14
        + names.len() as u16
        + drifted as u16
        + modal.message.is_some() as u16
        + if claude_plugin::jq_available() { 0 } else { 3 };
    let rect = widgets::centered_rect(WIDTH, height, area);
    let mut column = dialog(frame, rect, "Claude Integration", theme::primary());

    column.text(
        frame,
        "Keeps each session and its tmux tab under one name (rename either and".to_string(),
        theme::secondary(),
    );
    column.text(
        frame,
        "the other follows), and reports where every session is running, so".to_string(),
        theme::secondary(),
    );
    column.text(
        frame,
        "cards show live badges even for sessions started outside forestui.".to_string(),
        theme::secondary(),
    );
    column.gap();
    column.text(frame, modal.status.label(), theme::primary());
    if let Status::Drifted(files) = &modal.status {
        for file in files {
            column.text(frame, format!("  edited: {file}"), theme::muted());
        }
    }
    if let Some(message) = &modal.message {
        column.text(frame, message.clone(), theme::muted());
    }
    column.gap();
    // Said plainly, because "installs a plugin" is the part that deserves to
    // be specific: this writes one directory and nothing else.
    column.text(
        frame,
        "Installs a Claude Code plugin. Your settings.json is not touched.".to_string(),
        theme::secondary(),
    );
    // `~` rather than the absolute home, which is the half that pushes the
    // interesting end of the path off the dialog.
    let home = crate::util::home_dir();
    let dir = claude_plugin::plugin_dir();
    let shown = match dir.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => dir.display().to_string(),
    };
    column.text(frame, format!("  {shown}/"), theme::muted());
    for name in &names {
        column.text(frame, format!("    {name}"), theme::muted());
    }
    if !claude_plugin::jq_available() {
        column.gap();
        column.text(
            frame,
            "jq is not on PATH. The hook reads the session's own name with it,".to_string(),
            theme::destructive(),
        );
        column.text(
            frame,
            "so until jq is installed this does nothing at all.".to_string(),
            theme::destructive(),
        );
    }
    column.gap();

    let controls: Vec<_> = modal
        .actions()
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let variant = if action.is_destructive() {
                theme::Variant::Destructive
            } else {
                theme::Variant::Normal
            };
            control(action.label(), modal.focus == index, variant, index)
        })
        .collect();
    column.controls(frame, hits, controls, true);
}

// ------------------------------------------------------------------ Confirm

// ----------------------------------------------------------- Rename session

fn rename_session(frame: &mut Frame, modal: &RenameSessionModal, area: Rect, hits: &mut Hits) {
    // Label, input, mechanism note, error, gap, buttons.
    let rect = widgets::centered_rect(WIDTH, CHROME + 10, area);
    let mut column = dialog(frame, rect, "Rename Session", theme::title());

    column.line(frame, widgets::section("Session Name"));
    column.input(
        frame,
        hits,
        &modal.name,
        modal.focus,
        RenameSessionModal::FOCUS_NAME,
    );

    // Say which mechanism will run, so a tab renaming itself (live) or the
    // rename waiting for the next prompt is never a surprise.
    let note = match &modal.live_window {
        Some((_, window_name)) => format!(
            "Live session: renames tmux window '{}' too",
            util::truncate(window_name, 30)
        ),
        None => "Stopped session: the name is written to its transcript".to_string(),
    };
    column.text(frame, note, theme::secondary());
    if modal.error.is_empty() {
        column.gap();
    } else {
        column.text(frame, modal.error.clone(), theme::destructive());
    }
    column.gap();
    column.controls(
        frame,
        hits,
        vec![
            control(
                "Save",
                modal.focus == RenameSessionModal::FOCUS_SAVE,
                theme::Variant::Primary,
                RenameSessionModal::FOCUS_SAVE,
            ),
            control(
                "Cancel",
                modal.focus == RenameSessionModal::FOCUS_CANCEL,
                theme::Variant::Normal,
                RenameSessionModal::FOCUS_CANCEL,
            ),
        ],
        true,
    );
}

fn confirm(frame: &mut Frame, modal: &ConfirmModal, area: Rect, hits: &mut Hits) {
    let message: Vec<&str> = modal.message.split('\n').collect();
    // The message, gap, buttons, gap, key hints.
    let rect = widgets::centered_rect(WIDTH, CHROME + message.len() as u16 + 6, area);
    // Python gave this one title `label-destructive`, so it reads as a warning.
    let mut column = dialog(frame, rect, &modal.title, theme::destructive());

    for line in message {
        column.text(frame, line.to_string(), theme::secondary());
    }
    column.gap();
    // `ConfirmModal::set_focus` reads 0 as Cancel and 1 as Delete.
    column.controls(
        frame,
        hits,
        vec![
            control("Cancel", !modal.confirm_focused, theme::Variant::Normal, 0),
            control(
                modal.action.confirm_label(),
                modal.confirm_focused,
                // "Switch" painted destructive-red would warn against the
                // safe choice — the colour follows what confirming does.
                if modal.action.is_destructive() {
                    theme::Variant::Destructive
                } else {
                    theme::Variant::Primary
                },
                1,
            ),
        ],
        true,
    );
    column.gap();
    column.text(frame, "y confirm · n / Esc cancel", theme::muted());
}

fn back_key(frame: &mut Frame, modal: &BackKeyModal, area: Rect, hits: &mut Hits) {
    let glyph = back_key::glyph(&modal.key);
    match modal.step {
        BackKeyStep::Pick => {
            // Every key but Esc is an answer here, so there is nothing to
            // click: a click arrives as Enter, which would be picked.
            let rect = widgets::centered_rect(WIDTH, CHROME + 3, area);
            let mut column = dialog(frame, rect, "Back Key", theme::title());
            column.text(
                frame,
                "Press the key to use after the tmux prefix.",
                theme::secondary(),
            );
            column.gap();
            column.text(frame, "Esc to go back", theme::muted());
        }
        BackKeyStep::Offer => {
            let rect = widgets::centered_rect(WIDTH, CHROME + 10, area);
            let mut column = dialog(frame, rect, "Back Key", theme::title());
            column.text(
                frame,
                format!(
                    "forestui uses prefix {glyph} to go from a Claude window back to forestui,"
                ),
                theme::secondary(),
            );
            column.text(
                frame,
                format!("but you have bound prefix {glyph} yourself:"),
                theme::secondary(),
            );
            column.gap();
            column.text(frame, format!("  {}", modal.binding), theme::primary());
            column.gap();
            column.controls(
                frame,
                hits,
                vec![
                    control(
                        "Pick Another Key",
                        modal.focus == BackKeyModal::FOCUS_PICK,
                        theme::Variant::Primary,
                        BackKeyModal::FOCUS_PICK,
                    ),
                    control(
                        "Use It Anyway",
                        modal.focus == BackKeyModal::FOCUS_ANYWAY,
                        theme::Variant::Normal,
                        BackKeyModal::FOCUS_ANYWAY,
                    ),
                    control(
                        "Off",
                        modal.focus == BackKeyModal::FOCUS_OFF,
                        theme::Variant::Normal,
                        BackKeyModal::FOCUS_OFF,
                    ),
                ],
                true,
            );
            column.gap();
            column.text(
                frame,
                "Use it anyway: yours keeps working everywhere but Claude windows.",
                theme::muted(),
            );
        }
    }
}

#[cfg(test)]
mod tests;
