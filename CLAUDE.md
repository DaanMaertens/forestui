# CLAUDE.md - AI Development Guidelines

This file provides context for AI assistants (like Claude) working on forestui.

## Never touch the user's live tmux

**This rule applies to every task in this repo, not just testing.**

Whoever works on forestui is, almost by definition, *running* forestui. That
is what makes this different from an ordinary "be careful" note: a live tmux
server is up right now, holding their actual work — editors mid-edit,
terminals mid-command, Claude sessions mid-conversation, laid out the way they
left them. It is not a risk to weigh against convenience; it is the default
state of this repo's development environment, and a bare `tmux` command from a
shell attaches straight to it.

The session running this task is very likely one of those windows — forestui
opens Claude sessions in tmux windows, and this is a Claude session. A stray
`rename-window`, `kill-window` or `kill-server` can destroy the user's work,
up to and including the conversation that issued the command.

- **Never run a bare `tmux` command** — not to look, not to list, not "just
  once". `tmux list-sessions` is as dangerous as `tmux kill-server`, because
  it is the step that convinces you the next call is safe.
- **Drive tmux only through `tu`**, with `TMUX_TMPDIR` pointed at a throwaway
  directory. The `test-forestui` skill has the exact invocation.
- **Exploring tmux behaviour is not an exception.** Learning what a tmux
  command does, probing options or hooks, reproducing something from the
  docs — all of it goes on an isolated server under `tu`. A separate `-L`
  socket looks like it solves this, and it does not: it relies on never once
  forgetting the flag, against a default that reaches the user's live
  session.

The `test-forestui` skill carries the full harness recipe, and its description
asks to be loaded before *any* tmux command, not only before testing — because
tasks that do not look like testing are exactly where this gets broken: a
spike, doc research, "just checking how tmux options work". The rule is
repeated here because this file is always loaded and a skill is not.

## Project Overview

forestui is a terminal UI for managing Git worktrees, built with Rust and
ratatui. It runs inside tmux and gives developers one place to manage many
worktrees, their editors, their terminals, and their Claude Code sessions.

Rust (edition 2024) on ratatui + crossterm, tokio, serde and clap — versions
are in `Cargo.toml`. Git and tmux are driven by shelling out to the real
binaries, not through bindings: the commands stay inspectable and there is no
libgit2 build dependency. forestui requires tmux and re-executes itself into a
tmux session when started outside one, via `std::env::current_exe()` — so a
`cargo run` build re-launches itself rather than whatever `forestui` is on
`PATH`.

Each module opens with a `//!` line saying what it owns; `ls src/` plus those
headers is the map.

## Development Commands

```bash
make check         # lint + typecheck + format-check + test
make check-shipped # lint + tests under --features binary-release
make format        # cargo fmt
make run           # cargo run
```

CI also runs `make check-shipped`: the in-place updater is `#[cfg]`-ed out of the default
build, so a mistake inside the feature-gated block is invisible to
`make check`.

## Code Conventions

- Edition 2024. Prefer let-chains (`if let Some(x) = ... && cond`) over nesting.
- No `unwrap()` or `expect()` on anything that can fail at runtime. Config
  parsing, subprocess spawning, and filesystem access all degrade gracefully —
  a corrupt config file must never block startup.
- Errors that reach the user become a notification, not a panic.
- Colours come from `theme.rs`. Never hardcode a `Color::Rgb` in a UI module.
- Comments explain *why*, not *what*.
- Services are free functions in modules, not singletons; shared caches live
  behind a module-private `OnceLock<Mutex<...>>`.

Three event-loop rules hold everywhere (`doc/design/event-loop.md` has the
mechanics and reasons):

- **Never block the event loop.** Slow work (git, gh, scanning files) renders a
  loading state, runs in `tokio::spawn`, and comes back as an `AppEvent` that
  `App::handle_event` folds in — discarding it if the selection moved on.
- **The main loop is the single writer of `.forestui-config.json`.** Tasks
  send results as data; the fold persists and owns any success toast.
- **Every worktree-mutating flow goes through `WorktreeSync`**, and every exit
  path of its task sends an event carrying the change ticket.

## Area notes

Read the note before changing its area — each records load-bearing reasons
the code alone does not show:

- **Event loop** (`doc/design/event-loop.md`) — before adding an `AppEvent`,
  a spawned task, a worktree scan or mutation, or anything animated on the
  tick.
- **Rendering** (`doc/design/rendering.md`) — before adding or moving a
  detail-pane control, a clickable region, or a boxed button; before touching
  focus, hit testing or `App::drawn_items`.
- **Common tasks** (`doc/common-tasks.md`) — before adding a service,
  detail-pane action, key binding, modal, CLI option or setting, or per-
  repository Start work overrides.
- **Config files** (`doc/design/config.md`) — before adding, removing or
  renaming a field in `.forestui-config.json` or `settings.json`, or touching
  how forests are separated.
- **Claude integration** (`doc/design/claude-integration.md`) — before
  touching `assets/claude-plugin/`, `services/claude_plugin.rs`,
  `services/live.rs`, `services/claude_cli.rs`, the `@claude_*` window
  options, session states, or the flags `claude_command_line` passes
  (`-n`, `--remote-control`).
- **Claude inside a shell** (`doc/design/claude-in-shell.md`) — before
  touching how `services/tmux.rs` launches a Claude window: `startup_for`,
  the generated startup file, history push, clean-exit rule, or the
  `send-keys` fallback.
- **Terminal input modes** (`doc/design/terminal-input-modes.md`) — before
  touching `src/terminal.rs`, mouse/focus modes, `--no-hover`,
  `--no-focus-events`, or tmux's `focus-events`.
- **Back key** (`doc/design/back-key.md`) — before touching
  `services/tmux/back_key.rs`, `back_key`/`back_key_accepted`, or the
  `@forestui_*` tmux options.
- **Self-update** (`doc/design/self-update.md`) — before touching
  `version_check.rs`, the `binary-release` feature, the title-bar update
  notice, or release asset names.
- **Finished worktrees and prune** (`doc/design/finished-and-prune.md`) —
  before touching `app/finished.rs`, `work::is_finished`, `prune.rs`, or
  `forestui prune`.
- **Releasing** (`doc/releasing.md`) — before cutting a release or changing
  how the version or dev-mode window name is derived.

## Testing

Unit tests live in a sibling file, not inline: `foo.rs` ends with
`#[cfg(test)] mod tests;` and the tests are in `foo/tests.rs` (`bar/mod.rs` →
`bar/tests.rs`). As a child module they still reach private items through
`use super::*`. Keep new tests there rather than growing the source file; note
that an `include_str!` in a test file resolves one directory deeper than in its
parent.

The App tests are the one exception: there are too many for one file, so they
are split by feature under `src/app/tests/`. A new App test goes in the file
for its feature, or in a new one declared in `src/app/tests/mod.rs`. Each file starts with `use super::*;`, and a helper
used by more than one file lives in `mod.rs`.

They run with `make test`. Async code uses `#[tokio::test]`. Rendering is
tested against `ratatui::backend::TestBackend`, which renders into an
in-memory buffer you can assert on.

**Visual/behavioural verification:** forestui is a TUI app — lint, typecheck and
unit tests alone cannot catch visual bugs, wrong window names, broken
interactions, or UI regressions. Use the `test-forestui` skill to drive forestui
in a headless terminal with `tu` and verify the change works. Do this
proactively, not only when asked.

`doc/rust-rewrite/TU_USECASES.md` is the acceptance playbook: numbered
scenarios with exact keystrokes and expected output. The P0 cases are the
regression suite — run the relevant ones after any behavioural change. Before
running or extending `scripts/tu-sweep.sh`, or refreshing
`doc/rust-rewrite/baseline/`, read `doc/rust-rewrite/SWEEP.md`.

## Development

**Always work from a git worktree on its own branch — never edit files in the
main checkout, and never commit to `main` directly.** Before writing any code:

```bash
git fetch && git worktree add -b <branch> ../forestui-<topic> origin/main
```

and do all edits, builds and commits inside that worktree. The main checkout
stays on a clean `main`, so nothing half-done leaks into it and several tasks
can be in flight at once. Remove the worktree once its PR is merged.

1. Create a branch (in a worktree, as above), make changes
2. **Always run `make check` before committing, pushing, or opening a PR.** Do
   not push code that fails formatting, clippy, or tests — fix issues first.
3. Open a PR and merge to `main`

## Git Commits

- Do NOT include `Co-Authored-By` attribution
- Do NOT include "Generated with Claude Code" footer
- Write clear, concise commit messages
