#!/usr/bin/env bash
#
# tu-sweep.sh — drive forestui through the UC-53..UC-70 hotkey sweep and capture
# one PNG plus one text frame per case.
#
# The text frames are the comparable artifact: they diff cleanly, so the same
# sweep run against two builds shows exactly which screens differ. The PNGs are
# for human eyeballing (colour, focus highlight) and are not committed.
#
#   usage: scripts/tu-sweep.sh <label> <forestui-command...>
#
#   examples:
#     scripts/tu-sweep.sh rust    ./target/release/forestui
#     scripts/tu-sweep.sh python  /path/to/.venv/bin/forestui
#
# Output:
#   doc/rust-rewrite/baseline/<label>/UC-NN-*.txt   committed, diffable
#   doc/rust-rewrite/screenshots/<label>/UC-NN-*.png gitignored
#
# The sweep builds its own throwaway forest, HOME and tmux server, and stubs
# vim/mc/claude so the window-creating hotkeys produce windows that stay alive
# long enough to be named. It never touches the caller's ~/forest or tmux.
#
# SHELL is pinned rather than inherited, because it decides both what tmux runs
# in every window forestui opens and which startup-file hook a Claude window is
# launched through — and the fixture HOME is empty. On a machine whose shell is
# zsh, each of those windows was taken over by zsh-newuser-install ("you have
# no zsh startup files") and the command never ran. The windows still existed
# and were still named, so every frame assertion passed while the stubs had
# never once been executed.
#
# /bin/sh specifically, because it reads next to no startup files and its
# prompt is a bare `$`: bash would pull in the system-wide /etc/bash.bashrc,
# whose banner differs per distribution and would land in committed frames.

set -uo pipefail

LABEL="${1:?usage: tu-sweep.sh <label> <forestui-command...>}"
shift
FUI_CMD=("$@")
[ "${#FUI_CMD[@]}" -gt 0 ] || { echo "no forestui command given" >&2; exit 2; }

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRAMES="$REPO/doc/rust-rewrite/baseline/$LABEL"
SHOTS="$REPO/doc/rust-rewrite/screenshots/$LABEL"
SESS="sweep-$LABEL"

# tmux refuses long socket paths, so the server dir has to be short.
TMUXDIR="$(mktemp -d /tmp/fuisweepXXXX)"
# A fixed template, not the platform default: the LOCATION box hugs the path,
# so the path's *length* is part of every frame that shows it. macOS's default
# mktemp dir lands under /var/folders/<xx>/<random>/T with a variable-length
# prefix, which is why committed baselines did not reproduce across machines.
ROOT="$(mktemp -d /tmp/fui-sweep.XXXXXX)"

# `tu kill` ends a tmux *client*. The server that client started is a daemon
# and outlives it, still running the forestui under test, and once TMUXDIR is
# deleted nothing can reach its socket again: every sweep used to leave one
# behind. So the server is stopped first, through tu like every other tmux
# call this harness makes. -S names the throwaway socket outright, and TMUX is
# dropped so the session running the sweep, often itself inside the user's
# tmux, is never the server reached.
reap_tmux_server() {
  local socket="$TMUXDIR/tmux-$(id -u)/default"
  [ -S "$socket" ] || return 0
  tu run --name "$SESS-reap" --env TMUX_TMPDIR="$TMUXDIR" \
    -- env -u TMUX tmux -S "$socket" kill-server >/dev/null 2>&1
  local _
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    tu status --name "$SESS-reap" 2>/dev/null | grep -q '"alive":false' && break
    sleep 0.2
  done
  tu kill --name "$SESS-reap" >/dev/null 2>&1
}

cleanup() {
  # Every client this sweep may have started, should it stop part-way.
  tu kill --name "$SESS" >/dev/null 2>&1
  tu kill --name "$SESS-b" >/dev/null 2>&1
  tu kill --name "$SESS-c" >/dev/null 2>&1
  reap_tmux_server
  rm -rf "$TMUXDIR" "$ROOT"
}
trap cleanup EXIT

rm -rf "$FRAMES" "$SHOTS"
mkdir -p "$FRAMES" "$SHOTS" "$ROOT"/{home/.config/forestui,src,forest,bin}

# ---------------------------------------------------------------- fixture

for stub in vim mc claude; do
  printf '#!/bin/sh\necho "%s stub $*"\nexec sleep 600\n' "$stub" > "$ROOT/bin/$stub"
  chmod +x "$ROOT/bin/$stub"
done

# The Claude stub also records the birth stamp of the window it was started in.
# That stamp is what tells the title-sync hook a window is forestui's, and it is
# invisible in a screenshot — a build that stopped writing it would render an
# identical frame. Read with show-options rather than a #{@...} format: a
# server-scoped value of the same name shadows the window's one in formats.
#
# forestui also runs `claude --help` once at startup, to learn whether this
# Claude knows `--remote-control`. The stub answers it like a current Claude
# and exits: recording it would put the app's own window first in birth.txt,
# and sleeping would hold the probe until its timeout.
#
# A session's name suggestion is a headless `claude -p`. The stub reads the
# instruction and answers with one fixed name, after a pause long enough for
# UC-103 to see the spinner — a real model call would make the frames differ
# run to run, and cost money.
cat > "$ROOT/bin/claude" <<STUB
#!/bin/sh
if [ "\$1" = "--help" ]; then
  echo "  --remote-control [name]  Start an interactive session with Remote Control enabled"
  exit 0
fi
if [ "\$1" = "-p" ]; then
  cat > /dev/null
  sleep 2
  echo "caching-spike"
  exit 0
fi
echo "claude stub \$*"
# Once the sweep raises the flag (UC-100 onward), a fresh launch also leaves a
# transcript under its --session-id, as a real Claude would: the jump in
# UC-101 finds the running session through the worktree's session list.
# Earlier launches write none, so no compared frame grows a session card.
if [ -f "$ROOT/write-transcripts" ]; then
  sid=""; prev=""
  for arg in "\$@"; do
    [ "\$prev" = "--session-id" ] && sid="\$arg"
    prev="\$arg"
  done
  if [ -n "\$sid" ]; then
    dir="\$HOME/.claude/projects/\$(pwd -P | tr '/' '-')"
    mkdir -p "\$dir"
    printf '%s\n' '{"type":"user","timestamp":"2026-08-09T10:00:00Z","message":{"content":"start on the feature"}}' \
      > "\$dir/\$sid.jsonl"
  fi
fi
printf '%s\t%s\n' \
  "\$(tmux display-message -p '#{window_name}' 2>/dev/null)" \
  "\$(tmux show-options -wqv @claude_birth_name 2>/dev/null)" >> "$ROOT/birth.txt"
exec sleep 600
STUB
chmod +x "$ROOT/bin/claude"

# A `gh` that answers the four calls both builds make. Without it Start work's
# issue lists, which the repository pane prefetches, would only ever show their
# empty state, and the finished check would have nothing to mark.
cat > "$ROOT/bin/gh" <<'GH'
#!/bin/sh
case "$1 $2" in
  "auth status") echo "Logged in to github.com as sweep-user"; exit 0 ;;
  "api user")    echo "sweep-user"; exit 0 ;;
  "repo view")   echo '{"owner":{"login":"sweep-user"},"name":"alpha"}'; exit 0 ;;
  "issue list")
    # The --assignee query returns the rows; the --author one returns none
    # outside beta, so the de-duplication path is exercised too.
    case "$*" in
      *--assignee*)
        cat <<'JSON'
[{"number":326,"title":"Fansly conversation discovery has no backoff","state":"OPEN",
  "url":"https://example.invalid/326","createdAt":"2026-08-01T10:00:00Z",
  "updatedAt":"2026-08-02T10:00:00Z","author":{"login":"sweep-user"},
  "assignees":[{"login":"sweep-user"}],"labels":[{"name":"bug","color":"ff0000"}]},
 {"number":298,"title":"Proactive metrics: coalesced FIRE rows overcount","state":"OPEN",
  "url":"https://example.invalid/298","createdAt":"2026-07-20T10:00:00Z",
  "updatedAt":"2026-07-21T10:00:00Z","author":{"login":"sweep-user"},
  "assignees":[],"labels":[{"name":"metrics","color":"00ff00"},{"name":"p2","color":"0000ff"}]}]
JSON
        ;;
      # beta alone also has a feature issue, which Start work lists with the
      # rest and starts as a feature (UC-100). Only Start work shows it, so the frozen Python frames are
      # unaffected.
      *--author*)
        case "$PWD" in
          */beta)
            cat <<'JSON'
[{"number":77,"title":"Dark mode for the dashboard","state":"OPEN",
  "url":"https://example.invalid/77","createdAt":"2026-07-10T10:00:00Z",
  "updatedAt":"2026-07-11T10:00:00Z","author":{"login":"sweep-user"},
  "assignees":[],"labels":[{"name":"enhancement","color":"a2eeef"}]}]
JSON
            ;;
          *) echo "[]" ;;
        esac
        ;;
      # No filter is the `all` scope, which only Start work's "Not just
      # mine" asks for (UC-99): every open issue in beta, which adds #412,
      # neither assigned to nor opened by the sweep user.
      *)
        case "$PWD" in
          */beta)
            cat <<'JSON'
[{"number":326,"title":"Fansly conversation discovery has no backoff","state":"OPEN",
  "url":"https://example.invalid/326","createdAt":"2026-08-01T10:00:00Z",
  "updatedAt":"2026-08-02T10:00:00Z","author":{"login":"sweep-user"},
  "assignees":[{"login":"sweep-user"}],"labels":[{"name":"bug","color":"ff0000"}]},
 {"number":298,"title":"Proactive metrics: coalesced FIRE rows overcount","state":"OPEN",
  "url":"https://example.invalid/298","createdAt":"2026-07-20T10:00:00Z",
  "updatedAt":"2026-07-21T10:00:00Z","author":{"login":"sweep-user"},
  "assignees":[],"labels":[{"name":"metrics","color":"00ff00"},{"name":"p2","color":"0000ff"}]},
 {"number":77,"title":"Dark mode for the dashboard","state":"OPEN",
  "url":"https://example.invalid/77","createdAt":"2026-07-10T10:00:00Z",
  "updatedAt":"2026-07-11T10:00:00Z","author":{"login":"sweep-user"},
  "assignees":[],"labels":[{"name":"enhancement","color":"a2eeef"}]},
 {"number":412,"title":"Someone else's flaky export","state":"OPEN",
  "url":"https://example.invalid/412","createdAt":"2026-07-05T10:00:00Z",
  "updatedAt":"2026-07-06T10:00:00Z","author":{"login":"someone-else"},
  "assignees":[],"labels":[]}]
JSON
            ;;
          *) echo "[]" ;;
        esac
        ;;
    esac
    exit 0 ;;
  # beta alone has PRs, for Start work's pr-review mode (UC-104): one
  # awaiting the sweep user's review, one they opened themselves.
  "pr list")
    case "$PWD:$*" in
      */beta:*review-requested*)
        cat <<'JSON'
[{"number":45,"title":"Retry uploads with backoff","author":{"login":"octo-dev"},
  "isDraft":false,"headRefName":"fix/retry-backoff","createdAt":"2026-08-03T10:00:00Z"}]
JSON
        ;;
      */beta:*--author*)
        cat <<'JSON'
[{"number":46,"title":"Cache the session index","author":{"login":"sweep-user"},
  "isDraft":true,"headRefName":"feat/session-index-cache","createdAt":"2026-08-04T10:00:00Z"}]
JSON
        ;;
      *) echo "[]" ;;
    esac
    exit 0 ;;
  # The finished check (UC-105) asks each linked item's state. Everything
  # is open until the sweep raises the flag that merges #45, so no frame
  # before UC-105 shows a finished worktree.
  "issue view") echo '{"state":"OPEN"}'; exit 0 ;;
  "pr view")
    if [ "$3" = 45 ] && [ -f "$HOME/../pr-45-merged" ]; then
      echo '{"state":"MERGED"}'
    else
      echo '{"state":"OPEN"}'
    fi
    exit 0 ;;
  # What `gh pr checkout` leaves behind: the PR's head branch, checked out
  # in the directory it ran in.
  "pr checkout")
    case "$3" in
      45) git checkout -q -b fix/retry-backoff ;;
      46) git checkout -q -b feat/session-index-cache ;;
      *) exit 1 ;;
    esac
    exit $? ;;
esac
exit 1
GH
chmod +x "$ROOT/bin/gh"

cat > "$ROOT/home/.gitconfig" <<'EOF'
[user]
  name = Test
  email = test@example.com
[init]
  defaultBranch = main
EOF

for name in alpha beta gamma; do
  d="$ROOT/src/$name"
  mkdir -p "$d"
  git -c init.defaultBranch=main init -q "$d"
  echo "# $name" > "$d/README.md"
  git -C "$d" add -A
  git -C "$d" -c user.name=Test -c user.email=t@e.com commit -qm "init $name"
  git -C "$d" branch -q feat/other
  git -C "$d" branch -q release-2
done
git -C "$ROOT/src/alpha" worktree add -q -b feat/wt-a "$ROOT/forest/alpha/wt-a"
REF="$(git -C "$ROOT/src/alpha" rev-parse --short HEAD)"

# gamma is deliberately NOT in the config, and owns a worktree OUTSIDE the
# forest directory — that is what the add-time reconcile scan (python: the
# "Import existing worktrees" checkbox) has to discover.
git -C "$ROOT/src/gamma" worktree add -q -b feat/imported "$ROOT/outside/gamma-imported"

# Claude sessions, so the RECENT SESSIONS cards actually render. Without these
# both builds only ever show "No sessions found", and that whole section goes
# uncompared.
seed_sessions() {
  local target="$1" count="$2"
  # The app canonicalises before building the folder name, and on macOS that
  # turns /var/... into /private/var/..., so seed against the resolved path.
  local resolved folder
  resolved="$(cd "$target" && pwd -P)"
  folder="$(printf '%s' "$resolved" | tr '/' '-')"
  local dir="$ROOT/home/.claude/projects/$folder"
  mkdir -p "$dir"
  for i in $(seq 1 "$count"); do
    cat > "$dir/session-$i.jsonl" <<SESSION
{"type":"user","timestamp":"2026-08-0${i}T10:00:00Z","message":{"content":"session $i: refactor the detail pane so the cards line up"}}
{"type":"assistant","timestamp":"2026-08-0${i}T10:00:05Z","message":{"content":[{"type":"text","text":"looking at the card layout now"}]}}
{"type":"user","timestamp":"2026-08-0${i}T10:01:00Z","message":{"content":"and then check it against the python build"}}
SESSION
  done
}
# Repository only. Seeding the worktree too pushes Textual's RENAME inputs off
# a 70-row screen, and the rename flow then clicks on nothing.
seed_sessions "$ROOT/src/alpha" 3

# Written the way an older build left it, retired `default_terminal` and
# `theme` keys included: they must not stop the file loading — the custom
# button below would vanish from every frame if they did.
cat > "$ROOT/home/.config/forestui/settings.json" <<'EOF'
{"default_editor":"vim","default_terminal":"","branch_prefix":"feat/","theme":"system",
 "custom_buttons":[{"label":"Opus","prefix":"opus","command":"claude --model opus"}]}
EOF

cat > "$ROOT/forest/.forestui-config.json" <<EOF
{"repositories":[
 {"id":"11111111-1111-4111-8111-111111111111","name":"alpha","source_path":"$ROOT/src/alpha",
  "worktrees":[{"id":"22222222-2222-4222-8222-222222222222","name":"wt-a","branch":"feat/wt-a",
   "path":"$ROOT/forest/alpha/wt-a","is_archived":false,"sort_order":0,
   "last_modified":"2026-08-14T10:00:00Z","base_branch":"main","created_from_ref":"$REF"}]},
 {"id":"33333333-3333-4333-8333-333333333333","name":"beta","source_path":"$ROOT/src/beta","worktrees":[]}]}
EOF

# ---------------------------------------------------------------- helpers

press() { tu press --name "$SESS" "$@" >/dev/null 2>&1; }
# `--` matters: text starting with a dash is otherwise parsed as a flag,
# which silently types nothing at all.
type_()  { tu type  --name "$SESS" -- "$1" >/dev/null 2>&1; }
focus_app() { press Ctrl+B 0; sleep 0.7; }

# Wait until the screen shows <regex>, rather than sleeping a fixed amount.
# Textual repaints noticeably slower than ratatui, so a fixed sleep captures the
# two builds at different points in the same interaction.
# await <regex> [timeout-ms] — block until the screen shows something.
#
# A timeout is reported rather than swallowed. It used to return quietly and
# the caller captured anyway, so a case that never reached its state still
# wrote a frame — and that frame diffed clean against a baseline captured the
# same way, which is a green test for a screen nobody ever saw.
await() {
  if ! tu wait --name "$SESS" --text "$1" --timeout "${2:-12000}" >/dev/null 2>&1; then
    printf '  WAIT-TIMEOUT  %s\n' "$1" >&2
    WAIT_TIMEOUTS=$((WAIT_TIMEOUTS + 1))
  fi
}
WAIT_TIMEOUTS=0

# capture <UC-ID> <slug> [session] — one PNG plus one normalised text frame.
#
# Volatile cells (relative times, commit hashes, temp paths, the tmux clock and
# the session's own dev-mode timestamp) are masked so two runs of the same build
# diff clean and two builds differ only where behaviour differs.
capture() {
  local id="$1" slug="$2" sess="${3:-$SESS}"
  tu screenshot --name "$sess" --png -o "$SHOTS/$id-$slug.png" >/dev/null 2>&1
  tu screenshot --name "$sess" 2>/dev/null \
    | ROOT="$ROOT" HOST="$(hostname)" HOST_SHORT="$(hostname -s)" WHO="$(id -un)" \
      FUI_BIN="${FUI_CMD[0]}" python3 -c '
import json, os, re, sys
root = os.environ["ROOT"]
text = json.load(sys.stdin)["content"]
text = text.replace(root, "<ROOT>")
# The command line is echoed in frames that show a shell. SWEEP.md tells you
# to pass an absolute binary path when the tu cwd is not the repo, which would
# otherwise put a local path — and the branch worktree it was built in — into a
# committed frame, making the baseline depend on who ran the sweep and from
# where.
text = text.replace(os.environ["FUI_BIN"], "<forestui>")
# Fresh Claude sessions run under a pre-minted `--session-id <uuid>`, freshly
# random per launch — before the 7-hex rule below eats fragments of it.
text = re.sub(
    r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
    "<uuid>",
    text,
)
# A dev instance names its window "forestui-dev-HHMM" — masked before the pid
# rule below, whose pattern would otherwise swallow the timestamp as a pid.
# (This whole program lives in a single-quoted shell string: no apostrophes.)
text = re.sub(r"dev-\d{4}", "dev-<hhmm>", text)
# Grouped sessions are named "forestui-<forest>-<pid>". Before the sha rule:
# a 7-digit pid is also seven hex characters, and whichever rule ran first
# used to decide whether the frame said <pid> or <sha> — a coin flip per run
# that kept dirtying UC-82.
text = re.sub(r"(forestui-[a-z0-9-]+?)-\d+\b", r"\1-<pid>", text)
text = re.sub(r"\b[0-9a-f]{7}\b", "<sha>", text)
# Bare or parenthesised: session cards print "N days ago"
# with no parentheses, and an unmasked one rots the committed frames daily.
text = re.sub(r"\b(\d+|an?) (seconds?|minutes?|hours?|days?|months?|years?) ago\b", "<rel>", text)
text = re.sub(r"\ba moment ago\b", "<rel>", text)
text = re.sub(r"\(now\)", "(<rel>)", text)
text = re.sub(r"\d{2}:\d{2} \d{2}-\w{3}-\d{2}", "<clock>", text)
# tmux prints the machine name in its status bar and the shell prompt inside a
# terminal window prints `user@host`. These frames are committed and travel to a
# public PR, so neither the machine nor the account reaches the baseline. Longest
# first: the short name is a prefix of the FQDN.
text = text.replace(os.environ["HOST"], "<host>")
text = text.replace(os.environ["HOST_SHORT"], "<host>")
text = text.replace(os.environ["WHO"], "<user>")
text = re.sub(r"\"[0-9a-f-]{8,}\"", "\"<host>\"", text)
# The Rust build runs from source at 0.0.0 while the Python build is an
# installed release; masking the version keeps the header comparable.
text = re.sub(r"forestui v\S+", "forestui v<version>", text)
print("\n".join(line.rstrip() for line in text.splitlines()))
' > "$FRAMES/$id-$slug.txt"
  printf '  %s %s\n' "$id" "$slug"
}

# assert <UC-ID> <description> <expected> <actual>
#
# Screens prove what the user sees; these prove what actually happened on disk.
# Both builds write the same file, so the outcomes diff as easily as the frames.
ASSERTIONS="$FRAMES/ASSERTIONS.txt"
: > "$ASSERTIONS"
assert() {
  local id="$1" what="$2" expected="$3" actual="$4" verdict=FAIL
  [ "$expected" = "$actual" ] && verdict=PASS
  printf '%-4s %-9s %-46s expected=%-28s actual=%s\n' \
    "$verdict" "$id" "$what" "$expected" "$actual" >> "$ASSERTIONS"
  [ "$verdict" = PASS ] || printf '    !! %s %s: expected %s, got %s\n' \
    "$id" "$what" "$expected" "$actual" >&2
}

# ---------------------------------------------------------------- run

echo "sweeping $LABEL -> $FRAMES"

# The Python build's ensure_tmux re-execs the bare name `forestui`, so the
# command's own directory has to be on PATH or the tmux session dies instantly.
# The Rust build resolves itself via current_exe() and does not need this.
CMD_DIR="$(cd "$(dirname "${FUI_CMD[0]}")" && pwd)"

tu kill --name "$SESS" >/dev/null 2>&1
tu run --name "$SESS" --size 140x44 \
  --env TMUX_TMPDIR="$TMUXDIR" \
  --env HOME="$ROOT/home" \
  --env PATH="$CMD_DIR:$ROOT/bin:$(dirname "$(command -v tmux)"):/usr/bin:/bin:/usr/sbin:/sbin" \
  --env FORESTUI_NO_AUTO_UPDATE=1 \
  --env SHELL=/bin/sh \
  --cwd "$REPO" \
  -- env -u TMUX "${FUI_CMD[@]}" "$ROOT/forest" >/dev/null 2>&1
sleep 6

capture UC-53 boot-repository-detail

# UC-95/96: the fixture must actually exercise every section, and the capture
# must be tall enough to show it. A section that falls back to its empty state —
# or scrolls off the frame — is a section nobody is comparing. The session cards
# went unexamined that way for days. MANAGE is the pane's last section, so
# seeing it proves nothing below the sessions was cut off either.
tu resize --name "$SESS" 140x140 >/dev/null 2>&1
sleep 2.0
capture UC-96 repository-pane-full-height
tall="$FRAMES/UC-96-repository-pane-full-height.txt"

has_in_tall() { grep -qE "$1" "$tall" 2>/dev/null && echo yes || echo no; }
assert UC-96 sessions-rendered "yes" "$(has_in_tall 'msgs')"
assert UC-96 manage-rendered   "yes" "$(has_in_tall 'Remove Repository')"
assert UC-96 no-issues-section "no"  "$(has_in_tall 'GITHUB ISSUES|#326|#298')"
assert UC-96 no-empty-sections "yes" \
  "$(grep -qE 'No sessions found|No repositories' "$tall" && echo no || echo yes)"

tu resize --name "$SESS" 140x44 >/dev/null 2>&1
sleep 1.5

# UC-90: mouse reporting must be on, in any-motion mode, on both builds.
#
# This assertion used to demand the opposite of the Rust build — motion
# reporting was treated as a defect there, because every report woke the loop
# and repainted and that read as flicker. But `?1003h` is the only way a
# terminal reports a bare pointer move, so refusing it makes hover impossible
# rather than merely unstyled, and the stylesheet this port follows has 25
# `:hover` rules. The repaint is now gated on the hovered target *changing*, so
# motion is cheap and both builds legitimately want it on.
mouse_mode="$(tu mouse state --name "$SESS" 2>/dev/null | python3 -c "
import json,sys
try:
    s=json.load(sys.stdin)
    print(('on' if s.get('enabled') else 'off') + ':' + str(s.get('mode')))
except Exception:
    print('unknown')")"
capture UC-90 mouse-reporting-mode
assert UC-90 mouse-enabled "on" "${mouse_mode%%:*}"
assert UC-90 any-motion-for-hover "yes" \
  "$([ "${mouse_mode##*:}" = "AnyMotion" ] && echo yes || echo no)"

# The Textual tree needs one extra Down: the first press only highlights the
# root before selection follows the cursor.
press Down; sleep 1.2
if [ "$LABEL" = "python" ]; then press Down; sleep 1.2; fi
capture UC-54 worktree-detail

for spec in "a:UC-55:modal-add-repository:Repository Path" \
            "S:UC-57:modal-settings:BRANCH PREFIX" \
            "d:UC-58:modal-confirm-delete:Permanently delete"; do
  IFS=: read -r key id slug expect <<< "$spec"
  focus_app; press "$key"
  await "$expect"; sleep 0.8
  capture "$id" "$slug"
  focus_app; press Escape; sleep 1.0
done

# The plain worktree form is Start work's last mode. Three Downs reach it
# whether or not the GitHub probe has greyed out the modes before it, and Esc
# steps back to the mode list before a second one closes the flow.
focus_app; press s
await "Work Mode"
press Down; press Down; press Down; press Enter
await "Worktree Name"; sleep 0.8
capture UC-56 modal-add-worktree
focus_app; press Escape
await "Work Mode"
press Escape; sleep 1.0

for spec in "e:UC-60:window-editor:edit:alpha" \
            "t:UC-61:window-terminal:term:alpha" \
            "o:UC-62:window-files:files:alpha" \
            "n:UC-63:window-claude:claude:alpha" \
            "y:UC-64:window-claude-yolo:yolo:alpha"; do
  IFS=: read -r key id slug w1 w2 <<< "$spec"
  focus_app; press "$key"
  await "$w1:$w2"
  sleep 1.0
  capture "$id" "$slug"
done

# UC-97: a Claude window forestui opens carries a birth stamp equal to its own
# name. The title-sync hook treats an unstamped window as somebody else's and
# leaves it alone, so losing the stamp disables the feature silently while every
# frame stays identical. This also proves the stubs run at all — the assertion
# is written by the stub itself.
birth_line="$(head -1 "$ROOT/birth.txt" 2>/dev/null)"
assert UC-97 claude-stub-ran "yes" "$([ -n "$birth_line" ] && echo yes || echo no)"
assert UC-97 window-stamped-with-its-name "${birth_line%%	*}" "${birth_line##*	}"

# UC-98: the window outlives Claude. Claude is a job of the window's own shell
# rather than the window's command, and that is what keeps Ctrl-C — or Claude's
# own Ctrl-Z suspend, which no stub can imitate — from leaving a pane with
# nothing left in it to type at. Going back to a window whose command *is*
# Claude renders frames identical to these, so the proof has to be a shell
# answering a question after the command in front of it is gone.
#
# SHELL is /bin/sh here, so this drives the POSIX `ENV` hook specifically. The
# zsh and bash hooks are covered by unit tests on the generated files and were
# driven by hand; a sweep can only ever exercise the shell it pins.
#
# The stub UC-64 started is still in the foreground of the window it opened.
press Ctrl+C; sleep 1.0
type_ 'echo alive-$((6*7))'; press Enter
await 'alive-42'
alive="$(tu screenshot --name "$SESS" 2>/dev/null | python3 -c '
import json, sys
print("yes" if "alive-42" in json.load(sys.stdin)["content"] else "no")')"
assert UC-98 shell-outlives-claude "yes" "$alive"

# Editor reuses its window; terminal always opens a new, uniquified one.
focus_app; press e
# Nothing new appears — that is the whole point of the case — so there is no
# screen condition to wait on. `await "edit:alpha"` looked like one but matched
# the status bar UC-60 already left behind. The extra settle also carries the
# UC-60 editor toast safely past its 5 s expiry before UC-67 captures.
sleep 2.0
capture UC-65 window-editor-reused
focus_app; press t
await "term:alpha:wt-a:2"; sleep 1.0
capture UC-66 window-terminal-uniquified

focus_app; press A; sleep 1.2
capture UC-67 archived-section-toggle
focus_app; press h
# The MANAGE section's box is below the fold at 140x44; what archiving visibly
# changes is the sidebar, where the worktree moves into the Archived section.
# The settle also outlives the UC-66 terminal toast, keeping the frame clean.
await 'wt-a \(alpha\)'; sleep 2.5
capture UC-68 worktree-archived
focus_app; press h
await '└─ wt-a'; sleep 0.8
capture UC-69 worktree-unarchived

focus_app; press r; sleep 1.5
capture UC-70 refresh

# ------------------------------------------------- flows (UC-78 .. UC-82)
#
# Multi-step flows. Unlike the single-key sweep these also assert on disk, in
# ASSERTIONS.txt, because the interesting part of a rename or an import is what
# it did to the worktree and the config — not only what the screen said.
#
# The two builds reach the same controls differently: the Rust detail pane is a
# keyboard focus ring, the Textual one is a scrollable pane of mouse-first
# widgets (ratatui here ignores mouse entirely). Each flow therefore branches on
# $LABEL for the *interaction* while asserting the same *outcome*.

cfg() { python3 -c "
import json,sys
d=json.load(open('$ROOT/forest/.forestui-config.json'))
print(eval(sys.argv[1], {'d': d}))" "$1" 2>/dev/null || echo "<unreadable>"; }

# A taller terminal so the RENAME section is on screen in both builds; the
# Textual layout needs roughly 60 rows to reach it without scrolling.
tu resize --name "$SESS" 140x70 >/dev/null 2>&1; sleep 1.5
focus_app

# --- UC-78 / UC-79: rename the worktree, then its branch ---------------------
#
# Walk to the top first. Earlier phases moved the cursor, and a relative Down
# from wherever it happened to be landed on the wrong row — which silently made
# this flow rename nothing at all.
for _ in 1 2 3 4 5; do press Up; sleep 0.2; done
sleep 1.0
press Down; sleep 1.5
await "WORKTREE"
capture UC-78 rename-before

if [ "$LABEL" = "rust" ]; then
  # Click the field rather than counting Tab/Down steps: the focus ring's length
  # depends on custom buttons and sessions, and after an earlier action focus may
  # already be in the detail pane, so a blind Tab toggles the wrong way.
  tu mouse click --name "$SESS" --on-text "Worktree name" >/dev/null 2>&1
  sleep 0.8; press End; type_ "-renamed"; sleep 0.8
else
  # Textual: click into the pre-filled Input, then append. The double border
  # (`│  │`) is what distinguishes the input box from the label of the same name
  # elsewhere on screen, so target that rather than an occurrence count.
  tu mouse click --name "$SESS" --on-regex "│  │  wt-a" >/dev/null 2>&1
  sleep 0.8; press End; type_ "-renamed"; sleep 0.8
fi
capture UC-78 rename-typed
press Enter; sleep 3.5
capture UC-78 rename-after

assert UC-78 wt-name "wt-a-renamed" "$(cfg "d['repositories'][0]['worktrees'][0]['name']")"
assert UC-78 wt-dir-exists "yes" "$([ -d "$ROOT/forest/alpha/wt-a-renamed" ] && echo yes || echo no)"
assert UC-78 old-dir-gone "yes" "$([ -d "$ROOT/forest/alpha/wt-a" ] && echo no || echo yes)"
assert UC-78 git-worktree-ok "yes" \
  "$(git -C "$ROOT/forest/alpha/wt-a-renamed" rev-parse --git-dir >/dev/null 2>&1 && echo yes || echo no)"

focus_app
if [ "$LABEL" = "rust" ]; then
  tu mouse click --name "$SESS" --on-text "Branch name" >/dev/null 2>&1
  sleep 0.8; press End; type_ "-v2"; sleep 0.8
else
  tu mouse click --name "$SESS" --on-regex "│  │  feat/wt-a" >/dev/null 2>&1
  sleep 0.8; press End; type_ "-v2"; sleep 0.8
fi
press Enter; sleep 3.5
capture UC-79 rename-branch-after

assert UC-79 branch-in-config "feat/wt-a-v2" "$(cfg "d['repositories'][0]['worktrees'][0]['branch']")"
assert UC-79 branch-checked-out "feat/wt-a-v2" \
  "$(git -C "$ROOT/forest/alpha/wt-a-renamed" branch --show-current 2>/dev/null || echo '<none>')"

# --- UC-80: adding a repository adopts its existing worktrees ----------------
#
# The rust build has no "Import existing worktrees" checkbox: the reconcile
# scan adopts whatever `git worktree list` reports, at add time and forever
# after. The python build kept the one-shot opt-in checkbox.
focus_app
press a; await "Repository Path"; sleep 0.8
type_ "$ROOT/src/gamma"; sleep 1.0
if [ "$LABEL" != "rust" ]; then
  tu mouse click --name "$SESS" --on-text "Import existing worktrees" >/dev/null 2>&1
  sleep 0.6
fi
capture UC-80 import-checked
if [ "$LABEL" = "rust" ]; then
  press Enter
else
  tu mouse click --name "$SESS" --on-regex "│ Add Repository │" >/dev/null 2>&1
fi
await "gamma-imported" 15000
sleep 1
capture UC-80 import-after

assert UC-80 gamma-tracked "gamma" "$(cfg "[r['name'] for r in d['repositories']][-1]")"
assert UC-80 imported-worktree "['feat/imported']" \
  "$(cfg "[w['branch'] for w in d['repositories'][-1]['worktrees']]")"

tu resize --name "$SESS" 140x44 >/dev/null 2>&1; sleep 1

# --- UC-81: a second terminal joins as a grouped session ---------------------
#
# Same TMUX_TMPDIR means the same tmux server, so the second instance joins the
# window group instead of starting its own. Both terminals must see the same
# windows while navigating them independently.
SESS2="$SESS-b"
tu kill --name "$SESS2" >/dev/null 2>&1
tu run --name "$SESS2" --size 140x44 \
  --env TMUX_TMPDIR="$TMUXDIR" \
  --env HOME="$ROOT/home" \
  --env PATH="$CMD_DIR:$ROOT/bin:$(dirname "$(command -v tmux)"):/usr/bin:/bin:/usr/sbin:/sbin" \
  --env FORESTUI_NO_AUTO_UPDATE=1 \
  --env SHELL=/bin/sh \
  --cwd "$REPO" \
  -- env -u TMUX "${FUI_CMD[@]}" "$ROOT/forest" >/dev/null 2>&1
sleep 7
capture UC-81 grouped-terminal-b "$SESS2"
capture UC-81 grouped-terminal-a "$SESS"

winlist() {
  tu screenshot --name "$1" 2>/dev/null | python3 -c "
import json,sys,re
line=json.load(sys.stdin)['content'].splitlines()[-1]
print(len(re.findall(r'\s\d+:', line)))"
}
assert UC-81 both-see-same-window-count "$(winlist "$SESS")" "$(winlist "$SESS2")"

# Terminal B switches window; terminal A must not follow.
a_before="$(tu screenshot --name "$SESS" 2>/dev/null | python3 -c "
import json,sys,re
line=json.load(sys.stdin)['content'].splitlines()[-1]
m=re.search(r'(\d+):\S*\*', line); print(m.group(1) if m else '?')")"
tu press --name "$SESS2" Ctrl+B n >/dev/null 2>&1; sleep 2
a_after="$(tu screenshot --name "$SESS" 2>/dev/null | python3 -c "
import json,sys,re
line=json.load(sys.stdin)['content'].splitlines()[-1]
m=re.search(r'(\d+):\S*\*', line); print(m.group(1) if m else '?')")"
capture UC-81 grouped-b-switched "$SESS2"
capture UC-81 grouped-a-unmoved "$SESS"
assert UC-81 terminal-a-stayed-put "$a_before" "$a_after"

tu kill --name "$SESS2" >/dev/null 2>&1

# --- UC-82: detach, then relaunch and reattach -------------------------------
#
# Launching through a login shell so the shell survives the detach; if forestui
# were the tu process itself, detaching would kill the session outright.
SESS3="$SESS-c"
tu kill --name "$SESS3" >/dev/null 2>&1
tu run --name "$SESS3" --size 140x44 \
  --env TMUX_TMPDIR="$TMUXDIR" \
  --env HOME="$ROOT/home" \
  --env PATH="$CMD_DIR:$ROOT/bin:$(dirname "$(command -v tmux)"):/usr/bin:/bin:/usr/sbin:/sbin" \
  --env FORESTUI_NO_AUTO_UPDATE=1 \
  --env SHELL=/bin/sh \
  --cwd "$REPO" \
  -- env -u TMUX bash --noprofile --norc >/dev/null 2>&1
sleep 2
tu type --name "$SESS3" "${FUI_CMD[*]} $ROOT/forest" >/dev/null 2>&1
tu press --name "$SESS3" Enter >/dev/null 2>&1
tu wait --name "$SESS3" --text "MAIN REPOSITORY|WORKTREE|No repositories" --timeout 15000 >/dev/null 2>&1
sleep 2
capture UC-82 reattach-first-run "$SESS3"

tu press --name "$SESS3" Ctrl+B d >/dev/null 2>&1; sleep 2
capture UC-82 reattach-detached "$SESS3"

tu type --name "$SESS3" "${FUI_CMD[*]} $ROOT/forest" >/dev/null 2>&1
tu press --name "$SESS3" Enter >/dev/null 2>&1
tu wait --name "$SESS3" --text "MAIN REPOSITORY|WORKTREE|No repositories" --timeout 15000 >/dev/null 2>&1
sleep 2
capture UC-82 reattach-second-run "$SESS3"

# Reattaching must not leave a second forestui window behind.
fui_windows="$(tu screenshot --name "$SESS3" 2>/dev/null | python3 -c "
import json,sys,re
line=json.load(sys.stdin)['content'].splitlines()[-1]
print(len(re.findall(r'forestui(-dev-\d+)?[-*]?\s', line)))")"
assert UC-82 single-forestui-window "1" "$fui_windows"

tu kill --name "$SESS3" >/dev/null 2>&1

# --- UC-83 .. UC-86: mouse ---------------------------------------------------
#
# Textual is mouse-first; ratatui has no built-in notion of a clickable widget,
# so the Rust build records a rectangle per control each frame and resolves
# clicks against that. Both builds are driven the same way here, by clicking on
# label text, which is what a user actually does.

screen_has() {
  tu screenshot --name "$SESS" 2>/dev/null | python3 -c "
import json,re,sys
print('yes' if re.search(sys.argv[1], json.load(sys.stdin)['content']) else 'no')" "$1"
}
click_text() { tu mouse click --name "$SESS" --on-text "$1" >/dev/null 2>&1; }

focus_app
click_text "beta"; sleep 2
capture UC-83 click-sidebar-row
assert UC-83 detail-follows-click "yes" "$(screen_has 'Repository: beta')"

focus_app
click_text "Terminal"; sleep 3
capture UC-84 click-detail-control
assert UC-84 terminal-window-opened "yes" "$(screen_has 'term:beta')"

focus_app
press a; await "Repository Path"; sleep 0.8
click_text "Cancel"; sleep 1.5
capture UC-86 click-modal-cancel
assert UC-86 modal-dismissed-by-click "no" "$(screen_has 'Repository Path')"

# --- UC-99: Start work for an issue, from `s` to a running Claude ------------
#
# Rust only: the Python build never had Start work. Runs last because the
# launch adds a worktree, which would change every frame after it. beta is
# selected (UC-83) and the gh stub answers its issue list with #326 and #298.
if [ "$LABEL" = "rust" ]; then
  focus_app
  press s; await "Work Mode"; sleep 0.8
  capture UC-99 start-work-mode
  # Selecting beta prefetched its issues into the cache, so the list opens
  # filled rather than on its loading line.
  press Enter; await "Start Work: issue"
  assert UC-99 opens-already-filled "no" "$(screen_has 'Loading issues')"
  await "#298"; sleep 0.8
  capture UC-99 start-work-item
  assert UC-99 lists-only-mine "no" "$(screen_has '#412')"
  # Past the filter and Show blocked to Not just mine: the list widens for
  # this pick alone, and unticking it brings back the cached list of the
  # user's own.
  press Tab; press Tab; press Tab; press Enter; await "#412"; sleep 0.8
  capture UC-99 start-work-item-all
  assert UC-99 not-just-mine-ticked "yes" "$(screen_has '\[x\] Not just mine')"
  press Space; await "\[ \] Not just mine"; sleep 0.5
  assert UC-99 unticked-lists-only-mine "no" "$(screen_has '#412')"
  press Shift+Tab; press Shift+Tab; press Shift+Tab; sleep 0.3
  type_ "metrics"; sleep 0.5; press Enter
  await "Launch"; sleep 0.8
  capture UC-99 start-work-confirm
  assert UC-99 prompt-names-the-issue "yes" "$(screen_has 'gh issue view 298 --comments')"
  # Launch is the primary button after the checkbox; Enter on a text field
  # launches too, which is what the name field (focused on open) does here.
  press Enter
  await "[0-9]:issue-298" 15000; sleep 2
  capture UC-99 start-work-launched
  wt="$ROOT/forest/beta/issue-298-proactive-metrics-coalesced-fire-rows-ov"
  assert UC-99 worktree-created "yes" "$([ -d "$wt" ] && echo yes || echo no)"
  assert UC-99 branch-checked-out "fix/issue-298-proactive-metrics-coalesced-fire-rows-ov" \
    "$(git -C "$wt" branch --show-current 2>/dev/null)"
  assert UC-99 work-item-linked "{'kind': 'issue', 'number': 298}" \
    "$(cfg "[w.get('work_item') for r in d['repositories'] for w in r['worktrees'] if w['name'].startswith('issue-298')][0]")"
  assert UC-99 claude-window-named-after-worktree "yes" \
    "$(screen_has "[0-9]:issue-298")"
fi

# --- UC-100: Start work for a feature ----------------------------------------
#
# Rust only, and after UC-99 for the same reason. The selection is still in
# beta, whose gh stub also answers with #77, labelled `enhancement`. It is
# listed under `issue` with every other issue, and its label decides only how
# it starts.
if [ "$LABEL" = "rust" ]; then
  focus_app
  press s; await "Work Mode"; sleep 0.8
  assert UC-100 no-feature-mode "no" "$(screen_has ' feature {4,}[A-Z]')"
  press Enter; await "#77"; sleep 0.8
  capture UC-100 start-work-feature-item
  assert UC-100 bug-listed "yes" "$(screen_has '#326')"
  assert UC-100 other-issue-listed "yes" "$(screen_has '#298')"
  assert UC-100 counted-as-issues "yes" "$(screen_has '3 issues')"
  assert UC-100 no-free-text-entry "no" "$(screen_has 'Free-text feature')"
  type_ "dark mode"; await "1 match"; press Enter
  await "Launch"; sleep 0.8
  capture UC-100 start-work-feature-confirm
  assert UC-100 titled-as-a-feature "yes" "$(screen_has 'Start Work: Feature #77')"
  assert UC-100 prompt-asks-for-a-plan "yes" "$(screen_has 'approved the plan')"
  # This launch's Claude is the one UC-101 jumps back to.
  touch "$ROOT/write-transcripts"
  press Enter
  await "[0-9]:feat-77" 15000; sleep 2
  capture UC-100 start-work-feature-launched
  wt="$ROOT/forest/beta/feat-77-dark-mode-for-the-dashboard"
  assert UC-100 worktree-created "yes" "$([ -d "$wt" ] && echo yes || echo no)"
  assert UC-100 branch-checked-out "feat/77-dark-mode-for-the-dashboard" \
    "$(git -C "$wt" branch --show-current 2>/dev/null)"
  assert UC-100 work-item-linked "{'kind': 'issue', 'number': 77}" \
    "$(cfg "[w.get('work_item') for r in d['repositories'] for w in r['worktrees'] if w['name'].startswith('feat-77')][0]")"
fi

# --- UC-101: one work item, one worktree --------------------------------------
#
# Rust only, after UC-100: #77 now has a worktree with its Claude running in
# a window. Picking #77 again must go there, not to the confirm screen.
if [ "$LABEL" = "rust" ]; then
  focus_app
  # Move off the feature worktree, so the jump has to bring the selection back.
  press Up; sleep 0.8
  worktrees_before="$(cfg "sum(len(r['worktrees']) for r in d['repositories'])")"
  press s; await "Work Mode"; sleep 0.8
  press Enter; await "#77"; sleep 0.8
  capture UC-101 start-work-linked-item
  assert UC-101 linked-item-marked "yes" \
    "$(screen_has '#77 Dark mode for the dashboard +→ feat-77-dark-mode-for-the-dashboard')"
  type_ "dark mode"; await "1 match"; press Enter
  # The switch lands on the Claude window: the stub's output is on screen and
  # the window is the current one in the status bar.
  await "claude stub" 15000; sleep 1
  capture UC-101 start-work-jumped
  assert UC-101 switched-to-claude-window "yes" "$(screen_has '[0-9]:feat-77[^ ]*\*')"
  assert UC-101 nothing-created "$worktrees_before" \
    "$(cfg "sum(len(r['worktrees']) for r in d['repositories'])")"
  focus_app
  assert UC-101 worktree-selected "yes" "$(screen_has 'Worktree: +feat-77')"
fi

# --- UC-103: Start work for a session ------------------------------------------
#
# Rust only, after UC-101. `session` goes from the mode step straight to the
# prompt; the prompt's first words name the worktree until the name
# suggestion (the stub's `caching-spike`) replaces them. The prompt is the
# whole first prompt (the `session` template is `{body}`), and is what the
# worktree's link records, under the free-text feature's wire format. Nothing
# is asked of GitHub. Starting the same words a second time asks, on Launch,
# about the taken name instead of matching it.
if [ "$LABEL" = "rust" ]; then
  focus_app
  tu mouse click --name "$SESS" --on-regex '[▼▶] beta' >/dev/null 2>&1
  await "Repository: beta"
  press s; await "Work Mode"; sleep 0.8
  capture UC-103 session-mode
  assert UC-103 session-listed "yes" \
    "$(screen_has 'session +A Claude session for anything else')"
  press Down Enter; await "Prompt"; sleep 0.8
  type_ "Try a quick spike on caching"; sleep 0.5
  await "Worktree: session-try-a-quick-spike-on-caching"
  capture UC-103 session-prompt
  press Enter
  await "suggesting a name"
  assert UC-103 first-words-while-suggesting "yes" \
    "$(screen_has 'session/try-a-quick-spike-on-caching')"
  await "session/caching-spike" 15000; sleep 0.8
  capture UC-103 session-confirm
  assert UC-103 suggestion-stopped-spinning "no" "$(screen_has 'suggesting a name')"
  assert UC-103 titled-as-session "yes" "$(screen_has 'Start Work: Session')"
  assert UC-103 prompt-carries-the-prompt "yes" "$(screen_has 'Try a quick spike on caching')"
  assert UC-103 prompt-reads-no-issue "no" "$(screen_has 'gh issue view')"
  assert UC-103 prompt-asks-for-no-plan "no" "$(screen_has 'approved the plan')"
  press Enter
  await "[0-9]:session-caching-spike" 15000; sleep 2
  capture UC-103 session-launched
  wt="$ROOT/forest/beta/session-caching-spike"
  assert UC-103 worktree-created "yes" "$([ -d "$wt" ] && echo yes || echo no)"
  assert UC-103 branch-checked-out "session/caching-spike" \
    "$(git -C "$wt" branch --show-current 2>/dev/null)"
  assert UC-103 work-item-linked \
    "{'kind': 'free_text', 'description': 'Try a quick spike on caching'}" \
    "$(cfg "[w.get('work_item') for r in d['repositories'] for w in r['worktrees'] if w['name'].startswith('session-caching')][0]")"

  focus_app
  worktrees_before="$(cfg "sum(len(r['worktrees']) for r in d['repositories'])")"
  press s; await "Work Mode"; sleep 0.8
  press Down Enter; await "Prompt"; sleep 0.8
  type_ "Try a quick spike on caching"; sleep 0.5; press Enter
  await "session/caching-spike" 15000; sleep 0.8
  press Enter
  await "already exists"; sleep 0.8
  capture UC-103 session-clash
  assert UC-103 clash-offers-three-choices "yes" \
    "$(screen_has 'Open existing.*Create copy.*Edit name')"
  assert UC-103 clash-names-the-copy "yes" \
    "$(screen_has 'create session-caching-spike-2 instead')"
  # Esc walks back: clash → prompt → modes → closed. One press at a time:
  # ESC bytes sent back to back read as Alt+Escape, and a lost press leaves
  # the flow open under every case after this one.
  press Escape; await "What should Claude do|Prompt"; sleep 0.5
  press Escape; await "Work Mode"; sleep 0.5
  press Escape; sleep 1
  assert UC-103 flow-closed "no" "$(screen_has 'Start Work')"
  assert UC-103 nothing-created "$worktrees_before" \
    "$(cfg "sum(len(r['worktrees']) for r in d['repositories'])")"
fi

# --- UC-104: Start work for a pr-review ---------------------------------------
#
# Rust only, after UC-103. beta's gh stub lists #45, opened by someone else,
# and #46, a draft the sweep user opened. #45 is reviewed and reported on, in
# a worktree `gh pr checkout` put on the PR's own branch; #46 would get the
# self-review, and is only looked at.
if [ "$LABEL" = "rust" ]; then
  focus_app
  tu mouse click --name "$SESS" --on-regex '[▼▶] beta' >/dev/null 2>&1
  await "Repository: beta"
  press s; await "Work Mode"; sleep 0.8
  press Down Down Enter; await "#45"; sleep 0.8
  capture UC-104 pr-review-item
  assert UC-104 draft-marked "yes" "$(screen_has '#46 Cache the session index +draft')"
  type_ "retry"; sleep 0.5; press Enter
  await "Launch"; sleep 0.8
  capture UC-104 pr-review-confirm
  assert UC-104 titled-as-a-pr "yes" "$(screen_has 'Start Work: PR #45')"
  assert UC-104 someone-elses-review "yes" "$(screen_has "Review someone else's PR")"
  assert UC-104 prompt-reads-the-pr "yes" "$(screen_has 'gh pr view 45 --comments')"
  assert UC-104 no-base-branch "no" "$(screen_has 'Base Branch')"
  press Enter
  await "[0-9]:pr-45" 15000; sleep 2
  capture UC-104 pr-review-launched
  wt="$ROOT/forest/beta/pr-45-retry-uploads-with-backoff"
  assert UC-104 worktree-created "yes" "$([ -d "$wt" ] && echo yes || echo no)"
  assert UC-104 pr-branch-checked-out "fix/retry-backoff" \
    "$(git -C "$wt" branch --show-current 2>/dev/null)"
  assert UC-104 work-item-linked "{'kind': 'pull_request', 'number': 45}" \
    "$(cfg "[w.get('work_item') for r in d['repositories'] for w in r['worktrees'] if w['name'].startswith('pr-45')][0]")"

  focus_app
  worktrees_before="$(cfg "sum(len(r['worktrees']) for r in d['repositories'])")"
  press s; await "Work Mode"; sleep 0.8
  press Down Down Enter; await "#46"; sleep 0.8
  type_ "cache"; sleep 0.5; press Enter
  await "Launch"; sleep 0.8
  capture UC-104 own-pr-confirm
  assert UC-104 own-review "yes" "$(screen_has 'Review my own PR')"
  # Esc walks back: confirm → list → modes → closed, one press at a time.
  press Escape; await "Enter picks"; sleep 0.5
  press Escape; await "Work Mode"; sleep 0.5
  press Escape; sleep 1
  assert UC-104 flow-closed "no" "$(screen_has 'Start Work')"
  assert UC-104 nothing-created "$worktrees_before" \
    "$(cfg "sum(len(r['worktrees']) for r in d['repositories'])")"
fi

# --- UC-105: finished worktrees -----------------------------------------------
#
# Rust only, after UC-104. #45 merges on GitHub (the stub's flag); the next
# finished check — a selection change, past the per-repository throttle, or
# the 30-second sweep — marks pr-45's row with a ✓ and dims it. The issue and
# session worktrees beside it stay unmarked: #77 is still open, and beta has
# no remote whose default branch the session's branch could be merged into.
if [ "$LABEL" = "rust" ]; then
  touch "$ROOT/pr-45-merged"
  focus_app
  tu mouse click --name "$SESS" --on-regex '[▼▶] alpha' >/dev/null 2>&1
  await "Repository: alpha"
  tu mouse click --name "$SESS" --on-regex '[▼▶] beta' >/dev/null 2>&1
  await "Repository: beta"
  await "✓ pr-45" 45000; sleep 0.5
  capture UC-105 finished-marked
  assert UC-105 merged-pr-marked "yes" "$(screen_has '[├└]─ ✓ pr-45')"
  assert UC-105 open-issue-unmarked "no" "$(screen_has '✓ feat-77')"
  assert UC-105 session-unmarked "no" "$(screen_has '✓ session-caching')"
  assert UC-105 mark-not-saved "False" \
    "$(cfg "any('finished' in w for r in d['repositories'] for w in r['worktrees'])")"
fi

# --- UC-72: delete a worktree, clean and dirty ---------------------------------
#
# Rust only, and last: deleting changes the sidebar of every frame after it.
# Removal runs in the background and only the fold changes state, so the
# outcome is asserted on disk, in the config and in git — a frame captured
# while git still runs looks the same as one where the delete silently failed.
#
# wait_until <shell-test> — poll a condition on disk instead of sleeping, for
# the same reason `await` polls the screen. Reported like a wait timeout.
wait_until() {
  local tries=0
  until eval "$1"; do
    tries=$((tries + 1))
    if [ "$tries" -ge 75 ]; then
      printf '  WAIT-TIMEOUT  %s\n' "$1" >&2
      WAIT_TIMEOUTS=$((WAIT_TIMEOUTS + 1))
      return
    fi
    sleep 0.2
  done
}
tracked() { cfg "any(w['name'].startswith('$1') for r in d['repositories'] for w in r['worktrees'])"; }
listed_by_git() {
  git -C "$1" worktree list --porcelain 2>/dev/null | grep -qF "$2" && echo yes || echo no
}

if [ "$LABEL" = "rust" ]; then
  wt="$ROOT/forest/beta/issue-298-proactive-metrics-coalesced-fire-rows-ov"
  focus_app
  tu mouse click --name "$SESS" --on-regex '[├└]─ issue-298' >/dev/null 2>&1
  await 'Worktree: +issue-298'

  # `n` cancels: nothing is touched.
  press d; await "Permanently delete 'issue-298"; sleep 0.5
  capture UC-72 delete-confirm
  press n; sleep 1
  assert UC-72 cancel-closes-confirm "no" "$(screen_has 'Permanently delete')"
  assert UC-72 cancel-keeps-dir "yes" "$([ -d "$wt" ] && echo yes || echo no)"
  assert UC-72 cancel-keeps-entry "True" "$(tracked issue-298)"

  # `y` deletes: directory, config entry and git's own record all go.
  press d; await "Permanently delete 'issue-298"; sleep 0.5
  press y
  wait_until "[ \"\$(tracked issue-298)\" = False ]"
  await 'Repository: beta'; sleep 1
  capture UC-72 deleted
  assert UC-72 dir-removed "no" "$([ -d "$wt" ] && echo yes || echo no)"
  assert UC-72 entry-removed "False" "$(tracked issue-298)"
  assert UC-72 git-forgot-it "no" "$(listed_by_git "$ROOT/src/beta" "$wt")"

  # A worktree with uncommitted work is refused first, then asks again. That
  # second confirm comes from a background result, so it arms after a delay
  # and swallows a keypress that was already on its way.
  dirty="$ROOT/forest/alpha/wt-a-renamed"
  echo "unsaved" > "$dirty/scratch.txt"
  focus_app
  tu mouse click --name "$SESS" --on-regex '[├└]─ wt-a-renamed' >/dev/null 2>&1
  await 'Worktree: +wt-a-renamed'
  press d; await "Permanently delete 'wt-a-renamed"; sleep 0.5
  press y
  await 'Uncommitted Changes'; sleep 1
  capture UC-72 dirty-confirm
  assert UC-72 dirty-is-refused-first "yes" "$([ -f "$dirty/scratch.txt" ] && echo yes || echo no)"
  assert UC-72 dirty-names-the-change "yes" "$(screen_has 'untracked')"
  press y
  wait_until "[ \"\$(tracked wt-a-renamed)\" = False ]"
  assert UC-72 dirty-dir-removed "no" "$([ -d "$dirty" ] && echo yes || echo no)"
  assert UC-72 dirty-entry-removed "False" "$(tracked wt-a-renamed)"
  assert UC-72 dirty-git-forgot-it "no" "$(listed_by_git "$ROOT/src/alpha" "$dirty")"
fi

# --- UC-106: prune finished worktrees ----------------------------------------
#
# Rust only, after UC-72 (it gives beta a remote, which would change the
# Git Pull label in every frame before it). pr-45 is finished since UC-105.
# beta has no remote yet, so its commits are on no remote branch and prune
# must skip it as unpushed; once pushed, prune removes it and its branch.
if [ "$LABEL" = "rust" ]; then
  wt="$ROOT/forest/beta/pr-45-retry-uploads-with-backoff"
  focus_app
  tu mouse click --name "$SESS" --on-regex '[▼▶] beta' >/dev/null 2>&1
  await "Prune finished \(1\)"; sleep 0.5
  capture UC-106 prune-control
  assert UC-106 control-shows-the-count "yes" "$(screen_has 'Prune finished \(1\)')"

  click_text "Prune finished (1)"
  await "Prune Finished Worktrees"; sleep 0.5
  capture UC-106 prune-confirm
  assert UC-106 confirm-lists-the-worktree "yes" "$(screen_has '│ {2,}pr-45-retry-uploads-with-backoff')"
  press y
  await "Skipped 'pr-45"; sleep 0.5
  capture UC-106 unpushed-skipped
  # The toast wraps, so the name and the reason are matched apart.
  assert UC-106 unpushed-named "yes" "$(screen_has "Skipped 'pr-45-retry-uploads-with-backoff'")"
  assert UC-106 unpushed-reason "yes" "$(screen_has '\(no remote to push to\)')"
  assert UC-106 unpushed-kept "yes" "$([ -d "$wt" ] && echo yes || echo no)"
  assert UC-106 unpushed-still-tracked "True" "$(tracked pr-45)"

  git init -q --bare "$ROOT/beta-remote.git"
  git -C "$ROOT/src/beta" remote add origin "$ROOT/beta-remote.git"
  git -C "$wt" push -q origin HEAD:refs/heads/fix/retry-backoff
  git -C "$ROOT/src/beta" fetch -q origin
  focus_app
  click_text "Prune finished (1)"
  await "Prune Finished Worktrees"; sleep 0.5
  press y
  wait_until "[ \"\$(tracked pr-45)\" = False ]"
  await "Pruned 'pr-45"; sleep 0.5
  capture UC-106 pruned
  assert UC-106 dir-removed "no" "$([ -d "$wt" ] && echo yes || echo no)"
  assert UC-106 git-forgot-it "no" "$(listed_by_git "$ROOT/src/beta" "$wt")"
  assert UC-106 branch-deleted "no" \
    "$(git -C "$ROOT/src/beta" rev-parse --verify --quiet refs/heads/fix/retry-backoff >/dev/null && echo yes || echo no)"
  assert UC-106 control-gone "no" "$(screen_has 'Prune finished')"
fi

echo "done: $LABEL"
if [ "$WAIT_TIMEOUTS" -gt 0 ]; then
  # Loud on purpose: every timed-out wait captured a frame of whatever was on
  # screen instead, so the frames below are not all what they claim to be.
  echo "WARNING: $WAIT_TIMEOUTS wait(s) timed out — those captures are suspect"
fi
echo "assertions:"
sed 's/^/  /' "$ASSERTIONS"
