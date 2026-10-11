use super::*;

#[test]
fn editing_moves_the_cursor() {
    let mut input = TextInput::new("");
    for ch in "abc".chars() {
        input.insert(ch);
    }
    assert_eq!(input.value(), "abc");
    assert_eq!(input.cursor(), 3);

    input.move_left();
    input.insert('X');
    assert_eq!(input.value(), "abXc");

    input.backspace();
    assert_eq!(input.value(), "abc");

    input.move_home();
    input.delete();
    assert_eq!(input.value(), "bc");
}

/// A modified character is not text: `Alt+d` used to type a `d` into the
/// field, and every other `Ctrl` combination fell through to a letter
/// shortcut (Defect B of issue #51). `Ctrl+U` is the one the editor claims.
#[test]
fn only_the_editors_own_combination_survives_a_modifier() {
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    let modified = |code, modifiers| KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };

    let mut input = TextInput::new("hello");
    assert!(!input.apply_edit_key(modified(KeyCode::Char('d'), KeyModifiers::ALT)));
    assert!(!input.apply_edit_key(modified(KeyCode::Char('a'), KeyModifiers::CONTROL)));
    assert_eq!(input.value(), "hello", "a modified key was typed as text");

    assert!(input.apply_edit_key(modified(KeyCode::Char('!'), KeyModifiers::SHIFT)));
    assert_eq!(input.value(), "hello!", "Shift is part of typing");

    assert!(input.apply_edit_key(modified(KeyCode::Char('u'), KeyModifiers::CONTROL)));
    assert_eq!(input.value(), "", "Ctrl+U no longer clears the field");
}

#[test]
fn max_length_is_enforced() {
    let mut input = TextInput::new("").with_max_length(2);
    for ch in "abcd".chars() {
        input.insert(ch);
    }
    assert_eq!(input.value(), "ab");
}

#[test]
fn kill_to_start_clears_prefix() {
    let mut input = TextInput::new("hello");
    input.move_left();
    input.kill_to_start();
    assert_eq!(input.value(), "o");
    assert_eq!(input.cursor(), 0);
}

#[test]
fn handles_multibyte_text() {
    let mut input = TextInput::new("héllo");
    input.move_home();
    input.move_right();
    input.delete();
    assert_eq!(input.value(), "hllo");
}

#[test]
fn placeholder_shows_while_focused_and_empty() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let input = TextInput::new("").with_placeholder("Enter path...");
    for focused in [true, false] {
        let mut terminal = Terminal::new(TestBackend::new(40, 3)).expect("terminal");
        terminal
            .draw(|frame| input.render(frame, frame.area(), focused))
            .expect("draw");
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            screen.contains("Enter path..."),
            "placeholder missing when focused={focused}"
        );
    }

    // Once there is a value, the value wins.
    let typed = TextInput::new("/tmp").with_placeholder("Enter path...");
    let mut terminal = Terminal::new(TestBackend::new(40, 3)).expect("terminal");
    terminal
        .draw(|frame| typed.render(frame, frame.area(), true))
        .expect("draw");
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("/tmp"));
    assert!(!screen.contains("Enter path..."));
}

#[test]
fn centered_rect_clamps_to_area() {
    let area = Rect::new(0, 0, 40, 10);

    // Oversized: clamped, but with a margin so it still reads as a dialog
    // rather than running edge to edge.
    let r = centered_rect(80, 30, area);
    assert_eq!(r.width, 38);
    assert_eq!(r.height, 9);
    // Horizontal margin is what stops it reading as a full-width pane; a
    // one-row vertical gap rounds away and does not matter.
    assert!(r.x > 0, "no margin left around the dialog");

    // Fits: placed exactly, centred.
    let r = centered_rect(20, 4, area);
    assert_eq!((r.x, r.y, r.width, r.height), (10, 3, 20, 4));

    // Degenerate areas must still produce something drawable.
    let tiny = centered_rect(80, 30, Rect::new(0, 0, 1, 1));
    assert_eq!((tiny.width, tiny.height), (1, 1));
}

/// Draw `input` focused into a field 12 cells wide (10 inside the border)
/// and return the text row inside it and the caret's column within it.
fn draw_field(input: &TextInput) -> (String, u16) {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut terminal = Terminal::new(TestBackend::new(12, 3)).expect("terminal");
    terminal
        .draw(|frame| input.render(frame, frame.area(), true))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let row: String = (1..11).map(|x| buffer[(x, 1)].symbol()).collect();
    let caret = terminal.get_cursor_position().expect("cursor");
    assert_eq!(caret.y, 1, "caret left the text row");
    assert!((1..11).contains(&caret.x), "caret outside the field");
    (row, caret.x - 1)
}

/// A free-text description is routinely wider than its field. The field used
/// to keep drawing from the first character with the caret clamped to the
/// border, so the end being typed and anything edited there were invisible.
#[test]
fn a_long_value_scrolls_to_keep_the_caret_in_view() {
    let mut input = TextInput::new("abcdefghijklmnopqrstuvwxyz");

    // Typing at the end: the tail, with the caret in the cell after it.
    let (row, caret) = draw_field(&input);
    assert_eq!(row, "rstuvwxyz ");
    assert_eq!(caret, 9);

    // Moving left within the view moves the caret, not the text.
    for _ in 0..3 {
        input.move_left();
    }
    let (row, caret) = draw_field(&input);
    assert_eq!(row, "rstuvwxyz ");
    assert_eq!(caret, 6);

    // An edit there is visible.
    input.insert('#');
    let (row, caret) = draw_field(&input);
    assert_eq!(row, "rstuvw#xyz");
    assert_eq!(caret, 7);

    // Past the left edge, the text follows the caret back.
    for _ in 0..8 {
        input.move_left();
    }
    let (row, caret) = draw_field(&input);
    assert_eq!(caret, 0);
    assert_eq!(row, "qrstuvw#xy");

    input.move_home();
    let (row, caret) = draw_field(&input);
    assert_eq!(row, "abcdefghij");
    assert_eq!(caret, 0);

    input.move_end();
    let (row, caret) = draw_field(&input);
    assert_eq!(row, "stuvw#xyz ");
    assert_eq!(caret, 9);
}

#[test]
fn a_value_that_fits_draws_from_its_start() {
    let mut input = TextInput::new("short");
    assert_eq!(draw_field(&input), ("short     ".to_string(), 5));

    // A long value cut back down returns to the start rather than staying
    // scrolled into a half-empty field.
    input.set_value("abcdefghijklmnopqrstuvwxyz");
    draw_field(&input);
    input.kill_to_start();
    input.set_value("short");
    assert_eq!(draw_field(&input), ("short     ".to_string(), 5));
}

/// Widths are display cells: each of these takes two.
#[test]
fn wide_characters_neither_overflow_nor_misplace_the_caret() {
    let mut input = TextInput::new("日本語のテキストです");
    let (row, caret) = draw_field(&input);
    // Four two-cell glyphs fill eight cells, leaving one for the caret.
    assert_eq!(row.replace(' ', ""), "ストです");
    assert_eq!(caret, 8);

    input.move_home();
    let (row, caret) = draw_field(&input);
    assert_eq!(row.replace(' ', ""), "日本語のテ");
    assert_eq!(caret, 0);

    input.move_right();
    input.move_right();
    let (_, caret) = draw_field(&input);
    assert_eq!(caret, 4);
}
