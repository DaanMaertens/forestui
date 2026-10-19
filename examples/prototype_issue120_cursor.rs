//! PROTOTYPE — throwaway, never merge. Issue #120: how should the sidebar
//! cursor row show session status colours?
//!
//! One page per open question, its answers side by side, drawn with the real
//! palette values parsed out of `src/theme.rs`. Run:
//!
//!     cargo run --example prototype_issue120_cursor
//!
//! Tab / ←→ switch question, t / T cycle theme, q quits. Standalone: it never
//! touches tmux and does not import the forestui crate.

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

const THEME_SOURCE: &str = include_str!("../src/theme.rs");

#[derive(Clone)]
struct Theme {
    name: String,
    accent: u32,
    accent_dark: u32,
    bg: u32,
    bg_selected: u32,
    text: u32,
    secondary: u32,
    muted: u32,
    warning: u32,
    success: u32,
}

fn mix(a: u32, b: u32, pct: u32) -> u32 {
    let ch = |s: u32| ((((a >> s) & 255) * pct + ((b >> s) & 255) * (100 - pct)) / 100) << s;
    ch(16) | ch(8) | ch(0)
}

fn themes() -> Vec<Theme> {
    let mut out = vec![Theme {
        name: "Forest Dark".into(),
        accent: 0x52B788,
        accent_dark: 0x2D6A4F,
        bg: 0x1C1C1E,
        bg_selected: 0x48484A,
        text: 0xF5F5F5,
        secondary: 0xA8A8A8,
        muted: 0x7A7A7A,
        warning: 0xFFB347,
        success: 0x52B788,
    }];
    for line in THEME_SOURCE.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("build(\"") else {
            continue;
        };
        let Some(end) = rest.find('"') else { continue };
        let name = rest[..end].to_string();
        let hex: Vec<u32> = rest
            .split("0x")
            .skip(1)
            .filter_map(|t| u32::from_str_radix(t.get(..6)?, 16).ok())
            .collect();
        if hex.len() != 11 {
            continue;
        }
        // bg elevated selected border text secondary muted accent destruct warning success
        out.push(Theme {
            name,
            accent: hex[7],
            accent_dark: mix(hex[7], hex[0], 45),
            bg: hex[0],
            bg_selected: hex[2],
            text: hex[4],
            secondary: hex[5],
            muted: hex[6],
            warning: hex[9],
            success: hex[10],
        });
    }
    out
}

fn rgb(c: u32) -> Color {
    Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn lum(c: u32) -> f64 {
    let f = |s: u32| {
        let v = ((c >> s) & 255) as f64 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(16) + 0.7152 * f(8) + 0.0722 * f(0)
}

fn contrast(a: u32, b: u32) -> f64 {
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    NeedsYou,
    Busy,
    Idle,
}

struct Row {
    name: &'static str,
    branch: Option<&'static str>,
    state: Option<State>,
    count: u32,
    finished: bool,
}

const ROWS: &[Row] = &[
    Row { name: "issue-120", branch: Some("fix/cursor"), state: Some(State::NeedsYou), count: 2, finished: false },
    Row { name: "busy-work", branch: Some("feat/x"), state: Some(State::Busy), count: 1, finished: false },
    Row { name: "idle-work", branch: None, state: Some(State::Idle), count: 1, finished: false },
    Row { name: "done-work", branch: Some("old"), state: Some(State::Idle), count: 2, finished: true },
    Row { name: "plain", branch: None, state: None, count: 0, finished: false },
];

/// Every knob the questions turn. A variant is one setting of these.
#[derive(Clone, Copy)]
struct Opts {
    /// Today's bug: the cursor style overrides every span's fg.
    override_fg: bool,
    /// Only the status spans keep their colour; the rest go text_primary.
    only_status_keep: bool,
    bold: bool,
    /// Focused cursor uses the neutral bg_selected instead of accent_dark.
    neutral_focused: bool,
    /// Finished name turns text_primary on the cursor instead of staying dim.
    finished_primary_on_cursor: bool,
    busy_italic: bool,
    busy_glyph: bool,
}

const BASE: Opts = Opts {
    override_fg: false,
    only_status_keep: false,
    bold: false,
    neutral_focused: false,
    finished_primary_on_cursor: false,
    busy_italic: false,
    busy_glyph: false,
};

struct Variant {
    label: &'static str,
    note: &'static str,
    opts: Opts,
}

struct Question {
    title: &'static str,
    variants: Vec<Variant>,
}

fn questions() -> Vec<Question> {
    // Settled so far: keep accent_dark + BOLD, all spans keep their colour.
    let settled = Opts { bold: true, ..BASE };
    vec![
        Question {
            title: "Q1 · Focused cursor background (you picked C)",
            variants: vec![
                Variant { label: "Today (the bug)", note: "cursor forces text_primary", opts: Opts { override_fg: true, ..BASE } },
                Variant { label: "B · accent_dark, bg-only", note: "", opts: BASE },
                Variant { label: "C · accent_dark + BOLD", note: "", opts: Opts { bold: true, ..BASE } },
                Variant { label: "D · neutral bg when focused", note: "focused = bg_selected + bold", opts: Opts { bold: true, neutral_focused: true, ..BASE } },
            ],
        },
        Question {
            title: "Q2 · Which spans keep colour on the cursor (you picked B)",
            variants: vec![
                Variant { label: "A · only status spans", note: "rest text_primary", opts: Opts { only_status_keep: true, ..settled } },
                Variant { label: "B · all spans", note: "branch accent, twisty muted", opts: settled },
            ],
        },
        Question {
            title: "Q3 · Finished worktree on the cursor (see `done-work`)",
            variants: vec![
                Variant { label: "A · name stays dimmed", note: "finished still reads", opts: settled },
                Variant { label: "B · name primary on cursor", note: "count keeps colour", opts: Opts { finished_primary_on_cursor: true, ..settled } },
            ],
        },
        Question {
            title: "Q4 · Busy vs idle look the same in 5 themes (try Forest Dark, Monokai)",
            variants: vec![
                Variant { label: "A · today", note: "colour only", opts: settled },
                Variant { label: "B · busy in italic", note: "", opts: Opts { busy_italic: true, ..settled } },
                Variant { label: "C · busy gets a ⟳ glyph", note: "", opts: Opts { busy_glyph: true, ..settled } },
            ],
        },
    ]
}

fn state_fg(t: &Theme, s: State) -> u32 {
    match s {
        State::NeedsYou => t.warning,
        State::Busy => t.accent,
        State::Idle => t.success,
    }
}

/// Where the cursor sits: not on it, focused list, unfocused list.
#[derive(Clone, Copy, PartialEq)]
enum Cursor {
    None,
    Focused,
    Unfocused,
}

/// One worktree row, plus the fg/bg of its name for the contrast readout.
fn row_line(t: &Theme, row: &Row, o: Opts, cursor: Cursor) -> (Line<'static>, u32, u32) {
    let on = cursor != Cursor::None;
    let bg = match cursor {
        Cursor::None => t.bg,
        Cursor::Focused if o.neutral_focused => t.bg_selected,
        Cursor::Focused => t.accent_dark,
        Cursor::Unfocused => t.bg_selected,
    };
    let state = if row.finished { None } else { row.state };
    let status = row.state.map(|s| state_fg(t, s));
    let name_fg_raw = if row.finished {
        if on && o.finished_primary_on_cursor { t.text } else { t.muted }
    } else {
        status.unwrap_or(t.text)
    };
    let branch_fg_raw = if row.finished { t.muted } else { t.accent };

    // `is_status` spans survive "only status spans"; the rest go primary.
    let fg = |raw: u32, is_status: bool| -> u32 {
        if !on {
            raw
        } else if o.override_fg || (o.only_status_keep && !is_status) {
            t.text
        } else {
            raw
        }
    };
    let busy = state == Some(State::Busy);
    let style = |c: u32| {
        let mut s = Style::default().fg(rgb(c)).bg(rgb(bg));
        if on && o.bold {
            s = s.add_modifier(Modifier::BOLD);
        }
        s
    };
    let mut spans = vec![Span::styled(
        if std::ptr::eq(row, ROWS.last().unwrap_or(row)) { "└─ " } else { "├─ " },
        style(fg(t.text, false)),
    )];
    if row.finished {
        spans.push(Span::styled("✓ ", style(fg(t.success, true))));
    }
    if busy && o.busy_glyph {
        spans.push(Span::styled("⟳ ", style(fg(t.accent, true))));
    }
    if let Some(c) = status
        && row.count >= 2
    {
        spans.push(Span::styled(format!("{{{}}} ", row.count), style(fg(c, true))));
    }
    let name_fg = fg(name_fg_raw, !row.finished && status.is_some());
    let mut name_style = style(name_fg);
    if busy && o.busy_italic {
        name_style = name_style.add_modifier(Modifier::ITALIC);
    }
    spans.push(Span::styled(row.name, name_style));
    if let Some(b) = row.branch {
        spans.push(Span::styled(format!(" [{b}]"), style(fg(branch_fg_raw, false))));
    }
    (Line::from(spans), name_fg, bg)
}

fn pad(line: Line<'static>, width: u16, bg: u32) -> Line<'static> {
    let used = line.width() as u16;
    let mut spans = line.spans;
    if used < width {
        spans.push(Span::styled(
            " ".repeat((width - used) as usize),
            Style::default().bg(rgb(bg)),
        ));
    }
    Line::from(spans)
}

fn draw_variant(frame: &mut Frame, area: Rect, t: &Theme, v: &Variant) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(rgb(t.muted)))
        .title(Span::styled(
            format!(" {} ", v.label),
            Style::default().fg(rgb(t.text)).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(rgb(t.bg)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let w = inner.width;
    let head = |s: &str| Line::from(Span::styled(s.to_string(), Style::default().fg(rgb(t.secondary)).add_modifier(Modifier::BOLD)));
    let mut lines = vec![];
    if !v.note.is_empty() {
        lines.push(Line::from(Span::styled(v.note.to_string(), Style::default().fg(rgb(t.muted)))));
    }
    lines.push(head("not selected"));
    lines.push(Line::from(vec![
        Span::styled("▼ ", Style::default().fg(rgb(t.muted))),
        Span::styled("forestui", Style::default().fg(rgb(t.text)).add_modifier(Modifier::BOLD)),
    ]));
    for row in ROWS {
        lines.push(row_line(t, row, v.opts, Cursor::None).0);
    }
    for (title, cursor) in [("cursor, sidebar focused", Cursor::Focused), ("cursor, sidebar unfocused", Cursor::Unfocused)] {
        lines.push(Line::default());
        lines.push(head(title));
        for row in ROWS {
            let (line, fg, bg) = row_line(t, row, v.opts, cursor);
            let ratio = format!("{:>4.1}", contrast(fg, bg));
            let line = pad(line, w.saturating_sub(5), bg);
            let mut spans = line.spans;
            let rc = if contrast(fg, bg) < 3.0 { t.warning } else { t.muted };
            spans.push(Span::styled(format!(" {ratio}"), Style::default().fg(rgb(rc))));
            lines.push(Line::from(spans));
        }
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw(frame: &mut Frame, themes: &[Theme], qs: &[Question], ti: usize, qi: usize) {
    let t = &themes[ti];
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(rgb(t.bg))), area);
    let [top, body, foot] = Layout::vertical([Constraint::Length(2), Constraint::Min(0), Constraint::Length(1)]).areas(area);
    let q = &qs[qi];
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(q.title, Style::default().fg(rgb(t.text)).add_modifier(Modifier::BOLD))),
            Line::from(Span::styled(
                format!("theme {}/{}: {}   (number = name-text contrast, amber < 3:1)", ti + 1, themes.len(), t.name),
                Style::default().fg(rgb(t.secondary)),
            )),
        ]),
        top,
    );
    let n = q.variants.len() as u32;
    let cols = Layout::horizontal((0..n).map(|_| Constraint::Ratio(1, n))).split(body);
    for (v, col) in q.variants.iter().zip(cols.iter()) {
        draw_variant(frame, col.intersection(Rect { height: 25, ..*col }), t, v);
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            "PROTOTYPE   Tab/←→ question   t/T theme   q quit",
            Style::default().fg(rgb(t.bg)).bg(rgb(t.text)),
        )),
        foot,
    );
}

fn main() -> std::io::Result<()> {
    let themes = themes();
    let qs = questions();
    let (mut ti, mut qi) = (0usize, 0usize);
    let mut terminal = ratatui::init();
    let result = loop {
        if let Err(e) = terminal.draw(|f| draw(f, &themes, &qs, ti, qi)) {
            break Err(e);
        }
        match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match k.code {
                KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                KeyCode::Tab | KeyCode::Right => qi = (qi + 1) % qs.len(),
                KeyCode::BackTab | KeyCode::Left => qi = (qi + qs.len() - 1) % qs.len(),
                KeyCode::Char('t') => ti = (ti + 1) % themes.len(),
                KeyCode::Char('T') => ti = (ti + themes.len() - 1) % themes.len(),
                _ => {}
            },
            Ok(_) => {}
            Err(e) => break Err(e),
        }
    };
    ratatui::restore();
    result
}
