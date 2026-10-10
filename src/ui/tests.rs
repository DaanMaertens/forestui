use super::*;

fn header_row(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.width)
        .map(|col| buffer[(col, 0)].symbol().to_string())
        .collect()
}

/// With nothing pending the header is the bare, centred title — the same
/// bar every baseline frame was captured against.
#[tokio::test]
async fn the_header_is_the_bare_title_with_no_update_pending() {
    use crate::app::test_support::app_with_fixture;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let (_dir, mut app) = app_with_fixture();
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
    terminal.draw(|frame| draw(frame, &mut app)).expect("draw");

    let row = header_row(&terminal);
    let title = app.title();
    assert_eq!(row.trim(), title);
    assert_eq!(row.find(&title), Some((120 - title.width()) / 2));
}

/// The acceptance case: after an update installs, the header reads
/// `forestui vX (vY ready — restart to update)`, centred as one piece,
/// with the suffix in the warning colour so it is seen, not just read.
#[tokio::test]
async fn a_pending_update_extends_the_title_bar() {
    use crate::app::test_support::app_with_fixture;
    use crate::version_check::UpdateStatus;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    // The colours are asserted against the active theme, which other
    // tests switch; hold it still for the draw and the comparison.
    let _guard = crate::theme::test_lock();
    let (_dir, mut app) = app_with_fixture();
    app.handle_event(crate::event::AppEvent::UpdateChecked(
        UpdateStatus::Installed("9.9.9".into()),
    ));
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
    terminal.draw(|frame| draw(frame, &mut app)).expect("draw");

    let row = header_row(&terminal);
    let expected = format!("{} (v9.9.9 ready — restart to update)", app.title());
    assert_eq!(row.trim(), expected);
    assert_eq!(row.find(&expected), Some((120 - expected.width()) / 2));

    let buffer = terminal.backend().buffer();
    let suffix_col = row[..row.find("(v9.9.9").expect("suffix drawn")].width() as u16;
    let title_col = row[..row.find("forestui").expect("title drawn")].width() as u16;
    assert_eq!(buffer[(suffix_col, 0)].fg, theme::active().warning);
    assert_eq!(buffer[(title_col, 0)].fg, theme::active().text_primary);
}

/// A bar too narrow for the full sentence keeps the version and the verb
/// rather than clipping the part that says what to do.
#[tokio::test]
async fn a_narrow_header_uses_the_compact_update_notice() {
    use crate::app::test_support::app_with_fixture;
    use crate::version_check::UpdateStatus;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let (_dir, mut app) = app_with_fixture();
    app.handle_event(crate::event::AppEvent::UpdateChecked(
        UpdateStatus::Installed("9.9.9".into()),
    ));
    let full = format!("{} (v9.9.9 ready — restart to update)", app.title());
    let compact = format!("{} (v9.9.9 — restart)", app.title());
    let width = (full.width() - 1) as u16;
    assert!(compact.width() <= width as usize);

    let mut terminal = Terminal::new(TestBackend::new(width, 20)).expect("test terminal");
    terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
    assert_eq!(header_row(&terminal).trim(), compact);

    // Exactly wide enough for the full wording: it is used.
    let mut terminal =
        Terminal::new(TestBackend::new(full.width() as u16, 20)).expect("test terminal");
    terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
    assert_eq!(header_row(&terminal).trim(), full);

    // Too narrow even for the compact form beside the title: the running
    // version gives way, so "restart" is never the part that gets clipped.
    let mut terminal =
        Terminal::new(TestBackend::new((compact.width() - 1) as u16, 20)).expect("test terminal");
    terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
    assert_eq!(header_row(&terminal).trim(), "(v9.9.9 — restart)");
}

/// A cargo install cannot replace itself, so its notice names the command
/// to run instead of a restart — and is just as sticky.
#[tokio::test]
async fn a_cargo_install_header_points_at_cargo() {
    use crate::app::test_support::app_with_fixture;
    use crate::version_check::UpdateStatus;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let (_dir, mut app) = app_with_fixture();
    app.handle_event(crate::event::AppEvent::UpdateChecked(
        UpdateStatus::Available("9.9.9".into()),
    ));
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
    terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
    assert_eq!(
        header_row(&terminal).trim(),
        format!(
            "{} (v9.9.9 available — cargo install forestui)",
            app.title()
        )
    );
}

/// The toast is drawn over the detail pane, so the frame that draws it has
/// to claim its cells — otherwise a click there reaches the control beneath.
#[tokio::test]
async fn drawing_a_toast_records_its_region() {
    use crate::app::test_support::app_with_fixture;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let (_dir, mut app) = app_with_fixture();
    app.notify("something happened", Severity::Information);

    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
    terminal.draw(|frame| draw(frame, &mut app)).expect("draw");

    assert!(
        app.hits
            .iter()
            .any(|hit| hit.target == HitTarget::Notification),
        "the toast drew but claimed no cells"
    );
}

/// End-to-end wrap coverage: with the sweep's help-toast case retired,
/// this is what guards `draw_notifications`' own arithmetic — a long git
/// or update error must occupy several drawn rows, never clamp to one.
#[tokio::test]
async fn a_long_notification_draws_over_multiple_lines() {
    use crate::app::test_support::app_with_fixture;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let (_dir, mut app) = app_with_fixture();
    app.notify(
        "Could not delete 'wt-a': fatal: 'wt-a' contains modified or \
             untracked files, use --force to delete it anyway",
        Severity::Error,
    );

    let mut terminal = Terminal::new(TestBackend::new(80, 30)).expect("test terminal");
    terminal.draw(|frame| draw(frame, &mut app)).expect("draw");

    let buffer = terminal.backend().buffer();
    let rows_with_text = (0..buffer.area.height)
        .filter(|&row| {
            let line: String = (0..buffer.area.width)
                .map(|col| buffer[(col, row)].symbol().chars().next().unwrap_or(' '))
                .collect();
            line.contains("wt-a") || line.contains("--force") || line.contains("untracked")
        })
        .count();
    assert!(
        rows_with_text > 1,
        "the notification text occupies {rows_with_text} row(s); it must wrap"
    );
}

/// Hovering a row wider than the sidebar reveals the whole name across
/// the divider; the same row un-hovered stays clipped at the divider.
#[tokio::test]
async fn a_hovered_overlong_row_reveals_past_the_divider() {
    use crate::app::test_support::app_with_fixture;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let long_name = "a-worktree-name-far-wider-than-the-sidebar-allows";
    let (_dir, mut app) = app_with_fixture();
    if let Some(crate::app::SidebarRow::Worktree { name, .. }) = app.rows.get_mut(1) {
        *name = long_name.into();
    } else {
        panic!("fixture row 1 is not a worktree");
    }

    let row_text = |terminal: &Terminal<TestBackend>, row: u16| -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.width)
            .map(|col| buffer[(col, row)].symbol().chars().next().unwrap_or(' '))
            .collect()
    };
    // Header row + the 3-row gh-status box put the first tree row at y=4;
    // the mutated worktree is the row below it.
    let worktree_row = 5;

    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
    terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
    assert!(
        !row_text(&terminal, worktree_row).contains(long_name),
        "an un-hovered overlong row must clip at the divider"
    );

    app.hovered = Some(HitTarget::SidebarRow(1));
    terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
    let revealed = row_text(&terminal, worktree_row);
    let end = revealed
        .find(long_name)
        .expect("the hovered row must reveal its full name across the divider")
        + long_name.len();
    assert!(
        revealed[end..].trim_start().starts_with('│'),
        "the reveal must close with a right border, got: {:?}",
        &revealed[end..]
    );
    // Zero horizontal shift: the left border chomps the one-cell tree
    // angle in column 0 and everything after it keeps its column.
    assert!(
        revealed.starts_with("│─ "),
        "the left border must replace the angle without shifting the text, got: {:?}",
        &revealed[..8]
    );
    // The box: its border rows sit on the neighbouring lines.
    assert!(
        row_text(&terminal, worktree_row - 1).contains('┌'),
        "the reveal must draw a top border above the row"
    );
    assert!(
        row_text(&terminal, worktree_row + 1).contains('└'),
        "the reveal must draw a bottom border below the row"
    );
}

#[test]
fn wrap_words_breaks_on_word_boundaries() {
    let message = "Could not delete 'wt-a': fatal: 'wt-a' contains modified or untracked files";
    let wrapped = wrap_words(message, 20);
    assert!(wrapped.iter().all(|l| l.chars().count() <= 20));
    assert!(wrapped.len() > 1, "a long error has to wrap, not truncate");
    // Nothing may be lost: the words all survive, in order.
    let joined = wrapped.join(" ");
    assert_eq!(
        joined.split_whitespace().collect::<Vec<_>>(),
        message.split_whitespace().collect::<Vec<_>>()
    );
}

#[test]
fn wrap_words_splits_an_overlong_word() {
    let wrapped = wrap_words("supercalifragilistic", 6);
    assert!(wrapped.iter().all(|l| l.chars().count() <= 6));
    assert_eq!(wrapped.concat(), "supercalifragilistic");
    assert!(wrap_words("anything", 0).is_empty());
}
