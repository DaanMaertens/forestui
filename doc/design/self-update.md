# Self-update

forestui keeps itself current the way the Python build did — automatically, on
launch — but never on the UI thread. `App::check_for_update` spawns the check
once the terminal is up and sends its verdict back as `AppEvent::UpdateChecked`.
Success is **not a toast**: once the new version is in place,
`App::pending_update` is set and the title bar reads
`forestui v<current> (v<new> ready — restart to update)` for the rest of the
run — the running process *is* the old build until it exits, so the notice
stays true until then, and a toast would time out while the user looked
elsewhere. A too-narrow bar gets the compact `(v<new> — restart)`, and one too
narrow even for that drops the running version rather than clip "restart". An
instance whose binary another instance already replaced (Linux reports `<exe>
(deleted)`) counts as installed too. A `cargo install` build, which does not
replace itself, marks the title `(v<new> available — cargo install forestui)`
instead; that one returns on every launch until the user updates. Network
failures (offline, a download that dropped, a release whose assets have not
finished uploading) stay silent and retry next launch. Only a *persistent
local* failure — an unwritable install dir — shows an error notification, and
is remembered for an hour in
`~/.config/forestui/latest_version_check.json` (`install_failed`) so the
multi-MB download is not re-spent on every launch while it would fail
identically. That memo is the *only* thing that file now holds, and the only
state that survives a launch.

What it does depends on how the binary got there:

- **From a GitHub release** — built with `--features binary-release`, so it
  downloads the asset for its platform, verifies it against the published
  `.sha256` (no checksum, no update — release assets **must** ship their
  checksums), and atomically swaps it over `current_exe()` via the same
  fsync-then-rename writer the config files use. Replacing the file under a
  running process is safe on Unix; the new build takes effect on the next
  launch.
- **From `cargo install`** — the feature is off, so it only reports that a newer
  version exists. Recompiling a crate underneath a running TUI is not something
  to do unasked.
- **From source** (version `0.0.0`) — nothing at all, which is what stops
  `cargo run` in a checkout from overwriting itself.

**The version lookup is not cached.** GitHub is asked on every launch, so a
release reaches the user the next time they open forestui rather than whenever
a TTL happened to lapse — the check is already off the UI thread, so nobody
waits on the round trip. The corollary is that a failed lookup has nothing to
fall back on, which is the point: there is no remembered version to act on, so
offline is silence rather than a doomed download or a nag.
`--no-self-update` skips the check entirely.

`release_asset_url` must keep producing `forestui_<os>_<arch>`, matching
`install.sh` and the release workflow. Drift there is a silent 404 on every
update, which is why a test asserts the shape.
