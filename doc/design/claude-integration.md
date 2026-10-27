# The Claude integration: names follow tabs, liveness follows sessions

A tmux window forestui opened and the Claude session running in it carry the
same name, in both directions: renaming the tab renames the session, and
`/rename` renames the tab. The two are **one string, verbatim** — nothing is
added, stripped or parsed. A window called `yolo:thing` holds a session called
`yolo:thing`. Prefixes are forestui's opening move when it creates a window,
not a format anything reads back.

It is carried by a Claude Code **plugin** — a directory under the Claude config
dir, which Claude discovers on its own. Installing therefore writes nothing to
the user's `settings.json`, and hooks they configured themselves are never
parsed or rewritten; plugin hooks and settings hooks both run. Enabling and
disabling is Claude's own business (`claude plugin enable|disable`), which is
why nothing here edits `enabledPlugins`. Install it with
`forestui --claude-plugin install`; `status` prints every path an install would
write without writing any of them.

A new session is named at launch, by passing Claude's own `-n` flag with the
window's name. That is not interchangeable with setting the title from a hook:
Claude draws the name in the prompt box from the first frame with `-n`, whereas
a title a hook sets at `SessionStart` is stored and never drawn — the session is
correctly named and looks unnamed. A hook still names one on its first prompt if
`-n` never reached it.

**A resumed session is not given `-n`.** It already carries the name it was
given, and the window's name may have picked up a `:2` from uniquifying against
a window still open for the same conversation — passing it would overwrite the
real name with the suffixed one. The hook adopts the stored name onto the
window instead.

The window options and heartbeats behind that, each load-bearing:

- **`@claude_birth_name`**, a tmux window option stamped when the window is
  created, answers one question: did forestui open this window? A window the
  user made by hand carries none and is left alone, as is a bare `claude` in a
  plain terminal. It is written from *inside* the window, ahead of the command
  that needs it — stamping it from forestui after `new-window` returns is a
  separate tmux call, and a fast-starting Claude could reach its first hook
  before it lands.
- **`@claude_synced_name`** records the last agreed name. Two-way sync cannot
  tell which side moved from one value: equal names are ambiguous between
  nobody changing and both changing alike, and unequal names do not say who is
  stale. When both moved, the tab wins.
- **`@claude_session_id`**, a second window option stamped the same way as the
  birth name (from inside the window, ahead of the command), records which
  Claude session the window holds — the pre-minted `--session-id` for a fresh
  session, the resumed id otherwise. It is what makes window ↔ session a
  recorded fact: the live badge, the duplicate-open guard and the delete
  refusal all resolve against one `list-windows` sweep over these stamps, never
  against transcript mtimes. Corollary: the shell history line for a fresh
  session remembers the *resume* form (`claude -r '<id>'`), because
  `--session-id` refuses to run once the session exists and the up arrow exists
  precisely for the second run.
- **Heartbeats** complete the picture for sessions forestui did not start.
  The plugin's `heartbeat.sh` runs inside *every* Claude session on the
  machine and keeps one file per session under `~/.config/forestui/live/`,
  carrying the claude process id — validated against `ps` before it is
  believed, swept when stale. `services/live.rs` merges heartbeats with the
  window stamps: a heartbeat whose recorded pane sits on forestui's own tmux
  session (or a member of its session group, which shares its windows) is
  switchable like a stamped window. One in another tmux session on the same
  server is offered to be *brought here*: `move-window`, so Claude keeps
  running. The move goes ahead only if the window's heartbeat-healed
  `@claude_session_id` still names the session, and the window gets no birth
  name, so title sync and the back key still leave it alone. Anything else
  shows as "live elsewhere" and resuming it is an eyes-open confirm. The
  heartbeat records its pane's tmux *server* too (`$TMUX` minus the session
  index): pane ids are only unique per server, so a pane is resolved only
  when it was written on forestui's own server, and one with no server
  recorded is elsewhere, never a guessed window. The hook also re-stamps
  `@claude_session_id` on its window, healing a stale stamp left by an
  earlier launch.
- **Hand-offs.** ← in Claude Code moves a conversation into a background
  daemon (`claude daemon run --origin transient`) that carries on under a
  *new* session id, with a forked transcript of the same name. The old
  transcript ends with a `continued-in` record naming the successor. Read
  naively that is two live sessions: the window still carries the old
  `@claude_session_id` and its client is still alive, and the daemon's
  heartbeat has no pane, so the new id is "live elsewhere". `live::merge`
  therefore drops a superseded id however alive it looks, and gives the
  successor the predecessor's window — that window *is* where the user
  reaches the conversation, which the daemon's heartbeat cannot know, since
  its hooks never see the pane. A successor that is not live leaves nothing
  live: the conversation has ended. The superseded transcript gets no card.
  The link is read from `continued-in` rather than inferred from pids (the
  daemon is a child of the pane's `claude`): the record is Claude's own
  statement of which id is the conversation now, whereas a process tree
  says nothing about which session a process holds, and the daemon can host
  more than one id. As with titles, only a record stamped with the file's
  own `sessionId` counts — a fork copies its parent's hand-off verbatim. The
  live scan reads each transcript incrementally from where it stopped,
  because it runs every sweep over files that are still growing.
- **Session state** — *busy*, *needs you* or *idle* — rides in the same
  heartbeat file as an optional `state` field. `heartbeat.sh` writes it with
  the file (busy on `UserPromptSubmit`; idle on `SessionStart`, except a
  compaction restart, which keeps whatever it was), and `state.sh` updates it
  in place in between: `Notification` for a permission prompt or question
  and `PreToolUse` of `AskUserQuestion` → needs you, any tool use *after*
  needs you → busy (the user answered), `Stop` and the `idle_prompt`
  notification → idle. Only those two scripts write the file and never on
  the same event; their writes can still interleave across events (an
  `idle_prompt` landing as a prompt is submitted), a millisecond window the
  turn's `Stop` repairs, so it is documented in `state.sh` rather than
  locked. The tool-use hooks fire on every tool call, so their common path
  writes nothing. Esc fires no
  `Stop`, so an interrupted session reads busy until `idle_prompt` (~60 s)
  corrects it — accepted, not worked around. A heartbeat with no `state`
  (an older plugin) is *unknown* and shows as it always did; it is never a
  guessed state. The App re-reads just the states of the sessions the last
  live scan found once a second (`STATE_READ_INTERVAL`, files only, no tmux or
  `ps`) and repaints only when one changed. A session card's badge reads
  `● live · busy`; a worktree's sidebar row takes the colour of the most
  urgent state among sessions in forestui's *own* windows (needs you > busy >
  idle) and leads with `{N}` from two of them (a stateless one counts but
  lends no colour) — a finished worktree keeps its
  finished name colour, the count still wears the state's.
- **Window names reach a shell**, so they are quoted literally with single
  quotes rather than by any escaping heuristic. A session title can be set by a
  `SessionStart` hook in a repository's own `.claude/settings.json`, so by the
  time a name gets here it is not necessarily the user's own text. The shell
  reading it is *interactive*, where double quotes would not even stop zsh
  history expansion.

There is deliberately no off switch, per window or global. Installed means the
two names agree; not wanting that is an uninstall.

**Remote Control rides on the same name, so it is passed bare.** The Settings
checkbox (`remote_control`, on by default — a settings file without the key
loads it *on*, so updates pick it up; only an explicit `false` is off) adds
`--remote-control` to every launch
`claude_command_line` builds — both built-in buttons, custom buttons, resumes,
and the history line — and never gives it a value. With no value, Claude names
the remote session after the session itself: the `-n` name on a fresh launch,
the stored title on a resume, and it follows `/rename`; all three were checked
against the phone app. A value could only disagree with that, and on a resume
it would be the window's name — the `:2`-suffixed one the missing `-n` exists
to keep away. The value is optional (`--remote-control [name]`), so the flag is
always followed by `-r` or `-n`, never a bare word it could take as its name;
a test holds that line for built-in and custom launches alike. A custom command
that already asks for it (`--remote-control` or Claude's `--rc` alias, bare or
`=name`) is left alone — Claude reads `--remote-control` first, so ours would
override the button's name. A Claude too old to know the flag would refuse
every launch, so `services/claude_cli.rs` asks `claude --help` once at startup
and drops the flag only on a definite no; a probe that could not run keeps it.
