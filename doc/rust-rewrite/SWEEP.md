# The tu sweep and its baselines

The automated cases are the ones `scripts/tu-sweep.sh` captures (the script
is the list; this file keeps no copy of it), each as frames plus on-disk
assertions (`ASSERTIONS.txt`). Capture a build and compare
two builds with:

```bash
scripts/tu-sweep.sh rust ./target/release/forestui
scripts/tu-compare.sh rust python
scripts/tu-composite.sh rust python   # side-by-side PNGs, one per case
```

The committed `baseline/python/` frames are a **frozen reference**, captured
from the installed 1.3.0 release (the build users actually ran). That release
is no longer installed here, so treat those frames as read-only history:
re-sweep `rust` freely, never regenerate `python`. `tu-compare.sh` works from
the committed frames; `tu-composite.sh` needs PNGs from a live sweep of both
builds, which the Python side can no longer produce.

Each case writes a normalised text frame to
`doc/rust-rewrite/baseline/<build>/` (committed — this is the diffable
baseline) and a PNG to `doc/rust-rewrite/screenshots/<build>/` (gitignored,
for eyeballing colour and focus). After a UI change, re-run the sweep and
review the frame diff: an unexpected change there is a regression, an expected
one needs the baseline refreshed in the same commit.

**Text frames alone are not enough.** Colour, focus rings and selection
highlights exist only in the pixels — a button that lost its accent renders an
identical frame. `tu-composite.sh` pairs two builds' PNGs per case so those
differences are visible rather than inferred. With no live Python sweep left
to pair against, read the `screenshots/rust/` PNGs themselves after any change
to `theme.rs` or a renderer.

A sweep that does not exercise a section will happily report parity for it.
UC-96 guards against that specifically: it captures the repository pane at full
height and asserts that sessions and issues actually rendered and that no
section fell back to its empty state. Keep that guard honest when adding cases.

The harness waits on screen conditions, never fixed sleeps — Textual repaints
far slower than ratatui, and fixed sleeps produced false mismatches. Anything
added to the sweep must use `await <regex>`.

All of this runs under the isolation rule at the top of `CLAUDE.md`: tmux
is driven through `tu` with `TMUX_TMPDIR`, never directly.

Note that `ensure_tmux` re-executes the binary, so for `tu` runs you must
build first and pass the built binary's path as the sweep's command argument
(e.g. `scripts/tu-sweep.sh rust ./target/release/forestui`) — `cargo run` does
not work under `tu`. Pass an absolute path when the tu session's working
directory is not the repo. Run only one tu-driving script at a time:
concurrent drivers corrupt each other's sessions.
