use super::*;

#[test]
fn the_option_is_recognised_in_a_help_screen() {
    let current = "  -r, --resume [value]    Resume a conversation\n\
                       \x20 --remote-control [name]  Start an interactive session with Remote\n\
                       \x20 --remote-control-session-name-prefix <prefix>\n";
    assert!(help_offers_remote_control(current));
}

/// The sibling option alone does not make the flag exist, and a help screen
/// from before Remote Control must read as a definite no.
#[test]
fn an_older_help_screen_reads_as_unsupported() {
    let older = "  -r, --resume [value]    Resume a conversation\n\
                     \x20 -n, --name <name>       Set a display name\n";
    assert!(!help_offers_remote_control(older));
    assert!(!help_offers_remote_control(
        "  --remote-control-session-name-prefix <prefix>\n"
    ));
}

/// Until a probe has answered, launches keep the flag.
#[test]
fn unknown_means_supported() {
    assert!(remote_control_supported());
}
