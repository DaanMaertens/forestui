//! The Start Work Settings dialog: a grid per section, laid out from the
//! modal's field table, scrolled so the focused field is in view.

use super::{CHROME, Control, GAP, Hits, WIDE, checkbox, control, cycle, dialog, disabled};
use crate::app::ModalClick;
use crate::modal::{
    FieldKind, FieldSection, FieldState, START_WORK_FIELDS, StartWorkSettingsModal,
};
use crate::repo_settings::RepoKey;
use crate::theme;
use crate::ui::widgets::{self, BUTTON_HEIGHT};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Room on the left of the naming grid for the work mode and the
/// placeholders its patterns take.
const ROW_LABEL: u16 = 17;
/// Rows a field takes: a text box, or a boxed control.
const FIELD_HEIGHT: u16 = 3;

/// One horizontal strip of the body, in the order it is drawn.
enum Band {
    Gap,
    Header(FieldSection),
    /// The labels over a section's columns.
    Labels(FieldSection),
    /// One row of a section's grid: its label, hint and field indices.
    Fields {
        section: FieldSection,
        label: &'static str,
        hint: &'static str,
        fields: Vec<usize>,
    },
    /// The errors of the row above, each under its field.
    Errors(Vec<usize>),
    Text(String, ratatui::style::Style),
}

impl Band {
    fn height(&self) -> u16 {
        match self {
            Band::Fields { .. } => FIELD_HEIGHT,
            _ => 1,
        }
    }

    fn holds(&self, focus: usize) -> bool {
        matches!(self, Band::Fields { fields, .. } if fields.contains(&focus))
    }
}

fn bands(modal: &StartWorkSettingsModal) -> Vec<Band> {
    if let Some(problem) = modal.problem() {
        return vec![
            Band::Text(problem.to_string(), theme::destructive()),
            Band::Gap,
            Band::Text(
                "Fix it with Edit settings.json..., then open this dialog again.".to_string(),
                theme::muted(),
            ),
        ];
    }
    let mut bands = Vec::new();
    for section in FieldSection::ALL {
        bands.push(Band::Gap);
        bands.push(Band::Header(section));
        bands.push(Band::Labels(section));
        let in_section = START_WORK_FIELDS
            .iter()
            .enumerate()
            .filter(|(_, field)| field.section == section);
        for (index, field) in in_section {
            match bands.last_mut() {
                Some(Band::Fields { label, fields, .. }) if *label == field.row => {
                    fields.push(index)
                }
                _ => bands.push(Band::Fields {
                    section,
                    label: field.row,
                    hint: field.hint,
                    fields: vec![index],
                }),
            }
        }
        // A count is the one kind that can be wrong; its row says so under
        // it, on a line kept whether or not there is anything to say, so
        // the dialog does not jump while typing.
        let counts: Vec<usize> = START_WORK_FIELDS
            .iter()
            .enumerate()
            .filter(|(_, f)| f.section == section && f.kind == FieldKind::Count)
            .map(|(i, _)| i)
            .collect();
        if !counts.is_empty() {
            bands.push(Band::Errors(counts));
        }
    }
    bands
}

/// How many columns a section's grid has, and whether its rows carry a
/// label on the left.
fn grid(section: FieldSection) -> (usize, bool) {
    let fields = START_WORK_FIELDS.iter().filter(|f| f.section == section);
    let columns = fields.clone().map(|f| f.column + 1).max().unwrap_or(1);
    let labelled = fields.clone().any(|f| !f.row.is_empty());
    (columns, labelled)
}

/// Where column `column` of `section` sits within `area`.
fn cell(area: Rect, section: FieldSection, column: usize) -> Rect {
    let (columns, labelled) = grid(section);
    let left = if labelled { ROW_LABEL } else { 0 };
    let room = area.width.saturating_sub(left);
    let width = room / columns as u16;
    Rect {
        x: area.x + left + width * column as u16,
        y: area.y,
        width: width.saturating_sub(GAP),
        height: area.height,
    }
}

pub(super) fn render(
    frame: &mut Frame,
    modal: &StartWorkSettingsModal,
    area: Rect,
    hits: &mut Hits,
) {
    let bands = bands(modal);
    let body: u16 = bands.iter().map(Band::height).sum();
    // Subtitle, the body, a gap, the buttons.
    let rect = widgets::centered_rect(WIDE, CHROME + 1 + body + 1 + BUTTON_HEIGHT, area);
    let title = match &modal.target {
        crate::modal::StartWorkTarget::Global => "Start Work Settings".to_string(),
        crate::modal::StartWorkTarget::Repository { key, .. } => title_for(key),
    };
    let mut column = dialog(frame, rect, &title, theme::title());
    let subtitle = if modal.is_repository() {
        "Overrides of the global settings. An empty field inherits the global value, shown dimmed."
    } else {
        "Repositories can override these settings in Repository Settings."
    };
    column.text(frame, subtitle, theme::muted());

    // The body scrolls; the buttons stay put under it.
    let viewport = column.remaining().saturating_sub(1 + BUTTON_HEIGHT);
    let mut top = 0;
    let mut offset = 0;
    for band in &bands {
        if band.holds(modal.focus) {
            offset = (top + band.height()).saturating_sub(viewport);
            break;
        }
        top += band.height();
    }
    let Some(view) = column.take(viewport) else {
        return;
    };
    let mut y: u16 = 0;
    for band in &bands {
        let height = band.height();
        let fits = y >= offset && y + height <= offset + viewport;
        if fits {
            let rect = Rect {
                y: view.y + (y - offset),
                height,
                ..view
            };
            draw_band(frame, modal, band, rect, hits);
        }
        y += height;
    }

    column.gap();
    let mut buttons: Vec<Control> = Vec::new();
    if modal.problem().is_none() {
        buttons.push(if modal.can_save() {
            control(
                "Save",
                modal.focus == StartWorkSettingsModal::FOCUS_SAVE,
                theme::Variant::Primary,
                StartWorkSettingsModal::FOCUS_SAVE,
            )
        } else {
            disabled("Save", StartWorkSettingsModal::FOCUS_SAVE)
        });
    }
    buttons.push(control(
        "Cancel",
        modal.focus == StartWorkSettingsModal::FOCUS_CANCEL,
        theme::Variant::Normal,
        StartWorkSettingsModal::FOCUS_CANCEL,
    ));
    if modal.is_repository() {
        buttons.push(control(
            "Edit settings.json...",
            modal.focus == StartWorkSettingsModal::FOCUS_EDIT_FILE,
            theme::Variant::Normal,
            StartWorkSettingsModal::FOCUS_EDIT_FILE,
        ));
    }
    column.controls(frame, hits, buttons, true);
}

pub fn title_for(key: &RepoKey) -> String {
    format!("Start Work Settings: {key}")
}

fn draw_band(
    frame: &mut Frame,
    modal: &StartWorkSettingsModal,
    band: &Band,
    rect: Rect,
    hits: &mut Hits,
) {
    match band {
        Band::Gap => {}
        Band::Text(text, style) => {
            frame.render_widget(Paragraph::new(Span::styled(text.clone(), *style)), rect);
        }
        Band::Header(section) => {
            frame.render_widget(Paragraph::new(widgets::section(section.title())), rect);
        }
        Band::Labels(section) => {
            let (columns, _) = grid(*section);
            for column in 0..columns {
                let label = START_WORK_FIELDS
                    .iter()
                    .find(|f| f.section == *section && f.column == column)
                    .map_or("", |f| f.label);
                let at = cell(rect, *section, column);
                frame.render_widget(Paragraph::new(Span::styled(label, theme::secondary())), at);
            }
        }
        Band::Fields {
            section,
            label,
            hint,
            fields,
        } => {
            if !label.is_empty() {
                let lines = vec![
                    Line::default(),
                    Line::from(Span::styled(label.to_string(), theme::primary())),
                    Line::from(Span::styled(hint.to_string(), theme::muted())),
                ];
                let at = Rect {
                    width: ROW_LABEL.min(rect.width),
                    ..rect
                };
                frame.render_widget(Paragraph::new(lines), at);
            }
            for index in fields {
                let at = cell(rect, *section, START_WORK_FIELDS[*index].column);
                draw_field(frame, modal, *index, at, hits);
            }
        }
        Band::Errors(fields) => {
            for index in fields {
                if let Some(error) = modal.error(*index) {
                    let field = &START_WORK_FIELDS[*index];
                    // From under the field to the edge: the message is
                    // wider than the field it is about.
                    let under = cell(rect, field.section, field.column);
                    let at = Rect {
                        x: under.x,
                        width: (rect.x + rect.width).saturating_sub(under.x),
                        ..rect
                    };
                    frame.render_widget(
                        Paragraph::new(Span::styled(error, theme::destructive())),
                        at,
                    );
                }
            }
        }
    }
}

fn draw_field(
    frame: &mut Frame,
    modal: &StartWorkSettingsModal,
    index: usize,
    rect: Rect,
    hits: &mut Hits,
) {
    let focused = modal.focus == index;
    match &modal.fields[index] {
        FieldState::Text(input) => {
            input.render(frame, rect, focused);
            hits.push((rect, index, ModalClick::Focus));
        }
        FieldState::Choice(_) => {
            let piece =
                if START_WORK_FIELDS[index].kind == FieldKind::Toggle && !modal.is_repository() {
                    checkbox("Enabled", modal.is_on(index), focused, index)
                } else {
                    cycle(&modal.choice_label(index), focused, index)
                };
            draw_control(frame, piece, rect, hits);
        }
    }
}

/// One boxed control at the left of `rect`, clickable where it is drawn.
fn draw_control(
    frame: &mut Frame,
    (rows, width, index, click): Control,
    rect: Rect,
    hits: &mut Hits,
) {
    let width = width.min(rect.width);
    let at = Rect { width, ..rect };
    frame.render_widget(Paragraph::new(rows.map(Line::from).to_vec()), at);
    hits.push((at, index, click));
}
