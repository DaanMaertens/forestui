# The back key

prefix + ← (configurable, `back_key` in settings) takes a Claude window
forestui opened back to forestui's window; it lives in
`services/tmux/back_key.rs`. It is a **server-wide** binding and a global
`status-right` hint, so like `focus-events` it is put back on exit — and the
cleanup state lives in tmux, not the process:

- The binding is the same text for every forestui on the server: an
  `if-shell -F` on `@claude_birth_name` that runs the previous binding in
  every other window. It finds its target at press time — the window marked
  `@forestui_home` in the viewer's session, via `run-shell -C` (tmux does not
  expand formats in a plain `-t`) — so a rename, grouped sessions and two
  forests all work. `switch-client -T root` ends the key's repeat, or an
  arrow pressed right after landing would be eaten.
- `@forestui_home` holds the forestui's pid. The binding and hint are
  restored when no mark with a live pid is left, which is also how a killed
  run is healed by the next start — the pid matters because a forestui
  started from a shell leaves its window behind when killed.
- What the key and `status-right` were before is saved in global
  `@forestui_*` options, and restored only if the current value is still
  ours: a user who rebinds in the meantime keeps their change.
- Only a binding that is neither tmux's default nor the one the user already
  accepted (`back_key_accepted`) brings up the offer.
