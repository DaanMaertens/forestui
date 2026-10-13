//! Rendering. One `draw` per frame, one function per pane.

pub mod detail;
pub mod modals;
pub mod sidebar;
pub mod widgets;

use crate::app::{App, HitTarget};
use crate::event::Severity;
use crate::theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

pub const SIDEBAR_WIDTH: u16 = 40;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // Clickable regions are rebuilt every frame; nothing persists between them.
    app.clear_hits();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::active().bg)),
        area,
    );

    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, app, header_area);

    let sidebar_width = SIDEBAR_WIDTH.min(body_area.width.saturating_sub(10).max(1));
    let [sidebar_area, detail_area] =
        Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(0)])
            .areas(body_area);

    // Remembered so the wheel can be routed to the pane under the pointer.
    app.sidebar_rect = sidebar_area;
    app.detail_rect = detail_area;

    let hover_reveal = sidebar::draw(frame, app, sidebar_area);
    detail::draw(frame, app, detail_area);
    // Painted after the detail pane so a hovered row that is too wide for the
    // sidebar spills its full text over the divider instead of under it.
    // Skipped under modals: their backdrop repaints the panes anyway, and the
    // theme picker (which keeps the panes visible as its preview) must not
    // have a stray hover band floating through it.
    if app.modals.is_empty()
        && let Some(reveal) = hover_reveal
    {
        sidebar::draw_hover_reveal(frame, reveal);
    }
    draw_footer(frame, app, footer_area);
    // Toasts are hidden while any modal is open — under most modals the
    // backdrop wipes them anyway; the picker skips the backdrop, so it has to
    // skip the toasts explicitly or they float over the preview.
    if app.modals.is_empty() {
        draw_notifications(frame, app, body_area);
    }

    if !app.modals.is_empty() {
        // Textual's `ModalScreen` painted over the app rather than tinting it —
        // nothing behind a dialog is visible. A translucent backdrop leaves the
        // pane competing with the dialog for the eye. The theme picker is the
        // one exception: the panes behind it ARE its preview — moving the
        // highlight repaints the whole app in the candidate theme, so covering
        // the app would leave nothing to preview against.
        if !matches!(app.modals.last(), Some(crate::modal::Modal::ThemePicker(_))) {
            frame.render_widget(Clear, area);
            frame.render_widget(
                Block::default().style(Style::default().bg(theme::active().bg)),
                area,
            );
        }
        modals::draw(frame, app, area);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    // Textual's stock `Header` also drew a `⭘` hard left. There it was a live
    // control that opened the app menu; nothing here is behind it, and a button
    // that cannot be pressed is worse than no button. Deliberately dropped, so
    // this bar is one of the few places the Rust build does not match the
    // Textual frame — the committed `baseline/python` frames still show it.
    // The title is centred on the whole bar either way.
    //
    // A pending update rides along until the process exits, and the title
    // and notice are centred together as one piece. The notice falls back to
    // its compact wording when the full one would not fit, and past that the
    // running version gives way to it — the new version and "restart" matter
    // more than the rest, and a clipped notice would lose exactly the part
    // that says what to do.
    let title = app.title();
    let width = area.width as usize;
    let fits = |notice: &str| title.width() + 1 + notice.width() <= width;
    let (title, notice) = match (app.title_suffix(false), app.title_suffix(true)) {
        (Some(full), _) if fits(&full) => (Some(title), Some(full)),
        (_, Some(compact)) if fits(&compact) => (Some(title), Some(compact)),
        (_, Some(compact)) => (None, Some(compact)),
        _ => (Some(title), None),
    };
    let text = [title.as_deref(), notice.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let indent = width.saturating_sub(text.width()) / 2;
    let mut spans = vec![Span::raw(" ".repeat(indent))];
    if let Some(title) = title {
        spans.push(Span::styled(title, theme::title()));
    }
    if let Some(notice) = notice {
        if spans.len() > 1 {
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(notice, theme::title_notice()));
    }
    let line = Line::from(spans);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::active().bg_elevated)),
        area,
    );
}

fn draw_footer(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut spans = Vec::new();
    // Tracks where each entry lands so it can be registered as a click target.
    let mut x = area.x;
    for (key, label) in crate::app::BINDINGS {
        let hovered = app.hovered == Some(HitTarget::FooterKey(key));
        let badge = format!(" {key} ");
        // `{label} `, matching Textual exactly: the badge already carries a
        // space inside its own background on each side, so the key sits one
        // column from its label and the single trailing space here plus the next
        // badge's leading one put two between entries. The original
        // ` {label}  ` spent two columns in each place, which pushed the last
        // entries off a 150-column terminal that Textual fitted. The footer is
        // the complete key surface, so entries that do not fit simply clip —
        // a narrow terminal shows a prefix, never a different set.
        let text = format!("{label} ");
        let width = (badge.chars().count() + text.chars().count()) as u16;

        spans.push(Span::styled(
            badge,
            Style::default()
                .bg(theme::active().accent_dark)
                .fg(theme::active().text_primary),
        ));
        spans.push(Span::styled(
            text,
            if hovered {
                Style::default()
                    .bg(theme::active().bg_hover)
                    .fg(theme::active().text_primary)
            } else {
                theme::secondary()
            },
        ));

        // Only a fully visible entry is clickable. A clipped one would be an
        // invisible control: at some widths its only remaining cell is a bare
        // accent space that would still fire a state-mutating binding.
        if x + width <= area.x + area.width {
            app.push_hit(
                Rect {
                    x,
                    y: area.y,
                    width,
                    height: area.height,
                },
                HitTarget::FooterKey(key),
            );
        }
        x = x.saturating_add(width);
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::active().bg_elevated)),
        area,
    );
}

fn draw_notifications(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.notifications.is_empty() {
        return;
    }
    let width = area.width.saturating_sub(4).min(60);
    if width < 10 {
        return;
    }
    // Wrap rather than truncate: notifications carry full sentences (git
    // errors, update failures), and cutting them at one line hides the part
    // the user needed to read.
    let inner_width = width as usize - 2;
    let mut lines: Vec<Line> = Vec::new();
    let palette = theme::active();
    for notification in &app.notifications {
        let style = match notification.severity {
            Severity::Information => Style::default()
                .bg(palette.accent_dark)
                .fg(palette.text_primary),
            Severity::Warning => Style::default().bg(palette.warning).fg(palette.bg),
            Severity::Error => Style::default().bg(palette.destructive).fg(palette.bg),
        };
        for chunk in wrap_words(&notification.text, inner_width) {
            lines.push(Line::from(Span::styled(
                format!(" {chunk:<inner_width$} "),
                style,
            )));
        }
    }

    let height = (lines.len() as u16).min(area.height.saturating_sub(1));
    if height == 0 {
        return;
    }
    lines.truncate(height as usize);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width + 2),
        y: area.y + area.height.saturating_sub(height + 1),
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines), rect);
    // Recorded after the panes, so it wins the cells it covers.
    app.push_hit(rect, HitTarget::Notification);
}

/// Break text into lines of at most `width` characters, on word boundaries
/// where possible.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if candidate.chars().count() <= width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        // A single word longer than the line has to be split somewhere.
        let mut rest: Vec<char> = word.chars().collect();
        while rest.len() > width {
            lines.push(rest.drain(..width).collect());
        }
        current = rest.into_iter().collect();
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests;
