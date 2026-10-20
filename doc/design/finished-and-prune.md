# Finished worktrees and prune

**Finished worktrees ride on the worktree scans** (the background `git worktree
list` reconcile in `event-loop.md`). Each scan also kicks a check (throttled per
repository, `app/finished.rs`) that asks `gh` for each linked issue's or PR's
state and git whether a session's branch is merged into the base branch the
worktree recorded when it was created (`Worktree.base_branch`), or into the
remote default branch for a worktree from before that was recorded;
`work::is_finished` is the rule. A sub-issue's worktree
(`Worktree.parent_issue` set) is also asked whether a PR from its branch into
that base, the parent issue's branch, is merged: its issue normally stays open
until the parent's PR lands, so either one finishes it
(`work::is_sub_issue_finished`). The check never reads repository settings,
so `forestui prune` judges the same way without them. The marks are derived and never saved.
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

**A Delete shares prune's branch rule, `prune::delete_branch_if_pushed`.**
Start work always makes a fresh branch and derives the same name for the same
issue, so a branch left behind by a Delete blocks starting that issue again.
The order differs from prune: a Delete removes the worktree regardless (a tree
that is dirty, locked, or whose changes cannot be listed has its own confirm), then deletes the branch only if nothing on it
is unpushed and it is not the default branch (a worktree made on `main`
hands it back; with no remote default recorded, `main` and `master` are
spared) or the base branch the repository settings name (matched by its local
name, so `origin/develop` spares `develop`; read on the main loop and passed
into the task), and keeps it — saying why in the notification — on any doubt.
The branch comes from git's worktree list before removal, not from state, so
a branch switched since the last scan is not the one deleted.
Remote branches are never touched.
