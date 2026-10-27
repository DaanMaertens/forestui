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
so `forestui prune` judges the same way without them.

**The mark is saved on the worktree, and the check writes only changes.**
`Worktree.finished` is the mark; `Worktree.last_finished_check` is the
check's last answer. A verdict that differs from the last answer (or the
first one ever) sets the mark and is saved as the new last answer; the same
answer again leaves the mark alone (`Worktree::fold_finished_check`, applied
per repository by `AppState::fold_finished_verdicts`). The user toggles the
mark on any worktree — `f`, or **Mark finished** / **Unmark finished** beside
Archive — without touching the last answer. So whichever changed the mark
last wins: an Unmark of a closed issue's worktree sticks across sweeps and
restarts, and the issue being reopened (a changed answer) unmarks it again.
Who set a mark is not recorded; it looks the same either way. A worktree the
check could not answer for (`gh` signed out, no remote default branch) gets
no verdict, which changes neither field — "could not tell" must not unmark it.
The check task sends verdicts as data and the main loop's fold saves, so the
marks show at startup before any check has run.

**Prune is one routine, `prune::prune_worktree`**, shared by the repository
pane's **Prune finished (N)** and `forestui prune`. It skips a worktree with
uncommitted changes or commits on no remote branch (any remote branch counts,
so a merged PR whose branch was deleted upstream is still prunable), then
removes it and force-deletes its local branch. In the App each removal starts
through `WorktreeSync` like a Delete, and one task hands every ticket back in
`WorktreesPruned`, so the user gets one notification for the whole prune.
The CLI has no `WorktreeSync`: `prune::prune_forest` reconciles each
repository against git, runs the same finished check, folds it into the saved
marks by the same rule (saving them), and prunes every marked worktree —
hand-marked ones included, and saved marks even with `gh` signed out —
through `prune_tracked`. Its safety rules hold whatever set the mark. It is a one-shot dispatched in `main` before `ensure_tmux`,
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
