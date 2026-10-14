# Finished worktrees and prune

**Finished worktrees ride on the worktree scans** (the background `git worktree
list` reconcile in `event-loop.md`). Each scan also kicks a check (throttled per
repository, `app/finished.rs`) that asks `gh` for each linked issue's or PR's
state and git whether a session's branch is merged into the remote default
branch; `work::is_finished` is the rule. The marks are derived and never saved.
A worktree the check could not answer for (`gh` signed out, no remote default
branch) keeps its previous mark — "could not tell" must not unmark it.

**Prune is one routine, `prune::prune_worktree`**, shared by the repository
pane's **Prune finished (N)** and `forestui prune`. It skips a worktree with
uncommitted changes or commits on no remote branch (any remote branch counts,
so a merged PR whose branch was deleted upstream is still prunable), then
removes it and force-deletes its local branch. In the App each removal starts
through `WorktreeSync` like a Delete, and one task hands every ticket back in
`WorktreesPruned`, so the user gets one notification for the whole prune.
The CLI has no `WorktreeSync`: `prune::prune_forest` reconciles each
repository against git, runs the same finished check, and prunes through
`prune_tracked`. It is a one-shot dispatched in `main` before `ensure_tmux`,
so it never starts tmux. It is also the one writer of `.forestui-config.json`
outside the main loop, tolerated because a running App heals: its next save
restores the pruned entries and its next scan drops them, directories gone.
