# Claude runs inside a shell, not as the window's command

A tmux window forestui opens for Claude runs the user's shell, interactively,
and Claude is one of its *jobs* — not the window's own process.

That is the whole point. A window whose only process is Claude has nothing
underneath it: Ctrl-C leaves a dead pane, and Claude's own Ctrl-Z suspend stops
the process with no shell left to `fg` it back, so the session is unreachable
and the window gets closed to be rid of it — a lost conversation. With a shell
underneath, a suspended session lands at a prompt where `fg` resumes it, a
crash or a refused launch lands at one where the up arrow retries it, and
nothing has to be thrown away. A *clean* exit closes the window — see below.

The command is **not typed into the window**. It is written into a startup file
that forestui generates and points the shell at, so it is already there when
the shell reads it — nothing to race, nothing to arrive half-read. Each shell
has exactly one such hook, and each hook *replaces* one of the user's own files
rather than adding to it, so the generated file sources the file it displaced:

| shell | pointed at with | replaces, so sources back |
|---|---|---|
| zsh | `ZDOTDIR=<dir>` | `.zshenv` and `.zshrc` |
| bash | `--rcfile <file>` | `.bashrc` |
| sh, dash, ksh, mksh, ash, yash | `ENV=<file>` | whatever `ENV` already named |

The shell is started interactive and non-login, which is exactly what
`$SHELL -ic` gave before, so a Claude window's environment is unchanged from
the build that ran Claude directly. `startup_for` is pure and tested; the file
deletes itself once read, so nothing accumulates in the temp dir.

Four things in the generated file are load-bearing:

- **`set -m`.** Without it a command run from a startup file is not a job:
  bash hands it the shell's own ignored `SIGTSTP`, so Ctrl-Z does nothing at
  all and the window wedges with no way back. zsh already runs interactive
  shells this way; saying so costs nothing and makes the file mean the same
  thing under every shell.
- **The birth stamp goes in ahead of the command**, from inside the window, for
  the reason it always did: the shell starts Claude by itself, so a stamp
  forestui sends after `new-window` returns is a separate tmux call a
  fast-starting Claude can beat to its first hook.
- **The command is pushed into the shell's history** (`print -s`, `history -s`)
  where the shell has a builtin for it. Typing it used to do that for free, and
  after an accidental Ctrl-C the up arrow is the shortest way back. *When* it
  is pushed is load-bearing for zsh: zsh reads `HISTFILE` only after its
  startup files, and Claude runs inside this one, so a line pushed from it
  lands under the whole loaded file — with `share_history`, the up arrow then
  found another window's line and resumed a different conversation. zsh
  pushes from a one-shot `precmd` hook instead, at the first prompt after
  Claude exits or is suspended. bash reads its history before the rc file and
  needs no hook. `the_remembered_line_is_the_newest_history_entry` drives both
  real shells to hold this.
- **A clean exit takes the shell with it** (`[ $? -eq 0 ] && exit` after the
  command), so Ctrl-D twice closes the window and lands back in forestui
  rather than at a prompt that needs a third Ctrl-D. Claude exits 0 for Ctrl-D
  twice, Ctrl-C twice and `/exit` alike — the status is all the shell sees, so
  those three cannot be told apart and all close. Ctrl-Z leaves 148 and a crash
  or refused launch some other non-zero status, and those keep the shell.
  A session resumed with `fg` and then exited cleanly is still at a prompt:
  the line that tests the status already ran when it was suspended.
  `only_a_clean_exit_closes_the_shell` drives bash and sh; zsh refuses `set -m`
  without a terminal, so it is checked under `tu`.

A shell with no hook we have tested — fish, nushell — falls back to typing the
line into a plain shell with `send-keys`. That path is a real fallback, not a
leftover: it needs no per-shell knowledge at all. Its one weakness is why it is
not the default — keystrokes sent before the shell is ready wait in the
terminal for it, and a startup that *consumes* input (zsh-newuser-install in a
bare `HOME` is a real example) eats them. Because it is keystrokes, the line
must be one line of text: `send-keys -l` sends literal characters and `Enter`
is a separate key name, which is why it is two calls.

Either way the line itself is built by `claude_command_line` and stripped of
control characters by `single_line`. A newline in a window name would be an
Enter key on one path and a second line of the startup file on the other — and
window names come from session titles, which a repository's own
`.claude/settings.json` can set.

The typed fallback does not get the clean-exit rule: it knows nothing about
the shell it types into, so a window there still drops to a prompt when Claude
exits.
