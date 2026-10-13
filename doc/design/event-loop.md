# The event loop

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
