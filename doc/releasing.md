# Versioning and releasing

Version is `0.0.0` in source (`Cargo.toml` and `cli::VERSION`). Real versions
are derived from git tags at release time by the release workflow.

Running from source shows version `0.0.0` and auto-enables dev mode, which gives
the tmux window a timestamped name (`forestui-dev-HHMM`) so a dev instance never
collides with a real one.

## Releasing

```bash
gh release create v0.10.0 --generate-notes
```

This triggers the release workflow, which stamps the version from the tag,
builds the static musl binaries for Linux (x86_64, aarch64) and the macOS
Apple-silicon binary, attaches them to the release with their checksums, and
publishes to crates.io.
