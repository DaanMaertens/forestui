//! Shared helpers: path expansion, slugs, relative times, atomic writes.

use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

/// The user's home directory, falling back to the current directory.
///
/// `std::env::home_dir` was un-deprecated in Rust 1.87 with corrected
/// semantics, so no `dirs`-style crate is needed here.
pub fn home_dir() -> PathBuf {
    #[allow(deprecated)]
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Expand a leading `~` to the user's home directory.
pub fn expanduser(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().join(rest);
    }
    if path == "~" {
        return home_dir();
    }
    PathBuf::from(path)
}

/// Expand `~` and resolve the path, falling back to the expanded form when the
/// path does not exist yet.
pub fn expand_and_resolve(path: &str) -> PathBuf {
    let expanded = expanduser(path);
    std::fs::canonicalize(&expanded).unwrap_or(expanded)
}

/// Convert text to a safe slug for tmux session names.
pub fn slugify(text: &str) -> String {
    let lowered = text.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut pending_dash = false;
    for ch in lowered.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
        } else {
            pending_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Human-readable relative time, matching the phrasing of Python's
/// `humanize.naturaltime` so screen text is unchanged from the Textual build.
pub fn naturaltime(when: DateTime<Utc>) -> String {
    let delta = Utc::now().signed_duration_since(when);
    let future = delta.num_seconds() < 0;
    let secs = delta.num_seconds().unsigned_abs();

    if secs < 1 {
        return "now".to_string();
    }
    let magnitude = naturaldelta(secs);
    if future {
        format!("{magnitude} from now")
    } else {
        format!("{magnitude} ago")
    }
}

/// The bare magnitude phrase used by [`naturaltime`].
///
/// A port of `humanize.naturaldelta`, which the Textual build called directly.
/// Its shape is not obvious and the boundaries are not round numbers — sub-day
/// units are *rounded* rather than truncated, and anything from 16 days up is
/// fuzzed into months at 30.5 days each — so it is transcribed rather than
/// reinvented. Getting this wrong is quiet: every session and issue card grows
/// a slightly different timestamp than the Python build showed.
pub fn naturaldelta(secs: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    let total_days = secs / DAY;
    // `timedelta.seconds` is the remainder below a whole day, not the total.
    let seconds = secs % DAY;
    let years = total_days / 365;
    let days = total_days % 365;
    // `round(days / 30.5)`, kept in integers as `round(2 * days / 61)`.
    let months = round_half_even(days * 2, 61);

    if total_days == 0 {
        return match seconds {
            0 => "a moment".into(),
            1 => "a second".into(),
            s if s < MINUTE => format!("{s} seconds"),
            s if s < HOUR => match round_half_even(s, MINUTE) {
                1 => "a minute".into(),
                60 => "an hour".into(),
                minutes => format!("{minutes} minutes"),
            },
            s => match round_half_even(s, HOUR) {
                1 => "an hour".into(),
                24 => "a day".into(),
                hours => format!("{hours} hours"),
            },
        };
    }

    match (years, days, months) {
        (0, 1, _) => "a day".into(),
        (0, _, 0) => format!("{days} days"),
        (0, _, 1) => "a month".into(),
        (0, _, 12) => "a year".into(),
        (0, _, _) => format!("{months} months"),
        (1, 0, 0) => "a year".into(),
        (1, 1, 0) => "1 year, 1 day".into(),
        (1, _, 0) => format!("1 year, {days} days"),
        (1, _, 1) => "1 year, 1 month".into(),
        (1, _, 12) => "2 years".into(),
        (1, _, _) => format!("1 year, {months} months"),
        _ => format!("{years} years"),
    }
}

/// Divide and round half to even, the way Python's `round` does. The tie
/// matters: 90 seconds is 2 minutes but 150 seconds is also 2, not 3.
fn round_half_even(value: u64, unit: u64) -> u64 {
    let quotient = value / unit;
    let twice_remainder = (value % unit) * 2;
    if twice_remainder > unit || (twice_remainder == unit && quotient % 2 == 1) {
        quotient + 1
    } else {
        quotient
    }
}

/// A noun in both numbers, so a count of one reads "1 issue" rather than
/// "1 issues". Both forms are spelled out because English plurals are not
/// regular enough to derive ("branch" → "branches").
#[derive(Clone, Copy)]
pub struct Noun {
    one: &'static str,
    many: &'static str,
}

impl Noun {
    pub const fn new(one: &'static str, many: &'static str) -> Self {
        Self { one, many }
    }

    /// The form that agrees with `count`; zero takes the plural.
    pub fn of(self, count: usize) -> &'static str {
        if count == 1 { self.one } else { self.many }
    }

    /// `count` followed by the agreeing form: "1 issue", "3 issues".
    pub fn count(self, count: usize) -> String {
        format!("{count} {}", self.of(count))
    }
}

/// Truncate to `max` characters, appending `...` when the text was longer.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        let head: String = text.chars().take(max).collect();
        format!("{head}...")
    } else {
        text.to_string()
    }
}

/// Whether the directory exists on disk.
pub fn path_exists(path: &str) -> bool {
    Path::new(&expanduser(path)).exists()
}

/// Exit code of a finished subprocess, with signal-death mapped to `sentinel`.
///
/// `status.code()` is `None` when the process died to a signal; mapping that
/// to 0 — the easy default — reads a killed `git pull` as success. Callers
/// choose the sentinel because some reserve values: `gh` uses -1 to mean "the
/// binary is missing", so its killed processes must map to something else.
pub fn exit_code(status: std::process::ExitStatus, sentinel: i32) -> i32 {
    status.code().unwrap_or(sentinel)
}

/// Write a file via a sibling temp file and an atomic rename.
///
/// `.forestui-config.json` is the only record of every tracked repository and
/// worktree, and a corrupt file deliberately loads as *empty* state so a bad
/// byte never blocks startup — which turns a crash mid-`fs::write` into total,
/// silent data loss on the next launch. The rename makes that window
/// impossible: readers see either the old file or the new one, never a torn
/// half. The temp file lives beside the target because a rename is only atomic
/// within one filesystem.
///
/// Three details keep the swap from being a downgrade on `fs::write`, which it
/// replaced:
///
/// - It writes through a symlink. A rename would replace the link itself, so a
///   config symlinked into a dotfiles repo would silently stop syncing after
///   the first save.
/// - It carries the target's existing permissions onto the replacement. A fresh
///   temp file is born at the umask default, which would quietly widen a config
///   the user had locked down.
/// - It fsyncs the contents before the rename, and the directory after it.
///   Without the first, a power loss can land the rename but not the bytes,
///   leaving the empty file that loads as empty state — the exact loss this
///   function exists to prevent.
pub fn write_atomically(path: &Path, contents: &str) -> std::io::Result<()> {
    write_bytes_atomically(path, contents.as_bytes(), None)
}

/// [`write_atomically`] for binary content, with an optional mode floor.
///
/// `executable_mode` is applied when the target does not exist or is not
/// owner-executable — the self-updater passes `0o755` so the replacement
/// binary always comes out runnable, while a deliberately tightened install
/// (say 0o700) keeps its exact mode rather than being widened.
pub fn write_bytes_atomically(
    path: &Path,
    contents: &[u8],
    executable_mode: Option<u32>,
) -> std::io::Result<()> {
    // Resolve the link before choosing a sibling, so both the temp file and the
    // rename land in the directory that actually holds the data.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let mut temp = target.clone();
    let mut name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    name.push_str(&format!(".tmp-{}", std::process::id()));
    temp.set_file_name(name);

    let result = write_temp_then_rename(&temp, &target, contents, executable_mode);
    if result.is_err() {
        // Leave no half-written sibling behind; the original is untouched.
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// The fallible half of [`write_bytes_atomically`], split out so one cleanup on
/// the error path covers a failure at any step rather than only at the rename.
fn write_temp_then_rename(
    temp: &Path,
    target: &Path,
    contents: &[u8],
    executable_mode: Option<u32>,
) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::File::create(temp)?;
    file.write_all(contents)?;
    // Durability before visibility: the rename is only atomic for readers, not
    // for the disk.
    file.sync_all()?;
    drop(file);

    let existing = std::fs::metadata(target).ok().map(|m| m.permissions());
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        match (existing, executable_mode) {
            // The caller wants the file runnable and the current mode is not.
            (Some(p), Some(mode)) if p.mode() & 0o100 == 0 => {
                Some(std::fs::Permissions::from_mode(mode))
            }
            (Some(p), _) => Some(p),
            (None, Some(mode)) => Some(std::fs::Permissions::from_mode(mode)),
            (None, None) => None,
        }
    };
    #[cfg(not(unix))]
    let permissions = {
        let _ = executable_mode;
        existing
    };
    if let Some(permissions) = permissions {
        std::fs::set_permissions(temp, permissions)?;
    }

    std::fs::rename(temp, target)?;

    // Best effort: a directory fsync is what makes the rename itself durable,
    // and there is nothing useful to do when a platform refuses to open one.
    if let Some(parent) = target.parent()
        && let Ok(dir) = std::fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
    Ok(())
}
#[cfg(test)]
mod tests;
