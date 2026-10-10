//! Reading and relocating Claude Code session history.
//!
//! Claude Code stores one JSONL file per session under
//! `~/.claude/projects/<path-with-slashes-replaced-by-dashes>/`.

use crate::models::{ClaudeSession, SessionTurn, Speaker, TokenUsage};
use chrono::{DateTime, TimeZone, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Last known sessions per `(path, limit)`. There is no TTL: the panel renders
/// this immediately on selection change while a fresh scan runs behind it, so
/// the worst case is content a few seconds old for the blink before the scan
/// lands — instead of a "Loading…" flash on every switch (#29). The limit is
/// part of the key so a future caller with a different cap cannot poison the
/// entry another consumer peeks.
type SessionCacheKey = (String, usize);

/// What the directory looked like when a cached answer was built: how many
/// transcripts there were and the newest modification time among them.
///
/// This is what makes a frequent refresh affordable. Parsing is the expensive
/// half — every transcript is read whole and every line JSON-parsed, and a busy
/// conversation runs to megabytes — while listing the directory and stat-ing it
/// is not. An unchanged fingerprint means an unchanged answer, so the sweep
/// costs one `readdir` on the overwhelmingly common tick where nobody has said
/// anything.
type Fingerprint = (usize, Option<std::time::SystemTime>);

type CacheEntry = (Fingerprint, Vec<ClaudeSession>);

fn cache() -> &'static Mutex<HashMap<SessionCacheKey, CacheEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<SessionCacheKey, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The last result [`get_sessions_for_path`] produced for this path and limit,
/// if any. Safe on the render thread: the lock is held only for the map read.
pub fn peek_sessions(path: &str, limit: usize) -> Option<Vec<ClaudeSession>> {
    cache()
        .lock()
        .ok()?
        .get(&(path.to_string(), limit))
        .map(|(_, sessions)| sessions.clone())
}

/// Count of transcripts and the newest mtime among them. Agent transcripts are
/// skipped here for the same reason the scan skips them, so one starting does
/// not read as a change.
fn fingerprint(sessions_dir: &Path) -> Fingerprint {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return (0, None);
    };
    let mut count = 0usize;
    let mut newest: Option<std::time::SystemTime> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("agent-"))
        {
            continue;
        }
        count += 1;
        if let Ok(modified) = entry.metadata().and_then(|m| m.modified())
            && newest.is_none_or(|prev| modified > prev)
        {
            newest = Some(modified);
        }
    }
    (count, newest)
}

/// Convert a filesystem path to Claude's folder naming convention.
pub fn path_to_claude_folder(path: &str) -> String {
    let resolved = crate::util::expand_and_resolve(path);
    resolved.to_string_lossy().replace('/', "-")
}

pub fn claude_projects_dir() -> PathBuf {
    crate::util::home_dir().join(".claude").join("projects")
}

/// [`get_sessions_for_path`], plus every pinned session the recency cap would
/// have dropped.
///
/// The cap keeps the scan cheap by parsing only the newest transcripts, but a
/// pin is exactly a claim against recency — "keep this one visible however old
/// it gets" — so the pinned ids are read individually and appended. The merge
/// happens outside the directory cache: pins belong to the app's state file,
/// and keying the cache by them would split it per pin change for no gain.
pub fn get_sessions_with_pins(path: &str, limit: usize, pinned: &[String]) -> Vec<ClaudeSession> {
    let mut sessions = get_sessions_for_path(path, limit);
    for id in pinned {
        if sessions.iter().any(|s| &s.id == id) {
            continue;
        }
        if let Some(session) = refresh_one(path, id) {
            sessions.push(session);
        }
    }
    sort_sessions(&mut sessions, pinned);
    sessions
}

/// Pinned sessions first, in pin order; everything else newest first.
pub fn sort_sessions(sessions: &mut [ClaudeSession], pinned: &[String]) {
    let rank = |session: &ClaudeSession| {
        pinned
            .iter()
            .position(|id| id == &session.id)
            .unwrap_or(usize::MAX)
    };
    sessions.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.last_timestamp.cmp(&a.last_timestamp))
    });
}

/// Sessions for a path, newest first, capped at `limit`.
pub fn get_sessions_for_path(path: &str, limit: usize) -> Vec<ClaudeSession> {
    let sessions_dir = claude_projects_dir().join(path_to_claude_folder(path));
    let key = (path.to_string(), limit);
    let current = fingerprint(&sessions_dir);

    // Nothing in the directory has changed since the cached answer was built,
    // so re-reading every transcript would produce the same list.
    if let Ok(guard) = cache().lock()
        && let Some((cached, sessions)) = guard.get(&key)
        && *cached == current
    {
        return sessions.clone();
    }

    let sessions = read_sessions_dir(&sessions_dir, limit);
    if let Ok(mut guard) = cache().lock() {
        guard.insert(key, (current, sessions.clone()));
    }
    sessions
}

/// Re-read one transcript. `None` means it is gone or no longer has any
/// messages, which is how a session that was deleted while forestui was in the
/// background leaves the list.
///
/// This is the per-card refresh: five transcripts re-read concurrently answer
/// far sooner than one walk of a directory that may hold dozens, and each card
/// updates the moment its own read lands rather than all of them together.
pub fn refresh_one(path: &str, session_id: &str) -> Option<ClaudeSession> {
    let file = claude_projects_dir()
        .join(path_to_claude_folder(path))
        .join(format!("{session_id}.jsonl"));
    // Through the cache: this runs for every card on screen on a ten-second
    // timer, and a transcript nobody has written to parses to what it did
    // before. One of them is 60 MB.
    let modified = std::fs::metadata(&file).and_then(|m| m.modified()).ok()?;
    parse_cached(&file, modified)
}

/// Directory-scoped form of [`get_sessions_for_path`], for testing.
pub fn read_sessions_dir(sessions_dir: &Path, limit: usize) -> Vec<ClaudeSession> {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return Vec::new();
    };

    // Stat first, parse second. One real project directory holds 542 MB across
    // 51 transcripts, and parsing all of them to sort by their last message and
    // then show five took 1.9 seconds — on a sweep that now runs every ten
    // seconds. A transcript's mtime is its last activity, which is the same
    // ordering, and costs a stat.
    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let file_path = entry.path();
        if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        // Agent transcripts are not user-facing sessions.
        if file_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.starts_with("agent-"))
        {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        candidates.push((file_path, modified));
    }
    candidates.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));

    // Walk newest-first and stop as soon as no remaining file can qualify.
    //
    // A file's mtime is never older than its last message — writing that
    // message is what set it — so a candidate whose *mtime* already loses to
    // the oldest session held cannot win on its last message either, and
    // neither can anything after it, because they are sorted. That makes this
    // exactly the same answer as parsing all 51 and sorting, for the cost of
    // parsing about five. Sorting on mtime alone is not the same answer:
    // naming a session rewrites its transcript without adding a timestamped
    // record, so mtime moves and the last message does not.
    let mut sessions: Vec<ClaudeSession> = Vec::new();
    for (file_path, modified) in candidates {
        if sessions.len() >= limit
            && let Some(worst) = sessions.last()
            && modified <= std::time::SystemTime::from(worst.last_timestamp)
        {
            break;
        }
        if let Some(session) = parse_cached(&file_path, modified) {
            sessions.push(session);
            sessions.sort_by_key(|s| std::cmp::Reverse(s.last_timestamp));
            sessions.truncate(limit);
        }
    }
    sessions
}

/// [`parse_session_file`], skipped entirely when the file has not been written
/// to since it was last parsed.
///
/// The scan runs on a timer, so the same transcripts are read over and over;
/// an active conversation touches one of them and leaves the rest identical.
pub fn parse_cached(file_path: &Path, modified: std::time::SystemTime) -> Option<ClaudeSession> {
    if let Ok(guard) = parsed_cache().lock()
        && let Some((cached_at, session)) = guard.get(file_path)
        && *cached_at == modified
    {
        return Some(session.clone());
    }

    let session = parse_session_file(file_path)?;
    if let Ok(mut guard) = parsed_cache().lock() {
        // A transcript that has gone quiet is not worth remembering forever,
        // and this is a cache rather than an index: dropping it wholesale
        // costs one re-parse of whatever is still on screen.
        if guard.len() >= PARSED_CACHE_LIMIT {
            guard.clear();
        }
        guard.insert(file_path.to_path_buf(), (modified, session.clone()));
    }
    Some(session)
}

/// Enough for every transcript of several busy projects; the entries are a few
/// short strings each.
const PARSED_CACHE_LIMIT: usize = 512;

type ParsedCache = HashMap<PathBuf, (std::time::SystemTime, ClaudeSession)>;

fn parsed_cache() -> &'static Mutex<ParsedCache> {
    static CACHE: OnceLock<Mutex<ParsedCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|naive| Utc.from_utc_datetime(&naive))
        })
}

fn text_content(data: &serde_json::Value) -> Option<String> {
    let content = data
        .get("message")
        .and_then(|m| m.get("content"))
        .or_else(|| data.get("content"))?;

    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    // Block format: take the first text block.
    if let Some(blocks) = content.as_array() {
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                return block
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(str::to_string);
            }
        }
        return Some(String::new());
    }
    None
}

/// A non-empty string field, trimmed. Claude re-appends title records, so the
/// last one wins; an empty value is treated as no name at all.
fn title_field(data: &serde_json::Value, key: &str) -> Option<String> {
    let value = data.get(key)?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Whether a `user` record is something the person actually sent.
///
/// `user` is the transport, not the author: tool results come back as `user`
/// records, and so do messages the system injects — a background agent
/// reporting in, a task notification. `origin.kind` separates them. A record
/// with no `origin` predates the field, so it is taken at face value rather
/// than dropped, which would erase the older half of a long conversation.
fn is_from_the_user(data: &serde_json::Value) -> bool {
    match data
        .get("origin")
        .and_then(|o| o.get("kind"))
        .and_then(|k| k.as_str())
    {
        Some(kind) => kind == "human",
        None => true,
    }
}

/// One line of preview text.
///
/// Whitespace is flattened before clipping because a card draws a turn as a
/// single line: a real message whose first line ended a sentence came out as
/// `painho:**Repo` — two words from different lines with nothing between them.
fn clip(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(100)
        .collect()
}

/// Record a turn, keeping only the last two.
///
/// Consecutive records from the same speaker are one turn: a single answer is
/// usually several `assistant` records — text, a tool call, more text — and one
/// real session had runs of up to 77 of them. Taken as separate turns they
/// would fill both lines and hide the question they answer, which is the half
/// that identifies the conversation.
fn push_turn(turns: &mut Vec<SessionTurn>, speaker: Speaker, text: String) {
    match turns.last_mut() {
        Some(last) if last.speaker == speaker => last.text = text,
        _ => {
            turns.push(SessionTurn { speaker, text });
            if turns.len() > 2 {
                turns.remove(0);
            }
        }
    }
}

pub fn parse_session_file(file_path: &Path) -> Option<ClaudeSession> {
    let session_id = file_path.file_stem()?.to_string_lossy().to_string();
    let raw = std::fs::read_to_string(file_path).ok()?;

    let mut first_prompt = String::new();
    let mut custom_title: Option<String> = None;
    let mut ai_title: Option<String> = None;
    let mut turns: Vec<SessionTurn> = Vec::new();
    let mut last_timestamp: Option<DateTime<Utc>> = None;
    let mut message_count = 0usize;
    let mut git_branch: Option<String> = None;
    let mut tokens = TokenUsage::default();
    let mut model: Option<String> = None;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(data) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        if let Some(ts_raw) = data.get("timestamp").and_then(|t| t.as_str())
            && let Some(ts) = parse_timestamp(ts_raw)
            && last_timestamp.is_none_or(|prev| ts > prev)
        {
            last_timestamp = Some(ts);
        }

        // The last branch seen wins: a conversation is remembered by where it
        // ended up, and mid-session checkouts update `gitBranch` per record.
        if let Some(branch) = data.get("gitBranch").and_then(|b| b.as_str())
            && !branch.trim().is_empty()
        {
            git_branch = Some(branch.trim().to_string());
        }

        // The names Claude itself records. A forked transcript carries the
        // parent's entries verbatim, so only entries stamped with this file's
        // own session id are allowed to name it.
        if data.get("sessionId").and_then(|s| s.as_str()) == Some(session_id.as_str()) {
            match data.get("type").and_then(|t| t.as_str()) {
                Some("custom-title") => custom_title = title_field(&data, "customTitle"),
                Some("ai-title") => ai_title = title_field(&data, "aiTitle"),
                _ => {}
            }
        }

        let is_user = data.get("type").and_then(|t| t.as_str()) == Some("user")
            || data.get("role").and_then(|r| r.as_str()) == Some("user");
        // Everything below counts turns the person took, so the count and the
        // preview agree with each other and with what they remember saying. It
        // used to count every `user` record, which on a long conversation is
        // mostly tool results: 5528 where 335 messages were sent.
        if is_user
            && is_from_the_user(&data)
            && let Some(content) = text_content(&data)
            && !content.is_empty()
            && !content.starts_with('<')
        {
            message_count += 1;
            let clipped = clip(&content);
            if first_prompt.is_empty() {
                first_prompt = clipped.clone();
            }
            push_turn(&mut turns, Speaker::User, clipped);
        }

        if data.get("type").and_then(|t| t.as_str()) == Some("assistant") {
            // Every assistant record carries the usage of the API call that
            // produced it, so the sums are the transcript's whole spend.
            if let Some(message) = data.get("message") {
                if let Some(usage) = message.get("usage") {
                    let count = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
                    tokens.input += count("input_tokens");
                    tokens.cache_write += count("cache_creation_input_tokens");
                    tokens.cache_read += count("cache_read_input_tokens");
                    tokens.output += count("output_tokens");
                }
                if let Some(name) = message.get("model").and_then(|m| m.as_str())
                    && !name.is_empty()
                {
                    model = Some(name.to_string());
                }
            }
            // Assistant records with no text only called tools; they leave the
            // previous answer standing, which is the readable one anyway.
            if let Some(content) = text_content(&data)
                && !content.is_empty()
            {
                push_turn(&mut turns, Speaker::Claude, clip(&content));
            }
        }
    }

    if message_count == 0 {
        return None;
    }

    let last_timestamp = last_timestamp.unwrap_or_else(|| {
        file_path
            .metadata()
            .and_then(|m| m.modified())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| Utc::now())
    });

    let title = custom_title
        .clone()
        .or(ai_title)
        .unwrap_or(first_prompt)
        .trim()
        .to_string();

    Some(ClaudeSession {
        id: session_id,
        title: if title.is_empty() {
            "Untitled session".to_string()
        } else {
            title
        },
        custom_title,
        recent_turns: turns,
        last_timestamp,
        message_count,
        git_branch,
        tokens,
        model,
    })
}

/// Name a session by appending the same records Claude's own `/rename` writes.
///
/// The parser takes the last `custom-title` record stamped with the file's own
/// session id, so appending is the whole operation — nothing is rewritten, and
/// a transcript this large is never read in. Callers only use this on sessions
/// with no live window: Claude holds a live transcript open and interleaving
/// appends with its writer is not a race worth having when renaming the tmux
/// window does the same job through the sync plugin.
///
/// Claude keeps a session's name in *two* record types and a runtime rename
/// writes them as a pair: `custom-title` is the session title (the resume
/// picker, hook-input `session_title`), `agent-name` is what the badge in the
/// prompt box draws. Appending only the title looked renamed everywhere except
/// the resumed UI itself — and the sync plugin could not heal it, because the
/// title it compares against the window was already correct, so it never
/// pushed the runtime rename that would have rewritten the agent name.
pub fn rename_session(path: &str, session_id: &str, name: &str) -> Result<(), String> {
    let file = claude_projects_dir()
        .join(path_to_claude_folder(path))
        .join(format!("{session_id}.jsonl"));
    rename_transcript(&file, session_id, name)
}

/// File-scoped form of [`rename_session`], for testing.
pub fn rename_transcript(file: &Path, session_id: &str, name: &str) -> Result<(), String> {
    if !file.exists() {
        return Err("Session transcript no longer exists".to_string());
    }
    let title = serde_json::json!({
        "type": "custom-title",
        "customTitle": name,
        "sessionId": session_id,
    });
    let agent_name = serde_json::json!({
        "type": "agent-name",
        "agentName": name,
        "sessionId": session_id,
    });
    use std::io::{Read, Seek, SeekFrom, Write};
    std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(file)
        .and_then(|mut f| {
            // A transcript normally ends in a newline, but a torn tail (a
            // crashed writer) must not swallow the rename: appended straight
            // onto an unterminated line, the first record and that line both
            // stop parsing while the second still lands — the exact
            // title/badge disagreement this function exists to prevent.
            let mut needs_newline = false;
            if f.metadata()?.len() > 0 {
                let mut last = [0u8; 1];
                f.seek(SeekFrom::End(-1))?;
                f.read_exact(&mut last)?;
                needs_newline = last[0] != b'\n';
            }
            let prefix = if needs_newline { "\n" } else { "" };
            f.write_all(format!("{prefix}{title}\n{agent_name}\n").as_bytes())
        })
        .map_err(|error| error.to_string())
}

/// Delete a session's transcript. Permanent — the caller has already asked.
pub fn delete_session(path: &str, session_id: &str) -> Result<(), String> {
    let file = claude_projects_dir()
        .join(path_to_claude_folder(path))
        .join(format!("{session_id}.jsonl"));
    std::fs::remove_file(&file).map_err(|error| error.to_string())
}

/// Best-effort price table, per million tokens (input, output), matched on the
/// model id. Cache writes bill at 1.25× input and cache reads at 0.1×, which
/// has held across every generation so far. Prices drift with releases — that
/// is why the figure renders with a `~` and why an unknown model shows tokens
/// only rather than a made-up dollar amount.
fn model_prices(model: &str) -> Option<(f64, f64)> {
    if model.contains("opus-4-5") || model.contains("opus-5") || model.contains("fable") {
        Some((5.0, 25.0))
    } else if model.contains("opus") {
        Some((15.0, 75.0))
    } else if model.contains("sonnet") {
        Some((3.0, 15.0))
    } else if model.contains("haiku-3") {
        Some((0.8, 4.0))
    } else if model.contains("haiku") {
        Some((1.0, 5.0))
    } else {
        None
    }
}

/// Estimated dollar cost of a transcript, when the model's prices are known.
pub fn cost_estimate(model: Option<&str>, tokens: TokenUsage) -> Option<f64> {
    let (input, output) = model_prices(model?)?;
    let per = 1_000_000.0;
    Some(
        (tokens.input as f64 / per) * input
            + (tokens.cache_write as f64 / per) * input * 1.25
            + (tokens.cache_read as f64 / per) * input * 0.1
            + (tokens.output as f64 / per) * output,
    )
}

/// `999`, `12k`, `1.2M` — a count that fits a card's meta line.
pub fn fmt_tokens(count: u64) -> String {
    match count {
        0..=999 => count.to_string(),
        1_000..=999_499 => format!("{}k", (count + 500) / 1_000),
        _ => {
            let millions = count as f64 / 1_000_000.0;
            if millions >= 10.0 {
                format!("{millions:.0}M")
            } else {
                format!("{millions:.1}M")
            }
        }
    }
}

/// `~$0.42`, `~$123` — always marked as the estimate it is.
pub fn fmt_cost(cost: f64) -> String {
    if cost >= 100.0 {
        format!("~${cost:.0}")
    } else {
        format!("~${cost:.2}")
    }
}

/// Move session history from an old worktree path to a new one after a rename.
pub fn migrate_sessions(old_path: &Path, new_path: &Path) {
    let base = claude_projects_dir();
    let old_dir = base.join(path_to_claude_folder(&old_path.to_string_lossy()));
    let new_dir = base.join(path_to_claude_folder(&new_path.to_string_lossy()));
    migrate_dirs(&old_dir, &new_dir);
}

/// Directory-scoped form of [`migrate_sessions`], for testing.
pub fn migrate_dirs(old_dir: &Path, new_dir: &Path) {
    if !old_dir.exists() {
        return;
    }
    if std::fs::create_dir_all(new_dir).is_err() {
        return;
    }

    if let Ok(entries) = std::fs::read_dir(old_dir) {
        for entry in entries.flatten() {
            let src = entry.path();
            if src.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(name) = src.file_name() else {
                continue;
            };
            let dest = new_dir.join(name);
            if !dest.exists() {
                let _ = std::fs::rename(&src, &dest);
            }
        }
    }

    // Remove the old directory once it is empty.
    if let Ok(mut remaining) = std::fs::read_dir(old_dir)
        && remaining.next().is_none()
    {
        let _ = std::fs::remove_dir(old_dir);
    }
}

#[cfg(test)]
mod tests;
