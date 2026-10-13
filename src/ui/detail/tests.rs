use super::*;
use crate::app::{Action, DetailItem, Field};
use crate::event;
use crate::models::{ClaudeSession, CustomClaudeButton, Repository, Settings, Worktree};
use crate::services::settings as settings_service;
use crate::state::AppState;
use chrono::Utc;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

/// An app whose state file lives in a throwaway forest directory.
///
/// `App::new` reads the real settings and forest path, so both are pointed
/// at temporary locations first and then replaced outright.
fn test_app(dir: &tempfile::TempDir) -> App {
    settings_service::set_forest_path(dir.path().to_str());
    // Dropping the receiver is fine: sends are best-effort and no test reads
    // events back. Explicit default settings, never `App::new`: that reads
    // the developer's real ~/.config/forestui/settings.json and would
    // activate their personal theme, making test rendering machine-dependent.
    let (tx, _rx) = event::test_channel();
    let state = AppState::load_from(dir.path().join(".forestui-config.json"));
    App::with_state(tx, state, Settings::default())
}

fn with_repository(app: &mut App) {
    let repository = Repository::new("demo".into(), "/tmp/demo".into());
    let repo_id = repository.id;
    app.state.add_repository(repository);
    app.state.select_repository(repo_id);
}

fn with_worktree(app: &mut App) {
    let repository = Repository::new("demo".into(), "/tmp/demo".into());
    let repo_id = repository.id;
    app.state.add_repository(repository);
    let worktree = Worktree::new("wt".into(), "feat/x".into(), "/tmp/forest/demo/wt".into());
    let worktree_id = worktree.id;
    app.state.add_worktree(repo_id, worktree);
    app.state.select_worktree(repo_id, worktree_id);
}

fn a_session() -> ClaudeSession {
    ClaudeSession {
        custom_title: None,
        id: "abc".into(),
        title: "Refactor the detail pane".into(),
        recent_turns: Vec::new(),
        last_timestamp: Utc::now(),
        message_count: 12,
        git_branch: None,
        tokens: Default::default(),
        model: None,
    }
}

/// One frame into a throwaway terminal. `crate::ui::draw` is what clears
/// hits in the real app, so the test does it here.
fn render_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    app.clear_hits();
    terminal
        .draw(|frame| draw(frame, app, frame.area()))
        .expect("draw");
    terminal.backend().buffer().clone()
}

/// Three-row controls and padded cards make the pane far taller than the
/// terminal it usually runs in, so tests that want the whole thing on screen
/// unscrolled have to ask for the room.
const TALL: u16 = 80;

fn render(app: &mut App) -> String {
    buffer_text(&render_buffer(app, 100, TALL))
}

/// Cell of the first occurrence of `needle`. Byte offsets would not do: the
/// pill caps around every control are multi-byte.
fn find_cell(buffer: &Buffer, needle: &str) -> (u16, u16) {
    find_cell_nth(buffer, needle, 0)
}

/// Cell of the `nth` occurrence, top-to-bottom then left-to-right — for
/// labels the standardized launch rows deliberately repeat.
fn find_cell_nth(buffer: &Buffer, needle: &str, nth: usize) -> (u16, u16) {
    let width = buffer.area.width as usize;
    let mut seen = 0usize;
    for (y, row) in buffer.content.chunks(width).enumerate() {
        let line: String = row.iter().map(|cell| cell.symbol()).collect();
        let mut from = 0usize;
        while let Some(byte) = line[from..].find(needle) {
            let byte = from + byte;
            if seen == nth {
                return (as_u16(line[..byte].chars().count()), as_u16(y));
            }
            seen += 1;
            from = byte + needle.len();
        }
    }
    panic!("occurrence {nth} of {needle:?} is not on screen");
}

fn detail_hits(app: &App) -> usize {
    app.hits
        .iter()
        .filter(|hit| matches!(hit.target, HitTarget::DetailItem(_)))
        .count()
}

fn buffer_text(buffer: &Buffer) -> String {
    let width = buffer.area.width as usize;
    buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Textual put a `Rule()` between the major sections, and the pane read as
/// one flat list without them.
#[tokio::test]
async fn sections_are_separated_by_rules() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_repository(&mut app);
    // Loaded but empty, so the whole pane fits on screen unscrolled.
    app.sessions = Some(vec![]);

    /// Whether a rule sits between two lines. A rule is the only line that is
    /// nothing but the horizontal glyph; a card's edges start with a corner.
    fn rule_between(screen: &str, from: &str, to: &str) -> bool {
        let rows: Vec<&str> = screen.lines().collect();
        let row_of = |needle: &str| {
            rows.iter()
                .position(|row| row.contains(needle))
                .unwrap_or_else(|| panic!("{needle:?} is not on screen:\n{screen}"))
        };
        (row_of(from)..row_of(to)).any(|y| rows[y].trim().starts_with("──"))
    }

    let screen = render(&mut app);
    assert!(
        rule_between(&screen, "Repository: demo", "LOCATION"),
        "{screen}"
    );
    assert!(rule_between(&screen, "LOCATION", "OPEN IN"), "{screen}");
    assert!(rule_between(&screen, "OPEN IN", "CLAUDE"), "{screen}");
    assert!(
        rule_between(&screen, "RECENT SESSIONS", "MANAGE"),
        "{screen}"
    );
    // Nothing sits between the sessions and MANAGE any more, so one rule,
    // not the two the issues section left behind when it went.
    let rows: Vec<&str> = screen.lines().collect();
    let row_of = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} is not on screen:\n{screen}"))
    };
    let rules = rows[row_of("RECENT SESSIONS")..row_of("MANAGE")]
        .iter()
        .filter(|row| row.trim().starts_with("──"))
        .count();
    assert_eq!(rules, 1, "{screen}");
    assert!(!screen.contains("GITHUB ISSUES"), "{screen}");
    // CLAUDE ran straight into RECENT SESSIONS there, and does here too.
    assert!(
        !rule_between(&screen, "CLAUDE", "RECENT SESSIONS"),
        "{screen}"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    app.state = AppState::load_from(dir.path().join(".forestui-config.json"));
    with_worktree(&mut app);
    let screen = render(&mut app);
    assert!(
        rule_between(&screen, "RECENT SESSIONS", "RENAME"),
        "{screen}"
    );
    assert!(rule_between(&screen, "Branch name", "MANAGE"), "{screen}");
}

/// Sessions were `.session-item` boxes, not bare lines: a border in the
/// border colour over an elevated background.
#[tokio::test]
async fn sessions_render_as_cards() {
    // This test compares rendered colours against the active theme, so it
    // pins the theme for its whole render-and-assert window.
    let _guard = crate::theme::test_lock();
    crate::theme::set_active("forest-dark");
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_repository(&mut app);
    app.sessions = Some(vec![a_session()]);
    let buffer = render_buffer(&mut app, 100, TALL);
    // The pane is inset by its own padding and the card keeps a right margin
    // inside that, so the card's edge lands short of the screen edge.
    let right = buffer.area.width - PANE_PAD_X - RIGHT_MARGIN - 1;

    // A session card is unpadded — its name carries the separation
    // instead, so its top border sits directly above the name.
    let label = "Refactor the detail pane";
    let (x, y) = find_cell(&buffer, label);
    let cell = |x: u16, y: u16| {
        buffer
            .cell((x, y))
            .unwrap_or_else(|| panic!("{label}: no cell at {x},{y}"))
    };

    let left = x - CARD_INSET;
    assert_eq!(cell(left, y - 1).symbol(), "┌", "{label}: top left");
    assert_eq!(cell(right, y - 1).symbol(), "┐", "{label}: top right");
    assert_eq!(cell(left, y).symbol(), "│", "{label}: left border");
    assert_eq!(cell(right, y).symbol(), "│", "{label}: right border");
    assert_eq!(
        cell(left, y).fg,
        theme::active().border,
        "{label}: border colour"
    );
    // The background has to reach past the text, or the card is a frame
    // around the page colour rather than a filled box. Probed just
    // after the label rather than at the far edge: on a session card
    // the right side of the title row is a floating button, whose box
    // carries its own fill.
    let past_label = x + as_u16(label.chars().count()) + 2;
    for probe in [x, past_label] {
        assert_eq!(
            cell(probe, y).bg,
            theme::active().bg_elevated,
            "{label}: background at {probe}",
        );
    }
}

/// The session card's new furniture: the live badge for an open session,
/// the branch line, the spend on the meta line, the pin marker, and a Del
/// that cannot fire while a window holds the transcript.
#[tokio::test]
async fn session_cards_show_liveness_pins_and_spend() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_worktree(&mut app);
    let path = app.state.selected_path().expect("a selection");

    let mut session = a_session();
    session.git_branch = Some("feat/x".into());
    session.model = Some("claude-opus-5".into());
    session.tokens = crate::models::TokenUsage {
        input: 1_000,
        cache_write: 1_199_000,
        cache_read: 0,
        output: 50_000,
    };
    app.sessions = Some(vec![session]);
    app.state.toggle_pin(&path, "abc");
    app.live_sessions.insert(
        "abc".into(),
        crate::services::live::LiveSession {
            session_id: "abc".into(),
            place: crate::services::live::LivePlace::Window {
                window_id: "@9".into(),
                window_name: "claude:demo:wt".into(),
            },
            state: None,
            cwd: None,
        },
    );
    let screen = render(&mut app);
    // The badge rides the title line, naming the window only because it
    // differs from the session name.
    assert!(
        screen.contains("Refactor the detail pane · ● live in claude:demo:wt"),
        "{screen}"
    );
    assert!(screen.contains("on branch feat/x"), "{screen}");
    // The buttons float on the right of the card's own rows rather than
    // taking a full-width band: the launch row starts level with the
    // title, the manage row below it, and the left column keeps its own
    // order — branch above meta at the bottom.
    let rows: Vec<&str> = screen.lines().collect();
    let row_of = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} is not on screen:\n{screen}"))
    };
    assert!(row_of("Rename") > row_of("Claude │"), "{screen}");
    assert!(row_of("12 msgs") > row_of("on branch feat/x"), "{screen}");
    // The spend rides the meta line, beside the count it explains.
    assert!(
        screen.contains("12 msgs • 1.2M in / 50k out • ~$"),
        "{screen}"
    );
    assert!(screen.contains("◆ Refactor the detail pane"), "{screen}");
    assert!(screen.contains("Unpin"), "{screen}");

    // Del keeps its slot but is disabled while the window is open.
    let del = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Action(Action::DeleteSession(0)))
        .expect("Del renders");
    render_buffer(&mut app, 100, TALL);
    assert_eq!(
        app.drawn_items.get(del).map(|(_, enabled)| *enabled),
        Some(false),
        "Del must not fire on a live session"
    );

    // A window named exactly after the session — the designed steady
    // state — is not worth repeating: the badge alone says live.
    if let crate::services::live::LivePlace::Window { window_name, .. } =
        &mut app.live_sessions.get_mut("abc").unwrap().place
    {
        *window_name = "Refactor the detail pane".to_string();
    }
    let screen = render(&mut app);
    assert!(
        screen.contains("Refactor the detail pane · ● live"),
        "{screen}"
    );
    assert!(!screen.contains("● live in"), "{screen}");

    // No window at all: no badge, Del enabled again.
    app.live_sessions.clear();
    let screen = render(&mut app);
    assert!(!screen.contains("● live"), "{screen}");
    render_buffer(&mut app, 100, TALL);
    assert_eq!(
        app.drawn_items.get(del).map(|(_, enabled)| *enabled),
        Some(true)
    );
}

/// The live badge says what the session is doing, the word in its state's
/// colour, after the place — and a session with no known state keeps the
/// badge it always had.
#[tokio::test]
async fn the_live_badge_carries_the_session_state() {
    use crate::services::live::{LivePlace, LiveSession, SessionState};
    let _theme = crate::theme::test_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_worktree(&mut app);
    app.sessions = Some(vec![a_session()]);

    let places = [
        (
            LivePlace::Window {
                window_id: "@9".into(),
                window_name: "Refactor the detail pane".into(),
            },
            "● live",
        ),
        (
            LivePlace::Window {
                window_id: "@9".into(),
                window_name: "claude:demo:wt".into(),
            },
            "● live in claude:demo:wt",
        ),
        (
            LivePlace::OtherSession {
                window_id: "@9".into(),
                window_name: "hand-made".into(),
                session_name: "work".into(),
                last_window: false,
            },
            "● live in tmux session work",
        ),
        (LivePlace::Elsewhere { pid: 7 }, "● live elsewhere"),
    ];
    for (place, badge) in places {
        for state in [
            SessionState::Busy,
            SessionState::NeedsYou,
            SessionState::Idle,
        ] {
            app.live_sessions.insert(
                "abc".into(),
                LiveSession {
                    session_id: "abc".into(),
                    place: place.clone(),
                    state: Some(state),
                    cwd: None,
                },
            );
            let buffer = render_buffer(&mut app, 100, TALL);
            let screen = buffer_text(&buffer);
            let expected = format!("{badge} · {}", state.label());
            assert!(screen.contains(&expected), "{expected:?}:\n{screen}");
            let (x, y) = find_cell(&buffer, &format!("· {}", state.label()));
            assert_eq!(
                buffer[(x + 2, y)].fg,
                crate::theme::session_state(state).fg.unwrap(),
                "{state:?} is not in its colour"
            );
        }

        app.live_sessions.get_mut("abc").unwrap().state = None;
        let screen = render(&mut app);
        let line = screen
            .lines()
            .find(|line| line.contains("● live"))
            .expect("a badge");
        assert!(line.contains(badge), "{line}");
        assert!(
            !line.contains(&format!("{badge} ·")),
            "an unknown state adds nothing: {line}"
        );
    }
}

/// Turns routinely carry emoji, which occupy two columns each. Clipped by
/// character count instead of display width, a wide-charactered line ran
/// under the gap beside the floating buttons and pushed the card's right
/// border out of line. Every row of the card must keep its border in the
/// same column, whatever the text is made of.
#[tokio::test]
async fn wide_characters_never_break_the_card_border() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_worktree(&mut app);
    let mut session = a_session();
    session.recent_turns = vec![
            crate::models::SessionTurn {
                speaker: crate::models::Speaker::User,
                // Sixty bananas: at two columns each, far past where the
                // buttons start, so a char-counted clip visibly overflows.
                text: "🍌".repeat(60),
            },
            crate::models::SessionTurn {
                speaker: crate::models::Speaker::Claude,
                text: "Anytime, painho. 🍌 Two threads landed today, for the record and then some more words".into(),
            },
        ];
    app.sessions = Some(vec![session]);
    let buffer = render_buffer(&mut app, 100, TALL);

    let (x, y) = find_cell(&buffer, "Refactor the detail pane");
    let left = x - CARD_INSET;
    // Walk from the card's top border to its bottom border and demand the
    // right border column agrees with the top-right corner on every row.
    let top = y - 1;
    let width = buffer.area.width;
    let right = (left..width)
        .rev()
        .find(|&col| buffer.cell((col, top)).is_some_and(|c| c.symbol() == "┐"))
        .expect("a top-right corner");
    let mut row = top + 1;
    loop {
        let cell = buffer.cell((right, row)).expect("inside the buffer");
        if cell.symbol() == "┘" {
            break;
        }
        assert_eq!(
            cell.symbol(),
            "│",
            "row {row}: the right border broke:\n{}",
            buffer_text(&buffer)
        );
        row += 1;
        assert!(row < buffer.area.height, "no bottom corner found");
    }
}

#[tokio::test]
async fn empty_state_is_visible() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    let screen = render(&mut app);

    // The Textual build laid this out into zero height and showed nothing.
    assert!(screen.contains("forestui"), "{screen}");
    assert!(screen.contains("Git Worktree Manager"), "{screen}");
    assert!(
        screen.contains("Select a repository or worktree"),
        "{screen}"
    );
    assert!(
        screen.contains("or press [a] to add a repository"),
        "{screen}"
    );
}

#[tokio::test]
async fn repository_detail_renders_its_sections() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_repository(&mut app);
    let screen = render(&mut app);

    assert!(screen.contains("MAIN REPOSITORY"), "{screen}");
    assert!(screen.contains("Repository: demo"), "{screen}");
    assert!(screen.contains("LOCATION"), "{screen}");
    assert!(screen.contains("/tmp/demo"), "{screen}");
    assert!(screen.contains("CLAUDE"), "{screen}");
    assert!(screen.contains("│ Claude │"), "{screen}");
}

#[tokio::test]
async fn missing_worktree_directory_is_flagged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_worktree(&mut app);
    app.meta = event::DetailMeta {
        path: "/tmp/forest/demo/wt".into(),
        path_exists: false,
        ..event::DetailMeta::default()
    };
    let screen = render(&mut app);

    assert!(screen.contains("⚠ MISSING"), "{screen}");
    assert!(screen.contains("(missing)"), "{screen}");
    assert!(screen.contains("Git Pull (Directory missing)"), "{screen}");
}

/// The invariant: rendered control N is `detail_items()[N]`, so the counts
/// must agree for every selection and every combination of loaded data.
#[tokio::test]
async fn rendered_controls_match_detail_items() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    app.settings.custom_buttons = vec![
        CustomClaudeButton {
            label: "Opus".into(),
            prefix: "opus".into(),
            command: "claude --model opus".into(),
        },
        CustomClaudeButton {
            label: "Disc".into(),
            prefix: "disc".into(),
            command: "claude --dangerously-skip-permissions".into(),
        },
    ];

    with_repository(&mut app);
    for sessions in [None, Some(vec![]), Some(vec![a_session(), a_session()])] {
        app.sessions = sessions.clone();
        let pane = build(&app, 100).expect("repository pane");
        assert_eq!(
            pane.items.len(),
            app.detail_items().len(),
            "repository, {} sessions",
            app.sessions.as_ref().map_or(-1, |s| s.len() as i64),
        );
    }

    let dir = tempfile::tempdir().expect("tempdir");
    app.state = AppState::load_from(dir.path().join(".forestui-config.json"));
    with_worktree(&mut app);
    for sessions in [None, Some(vec![]), Some(vec![a_session()])] {
        app.sessions = sessions.clone();
        let pane = build(&app, 100).expect("worktree pane");
        assert_eq!(
            pane.items.len(),
            app.detail_items().len(),
            "worktree, {} sessions",
            app.sessions.as_ref().map_or(-1, |s| s.len() as i64),
        );
    }
}

#[tokio::test]
async fn focused_item_is_scrolled_into_view() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_repository(&mut app);
    app.sessions = Some(vec![a_session(), a_session(), a_session()]);
    app.focus = Focus::Detail;
    app.detail_index = app.detail_items().len() - 1;

    app.detail_follow_focus = true;
    let pane = build(&app, 100).expect("repository pane");
    let row = pane.items[app.detail_index].line;
    let offset = pane.resolve_offset(&mut app, 20);
    assert!(
        row >= offset && row < offset + 20,
        "row {row}, offset {offset}"
    );
    // Following is a one-shot: it must not fight the wheel on later frames.
    assert!(!app.detail_follow_focus);
}

/// `draw` is the only thing that ever writes `App::drawn_items`, and every
/// activation — Enter and click alike — resolves against it. Delete that one
/// line and the whole pane goes inert while every other test stays green,
/// because they all snapshot it by hand.
#[tokio::test]
async fn drawing_the_pane_snapshots_what_it_drew() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_repository(&mut app);
    app.sessions = Some(vec![a_session()]);
    app.drawn_items.clear();

    render_buffer(&mut app, 100, TALL);

    assert!(!app.drawn_items.is_empty(), "the frame snapshotted nothing");
    assert_eq!(
        app.drawn_items,
        crate::app::detail::drawn(&crate::app::detail::content(&app)),
        "the snapshot is not what the frame drew"
    );
}

/// The pane shrinks under the cursor whenever a session scan comes back
/// with fewer sessions than last time. An index left past the end is a
/// focus ring that vanishes and an Enter that does nothing.
#[tokio::test]
async fn a_shrinking_pane_pulls_the_cursor_back_into_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_repository(&mut app);
    app.sessions = Some(vec![a_session(), a_session(), a_session()]);
    app.focus = Focus::Detail;
    app.detail_index = app.detail_items().len() - 1;
    render_buffer(&mut app, 100, TALL);

    // Two transcripts were deleted elsewhere since the last scan.
    app.sessions = Some(vec![a_session()]);
    render_buffer(&mut app, 100, TALL);

    assert!(
        app.detail_index < app.drawn_items.len(),
        "index {} is past the end of {} items",
        app.detail_index,
        app.drawn_items.len()
    );
    assert!(
        app.drawn_items.get(app.detail_index).is_some(),
        "the focused slot resolves to nothing, so Enter is dead"
    );
}

/// The wheel scrolls the pane whoever has focus, which is the whole point of
/// keeping the offset on the app rather than deriving it from the cursor.
#[tokio::test]
async fn the_pane_scrolls_while_the_sidebar_has_focus() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_repository(&mut app);
    app.sessions = Some(vec![a_session(), a_session(), a_session()]);
    app.focus = Focus::Sidebar;

    app.detail_scroll = 6;
    app.detail_follow_focus = false;
    let pane = build(&app, 100).expect("repository pane");
    assert_eq!(pane.resolve_offset(&mut app, 20), 6);
}

/// Scrolling past the end would leave the pane showing blank rows.
#[tokio::test]
async fn the_offset_is_clamped_to_the_content() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_repository(&mut app);
    app.focus = Focus::Sidebar;
    app.detail_scroll = u16::MAX;
    app.detail_follow_focus = false;

    let pane = build(&app, 100).expect("repository pane");
    let total = as_u16(pane.lines.len());
    let offset = pane.resolve_offset(&mut app, 20);
    assert_eq!(offset, total.saturating_sub(20));
    assert_eq!(app.detail_scroll, offset, "the clamp is written back");
}

/// Clicking a control has to reach the item drawn under the pointer, which
/// is what the bug report said was broken.
#[tokio::test]
async fn clicking_a_control_maps_to_its_item() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_worktree(&mut app);
    app.sessions = Some(vec![a_session()]);
    let buffer = render_buffer(&mut app, 100, TALL);

    let index_of = |item: &DetailItem| {
        app.detail_items()
            .iter()
            .position(|candidate| candidate == item)
            .expect("item is rendered")
    };

    // Editor, Terminal and Files share a line, so getting these three right
    // is what proves the x extents are, and not just the rows. "Claude"
    // appears twice by design — the CLAUDE section's launch button and the
    // card's resume button share the standardized label — so both
    // occurrences are asserted onto their own actions.
    for (label, nth, item) in [
        ("Editor", 0, DetailItem::Action(Action::Editor)),
        ("Terminal", 0, DetailItem::Action(Action::Terminal)),
        ("Files", 0, DetailItem::Action(Action::Files)),
        ("Claude", 0, DetailItem::Action(Action::ClaudeNew)),
        ("Claude", 1, DetailItem::Action(Action::ResumeSession(0))),
        ("Delete", 0, DetailItem::Action(Action::Delete)),
        ("Worktree name", 0, DetailItem::Field(Field::WorktreeName)),
    ] {
        let (x, y) = find_cell_nth(&buffer, label, nth);
        assert_eq!(
            app.hit_at(x, y),
            Some(HitTarget::DetailItem(index_of(&item))),
            "{label}#{nth} at {x},{y}"
        );
    }
}

/// Textual drew every `Button` as a bordered box three rows tall, and its
/// border is part of the button: clicking the top edge has to fire the same
/// action as clicking the label under it.
#[tokio::test]
async fn a_control_is_three_rows_and_clickable_on_its_border() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    with_repository(&mut app);
    app.sessions = Some(vec![]);
    let buffer = render_buffer(&mut app, 100, TALL);

    let (x, y) = find_cell(&buffer, "Start work");
    let symbol = |x: u16, y: u16| {
        buffer
            .cell((x, y))
            .unwrap_or_else(|| panic!("no cell at {x},{y}"))
            .symbol()
            .to_string()
    };

    // The label sits behind the left border and its padding cell.
    let left = x - 2;
    assert_eq!(symbol(left, y - 1), "┌", "top left");
    assert_eq!(symbol(left, y), "│", "left border");
    assert_eq!(symbol(left, y + 1), "└", "bottom left");

    let target = app.hit_at(x, y);
    assert!(
        matches!(target, Some(HitTarget::DetailItem(_))),
        "the label is clickable: {target:?}"
    );
    assert_eq!(app.hit_at(x, y - 1), target, "top border row");
    assert_eq!(app.hit_at(x, y + 1), target, "bottom border row");
    // One row further out is a different item, or none at all.
    assert_ne!(app.hit_at(x, y + 2), target, "below the box");
}

#[tokio::test]
async fn every_focusable_item_is_clickable_when_visible() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = test_app(&dir);
    app.settings.custom_buttons = vec![CustomClaudeButton {
        label: "Opus".into(),
        prefix: "opus".into(),
        command: "claude --model opus".into(),
    }];
    app.sessions = Some(vec![a_session()]);

    with_worktree(&mut app);
    // Tall and wide enough that nothing is scrolled or clipped away.
    render_buffer(&mut app, 120, TALL);
    assert_eq!(detail_hits(&app), app.detail_items().len(), "worktree");

    let dir = tempfile::tempdir().expect("tempdir");
    app.state = AppState::load_from(dir.path().join(".forestui-config.json"));
    with_repository(&mut app);
    render_buffer(&mut app, 120, TALL);
    assert_eq!(detail_hits(&app), app.detail_items().len(), "repository");
}
