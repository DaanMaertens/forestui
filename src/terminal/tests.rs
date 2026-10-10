use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A sink the test can still read after the guard that owns it is dropped.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<u8>>>);

impl Recorder {
    fn written(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    }
}

impl Write for Recorder {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Both of these are load-bearing and neither fails loudly.
///
/// Without `?1003h` the terminal never reports a bare pointer move, so
/// hover cannot work. Without `?1004h` it never reports focus at all, so
/// `Event::FocusGained` never arrives and everything hanging off it —
/// refreshing sessions and worktrees when the user comes back — is dead
/// code that still compiles and still has passing tests.
#[test]
fn the_terminal_modes_we_depend_on_are_requested() {
    for (mode, why) in [
        ("?1003h", "any-motion, for hover"),
        ("?1004h", "focus reporting, for refresh on return"),
        ("?1006h", "SGR coordinates, for columns past 223"),
    ] {
        assert!(MODES_ON.contains(mode), "{mode} ({why}) is not requested");
        let off = mode.replace('h', "l");
        assert!(
            MODES_OFF.contains(&off),
            "{mode} ({why}) is requested but never turned off"
        );
    }
}

/// The general form of the above: a mode added to ON with no counterpart
/// in OFF is a new leak, whatever it is called.
#[test]
fn every_mode_we_turn_on_has_a_way_off() {
    let modes: Vec<&str> = MODES_ON
        .split("\x1b[")
        .filter(|part| !part.is_empty())
        .collect();
    assert_eq!(modes.len(), 5, "MODES_ON no longer parses as we expect");

    for mode in modes {
        let off = mode.replace('h', "l");
        assert!(
            MODES_OFF.contains(&off),
            "\x1b[{mode} is set but \x1b[{off} is never sent"
        );
    }
}

/// The self-heal. A terminal a `SIGKILL`ed run left stranded is fixed by
/// the next launch, so the bug costs one restart rather than a printf the
/// user has to be told about.
#[test]
fn startup_turns_the_modes_off_before_turning_them_on() {
    let sequence = startup_sequence(InputModes::default());
    let off = sequence.find(MODES_OFF).expect("startup must reset first");
    let on = sequence.find(MODES_ON).expect("startup must set the modes");
    assert!(off < on, "startup set the modes before resetting them");
}

/// Declining a mode drops that request and nothing else — and never
/// narrows the reset, which has to cover whatever a previous run set.
#[test]
fn declining_a_mode_drops_only_that_request() {
    let without_hover = modes_on(InputModes {
        hover: false,
        ..InputModes::default()
    });
    assert!(
        !without_hover.contains("?1003h"),
        "hover survived --no-hover"
    );
    for kept in ["?1000h", "?1002h", "?1006h", "?1004h"] {
        assert!(
            without_hover.contains(kept),
            "--no-hover also dropped {kept}"
        );
    }

    let without_focus = modes_on(InputModes {
        focus: false,
        ..InputModes::default()
    });
    assert!(
        !without_focus.contains("?1004h"),
        "focus survived --no-focus-events"
    );
    assert!(
        without_focus.contains("?1003h"),
        "--no-focus-events took hover"
    );

    let neither = modes_on(InputModes {
        hover: false,
        focus: false,
    });
    assert_eq!(neither, "\x1b[?1000h\x1b[?1002h\x1b[?1006h");
    assert!(
        startup_sequence(InputModes {
            hover: false,
            focus: false,
        })
        .starts_with(MODES_OFF),
        "a narrowed run must still reset every mode first"
    );
}

#[test]
fn dropping_the_guard_turns_the_modes_off() {
    let recorder = Recorder::default();
    drop(ModeGuard::enable(recorder.clone(), InputModes::default()));

    let written = recorder.written();
    assert!(written.ends_with(MODES_OFF), "the guard left the modes on");
}

/// The path `ratatui`'s panic hook leaves uncovered: the screen is
/// restored, and without this the modes are not.
#[test]
fn the_guard_turns_the_modes_off_when_the_frame_unwinds() {
    let recorder = Recorder::default();
    let held = recorder.clone();
    let result = std::panic::catch_unwind(move || {
        let _guard = ModeGuard::enable(held, InputModes::default());
        panic!("the event loop fell over");
    });

    assert!(result.is_err(), "the panic did not propagate");
    assert!(
        recorder.written().ends_with(MODES_OFF),
        "an unwinding panic left the modes on"
    );
}

/// `Drop` does not run under `panic = "abort"`, so the hook has to emit the
/// reset itself — and still hand over to `ratatui`'s hook, which is what
/// leaves the alternate screen.
///
/// This swaps the process-wide panic hook for the length of the test. No
/// other test panics deliberately, so the only thing it can swallow is the
/// message from a test that is already failing.
#[test]
fn the_panic_hook_resets_the_modes_and_still_runs_the_previous_hook() {
    static RESET: AtomicBool = AtomicBool::new(false);
    static PREVIOUS_RAN: AtomicBool = AtomicBool::new(false);

    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| PREVIOUS_RAN.store(true, Ordering::SeqCst)));
    install_panic_hook(|| RESET.store(true, Ordering::SeqCst));

    let _ = std::panic::catch_unwind(|| panic!("boom"));
    std::panic::set_hook(original);

    assert!(RESET.load(Ordering::SeqCst), "the hook left the modes on");
    assert!(
        PREVIOUS_RAN.load(Ordering::SeqCst),
        "the hook swallowed ratatui's screen restore"
    );
}
