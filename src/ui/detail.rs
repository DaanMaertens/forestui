//! The detail pane's renderer, and the empty state.
//!
//! The pane is immediate-mode, so there is nothing that remembers where a
//! control was drawn. Instead every frame interprets the node list
//! `app::detail::content()` produces — the same list `App::detail_items()`
//! derives the focusable items from — and records the cells each control
//! landed on. Because renderer and key handler consume one walk, item N on
//! screen *is* item N in the handler by construction; the recorded cells are
//! what a click resolves against.

use crate::app::detail::{ControlSpec, DetailNode, Field};
use crate::app::{App, Focus, HitTarget, ScrollbarGeom};
use crate::theme;
use crate::ui::widgets::{TextInput, centered_rect};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

/// Width of the label column in the rename section.
const FIELD_LABEL_WIDTH: usize = 14;

/// Cells a card's border and padding take up on the left of its content, and
/// again on the right. Textual's cards were `border: solid` plus `padding: 1`.
const CARD_INSET: u16 = 2;

/// Cells left clear on the right of a card or a rule, from the `margin: 0 2 1 0`
/// Textual gave `.session-item` and `.issue-row` and the `margin: 1 2 1 0` it
/// gave `Rule.-horizontal`. Without it they run into the edge of the pane.
/// Columns kept clear to the right of every card. Three rather than two so a
/// card stops short of the scrollbar instead of running into it.
const RIGHT_MARGIN: u16 = 3;

/// Rows a control occupies, from Textual's `Button { border: solid; height: 3 }`:
/// its top border, its label, and its bottom border.
const CONTROL_HEIGHT: u16 = 3;

/// Columns the scrollbar occupies on the right of the pane, matching the two
/// Textual's scrollbar used.
const SCROLLBAR_WIDTH: u16 = 2;

/// `#detail-pane { padding: 1 2 }` — the pane's own inset from the divider.
const PANE_PAD_X: u16 = 2;
const PANE_PAD_Y: u16 = 1;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // `#detail-pane { padding: 1 2 }` — two columns each side and a blank row
    // at the top. The sidebar already draws the divider.
    let inner = Rect {
        x: area.x + PANE_PAD_X,
        y: area.y + PANE_PAD_Y,
        width: area.width.saturating_sub(2 * PANE_PAD_X),
        height: area.height.saturating_sub(PANE_PAD_Y),
    };

    let nodes = crate::app::detail::content(app);
    // Snapshot what this frame shows, enabled bits included: clicks and Enter
    // resolve against the frame the user saw, not a list a background event
    // may have reshaped since (see `App::drawn_items`).
    app.drawn_items = crate::app::detail::drawn(&nodes);
    // The pane can shrink under the cursor — the issue auto-refresh returning
    // fewer issues than last time is enough — and an index left past the end
    // resolves to nothing: the focus ring disappears and Enter does nothing
    // until the user presses Down. Clamped here because this is the one place
    // both consumers of the snapshot agree on its length.
    if app.detail_index >= app.drawn_items.len() {
        app.detail_index = app.drawn_items.len().saturating_sub(1);
    }
    if nodes.is_empty() {
        app.scrollbar = None;
        empty_state(frame, area);
        return;
    }
    let pane = render_nodes(app, nodes, inner.width);
    let offset = pane.resolve_offset(app, inner.height);
    let total = as_u16(pane.lines.len());
    frame.render_widget(Paragraph::new(pane.lines).scroll((offset, 0)), inner);

    // Published for the drag handler, which needs the same geometry the bar was
    // drawn with rather than its own copy of the arithmetic.
    app.scrollbar = if inner.width > SCROLLBAR_WIDTH {
        ScrollbarGeom::new(
            Rect {
                x: inner.x + inner.width - SCROLLBAR_WIDTH,
                y: inner.y,
                width: SCROLLBAR_WIDTH,
                height: inner.height,
            },
            total,
        )
    } else {
        None
    };
    draw_scrollbar(frame, app.scrollbar, offset);

    // The scroll offset decides which line ends up on which screen row, so this
    // is the earliest the clickable regions can be worked out.
    for (index, item) in pane.items.iter().enumerate() {
        // A control is three rows tall and so can straddle either edge of the
        // window. Clip it to what is on screen instead of dropping the region,
        // or a half-scrolled button would stop answering the mouse.
        let end = item.line.saturating_add(item.height);
        if end <= offset {
            continue;
        }
        let top = item.line.max(offset) - offset;
        if top >= inner.height {
            break;
        }
        app.push_hit(
            Rect {
                x: inner.x + item.x,
                y: inner.y + top,
                width: item.width,
                height: (end - offset - top).min(inner.height - top),
            },
            HitTarget::DetailItem(index),
        );
    }
}

/// Render the pane for the current selection, or `None` for the empty state.
/// Only tests call this; [`draw`] walks the content itself so it can snapshot
/// the drawn items first.
#[cfg(test)]
fn build(app: &App, width: u16) -> Option<Pane> {
    let nodes = crate::app::detail::content(app);
    if nodes.is_empty() {
        return None;
    }
    Some(render_nodes(app, nodes, width))
}

/// Interpret a node list into a laid-out pane.
fn render_nodes(app: &App, nodes: Vec<DetailNode>, width: u16) -> Pane {
    let focused = match app.focus {
        Focus::Detail => Some(app.detail_index),
        Focus::Sidebar => None,
    };
    // A hover only means something for the pane's own controls.
    let hovered = match app.hovered {
        Some(HitTarget::DetailItem(index)) => Some(index),
        _ => None,
    };
    let mut pane = Pane::new(focused, hovered, width);
    for node in nodes {
        render_node(&mut pane, app, node);
    }
    pane
}

/// Interpret one content node. This is the whole pane vocabulary; anything the
/// pane shows is one of these.
fn render_node(pane: &mut Pane, app: &App, node: DetailNode) {
    match node {
        DetailNode::Section(title) => pane.section(title),
        DetailNode::Text { text, style } => pane.text(text, style),
        DetailNode::Blank => pane.blank(),
        DetailNode::Rule => pane.rule(),
        DetailNode::PathBox { path, style } => path_box(pane, path, style),
        DetailNode::CardStart { padded } => {
            pane.card_start(pane.width.saturating_sub(RIGHT_MARGIN), padded);
        }
        DetailNode::CardEnd => pane.card_end(),
        DetailNode::Controls { lead, controls } => {
            let lead = lead
                .map(|(text, style)| vec![Span::styled(text, style)])
                .unwrap_or_default();
            pane.controls(lead, &controls);
        }
        DetailNode::CardBody { left, rows } => card_body(pane, left, rows),
        DetailNode::Field { field, label } => {
            let input = match field {
                Field::WorktreeName => &app.name_input,
                Field::BranchName => &app.branch_input,
            };
            rename_field(pane, label, input);
        }
        DetailNode::IssuesHeader { title, glyph } => issues_header(pane, title, glyph),
    }
}

/// The issues header with its inline refresh control.
fn issues_header(pane: &mut Pane, title: &'static str, glyph: char) {
    // `.refresh-btn { min-width: 3; width: 3 }` — the glyph is narrower than
    // the control, and it is the control that takes the click.
    let (focused, hovered) = pane.claim(as_u16(title.chars().count() + 1), 3, 1);
    pane.row(vec![
        Span::styled(title, theme::section_header()),
        Span::raw(" "),
        Span::styled(
            glyph.to_string(),
            if focused || hovered {
                theme::accent()
            } else {
                theme::secondary()
            },
        ),
    ]);
}

/// Where a focusable item landed. The line alone would not do: `Editor`,
/// `Terminal` and `Files` share one, so a click needs the x extent too.
struct Item {
    line: u16,
    x: u16,
    width: u16,
    /// Controls are [`CONTROL_HEIGHT`] rows; a rename field is one.
    height: u16,
}

/// Styles for a control's box: its border, then its label. The border carries
/// the state Textual gave it — accent under the cursor, the destructive shade
/// on a dangerous action — and the fill runs under the border cells too, which
/// is what `border: solid` did there.
fn control_styles(control: &ControlSpec, focused: bool, hovered: bool) -> (Style, Style) {
    // A control that cannot run must not light up under the pointer either:
    // an affordance that promises a click it will not honour is worse than
    // none. Textual disabled buttons the same way.
    let hovered = hovered && control.enabled;
    let fill = theme::action_bg(control.variant, hovered);
    let border = theme::action_border(focused, control.variant, hovered);
    // A control that cannot run keeps its box, so neither the layout nor the
    // hit region shifts; only the label goes grey — except under the cursor,
    // which has to stay legible.
    let label = if control.enabled || focused {
        theme::action(focused, control.variant, hovered)
    } else {
        theme::muted().bg(fill)
    };
    (border.bg(fill), label)
}

/// Rendered width of a control's box. Unlike the modal buttons, these have no
/// minimum width — the box hugs the label, which is how the pane laid out from
/// the start and what the committed sweep baselines show.
fn control_width(label: &str) -> u16 {
    crate::ui::widgets::boxed_width(label, 0)
}

/// The three rows of one control, drawn by the same box builder the modal
/// buttons use so the two cannot drift apart in shape.
fn control_box(control: &ControlSpec, focused: bool, hovered: bool) -> [Vec<Span<'static>>; 3] {
    let (border, label) = control_styles(control, focused, hovered);
    crate::ui::widgets::boxed_rows(&control.label, border, label, 0)
}

/// A card under construction — the bordered, elevated box Textual gave
/// `.session-item`, `.issue-row` and `.path-display`.
#[derive(Clone, Copy)]
struct Card {
    width: u16,
    /// Whether the card pads its content vertically. `.session-item` and
    /// `.issue-row` had `padding: 1`; `.path-display` padded only horizontally,
    /// so its box hugs the path.
    padded: bool,
}

/// Content under construction: the lines to render plus the cells each
/// focusable item claimed, in `App::detail_items()` order.
struct Pane {
    lines: Vec<Line<'static>>,
    items: Vec<Item>,
    /// Index of the focused item, or `None` while the sidebar owns focus.
    focused: Option<usize>,
    /// Index of the item the pointer is over, if any.
    hovered: Option<usize>,
    /// Width of the content area, so rules and cards can be filled out to it.
    width: u16,
    /// The card currently open, if any. See [`Pane::card_start`].
    card: Option<Card>,
}

impl Pane {
    fn new(focused: Option<usize>, hovered: Option<usize>, width: u16) -> Self {
        Self {
            lines: Vec::new(),
            items: Vec::new(),
            focused,
            hovered,
            width,
            card: None,
        }
    }

    fn text(&mut self, text: impl Into<String>, style: Style) {
        self.push_line(vec![Span::styled(text.into(), style)]);
    }

    /// A blank line — inside an open card, a *filled* one, so the card's
    /// background and borders carry through it rather than punching a hole.
    fn blank(&mut self) {
        self.push_line(Vec::new());
    }

    /// A section header. `.section-header { margin: 1 0 0 0 }` — the blank row
    /// above is unconditional, so the very first header is inset from the top of
    /// the pane exactly like every one after it.
    fn section(&mut self, title: &str) {
        self.blank();
        self.text(title.to_string(), theme::section_header());
    }

    /// The horizontal `Rule()` Textual drew between the major sections, with the
    /// blank line above it that its `margin` produced. The blank below is left to
    /// [`Pane::section`], which already spaces a header away from what precedes it.
    fn rule(&mut self) {
        self.blank();
        let width = self.width.saturating_sub(RIGHT_MARGIN);
        self.text("─".repeat(width as usize), theme::border());
    }

    /// Push a line built from spans returned by [`Pane::control`].
    fn row(&mut self, spans: Vec<Span<'static>>) {
        self.push_line(spans);
    }

    /// Open a card. Lines pushed until [`Pane::card_end`] land inside it.
    fn card_start(&mut self, width: u16, padded: bool) {
        self.lines.push(card_edge(width, '┌', '┐'));
        self.card = Some(Card { width, padded });
        if padded {
            self.blank();
        }
    }

    fn card_end(&mut self) {
        let Some(card) = self.card else {
            return;
        };
        if card.padded {
            self.blank();
        }
        self.card = None;
        self.lines.push(card_edge(card.width, '└', '┘'));
    }

    /// Push one content line, wrapped in the open card's border and background.
    fn push_line(&mut self, mut spans: Vec<Span<'static>>) {
        let Some(card) = self.card else {
            self.lines.push(Line::from(spans));
            return;
        };
        // One theme read for the whole line: consistent by construction, and
        // spares the lock a handful of reads per content row.
        let elevated = theme::active().bg_elevated;
        let width = card.width;
        let filled = Style::default().bg(elevated);
        // Text styles carry a foreground only, so without this the page colour
        // shows through the card. Spans that set their own background — the
        // controls — keep it, which is what makes one read as raised.
        for span in &mut spans {
            if span.style.bg.is_none() {
                span.style = span.style.bg(elevated);
            }
        }
        let content = as_u16(spans.iter().map(Span::width).sum());
        // Left border, left padding, content, then whatever is left over before
        // the padding and border on the right.
        let fill = width.saturating_sub(CARD_INSET + content + 1);
        let mut line = vec![
            Span::styled("│", theme::border().bg(elevated)),
            Span::styled(" ", filled),
        ];
        line.append(&mut spans);
        line.push(Span::styled(" ".repeat(fill as usize), filled));
        line.push(Span::styled("│", theme::border().bg(elevated)));
        self.lines.push(Line::from(line));
    }

    /// Claim the next focusable slot at `x`, `height` rows tall, starting on the
    /// line that is about to be pushed, and return whether it is the focused one.
    /// Callers must push those lines before adding any other, or the recorded
    /// cells drift from the content.
    fn claim(&mut self, x: u16, width: u16, height: u16) -> (bool, bool) {
        let focused = self.focused == Some(self.items.len());
        let hovered = self.hovered == Some(self.items.len());
        self.items.push(Item {
            line: as_u16(self.lines.len()),
            x,
            width,
            height,
        });
        (focused, hovered)
    }

    /// Lay out a row of controls after `lead`, which is whatever non-clickable
    /// text shares the line. Each control's extent is recorded as it is placed,
    /// because that running x offset is the only place it is ever known.
    ///
    /// The row costs three lines, since every control is a box: `lead` sits on
    /// the middle one, level with the labels.
    fn controls(&mut self, lead: Vec<Span<'static>>, controls: &[ControlSpec]) {
        // `.action-row { margin: 1 0 }` — a blank line above every row of
        // buttons. Inside a card the `padding: 1` already supplies it.
        let inset = match self.card {
            Some(_) => CARD_INSET,
            None => {
                self.blank();
                0
            }
        };
        let lead_width = as_u16(lead.iter().map(Span::width).sum());

        // `.session-buttons { align: right middle }` and `.issue-info { width:
        // 1fr }` both push a card's buttons against its right edge, one cell
        // clear of the border. Outside a card they stay left-aligned.
        let row_width = controls
            .iter()
            .map(|control| control_width(&control.label))
            .sum::<u16>()
            .saturating_add(as_u16(controls.len().saturating_sub(1)));
        let pad = self.card.map_or(0, |card| {
            card.width
                .saturating_sub(CARD_INSET + 2)
                .saturating_sub(lead_width.saturating_add(row_width))
        });

        let blank = |width: u16| Span::raw(" ".repeat(width as usize));
        let mut x = inset.saturating_add(lead_width).saturating_add(pad);
        let mut lead = lead;
        lead.push(blank(pad));
        // The lead rides the *last* of the three rows a boxed control occupies,
        // so a card's meta line sits along its bottom edge. On the middle row it
        // lands level with the button labels, which reads as text floating in
        // the middle of the card rather than as a caption under its content.
        let mut rows = [
            vec![blank(lead_width.saturating_add(pad))],
            vec![blank(lead_width.saturating_add(pad))],
            lead,
        ];

        for (index, control) in controls.iter().enumerate() {
            if index > 0 {
                for row in &mut rows {
                    row.push(blank(1));
                }
                x = x.saturating_add(1);
            }
            let width = control_width(&control.label);
            let (focused, hovered) = self.claim(x, width, CONTROL_HEIGHT);
            for (row, mut spans) in rows.iter_mut().zip(control_box(control, focused, hovered)) {
                row.append(&mut spans);
            }
            x = x.saturating_add(width);
        }

        for row in rows {
            self.push_line(row);
        }
    }

    /// The pane's scroll position for this frame.
    ///
    /// The offset lives on the app so the wheel can move it independently of the
    /// focus ring. Only content height — which is not known until the pane has
    /// been built — can clamp it, so the clamp happens here and is written back.
    fn resolve_offset(&self, app: &mut App, height: u16) -> u16 {
        let max = as_u16(self.lines.len()).saturating_sub(height);
        let mut offset = app.detail_scroll.min(max);

        // Keyboard navigation drags the viewport along so the cursor stays
        // visible. The wheel does not set this flag, which is what lets the user
        // scroll away from the focused control and read.
        if app.detail_follow_focus
            && let Some(item) = self.focused.and_then(|index| self.items.get(index))
        {
            let top = item.line;
            // Aim to clear the whole control plus two lines of context below it.
            let bottom = item.line.saturating_add(item.height).saturating_add(2);
            if top < offset {
                offset = top;
            } else if bottom > offset.saturating_add(height) {
                offset = bottom.saturating_sub(height);
            }
            offset = offset.min(max);
        }

        app.detail_follow_focus = false;
        app.detail_scroll = offset;
        offset
    }
}

/// A session card's body: text lines on the left, stacked button rows
/// floating flush right, sharing the same screen rows. Each visual row is
/// composed as left text (clipped so it can never run under a button), a
/// stretch of card fill, then that row's slice of the button boxes.
///
/// Claims happen exactly as [`Pane::controls`] makes them — top sub-row, three
/// rows tall, left to right, row after row — so the flattened order matches
/// what `detail::drawn` reports for the node.
fn card_body(pane: &mut Pane, left: Vec<Vec<(String, Style)>>, rows: Vec<Vec<ControlSpec>>) {
    let Some(card) = pane.card else {
        return;
    };
    // What `push_line` can hold between the card's left border+pad and its
    // right border.
    let content = card.width.saturating_sub(CARD_INSET + 1);
    // Right-aligned x (in content coordinates) and width of each button row.
    let geometry: Vec<(u16, u16)> = rows
        .iter()
        .map(|row| {
            let width = row
                .iter()
                .map(|control| control_width(&control.label))
                .sum::<u16>()
                .saturating_add(as_u16(row.len().saturating_sub(1)));
            (content.saturating_sub(width), width)
        })
        .collect();
    let button_rows = as_u16(rows.len()) * CONTROL_HEIGHT;
    let total = as_u16(left.len()).max(button_rows);

    // The three composed sub-rows of the button row currently being emitted.
    let mut pending: Vec<[Vec<Span<'static>>; 3]> = Vec::new();
    for i in 0..total {
        let sub = (i % CONTROL_HEIGHT) as usize;
        let row_index = (i / CONTROL_HEIGHT) as usize;
        let has_buttons = i < button_rows;

        if has_buttons && sub == 0 {
            pending.clear();
            let mut x = geometry[row_index].0;
            for (j, control) in rows[row_index].iter().enumerate() {
                if j > 0 {
                    x = x.saturating_add(1);
                }
                let width = control_width(&control.label);
                // +CARD_INSET: item x is measured from the pane edge, and the
                // card's border and padding sit before the content.
                let (focused, hovered) = pane.claim(x + CARD_INSET, width, CONTROL_HEIGHT);
                pending.push(control_box(control, focused, hovered));
                x = x.saturating_add(width);
            }
        }

        // The left text, clipped so it always stops short of the buttons.
        let avail = if has_buttons {
            geometry[row_index].0.saturating_sub(2)
        } else {
            content
        };
        let mut spans = clip_spans(left.get(i as usize), avail as usize);
        if has_buttons {
            let used = as_u16(spans.iter().map(Span::width).sum::<usize>());
            spans.push(Span::raw(
                " ".repeat(geometry[row_index].0.saturating_sub(used) as usize),
            ));
            for (j, tri) in pending.iter().enumerate() {
                if j > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.extend(tri[sub].iter().cloned());
            }
        }
        pane.push_line(spans);
    }
}

/// A left-column line cut to `avail` *columns*, span boundaries respected.
///
/// Display width, not chars: turns routinely carry emoji, which occupy two
/// columns each — clipped by character count, a line of them ran under the
/// gap and pushed the card's right border out of alignment. Measured with
/// the same `unicode-width` the padding math (`Span::width`) resolves to, so
/// the two can never disagree about where the text ends.
fn clip_spans(line: Option<&Vec<(String, Style)>>, avail: usize) -> Vec<Span<'static>> {
    use unicode_width::UnicodeWidthChar;
    let mut spans = Vec::new();
    let mut used = 0usize;
    for (text, style) in line.into_iter().flatten() {
        if used >= avail {
            break;
        }
        let mut piece = String::new();
        for ch in text.chars() {
            let width = ch.width().unwrap_or(0);
            if used + width > avail {
                break;
            }
            used += width;
            piece.push(ch);
        }
        spans.push(Span::styled(piece, *style));
        if used >= avail {
            break;
        }
    }
    spans
}

/// The scrollbar on the right edge, drawn only when the content is taller than
/// the pane. Without one nothing on screen says the pane continues below the
/// fold.
///
/// Painted the way Textual painted it — two columns of background colour on
/// blank cells, rather than a glyph per row. A column of `│` would read as a
/// pane border, and it would also put a character on every row of the captured
/// text frames, burying real changes in the sweep diffs.
fn draw_scrollbar(frame: &mut Frame, geom: Option<ScrollbarGeom>, offset: u16) {
    let Some(geom) = geom else {
        return;
    };
    let top = geom.thumb_top(offset);
    let palette = theme::active();
    for row in 0..geom.track.height {
        let inside = row >= top && row < top.saturating_add(geom.thumb);
        let colour = if inside {
            palette.accent_dark
        } else {
            palette.scrollbar_trough
        };
        frame.render_widget(
            Block::default().style(Style::default().bg(colour)),
            Rect {
                y: geom.track.y + row,
                height: 1,
                ..geom.track
            },
        );
    }
}

/// Terminal geometry is `u16` while content lengths are `usize`.
fn as_u16(len: usize) -> u16 {
    u16::try_from(len).unwrap_or(u16::MAX)
}

/// Top or bottom edge of a card.
fn card_edge(width: u16, left: char, right: char) -> Line<'static> {
    let span = width.saturating_sub(2) as usize;
    Line::from(Span::styled(
        format!("{left}{}{right}", "─".repeat(span)),
        theme::border().bg(theme::active().bg_elevated),
    ))
}

/// A path in the boxed, elevated `.path-display` Textual used. The box hugs the
/// path rather than filling the pane, which is how it laid out there.
fn path_box(pane: &mut Pane, path: String, style: Style) {
    // A forest path routinely outruns the pane. Cutting it here rather than
    // letting the paragraph clip it keeps the box's right edge on screen; the
    // tail was lost either way.
    let room = pane.width.saturating_sub(2 * CARD_INSET) as usize;
    // `truncate` adds an ellipsis, so it needs three of those cells to itself.
    let path = if path.chars().count() > room {
        crate::util::truncate(&path, room.saturating_sub(3))
    } else {
        path
    };
    let width = as_u16(path.chars().count()).saturating_add(2 * CARD_INSET);
    pane.card_start(width, false);
    pane.text(path, style);
    pane.card_end();
}

/// A rename field. The caret is drawn in the text because the pane renders as
/// one scrolled paragraph and has no real terminal cursor to place.
fn rename_field(pane: &mut Pane, label: &str, input: &TextInput) {
    // Measured without consulting focus: the caret only ever occupies the one
    // trailing cell already counted here, so the click target is stable.
    let width = as_u16(FIELD_LABEL_WIDTH + 2 + input.value().chars().count());
    let (focused, _hovered) = pane.claim(0, width, 1);
    let mut spans = vec![Span::styled(
        format!("{label:<FIELD_LABEL_WIDTH$} "),
        theme::secondary(),
    )];

    if focused {
        let chars: Vec<char> = input.value().chars().collect();
        let cursor = input.cursor().min(chars.len());
        let before: String = chars[..cursor].iter().collect();
        let after: String = chars
            .get(cursor + 1..)
            .map(|rest| rest.iter().collect())
            .unwrap_or_default();
        // A cursor past the last character needs a cell of its own to sit in.
        let at = chars.get(cursor).copied().unwrap_or(' ');
        spans.push(Span::styled(before, theme::primary()));
        spans.push(Span::styled(at.to_string(), theme::cursor()));
        spans.push(Span::styled(after, theme::primary()));
    } else {
        spans.push(Span::styled(input.value().to_string(), theme::primary()));
    }
    pane.row(spans);
}

/// Nothing selected. The Textual version put this inside a zero-height
/// container, so it never appeared; centring it is what that build intended.
fn empty_state(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(" forestui", theme::accent())),
        Line::from(Span::styled("Git Worktree Manager", theme::secondary())),
        Line::default(),
        Line::from(Span::styled(
            "Select a repository or worktree",
            theme::muted(),
        )),
        Line::from(Span::styled(
            "or press [a] to add a repository",
            theme::muted(),
        )),
    ];
    let rect = centered_rect(area.width, as_u16(lines.len()), area);
    frame.render_widget(Paragraph::new(lines).centered(), rect);
}

#[cfg(test)]
mod tests;
