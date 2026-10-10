Review my own pull request #{number}: {title}

Start by reading it with `gh pr view {number} --comments`, so you have its current description and every comment, then the change itself with `gh pr diff {number}`.

You are in the `{worktree}` worktree of {repo}, on the PR's branch `{branch}`.

Review it the way a reviewer would: check that it does what it says, and look for bugs, missing tests and anything that does not fit the code around it. First list your findings by severity, with file and line, then wait for my go-ahead before changing any code. Once I have said which to fix, fix them on this branch.
