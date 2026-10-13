# Config files and forests

## Config compatibility

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

## Multiple forests

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

## Coexistence with forest (macOS)

- forestui uses `.forestui-config.json`
- forest uses `.forest-config.json`
- Both can share the same `~/forest` directory safely
