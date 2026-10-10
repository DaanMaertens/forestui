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
