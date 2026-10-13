# Ways a Claude session can talk to forestui

Research for [#92](https://github.com/DaanMaertens/forestui/issues/92), part of
the map [#91](https://github.com/DaanMaertens/forestui/issues/91) (one
coordinator session over a queue of workers). Two questions:

- **Session → forestui**: how does a running Claude Code session hand
  *structured* data (a plan, a queue of work items, a result) to forestui?
- **forestui → session**: how does forestui push an event (a worker finished,
  the queue drained) *into* a session that is already running, ideally waking
  it if it is idle?

Checked against Claude Code **2.1.296** (installed locally), the official docs
at code.claude.com (fetched 2026-10-13) and the
[Claude Code changelog](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md).
Each claim cites its page; "CL x.y.z" means the changelog entry for that
version.

## Summary

| Mechanism | Session → forestui | forestui → session | Wakes an idle session | Status |
|---|---|---|---|---|
| Session runs a `forestui` CLI (Bash tool, plugin `bin/`) | **Yes**, any JSON on stdin/args, validated by forestui | Only as the command's stdout, in the turn that ran it | n/a | Stable (`bin/` since CL 2.1.91) |
| Plugin skill / slash command | Indirect: instructs Claude to call the CLI or MCP tool; `` !`cmd` `` pulls forestui data *into* the prompt | Pull only, at invocation | No | Stable |
| Hooks (command / http / mcp_tool) | Yes, every event's JSON input (incl. `last_assistant_message` on Stop) | `additionalContext` / `reason` on the *session's own* events; `asyncRewake` exit 2 | **Only `asyncRewake`**, and only from a hook the session itself started | Stable |
| MCP server shipped by the plugin (stdio) | **Yes**, typed tool calls with a JSON Schema `inputSchema` | Tool results only (no general push) | No | Stable |
| MCP **channel** (`claude/channel`) | Yes (reply tool) | **Yes**, `notifications/claude/channel` | Yes (queued to next turn) | Research preview, allowlist, needs `--dangerously-load-development-channels` for a custom one |
| **Cross-session inbox socket** (`CLAUDE_CODE_MESSAGING_SOCKET`) | — | **Yes**, a line-based message on a per-session Unix socket | **Yes**, starts a new turn | GA since 2.1.224; wire format beyond the auth line undocumented |
| Monitor tool / plugin monitors | — | Yes, each stdout line of a watcher becomes an event | Yes (interjects) | Monitor tool GA (CL 2.1.98); plugin monitors `experimental` |
| Headless `claude -p --output-format json --json-schema` | **Yes**, `structured_output` validated against a schema | `--input-format stream-json` into a process forestui owns; `--resume <id>` for a new run | n/a (forestui drives every turn) | Stable |
| Background sessions (`claude --bg`, `claude agents --json`) | Status only (`state`, `status`) | `--resume <id> --bg "<prompt>"` starts a run; no "send to running session" CLI | — | Stable |
| Agent teams | — | Mailbox files are Claude Code internals, not an API | — | Experimental, env-gated |
| Remote Control | — | Human from claude.ai / phone only | — | Not for local processes |

**Gist.** For *session → forestui*, the robust path is a forestui CLI
subcommand (or a plugin-shipped stdio MCP server wrapping the same thing) that
takes a JSON plan and validates it; the plugin's skill tells the coordinator
how to call it. For *forestui → session*, the one stable, local, no-flag
mechanism that also wakes an idle session is the **cross-session inbox
socket**: forestui already learns every session's identity through
`heartbeat.sh`, and the same hook can record `CLAUDE_CODE_MESSAGING_SOCKET`.
Channels do the same job with a documented contract but are a research preview
that requires a scary dev flag for any non-Anthropic channel. An `asyncRewake`
hook is a fallback that needs no socket. Headless `-p` sidesteps the question
entirely but gives up the interactive coordinator window.

## What forestui already ships

`assets/claude-plugin/` (plugin `forestui-integration`, version 7) is hooks
only — no skills, commands, MCP servers, monitors or `bin/`:

| Event | Script | What it does |
|---|---|---|
| `SessionStart`, `UserPromptSubmit`, `Stop` | `tmux-title.sh` | Two-way window ↔ session name sync |
| `SessionStart`, `UserPromptSubmit`, `SessionEnd` | `heartbeat.sh` | Writes `~/.config/forestui/live/<session_id>.json` (claude pid, pane, tmux server, state); removes it on end |
| `PreToolUse`, `PostToolUse`, `Notification`, `Stop` | `state.sh` | Updates `state` (busy / needs_you / idle) in the heartbeat file |

So forestui already has a **session → forestui** channel of the "write a file
forestui polls" kind, keyed by `session_id`, and it already runs inside every
session at `SessionStart`. Both facts matter below: the heartbeat is the
natural place to record the session's inbox socket, and the plugin is the
natural vehicle for any skill, MCP server, or `bin/` executable.

## Mechanisms in detail

### 1. The session shells out to a forestui CLI

Claude runs commands with the Bash tool, so `forestui queue submit --json -`
(any subcommand forestui adds) is available today with no Claude Code feature
at all. Data shape is whatever forestui defines; forestui validates and
rejects with a non-zero exit and a message Claude reads.

- A plugin can ship executables in `bin/`; "Files here are on the Bash tool's
  `PATH` while the plugin is enabled, so Claude runs them as bare commands"
  ([plugins reference, Standard layout](https://code.claude.com/docs/en/plugins-reference#standard-layout);
  CL 2.1.91). claude.ai/Cowork refuse to install a plugin with `bin/`, which
  does not affect forestui's local install. forestui's own binary is
  normally already on `PATH`, so `bin/` is only needed to pin the exact
  binary.
- `CLAUDE_PLUGIN_ROOT` etc. are *not* in the Bash tool's environment
  ([plugins reference, Environment variables](https://code.claude.com/docs/en/plugins-reference#environment-variables)),
  but `CLAUDE_CODE_MESSAGING_SOCKET`/`_TOKEN` are (see §6), and so is
  `$TMUX`/`$TMUX_PANE` from the window. A CLI call can therefore identify its
  caller's pane and session without Claude passing it.
- The single-writer rule in `CLAUDE.md` applies: a CLI subcommand writing
  `.forestui-config.json` while the App runs is a second writer. The CLI
  should hand the plan to the running App (a file in a spool dir the App
  folds, or a socket the App listens on), not save state itself.
- Direction: session → forestui only. Anything coming back is the command's
  stdout within the same turn.

### 2. Plugin skills and slash commands

A skill (`skills/<name>/SKILL.md` in the plugin, invoked as
`/forestui-integration:<name>` or by Claude on its own) is instructions, not a
transport. It is how the coordinator learns the plan format and which command
to run.

- Arguments: `$ARGUMENTS`, `$0`/`$1`, named `arguments:` frontmatter
  ([skills](https://code.claude.com/docs/en/skills)).
- `` !`command` `` runs a shell command *before* the content reaches Claude
  and splices its output in — e.g. `` !`forestui queue show --json` `` to load
  the current queue and a JSON Schema into the coordinator's prompt. A
  non-zero exit aborts the invocation; the command is checked against
  permission rules ([skills, dynamic context injection](https://code.claude.com/docs/en/skills)).
- `allowed-tools` pre-approves e.g. `Bash(forestui queue *)` for the turn
  that invokes the skill; `disable-model-invocation: true` keeps side-effect
  workflows user-triggered.
- Skills may declare `hooks` in frontmatter, registered when invoked and live
  for the rest of the session — a way to arm coordinator-only hooks without
  putting them on every session.
- Direction: pull from forestui at invocation; no push.

### 3. Hooks

The full event list and outputs are in the
[hooks reference](https://code.claude.com/docs/en/hooks). Relevant parts:

**Session → forestui.** Every hook receives JSON on stdin with `session_id`,
`transcript_path`, `cwd`, `hook_event_name` plus event fields. `Stop` and
`SubagentStop` receive `last_assistant_message` (the final response text —
the docs recommend it over parsing the transcript). That is how forestui
could capture a worker's or the coordinator's final answer without Claude
calling anything. Hook types: `command`, `http` (POSTs the JSON to a URL;
2xx JSON is parsed with the same schema; CL 2.1.63),
`mcp_tool`, `prompt`, `agent`.

**forestui → session, synchronously.** Only on the session's own events:

- `SessionStart`: `additionalContext` (start of conversation), plus
  `initialUserMessage` (first user message, `-p` only), `sessionTitle`,
  `watchPaths`, `reloadSkills`.
- `UserPromptSubmit`: `additionalContext` added as a system reminder next to
  the prompt; cannot replace the prompt. Default timeout 30 s.
- `Stop` / `SubagentStop`: `decision: "block"` + `reason` sends Claude back
  to work; `hookSpecificOutput.additionalContext` continues without an error
  framing. Capped at 8 consecutive continuations
  (`CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`). `Stop` input also has
  `background_tasks` and `session_crons` to tell "done" from "waiting".
- `PostToolUse`, `PostToolBatch`, `SubagentStart`, `PostToolUseFailure`:
  `additionalContext`.
- Every text field (`additionalContext`, `systemMessage`,
  `initialUserMessage`, plain stdout) is capped at **10,000 characters**;
  more is spilled to a file with a 2,000-char preview Claude is not told to
  read.

So a coordinator's `Stop` hook could ask forestui "anything for me?" and, if
workers have finished, block the stop with their results — but only while the
coordinator is finishing a turn. It cannot reach an idle session.

**forestui → session, asynchronously.** `async: true` runs a command hook in
the background; its `additionalContext`/`systemMessage` arrive "on the next
conversation turn. If the session is idle, the response waits until the next
user interaction." The exception: **`asyncRewake: true`** — "an `asyncRewake`
hook that exits with code 2 wakes Claude immediately even when the session is
idle", delivering stderr (or stdout) as a system reminder
([hooks, Run hooks in the background](https://code.claude.com/docs/en/hooks)).
`timeout` *is* enforced on `asyncRewake` hooks (default 600 s, settable), so
a long wait means a long timeout. A coordinator could arm a `SessionStart`
`asyncRewake` hook that blocks on "next queue event" and exits 2 with it;
after waking, nothing re-arms it until the next matching event fires, so
re-arming must ride on an event that recurs (`Stop`, `UserPromptSubmit`).
It works on any version with `asyncRewake` (present by CL 2.1.287, which
fixes a bug in it) and needs no flag, but it is a hook holding a blocking
wait, which is awkward.

`FileChanged` (with `watchPaths` from `SessionStart`/`CwdChanged`) fires when
a watched file changes but "has no decision control"; its `systemMessage` is a
terminal notice only — not a way in.

### 4. MCP server shipped by the plugin

A plugin declares servers in `.mcp.json` at its root or `mcpServers` in
`plugin.json`; "When you enable a plugin, Claude Code starts its MCP servers
automatically" ([mcp](https://code.claude.com/docs/en/mcp),
[plugins reference, `mcpServers`](https://code.claude.com/docs/en/plugins-reference#mcpservers)).

- Transports: stdio, HTTP (recommended for remote), SSE (deprecated),
  WebSocket. For forestui, a stdio server (`forestui mcp`, spawned per
  session with `${CLAUDE_PLUGIN_ROOT}` / `${CLAUDE_PLUGIN_DATA}` in its env)
  is the natural fit; it then talks to the running App over a local socket.
  An HTTP server hosted by the App itself also works but needs a port and a
  URL in the plugin config.
- **Session → forestui: the best-typed option.** Tools carry a JSON Schema
  `inputSchema`, so a `submit_plan` tool makes Claude produce schema-shaped
  arguments, and the server can return precise validation errors. Tool name
  is `mcp__plugin_forestui-integration_<server>__<tool>`; that full name is
  what permission rules and `allowed-tools` must use.
- Output: warns over 10k tokens, default cap 25k (`MAX_MCP_OUTPUT_TOKENS`);
  text over 50,000 chars spills to a file unless `_meta`
  `anthropic/maxResultSizeChars` raises it. `MCP_TOOL_TIMEOUT` default ~28 h;
  idle timeout 30 min for stdio; calls past 2 min move to a background task.
- **forestui → session: no.** "The only documented way for a server to push
  messages into your session is a channel." `list_changed` only refreshes the
  tool/prompt/resource lists.
- Cost: a Rust MCP server is a new dependency surface (an MCP SDK or a
  hand-rolled JSON-RPC over stdio), versus a CLI subcommand that is free.

### 5. MCP channels (research preview)

A channel is a stdio MCP server that declares
`capabilities.experimental['claude/channel'] = {}` and emits
`notifications/claude/channel` with `{ content: string, meta?: Record<string,string> }`.
It arrives in context as `<channel source="<server>" key="value">content</channel>`;
`meta` keys must be identifiers or they are silently dropped. Events queue and
are delivered in order; several that arrive while Claude is busy are
delivered together on the next turn. There is no acknowledgement — "If the
session hasn't loaded your server as a channel ... Claude Code drops the
events silently." A reply tool makes it two-way, and
`claude/channel/permission` opts into relaying permission prompts
([channels reference](https://code.claude.com/docs/en/channels-reference)).

This is exactly the "forestui → session" primitive, with a documented
contract, **but**:

- Research preview; "the `--channels` flag syntax and protocol contract may
  change". Neither flag appears in `claude --help` (confirmed on 2.1.296).
- Only plugins on Anthropic's allowlist (or an org's `allowedChannelPlugins`)
  register with `--channels`. A forestui channel would have to run under
  `--dangerously-load-development-channels plugin:forestui-integration@<mkt>`,
  which shows a full-screen warning dialog at every launch and is ignored in
  `-p` mode ([channels](https://code.claude.com/docs/en/channels#research-preview)).
- Requires claude.ai or Console auth; not on Bedrock/Vertex/Foundry;
  Team/Enterprise orgs must set `channelsEnabled`.
- Opt-in per session at launch: forestui would add the flag to the
  coordinator's command line (as it does `--remote-control`), never to
  workers.
- With `--channels` active, `AskUserQuestion` and plan-mode tools were
  disabled (CL 2.1.83); plan-mode tools came back for interactive sessions
  in CL 2.1.126 — check before relying on plan mode in the coordinator.
- A v2 MCP runtime caveat: a channel server that negotiates MCP protocol
  revision 2026-07-28 is not registered; set `MCP_PROTOCOL_NEGOTIATION=legacy`
  or don't support that revision ([mcp](https://code.claude.com/docs/en/mcp)).
- Introduced CL 2.1.80; permission relay CL 2.1.81; console-auth support
  CL 2.1.128.

### 6. Cross-session messaging inbox socket — the surprise

Since **v2.1.224** (macOS/Linux; 2.1.234 native Windows) every session
"binds an inbox socket ... where other sessions on the machine deliver
messages", on by default
([cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging);
CL 2.1.224). The docs explicitly cover "when you want a script or hook to post
into a session":

- The socket path is exported to hooks and Bash commands as
  `CLAUDE_CODE_MESSAGING_SOCKET` — "before any hook runs, including
  `SessionStart`" — and shown in `/status` as `Peer address uds:<path>`. A
  per-session token is exported as `CLAUDE_CODE_MESSAGING_TOKEN` (2.1.228+)
  ([env vars](https://code.claude.com/docs/en/env-vars)).
- A poster may send `{"type":"auth","token":"<token>"}` as its first line
  (optional on Linux/macOS). The connection must send a complete line within
  30 s (CL 2.1.243).
- **Delivery wakes the session**: "When the receiving session is idle, Claude
  Code starts a new turn with the message." During a turn it is read between
  tool calls. It also fires `UserPromptSubmit` hooks.
- Inbound controls (`crossSessionInbound`: `accept` / `hold` / `refuse`).
  With no value set, a *prompting* session (manual, auto, acceptEdits,
  dontAsk) delivers; a session that bypasses permissions holds the message
  for approval. A message from an unverified poster "asserts no permission
  class", so it is delivered to a normal coordinator and held for a
  `--dangerously-skip-permissions` one. forestui can set
  `crossSessionInbound: accept` on the coordinator via `--settings`.
- Limits: plain text only, ~1M chars per message, burst-limited, at most 50
  queued; the receiving Claude is told the text came from another session,
  not the user, so it carries no consent (cannot approve prompts or change
  config). That is the right trust level for "worker 3 finished".
- Not bound in `--bare` sessions, nor in `--restricted` ones (CL 2.1.290).

**What is missing:** the docs specify only the auth line, not the message
line's JSON shape (sender name, body, reply address). That shape is what
`SendMessage` writes and is internal. Two ways forward, both for the
design ticket rather than this one:

1. Reverse-engineer and pin the line format on an isolated, throwaway session
   (risk: undocumented, may change between releases — forestui updates
   itself on launch but Claude Code does too).
2. Let Claude do the sending: have the *worker's* Claude call `SendMessage`
   to the coordinator by name (forestui names sessions after windows, and
   `SendMessage` addresses sessions by `/rename` name), or have the
   coordinator subscribe with `SendMessage`'s `notify_when_idle` to each
   worker (2.1.236+; one notice per subscription, 12 h expiry, no tokens spent
   in the watched session). This uses only documented behaviour, but it is
   Claude-to-Claude, not forestui-to-Claude.

forestui's heartbeat hook can record `CLAUDE_CODE_MESSAGING_SOCKET` today with
one more field, which makes option 1 a small change once the format is known.

### 7. Monitor tool and plugin monitors

The `Monitor` tool "runs a command in the background and feeds each output
line back to Claude" and "Claude interjects when an event arrives"; it can
also open a WebSocket and treat each message as an event. Every watch has a
deadline — 5 min default, **30 min max** — after which Claude gets one notice
and may restart it ([tools reference, Monitor](https://code.claude.com/docs/en/tools-reference#monitor-tool);
CL 2.1.98). Not on Bedrock/Vertex/Foundry or with telemetry disabled.

A plugin can declare `experimental.monitors` (`name`, `command`,
`description`, `when: "always" | "on-skill-invoke:<skill>"`) that Claude Code
runs as a persistent background process; interactive sessions only
([plugins reference, monitors](https://code.claude.com/docs/en/plugins-reference#monitors);
CL 2.1.105). A `forestui watch --session $X` monitor printing one line per
queue event would push into the coordinator using only plugin machinery, no
flags. Caveats: the manifest key is `experimental`, a monitor armed
`on-skill-invoke:coordinate` would scope it to the coordinator, and the docs
reviewed do not say whether plugin monitors share the Monitor tool's 30-min
deadline — verify before relying on it.

### 8. Headless `claude -p` with structured output

`claude -p ... --output-format json --json-schema '<schema>'` returns JSON
with `result`, `session_id`, `total_cost_usd`, and the schema-conforming value
in **`structured_output`**. An invalid schema is a hard error since 2.1.205
([headless](https://code.claude.com/docs/en/headless)). `--resume <id>` continues
the same conversation in a new run; `--input-format stream-json` keeps one
process open for streamed user messages. Workers could run this way and hand
forestui a typed result for free.

For the *coordinator* it changes the product: the planning conversation
would no longer live in a tmux window the user can talk to, and forestui
would have to host the dialogue. `-p` sessions also run project hooks and
`.mcp.json` servers without a trust prompt, ignore the dev-channel flag, and
can't spawn agent-team teammates.

### 9. Background sessions, agent teams, Remote Control

- **Background sessions** (`claude --bg`, supervisor daemon):
  `claude agents --json [--all]` gives `id`, `state`
  (`working`/`blocked`/`done`/`failed`/`stopped`), `status`
  (`busy`/`waiting`/`idle`), `sessionId`, `name`, `cwd`. There is **no CLI to
  send a message into a running background session**; `claude --resume <id>
  --bg "<prompt>"` continues it as a new run (2.1.257+)
  ([agent view](https://code.claude.com/docs/en/agent-view)). A competing
  host model to forestui's tmux windows rather than a channel.
- **Agent teams** (`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`): mailbox files
  `~/.claude/teams/{team}/inboxes/{agent}.json` and tasks under
  `~/.claude/tasks/{team}/` are runtime state ("don't edit it by hand");
  one team per session, no resume of in-process teammates, teammates can't
  run in `-p` ([agent teams](https://code.claude.com/docs/en/agent-teams)).
  Overlaps with forestui's queue but is not an integration surface.
- **Remote Control** lets a *person* drive a session from claude.ai or the
  phone app; it is not a local IPC. It does extend cross-session messaging to
  other machines.

## Implications for the boundary decision (#94)

1. **Plan into forestui**: CLI subcommand first (zero Claude Code
   dependency, works on every version), described by a plugin skill that
   embeds the schema via `` !`forestui ... --schema` ``. An MCP tool is the
   upgrade if schema-typed arguments are worth a Rust MCP server.
2. **Events into the coordinator**, in order of preference:
   inbox socket (stable, wakes idle, but message format must be pinned) →
   plugin monitor (documented, plugin-only, `experimental`) →
   `asyncRewake` hook (documented, clunky) →
   channel (best contract, worst gating) →
   typing into the pane with `send-keys` (always works, already used as a
   fallback for fish/nushell; races with the user's own typing).
3. **Results out of workers**: `Stop` hook input carries
   `last_assistant_message`; `heartbeat.sh`/`state.sh` already run there, so
   capturing a worker's final summary is an extension of what ships.
4. Record `CLAUDE_CODE_MESSAGING_SOCKET` in the heartbeat file regardless —
   cheap, and every option above that targets a specific session needs it or
   the pane id forestui already records.

## Open questions

- Exact line format the inbox socket accepts for a message from a
  non-Claude poster (not documented; needs an isolated experiment).
- Whether plugin `experimental.monitors` processes have a deadline.
- Whether a `--dangerously-load-development-channels` warning dialog can be
  pre-accepted, which decides whether channels are usable at all for a
  forestui-launched coordinator.
