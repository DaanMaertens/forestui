Review pull request #{number}: {title}

Start by reading it with `gh pr view {number} --comments`, so you have its current description and every comment, then the change itself with `gh pr diff {number}`.

You are in the `{worktree}` worktree of {repo}, on the PR's branch `{branch}`.

Someone else opened this PR. Review it and report: check that it does what it says, and look for bugs, missing tests and anything that does not fit the code around it. List your findings by severity, with file and line. Do not change any code.
