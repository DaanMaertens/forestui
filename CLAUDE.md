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

It was a Python/Textual application through v0.9.x. The rewrite is in
`doc/rust-rewrite/` — architecture, migration plan, and the `tu`-driven
acceptance playbook. The behavioural spec of the Python build was removed once
the port shipped; it lives in git history at `ae9cb1a`. The code, not that
spec, is the reference for how forestui behaves now.

## Tech Stack

- **Rust 1.88+**, edition 2024
- **ratatui 0.30** for the TUI, **crossterm 0.29** (re-exported as `ratatui::crossterm`)
- **tokio** for background work and subprocesses
- **serde / serde_json** for the two JSON config files
- **clap** (derive) for the CLI

Git and tmux are driven by shelling out to the real binaries, not through
bindings. That keeps the commands inspectable and avoids a libgit2 build
dependency.

## Project Structure

```
forestui/
├── Cargo.toml
├── src/
│   ├── main.rs               # Entry point, tokio runtime, crash reporting
│   ├── cli.rs                # clap args + the tmux bootstrap (ensure_tmux)
│   ├── terminal.rs           # Terminal input modes and every way to undo them
│   ├── app/
│   │   ├── mod.rs            # App state, startup, sidebar, event folding
│   │   ├── detail.rs         # Detail-pane content walk (single source of truth)
│   │   ├── keys.rs           # Keyboard input and the rename fields
│   │   ├── mouse.rs          # Hit testing, hover, wheel routing, scrollbar
│   │   ├── repo_settings.rs  # Per-checkout GitHub key and settings overrides
│   │   ├── worktree_sync.rs  # Keeps scans from undoing forestui's own changes
│   │   ├── finished.rs       # Which worktrees are finished
│   │   └── actions.rs        # What activating a control does; spawned flows
│   ├── event.rs              # AppEvent + the single event channel
│   ├── modal.rs              # Modal stack: state, focus model, key handling
│   ├── picker.rs             # Searchable list used by modals
│   ├── fuzzy.rs              # Fuzzy scoring for pickers and branch search
│   ├── models.rs             # Serde models (config file schemas)
│   ├── prompt.rs             # Prompt templates (built-ins in assets/prompts/)
│   ├── prune.rs              # Removing finished worktrees (App and CLI)
│   ├── repo_settings.rs      # Repository settings: key, merge, pre-filled files
│   ├── state.rs              # Persisted repositories (.forestui-config.json)
│   ├── theme.rs              # Colour palette and shared styles
│   ├── util.rs               # Paths, slugs, relative times, atomic writes
│   ├── version_check.rs      # Startup self-update
│   ├── work.rs               # Start work's pure rules: modes, names, finished
│   ├── test_support.rs       # Helpers shared by several modules' unit tests
│   ├── ui/
│   │   ├── mod.rs            # draw(): layout, header, footer, notifications
│   │   ├── sidebar.rs        # Repository/worktree tree
│   │   ├── detail.rs         # Repository and worktree detail panes
│   │   ├── modals.rs         # Modal overlays
│   │   └── widgets.rs        # TextInput and small helpers
│   └── services/
│       ├── git.rs            # Async git operations
│       ├── tmux.rs           # tmux window management
│       ├── tmux/back_key.rs  # The prefix + ← back key
│       ├── github.rs         # gh CLI integration
│       ├── claude_cli.rs     # What the installed `claude` understands
│       ├── claude_plugin.rs  # Installing the Claude Code title-sync plugin
│       ├── claude_session.rs # Claude Code session tracking
│       ├── live.rs           # Where each Claude session is running now
│       ├── name_suggestion.rs # Headless `claude -p` worktree name suggestion
│       └── settings.rs       # User preferences + runtime forest path
├── assets/
│   ├── claude-plugin/        # The Claude Code plugin, embedded in the binary
│   └── prompts/              # Built-in prompt templates
├── doc/
│   ├── design/               # Per-area design notes (see Design notes below)
│   └── rust-rewrite/         # Rewrite history, tu playbook, sweep baselines
├── scripts/                  # tu sweep/compare/composite, terminal-modes probe
├── Makefile                  # Development commands
├── install.sh                # Installation script
└── README.md
```

## Development Commands

```bash
make lint         # cargo clippy --all-targets -- -D warnings
make typecheck    # cargo check --all-targets
make format-check # cargo fmt --check
make test         # cargo test
make check        # all of the above
make check-shipped # lint + tests under --features binary-release
make format       # cargo fmt
make run          # cargo run
make clean        # cargo clean
```

Always run `make check` before committing changes.

`make check-shipped` runs the same lint and tests with `--features binary-release`,
which is the configuration the release workflow builds. The default build
compiles strictly less code — the in-place updater is `#[cfg]`-ed out — so the
two fail differently, and a mistake inside the feature-gated block is invisible
to `make check`. CI runs both.

## Code Conventions

### Rust
- Edition 2024. Prefer let-chains (`if let Some(x) = ... && cond`) over nesting.
- No `unwrap()` or `expect()` on anything that can fail at runtime. Config
  parsing, subprocess spawning, and filesystem access all degrade gracefully —
  a corrupt config file must never block startup.
- Errors that reach the user become a notification, not a panic.
- Colours come from `theme.rs`. Never hardcode a `Color::Rgb` in a UI module.
- Comments explain *why*, not *what*.

### Immediate-mode UI
ratatui redraws the whole frame every event; there are no persistent widget
objects and no CSS. Three consequences shape the code:

1. **Focus is an index, not a widget.** `app/detail.rs::content()` walks the
   current selection once and produces the pane's full node list — text, cards,
   and every focusable control in order. `App::detail_items()` collects the
   focusable items from that list and `ui/detail.rs` renders the same list, so
   item N on screen is item N in the key handler *by construction*. To add or
   move a control, change the content walk; both consumers follow automatically.

   Activation goes through one more indirection: each frame the renderer
   snapshots the drawn items onto `App::drawn_items` (with each control's
   enabled bit), and a click or `Enter` resolves against that snapshot — never
   against a freshly derived list, which a background event drained in the same
   batch may already have reshaped. The snapshot is what was on screen, which
   is what the user acted on; a control drawn as disabled must not fire.
2. **Layout is `Layout`/`Constraint`, not stylesheets.** Sizes are computed per
   frame; nothing is "auto height".
3. **Mouse support is manual.** There is no widget tree to hit-test against, so
   every frame the renderers record the rectangle of each clickable thing via
   `App::push_hit(rect, HitTarget::…)`, and `App::handle_mouse` resolves a click
   against that list (last recorded wins, so a modal takes clicks from the panes
   it covers). A control that is drawn but never recorded is dead to the mouse —
   that was a real regression: the first Rust build enabled no mouse capture at
   all and every click did nothing.

   The mouse modes behind hover are terminal state owned by
   `src/terminal.rs` (see Design notes).

Controls are drawn as three-row bordered boxes from one builder in
`ui/widgets.rs`: `button_box`/`button_box_width` for modal buttons (Textual's
`min-width: 10`) and the same code via `boxed_rows`/`boxed_width` with no
minimum for the detail pane, whose boxes hug their labels. Use the matching
width function for the rectangle you record, so the hit region matches what
was drawn.

### Async and the event loop
Everything funnels through one `mpsc::UnboundedReceiver<AppEvent>` in
`event.rs`. Terminal input arrives from a blocking reader thread; background
work arrives from `tokio::spawn`.

**Never block the event loop.** For any operation that may take time (git, gh,
scanning session files):
1. Render immediately with a loading state (`app.sessions` is an `Option`,
   where `None` means "still loading").
2. `tokio::spawn` the work with a cloned `EventTx`.
3. Send an `AppEvent` when it completes; `App::handle_event` folds it into state.

Late results must be discarded when the selection has moved on — compare the
event's `path` against the current one before applying it.

**The main loop is the single writer of `.forestui-config.json`.** Background
tasks never load or save state themselves — they send their results as data
(`WorktreeAdded`, `WorktreesScanned`, `WorktreeRenamed`) and `App::handle_event`
folds them in and persists. Two writers means last-write-wins, and a user
action mid-flight silently clobbers the task's save. Success toasts belong in
the fold, not the task: a result the fold drops must not be announced. The
one tolerated exception is `forestui prune` (`prune::prune_forest`); see
`doc/design/finished-and-prune.md`.

**The worktree list is reconciled against git, not owned by the config.**
`git worktree list` is scanned in the background (startup, repo add, selection
change, focus return, a 30s sweep) and the fold adopts, prunes and
branch-corrects via `AppState::reconcile_worktrees` — the config only
annotates worktrees (name, archived flag, sort order). A scan result is a
*snapshot*, so it must never outrank a fresher mutation. `app/worktree_sync.rs`
owns that rule: a scan starts with a `ScanTicket`, and handing the ticket back
with the listing says whether to ignore it (stale, or a create running on that
repository), keep state (git could not list), or reconcile it while leaving
the worktrees with a change in flight alone.

**Every worktree-mutating flow goes through `WorktreeSync`.** Starting the
change returns a change ticket — or nothing, if the worktree already has a
change running, since a worktree takes one change at a time — the task carries
it in its result event, and the fold hands it back. Finishing always moves the
repository's epoch on, success or not, so every exit path of the task must
send an event that carries the ticket; a path that only toasts leaves the
change in flight for good.

**The tick must earn its repaints.** Ticks arrive ten times a second; an idle
frame repaints once a second (matching the second granularity of the relative
times on screen) and at full rate only while a spinner is actually visible.
Anything animated that is added later has to mark the frame dirty from
`App::on_tick`, or it freezes; anything that is not visibly changing must not.

### Services
Services are free functions in modules, not singletons. Shared caches (the
GitHub issue cache, the auth status) live behind a `OnceLock<Mutex<...>>` inside
the module that owns them.

## Key Design Decisions

### tmux Requirement
forestui requires tmux and re-executes itself into a tmux session when started
outside one. This enables TUI editors in tmux windows, session persistence, and
fast switching between editor, app, and Claude windows.

The re-exec uses `std::env::current_exe()`, so a `cargo run` build re-launches
itself rather than whatever `forestui` happens to be on `PATH`.

### Multi-Forest Support
The forest directory is a CLI argument, not a setting:

```bash
forestui ~/forest      # default
forestui ~/work        # different forest
```

Each forest has its own `.forestui-config.json` state file. Note that the
worktree *list* under a repository is reconciled from `git worktree list`,
which knows nothing about forests: tracking the same repository in two
forests shows the same worktrees in both, and removing one removes it for
both. Forests separate which *repositories* you look at, not worktree
ownership — git owns that.

### Config Compatibility
forestui updates itself on launch, so the builds that matter are the newest
release and the one before it (`n-1`) — the Python build is history, not a
constraint. `.forestui-config.json` and `~/.config/forestui/settings.json` must
load in both directions across that one-release gap:

- **Add** fields with a `#[serde(default)]` (or `default = "…"`), so a file
  written by `n-1` loads here. `n-1` ignores the key it does not know — and
  drops it on its next save, so a new field's default must be a value that
  is fine to fall back to.
- **Remove** a field by deleting it: unknown keys are ignored on load
  (never add `deny_unknown_fields`) and vanish on the next save.
- **Rename** a field only across two releases, reading both names in the
  first, or `n-1` and `n` silently lose the value to each other.

Partial files load cleanly because every field is defaulted.

Both files are written via `util::write_atomically` (sibling temp file +
rename). A corrupt config deliberately loads as *empty* state so a bad byte
never blocks startup — which is exactly why the write itself must never be
able to produce a torn file.

### Coexistence with forest (macOS)
- forestui uses `.forestui-config.json`
- forest uses `.forest-config.json`
- Both can share the same `~/forest` directory safely

## Design notes

These areas each carry a design write-up in `doc/design/`. Read the note
before changing its area — each records load-bearing reasons the code alone
does not show:

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

## Common Tasks

### Adding a New Service
1. Create `src/services/myservice.rs`
2. Add `pub mod myservice;` to `src/services/mod.rs`
3. Expose free functions; keep any shared state in a module-private `OnceLock`

### Adding a Detail-Pane Action
1. Add a variant to `Action` in `src/app/detail.rs`
2. Emit a `ControlSpec` for it from the content walk in `src/app/detail.rs`
   (`repository()` / `worktree()` / their sections) — the item list and the
   rendering both derive from that one walk, so there is no second place to
   keep in sync
3. Handle it in `App::run_action` in `src/app/actions.rs`
4. If it needs a new visual shape (not a boxed control), add a `DetailNode`
   variant and render it in `render_node` in `src/ui/detail.rs`

### Adding a Key Binding
1. Add it to `BINDINGS` in `src/app/keys.rs` — the footer renders this table
   and is the app's *only* key-discovery surface (there is no help screen; the
   `?` toast was removed with issue #30). A test asserts the whole footer fits
   140 columns, so a new entry means shortening a label somewhere; entries
   that do not fit a narrower terminal clip from the right and lose their
   click target
2. Handle the key in the `handle_key` match in the same file

### Adding a Modal
1. Add the state struct and its `handle_key` to `src/modal.rs`, declaring a
   `FOCUS_*` constant per control — the constants are the contract the
   renderer uses too
2. Add a variant to the `Modal` enum and wire it into `handle_key` / `tick`
3. Render it in `src/ui/modals.rs`
4. If it returns a value, add a `ModalResult` variant and handle it in
   `App::apply_modal_result`

### Adding a New CLI Option
1. Add the field to `Args` in `src/cli.rs`
2. Carry it through `self_command` so the tmux re-exec preserves it
3. Handle it in `main`

### Modifying Settings
1. Update `Settings` in `src/models.rs` (keep `#[serde(default)]`)
2. Update `SettingsModal` in `src/modal.rs` and its rendering in `src/ui/modals.rs`
3. Settings are persisted to `~/.config/forestui/settings.json`

Start work settings and prompt templates can also be overridden per
repository, under `~/.config/forestui/repos/<owner>/<name>/` (a
`settings.json` read only for its `start_work` section, plus a `prompts/`
directory) — never inside a checkout. The key is the `owner/name` `gh`
resolves, learned in the background, so code that needs Start work settings
asks `App::start_work_for(path)` and template lookups go through
`App::prompt_layers_for(path)` (repository, then global, then built-in),
never `settings.start_work` or `prompts_dir()` directly. A repository with no
GitHub repository has no key and uses the global settings.

## Testing

Unit tests live in a sibling file, not inline: `foo.rs` ends with
`#[cfg(test)] mod tests;` and the tests are in `foo/tests.rs` (`bar/mod.rs` →
`bar/tests.rs`). As a child module they still reach private items through
`use super::*`. Keep new tests there rather than growing the source file; note
that an `include_str!` in a test file resolves one directory deeper than in its
parent.

The App tests are the one exception: there are too many for one file, so they
are split by feature under `src/app/tests/` — `navigation.rs`, `worktrees.rs`,
`sessions.rs`, `mouse.rs`, `start_work.rs`, `repository_settings.rs` and so
on. A new App test goes in the file for its feature, or in a new one declared
in `src/app/tests/mod.rs`. Each file starts with `use super::*;`, and a helper
used by more than one file lives in `mod.rs`.

They run with `make test`. Async code uses `#[tokio::test]`. Rendering is tested against
`ratatui::backend::TestBackend`, which renders into an in-memory buffer you can
assert on.

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

## Versioning

Version is `0.0.0` in source (`Cargo.toml` and `cli::VERSION`). Real versions
are derived from git tags at release time by the release workflow.

Running from source shows version `0.0.0` and auto-enables dev mode, which gives
the tmux window a timestamped name (`forestui-dev-HHMM`) so a dev instance never
collides with a real one.

### Development

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

### Releasing

```bash
gh release create v0.10.0 --generate-notes
```

This triggers the release workflow, which stamps the version from the tag,
builds the static musl binaries for Linux (x86_64, aarch64) and the macOS
Apple-silicon binary, attaches them to the release with their checksums, and
publishes to crates.io.

## Git Commits

- Do NOT include `Co-Authored-By` attribution
- Do NOT include "Generated with Claude Code" footer
- Write clear, concise commit messages

## References

- [forest (macOS)](https://github.com/ricwo/forest) - Original inspiration
- [ratatui Documentation](https://ratatui.rs)
- [crossterm Documentation](https://docs.rs/crossterm)
- [tokio Documentation](https://tokio.rs)
