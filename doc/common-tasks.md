# Common tasks

## Adding a New Service
1. Create `src/services/myservice.rs`
2. Add `pub mod myservice;` to `src/services/mod.rs`
3. Expose free functions; keep any shared state in a module-private `OnceLock`

## Adding a Detail-Pane Action
1. Add a variant to `Action` in `src/app/detail.rs`
2. Emit a `ControlSpec` for it from the content walk in `src/app/detail.rs`
   (`repository()` / `worktree()` / their sections) — the item list and the
   rendering both derive from that one walk, so there is no second place to
   keep in sync
3. Handle it in `App::run_action` in `src/app/actions.rs`
4. If it needs a new visual shape (not a boxed control), add a `DetailNode`
   variant and render it in `render_node` in `src/ui/detail.rs`

## Adding a Key Binding
1. Add it to `BINDINGS` in `src/app/keys.rs` — the footer renders this table
   and is the app's *only* key-discovery surface (there is no help screen; the
   `?` toast was removed with issue #30). A test asserts the whole footer fits
   140 columns, so a new entry means shortening a label somewhere; entries
   that do not fit a narrower terminal clip from the right and lose their
   click target
2. Handle the key in the `handle_key` match in the same file

## Adding a Modal
1. Add the state struct and its `handle_key` to `src/modal.rs`, declaring a
   `FOCUS_*` constant per control — the constants are the contract the
   renderer uses too
2. Add a variant to the `Modal` enum and wire it into `handle_key` / `tick`
3. Render it in `src/ui/modals.rs`
4. If it returns a value, add a `ModalResult` variant and handle it in
   `App::apply_modal_result`

## Adding a New CLI Option
1. Add the field to `Args` in `src/cli.rs`
2. Carry it through `self_command` so the tmux re-exec preserves it
3. Handle it in `main`

## Modifying Settings
1. Update `Settings` in `src/models.rs` (keep `#[serde(default)]`)
2. Update `SettingsModal` in `src/modal.rs` and its rendering in `src/ui/modals.rs`
3. Settings are persisted to `~/.config/forestui/settings.json`

A Start work setting goes in the Start Work Settings dialog instead
(`src/modal/start_work_settings.rs`): add a `FOCUS_*` constant and a row in
`FIELDS` giving its path in the `start_work` section, its kind and its grid
slot. Loading, the inherited placeholder, validation, the global save and a
repository's overrides all follow from that row, in both of the dialog's
modes.

Start work settings and prompt templates can also be overridden per
repository, under `~/.config/forestui/repos/<owner>/<name>/` (a
`settings.json` read only for its `start_work` section, plus a `prompts/`
directory) — never inside a checkout. The key is the `owner/name` `gh`
resolves, learned in the background, so code that needs Start work settings
asks `App::start_work_for(path)` and template lookups go through
`App::prompt_layers_for(path)` (repository, then global, then built-in),
never `settings.start_work` or `prompts_dir()` directly. A repository with no
GitHub repository has no key and uses the global settings.

One key exists only in a repository's file: `start_work.base_branch`, what
new work stems from (`origin/develop`, or a local `develop`). It is not on
`StartWorkSettings` — the global settings have no such field, the remote's
default branch being the only sensible global value — so it is read with
`App::base_branch_for(path)` rather than through `start_work_for`. It
prefills the Base field of Start work's confirm screen and of the plain
worktree form (`modal::initial_base_branch`), falling back to
`default_base_branch` with an inline note when the branch list lacks it;
Delete keeps it as it keeps the default branch. The prefilled settings file
writes it as `null`, so it can be found.
