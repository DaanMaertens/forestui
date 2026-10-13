#!/bin/sh
# Record what a Claude session is doing — busy, needs you, idle — in its
# heartbeat file, so forestui can show it on the session's card and on its
# worktree's sidebar row.
#
# Installed and removed by forestui; edits here are reported as drift rather
# than silently overwritten.
#
#   Notification (permission prompt, question)  -> needs_you
#   PreToolUse AskUserQuestion                  -> needs_you
#   PreToolUse / PostToolUse, after needs_you   -> busy  (the user answered)
#   Stop, Notification idle_prompt              -> idle
#
# UserPromptSubmit -> busy is written by heartbeat.sh, which rewrites the whole
# file on that event anyway; two hooks rewriting one file on the same event
# would race, and the loser's write would vanish.
#
# Claude fires no Stop when the user interrupts with Esc, so a session can
# read busy until the idle_prompt notification about a minute later. That
# self-heal is accepted on purpose.
#
# heartbeat.sh rewrites the file on other events, so a write here can still
# interleave with one there — the idle_prompt notification landing in the
# same instant as a new prompt could put idle back over busy. The window is a
# few milliseconds and the turn's Stop rewrites the state anyway, so it is
# left at that rather than locked.
#
# The tool-use hooks fire on every tool call, so the common path — busy and
# still busy — costs one jq and one grep, and writes nothing.
set -u

event="${1:-}"
input=$(cat)
[ -n "$event" ] || exit 0
command -v jq >/dev/null 2>&1 || exit 0

# One jq for the lot: the session id and what this event means for it.
# "resume" is "busy, but only if it was waiting on the user" — tool use while
# already busy, or after the turn ended, says nothing new.
out=$(printf '%s' "$input" | jq -r --arg event "$event" '
  # Only the typed notifications: guessing from the message text would read
  # any notification that mentions a permission as one asking for it.
  def notified:
    (.notification_type // "") as $type
    | if $type == "idle_prompt" then "idle"
      elif $type == "permission_prompt" or $type == "elicitation_dialog" then "needs_you"
      else "" end;
  (.session_id // "") + " " +
  (if $event == "Stop" then "idle"
   elif $event == "Notification" then notified
   elif $event == "PreToolUse" and .tool_name == "AskUserQuestion" then "needs_you"
   elif $event == "PreToolUse" or $event == "PostToolUse" then "resume"
   else "" end)
' 2>/dev/null) || exit 0

# The id names a file, so nothing but the characters a session id can contain
# survives — a hostile hook input must not traverse out of the live directory.
sid=$(printf '%s' "${out% *}" | tr -cd 'a-zA-Z0-9-')
state=${out##* }
[ -n "$sid" ] && [ -n "$state" ] || exit 0

# Only a session heartbeat.sh has recorded — and validated as a real claude
# process — gets a state. Writing a file here would be a heartbeat with no pid.
file="$HOME/.config/forestui/live/$sid.json"
[ -f "$file" ] || exit 0

if [ "$state" = "resume" ]; then
  grep -q '"state":"needs_you"' "$file" 2>/dev/null || exit 0
  state=busy
fi
grep -q "\"state\":\"$state\"" "$file" 2>/dev/null && exit 0

# Written whole then renamed, so forestui never reads half a file.
tmp="$HOME/.config/forestui/live/.$sid.$$.tmp"
jq -c --arg s "$state" '.state = $s' "$file" > "$tmp" 2>/dev/null \
  && mv -f "$tmp" "$file" 2>/dev/null || rm -f "$tmp" 2>/dev/null
exit 0
