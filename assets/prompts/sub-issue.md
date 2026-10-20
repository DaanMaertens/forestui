Work on GitHub issue #{number}: {title}

It is a sub-issue of #{parent_number}. Start by reading both with `gh issue view {number} --comments` and `gh issue view {parent_number} --comments`, so you have their current text and every comment.

You are in the `{worktree}` worktree of {repo}, on the branch `{branch}`, which stems from `{parent_branch}`, the branch of #{parent_number}.

Investigate what it needs and propose a plan before changing any code. Wait for my go-ahead, then implement it.

When it is done, open its pull request into `{parent_branch}`, not the default branch: `gh pr create --base {parent_branch}`. Start its description with "Part of #{parent_number}".
