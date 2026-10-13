use super::*;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What tmux 3.7 prints for its own default prefix table, trimmed to the
/// lines these tests need — including the escaped keys and the padding.
const DEFAULT_LISTING: &str = "\
bind-key    -T prefix Space   next-layout
bind-key    -T prefix \\\"      split-window
bind-key    -T prefix \\#      list-buffers
bind-key    -T prefix x       confirm-before -p \"kill-pane #P? (y/n)\" kill-pane
bind-key -r -T prefix Left    select-pane -L
bind-key -r -T prefix M-Left  resize-pane -L 5
bind-key -r -T prefix C-Left  resize-pane -L
";

fn binding(listing: &str, key: &str) -> Binding {
    parse_binding(listing, key).expect("bound")
}

#[test]
fn the_default_left_binding_is_read_with_its_repeat_flag() {
    let left = binding(DEFAULT_LISTING, "Left");
    assert!(left.repeat);
    assert_eq!(left.command, "select-pane -L");
    assert_eq!(left.line(), "bind-key -r -T prefix Left select-pane -L");
}

/// `C-Left` and `M-Left` share a prefix with `Left`; only the exact key may
/// match, or the back key would wrap a resize.
#[test]
fn only_the_exact_key_matches() {
    assert_eq!(binding(DEFAULT_LISTING, "C-Left").command, "resize-pane -L");
    assert_eq!(
        binding(DEFAULT_LISTING, "M-Left").command,
        "resize-pane -L 5"
    );
    assert!(parse_binding(DEFAULT_LISTING, "Right").is_none());
}

/// tmux escapes keys its parser treats specially. The listing is matched on
/// the key itself, and the escaped token is what goes back into a command.
#[test]
fn escaped_keys_match_their_plain_name_and_keep_their_escape() {
    let quote = binding(DEFAULT_LISTING, "\"");
    assert_eq!(quote.command, "split-window");
    assert_eq!(quote.line(), "bind-key -T prefix \\\" split-window");
    assert_eq!(binding(DEFAULT_LISTING, "#").command, "list-buffers");
}

#[test]
fn a_command_with_quotes_survives_parsing_verbatim() {
    assert_eq!(
        binding(DEFAULT_LISTING, "x").command,
        "confirm-before -p \"kill-pane #P? (y/n)\" kill-pane"
    );
}

#[test]
fn an_unbound_or_default_key_is_bound_without_asking() {
    assert_eq!(decide("Left", None, None), Decision::Bind);
    let left = binding(DEFAULT_LISTING, "Left");
    assert_eq!(decide("Left", Some(&left), None), Decision::Bind);
}

#[test]
fn a_users_own_binding_is_a_conflict() {
    let listing = "bind-key    -T prefix Left    previous-window\n";
    let left = binding(listing, "Left");
    assert_eq!(decide("Left", Some(&left), None), Decision::Conflict);
}

/// Another key's default is still the user's choice for *this* key: `Up`'s
/// default bound on `Left` is not tmux's default for `Left`.
#[test]
fn a_default_for_another_key_is_still_a_conflict() {
    let listing = "bind-key -r -T prefix Left    select-pane -U\n";
    let left = binding(listing, "Left");
    assert_eq!(decide("Left", Some(&left), None), Decision::Conflict);
}

/// "Use it anyway" remembers the binding it was answered for. The same
/// binding never asks again; a different one is a new conflict.
#[test]
fn an_accepted_binding_only_asks_again_once_it_changes() {
    let listing = "bind-key    -T prefix Left    previous-window\n";
    let left = binding(listing, "Left");
    assert_eq!(
        decide("Left", Some(&left), Some("previous-window")),
        Decision::Bind
    );
    assert_eq!(
        decide("Left", Some(&left), Some("next-window")),
        Decision::Conflict
    );
}

#[test]
fn our_own_binding_is_kept() {
    let ours = bind_line("Left", Some(&binding(DEFAULT_LISTING, "Left")));
    let listed = format!("{ours}\n");
    let current = binding(&listed, "Left");
    assert!(is_ours(&current.command));
    assert_eq!(decide("Left", Some(&current), None), Decision::Keep);
}

/// Other windows get exactly what the key did before, repeat flag included.
#[test]
fn the_back_key_falls_back_to_the_previous_binding() {
    let line = bind_line("Left", Some(&binding(DEFAULT_LISTING, "Left")));
    assert!(
        line.starts_with("bind-key -r -T prefix Left if-shell -F "),
        "{line}"
    );
    assert!(line.ends_with(" { select-pane -L }"), "{line}");
}

/// An unbound key did nothing outside a Claude window, and still does not.
#[test]
fn an_unbound_key_gets_no_fallback() {
    let line = bind_line("F9", None);
    assert!(
        line.starts_with("bind-key -T prefix F9 if-shell -F "),
        "{line}"
    );
    assert!(line.ends_with("switch-client -T root }"), "{line}");
}

/// The Claude branch ends the key's repeat: without it, an arrow pressed
/// right after landing on forestui was taken by the repeating binding
/// instead of reaching forestui.
#[test]
fn landing_on_forestui_ends_the_repeat() {
    let line = bind_line("Left", Some(&binding(DEFAULT_LISTING, "Left")));
    assert!(line.contains("; switch-client -T root }"), "{line}");
}

/// Keys tmux's parser treats specially are quoted when we bind them.
#[test]
fn special_keys_are_quoted_for_the_parser() {
    assert!(bind_line(";", None).starts_with("bind-key -T prefix ';' "));
    assert!(bind_line("'", None).starts_with("bind-key -T prefix \"'\" "));
    assert!(bind_line("C-Left", None).starts_with("bind-key -T prefix C-Left "));
}

#[test]
fn the_hint_goes_in_front_of_the_previous_status() {
    let status = hinted_status("%H:%M", "Left");
    assert!(is_hinted(&status));
    assert!(status.ends_with("%H:%M"), "{status}");
    assert!(status.contains("#{prefix} ← forestui │ "), "{status}");
    assert!(!is_hinted("%H:%M"));
}

/// The hint sits inside a format conditional, where `,` and `}` end the
/// branch early.
#[test]
fn the_hint_escapes_format_characters_in_the_key() {
    let status = hinted_status("", ",");
    assert!(status.contains("#{prefix} #, forestui"), "{status}");
    let status = hinted_status("", "}");
    assert!(status.contains("#{prefix} #} forestui"), "{status}");
}

#[test]
fn arrows_are_shown_as_arrows() {
    assert_eq!(glyph("Left"), "←");
    assert_eq!(glyph("Right"), "→");
    assert_eq!(glyph("Up"), "↑");
    assert_eq!(glyph("Down"), "↓");
    assert_eq!(glyph("C-Left"), "C-Left");
    assert_eq!(glyph("b"), "b");
}

fn press(code: KeyCode, modifiers: KeyModifiers) -> Option<String> {
    key_name(KeyEvent::new(code, modifiers))
}

#[test]
fn pressed_keys_become_tmux_key_names() {
    assert_eq!(
        press(KeyCode::Left, KeyModifiers::NONE).as_deref(),
        Some("Left")
    );
    assert_eq!(
        press(KeyCode::Char('b'), KeyModifiers::NONE).as_deref(),
        Some("b")
    );
    // A capital arrives with Shift; tmux names it by the character alone.
    assert_eq!(
        press(KeyCode::Char('B'), KeyModifiers::SHIFT).as_deref(),
        Some("B")
    );
    assert_eq!(
        press(KeyCode::Char('a'), KeyModifiers::CONTROL).as_deref(),
        Some("C-a")
    );
    assert_eq!(
        press(KeyCode::Char('h'), KeyModifiers::ALT).as_deref(),
        Some("M-h")
    );
    assert_eq!(
        press(KeyCode::Left, KeyModifiers::CONTROL | KeyModifiers::SHIFT).as_deref(),
        Some("C-S-Left")
    );
    assert_eq!(
        press(KeyCode::Char(' '), KeyModifiers::NONE).as_deref(),
        Some("Space")
    );
    assert_eq!(
        press(KeyCode::PageUp, KeyModifiers::NONE).as_deref(),
        Some("PPage")
    );
    assert_eq!(
        press(KeyCode::F(5), KeyModifiers::NONE).as_deref(),
        Some("F5")
    );
    assert_eq!(press(KeyCode::Esc, KeyModifiers::NONE), None);
}

/// A home is a window a forestui marked with its pid. Our own does not
/// count, and a mark whose forestui is gone — killed in a window that
/// outlived it — is stale, to be swept.
#[test]
fn homes_split_live_marks_from_stale_ones() {
    let listing = "@1\t100\n@2\t\n@3\t200\n@4\t300\n@5\tjunk\n";
    let alive = |pid| pid != 200;
    assert_eq!(homes(listing, Some("@1"), alive), (1, vec!["@3", "@5"]));
    assert_eq!(homes(listing, None, alive), (2, vec!["@3", "@5"]));
    assert_eq!(homes("", None, alive), (0, vec![]));
}

#[test]
fn this_process_is_alive() {
    assert!(process_alive(std::process::id()));
}
