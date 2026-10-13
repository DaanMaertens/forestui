# Terminal input modes

forestui asks the terminal for five DEC private modes (`src/terminal.rs`):
mouse buttons, drag, any-motion, SGR coordinates, focus reporting. They are
*terminal* state, not process state, so every way out of the process has to put
them back. `src/terminal.rs` documents the four paths that do — the guard's
`Drop`, a chained panic hook, a signal handler, and OFF-before-ON at startup,
which heals whatever a `SIGKILL`ed run left behind. A mode added to `MODES_ON`
fails a test until `MODES_OFF` covers it too.

`src/terminal.rs` writes the mouse modes by hand rather than using crossterm's
`EnableMouseCapture`, and **must keep `?1003h` (any-motion) among them** —
without it the terminal never reports a bare pointer move, so hover becomes
impossible rather than merely unstyled. Motion is affordable because
`App::handle_mouse` sets `App::redraw` only when the *hovered target changes*.
That gate is what stops a moving mouse repainting the app continuously;
removing it brings back the flicker that motion reporting was first disabled
for.

Two of the five are optional, because each costs something inside tmux.
`?1003h` (any-motion) is what hover needs; `?1004h` plus tmux's server-wide
`focus-events` is what refresh-on-return needs. tmux cancels a pending prefix
on any key it cannot find in the `prefix` table, and a mouse or focus report is
such a key — so with these modes on, a pointer move or a focus change landing
between the prefix and the next key eats the command. Neither event has a key
name to bind in tmux 3.4, so taking the mode away is the only defence, which is
what `--no-hover` and `--no-focus-events` are. Both must stay in
`self_command`, or the tmux re-exec silently drops them.

`focus-events` is a **server** option: turning it on reaches every session and
client on that server, and it used to stay on long after forestui exited.
`App::release_tmux_options` puts it back. Ownership is recorded in a tmux user
option rather than in memory, so a run that is killed before its cleanup does
not strand the setting forever — the next run adopts it and hands it back. A
user who set the option themselves keeps it, marker absent.

**Terminal handover:** `scripts/terminal-modes-probe.py` runs the built binary
on a private pty, kills it every way that used to leak the input modes, and
asserts the reset arrived — plus that raw mode and the alternate screen were
undone and the exit status still reports the signal. Unit tests can pin the
mode strings and the guard's `Drop`; only a real terminal can show what a
signal handler actually emitted. Run it after touching `src/terminal.rs`; it
isolates itself (own forest, own `TMUX_TMPDIR`) and cleans up.
