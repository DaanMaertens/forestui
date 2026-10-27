use super::*;

/// The peek is what the panel renders on selection change while the scan
/// re-runs behind it (#29): whatever the last scan produced, instantly.
#[test]
fn peek_returns_the_last_scan() {
    let path = "/nowhere/forestui-peek-test";
    assert!(peek_sessions(path, 5).is_none());
    let scanned = get_sessions_for_path(path, 5);
    let peeked = peek_sessions(path, 5).expect("the scan populates the cache");
    assert_eq!(peeked.len(), scanned.len());
    // A different limit is a different entry, never a poisoned shared one.
    assert!(peek_sessions(path, 50).is_none());
}

#[test]
fn folder_naming_replaces_slashes() {
    let folder = path_to_claude_folder("/tmp");
    assert!(!folder.contains('/'));
    assert!(folder.starts_with('-') || folder.contains("tmp"));
}

/// The session list is only useful if it shows the name the user gave the
/// session. The first prompt is the last resort, not the first choice.
/// `user` is the transport, not the author. A long conversation is mostly
/// tool results — one real session showed 5528 `user` records against 335
/// messages actually sent — and the system injects `user` records too, so
/// the preview could show a background agent reporting in rather than
/// anything the person wrote.
#[test]
fn only_what_the_person_sent_is_counted_or_previewed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sess-turns.jsonl");
    let lines = [
        // Typed.
        r#"{"type":"user","timestamp":"2026-08-01T10:00:00Z","origin":{"kind":"human"},"message":{"content":"first thing I said"}}"#,
        // A tool result: content is a block array, not text.
        r#"{"type":"user","timestamp":"2026-08-01T10:00:01Z","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
        // Injected by the system, not by the person.
        r#"{"type":"user","timestamp":"2026-08-01T10:00:02Z","origin":{"kind":"peer"},"message":{"content":"Background agent finished"}}"#,
        r#"{"type":"user","timestamp":"2026-08-01T10:00:03Z","origin":{"kind":"task-notification"},"message":{"content":"a task completed"}}"#,
        // A reminder the harness wraps in a tag.
        r#"{"type":"user","timestamp":"2026-08-01T10:00:04Z","origin":{"kind":"human"},"message":{"content":"<system-reminder>ignore me</system-reminder>"}}"#,
        // Typed again — this is the last thing the person said.
        r#"{"type":"user","timestamp":"2026-08-01T10:00:05Z","origin":{"kind":"human"},"message":{"content":"last thing I said"}}"#,
        // No origin at all: an older transcript, taken at face value.
        r#"{"type":"user","timestamp":"2026-08-01T10:00:06Z","message":{"content":"from before origin existed"}}"#,
    ];
    std::fs::write(&file, lines.join("\n") + "\n").unwrap();

    let session = parse_session_file(&file).expect("a session");
    assert_eq!(
        session.message_count, 3,
        "counted something the person did not send"
    );
    assert_eq!(
        session.recent_turns.last().map(|t| t.text.as_str()),
        Some("from before origin existed")
    );
    assert_eq!(session.title, "first thing I said");
}

/// The card shows the exchange a conversation stopped on, whichever way
/// round it happens to be — and a single answer split across several
/// `assistant` records is one turn, not several. Real sessions have runs of
/// dozens; counted separately they would fill both lines and hide the
/// question, which is the half that identifies the conversation.
#[test]
fn the_last_two_turns_are_kept_and_same_speaker_runs_collapse() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sess-turns2.jsonl");
    let user = |text: &str| {
        format!(
            r#"{{"type":"user","timestamp":"2026-08-01T10:00:00Z","origin":{{"kind":"human"}},"message":{{"content":"{text}"}}}}"#
        )
    };
    let claude = |text: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-08-01T10:00:00Z","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    };

    let lines = [
            user("an older question"),
            claude("an older answer"),
            user("what is going on?"),
            // One answer, three records: text, a tool call, more text.
            claude("let me look"),
            r#"{"type":"assistant","timestamp":"2026-08-01T10:00:00Z","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#.to_string(),
            claude("here is what I found"),
        ];
    std::fs::write(&file, lines.join("\n") + "\n").unwrap();

    let session = parse_session_file(&file).expect("a session");
    let turns: Vec<_> = session
        .recent_turns
        .iter()
        .map(|t| (t.speaker, t.text.as_str()))
        .collect();
    assert_eq!(
        turns,
        vec![
            (Speaker::User, "what is going on?"),
            (Speaker::Claude, "here is what I found"),
        ]
    );

    // Ending on a question leaves that question as the newest turn, with
    // the answer before it — never an older answer pretending to reply.
    let mut raw = std::fs::read_to_string(&file).unwrap();
    raw.push_str(&user("and one more thing"));
    raw.push('\n');
    std::fs::write(&file, &raw).unwrap();

    let session = parse_session_file(&file).expect("a session");
    let turns: Vec<_> = session
        .recent_turns
        .iter()
        .map(|t| (t.speaker, t.text.as_str()))
        .collect();
    assert_eq!(
        turns,
        vec![
            (Speaker::Claude, "here is what I found"),
            (Speaker::User, "and one more thing"),
        ]
    );
}

/// The scan stops early instead of parsing every transcript. That is only
/// sound because a file's mtime is never older than its last message, so
/// this pins the case where the two disagree: a transcript touched after
/// its last message — which is what naming a session does, since a title
/// record carries no timestamp — must not displace a genuinely newer
/// conversation.
///
/// Measured on a real 542 MB project directory, this took the full scan
/// from 1.9 s to 56 ms cold and 0.2 ms warm, on a sweep that runs every
/// ten seconds.
#[test]
fn the_early_exit_picks_what_a_full_scan_would() {
    use std::time::{Duration, SystemTime};
    let dir = tempfile::tempdir().unwrap();

    // `age` is how old the last message is; `touched` how long ago the file
    // was written. The two disagree on purpose.
    let write = |name: &str, age_h: u64, touched_h: u64| {
        let path = dir.path().join(format!("{name}.jsonl"));
        let when = chrono::Utc::now() - chrono::Duration::hours(age_h as i64);
        std::fs::write(
                &path,
                format!(
                    "{{\"type\":\"user\",\"timestamp\":\"{}\",\"message\":{{\"content\":\"{name}\"}}}}\n",
                    when.to_rfc3339()
                ),
            )
            .unwrap();
        let mtime = SystemTime::now() - Duration::from_secs(touched_h * 3600);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(mtime))
            .unwrap();
    };

    write("newest-message", 1, 1);
    write("touched-but-stale", 40, 0); // freshest file, oldest conversation
    write("second", 2, 2);
    write("third", 3, 3);
    write("oldest", 30, 30);

    let picked: Vec<String> = read_sessions_dir(dir.path(), 3)
        .iter()
        .map(|s| s.id.clone())
        .collect();
    assert_eq!(
        picked,
        vec!["newest-message", "second", "third"],
        "the early exit dropped a session a full scan would have kept"
    );
}

#[test]
fn title_prefers_the_recorded_name_over_the_first_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sess-name.jsonl");
    std::fs::write(
        &file,
        "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
              \"message\":{\"content\":\"please refactor the flaky retry loop\"}}\n\
             {\"type\":\"ai-title\",\"aiTitle\":\"Refactor the flaky retry loop\",\
              \"sessionId\":\"sess-name\"}\n",
    )
    .unwrap();

    let ai = parse_session_file(&file).expect("a session with an ai title");
    assert_eq!(ai.title, "Refactor the flaky retry loop");
    assert_eq!(ai.custom_title, None, "an ai title is not a chosen name");

    // A name the user chose outranks the generated one.
    let mut raw = std::fs::read_to_string(&file).unwrap();
    raw.push_str(
        "{\"type\":\"custom-title\",\"customTitle\":\"retry loop\",\
              \"sessionId\":\"sess-name\"}\n",
    );
    std::fs::write(&file, &raw).unwrap();

    let named = parse_session_file(&file).expect("a named session");
    assert_eq!(named.title, "retry loop");
    assert_eq!(named.custom_title.as_deref(), Some("retry loop"));
}

/// A fork copies the parent's transcript verbatim, parent title records
/// included. Adopting those would name every fork after its parent.
#[test]
fn a_forked_transcript_does_not_inherit_the_parent_name() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("the-fork.jsonl");
    std::fs::write(
        &file,
        "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
              \"message\":{\"content\":\"carried over from the parent\"}}\n\
             {\"type\":\"custom-title\",\"customTitle\":\"the parent name\",\
              \"sessionId\":\"the-parent\"}\n",
    )
    .unwrap();

    let forked = parse_session_file(&file).expect("a forked session");
    assert_eq!(forked.custom_title, None);
    assert_eq!(forked.title, "carried over from the parent");
}

#[test]
fn parses_a_session_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sess-1.jsonl");
    std::fs::write(
        &file,
        "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
              \"message\":{\"content\":\"first question\"},\"gitBranches\":[\"main\"]}\n\
             {\"type\":\"assistant\",\"timestamp\":\"2026-08-01T10:00:05Z\"}\n\
             {\"type\":\"user\",\"timestamp\":\"2026-08-01T10:01:00Z\",\
              \"message\":{\"content\":[{\"type\":\"text\",\"text\":\"second\"}]}}\n",
    )
    .unwrap();

    let session = parse_session_file(&file).unwrap();
    assert_eq!(session.id, "sess-1");
    assert_eq!(session.title, "first question");
    assert_eq!(
        session.recent_turns.last().map(|t| t.text.as_str()),
        Some("second")
    );
    assert_eq!(session.message_count, 2);
}

#[test]
fn skips_agent_files_and_empty_sessions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("agent-x.jsonl"),
        "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("empty.jsonl"), "{\"type\":\"system\"}\n").unwrap();
    assert!(read_sessions_dir(dir.path(), 5).is_empty());
}

#[test]
fn sessions_sorted_newest_first_and_limited() {
    let dir = tempfile::tempdir().unwrap();
    for (i, ts) in [
        "2026-08-01T10:00:00Z",
        "2026-08-03T10:00:00Z",
        "2026-08-02T10:00:00Z",
    ]
    .iter()
    .enumerate()
    {
        std::fs::write(
            dir.path().join(format!("s{i}.jsonl")),
            format!(
                "{{\"type\":\"user\",\"timestamp\":\"{ts}\",\"message\":{{\"content\":\"q\"}}}}\n"
            ),
        )
        .unwrap();
    }
    let sessions = read_sessions_dir(dir.path(), 2);
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].id, "s1");
    assert_eq!(sessions[1].id, "s2");
}

/// Tokens, model and branch ride the same parse as everything else — the
/// card's recognition line costs no extra read of a multi-MB transcript.
#[test]
fn usage_branch_and_model_are_summed_from_the_records() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sess-usage.jsonl");
    let lines = [
        r#"{"type":"user","timestamp":"2026-08-01T10:00:00Z","gitBranch":"main","message":{"content":"question"}}"#,
        r#"{"type":"assistant","timestamp":"2026-08-01T10:00:01Z","gitBranch":"main","message":{"model":"claude-opus-5","usage":{"input_tokens":10,"cache_creation_input_tokens":1000,"cache_read_input_tokens":0,"output_tokens":50}}}"#,
        // A mid-session checkout: the last branch wins.
        r#"{"type":"assistant","timestamp":"2026-08-01T10:00:02Z","gitBranch":"feat/x","message":{"model":"claude-opus-5","usage":{"input_tokens":5,"cache_creation_input_tokens":0,"cache_read_input_tokens":2000,"output_tokens":25}}}"#,
    ];
    std::fs::write(&file, lines.join("\n") + "\n").unwrap();

    let session = parse_session_file(&file).expect("a session");
    assert_eq!(session.git_branch.as_deref(), Some("feat/x"));
    assert_eq!(session.model.as_deref(), Some("claude-opus-5"));
    assert_eq!(session.tokens.input, 15);
    assert_eq!(session.tokens.cache_write, 1000);
    assert_eq!(session.tokens.cache_read, 2000);
    assert_eq!(session.tokens.output, 75);
    assert_eq!(session.tokens.total_in(), 3015);

    // An old minimal transcript has none of this, and must not pretend to.
    let bare = dir.path().join("sess-bare.jsonl");
    std::fs::write(
        &bare,
        "{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n",
    )
    .unwrap();
    let session = parse_session_file(&bare).expect("a session");
    assert!(session.tokens.is_zero());
    assert_eq!(session.git_branch, None);
    assert_eq!(session.model, None);
}

/// The estimate exists for known models only: a made-up price is worse
/// than showing tokens alone.
#[test]
fn cost_is_estimated_for_known_models_only() {
    let tokens = TokenUsage {
        input: 1_000_000,
        cache_write: 0,
        cache_read: 0,
        output: 1_000_000,
    };
    let opus5 = cost_estimate(Some("claude-opus-5"), tokens).expect("a known model");
    assert!((opus5 - 30.0).abs() < 0.001, "5 in + 25 out, got {opus5}");
    assert!(cost_estimate(Some("some-future-model"), tokens).is_none());
    assert!(cost_estimate(None, tokens).is_none());

    // Cache tiers price off the input rate.
    let cached = TokenUsage {
        input: 0,
        cache_write: 1_000_000,
        cache_read: 1_000_000,
        output: 0,
    };
    let cost = cost_estimate(Some("claude-sonnet-5"), cached).expect("sonnet is known");
    assert!((cost - (3.0 * 1.25 + 3.0 * 0.1)).abs() < 0.001, "{cost}");
}

#[test]
fn token_and_cost_formatting() {
    assert_eq!(fmt_tokens(0), "0");
    assert_eq!(fmt_tokens(999), "999");
    assert_eq!(fmt_tokens(1_000), "1k");
    assert_eq!(fmt_tokens(12_499), "12k");
    assert_eq!(fmt_tokens(999_499), "999k");
    assert_eq!(fmt_tokens(1_200_000), "1.2M");
    assert_eq!(fmt_tokens(14_700_000), "15M");
    assert_eq!(fmt_cost(0.416), "~$0.42");
    assert_eq!(fmt_cost(123.4), "~$123");
}

/// Renaming a stopped session appends the records `/rename` writes — the
/// `custom-title`/`agent-name` pair — so the next parse sees the new name
/// and a resumed Claude draws it in its badge, which renders the agent
/// name, not the title. An empty transcript path errors instead of
/// creating a stray file.
#[test]
fn renaming_a_stopped_session_appends_a_title_record() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sess-r.jsonl");
    std::fs::write(
        &file,
        "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
              \"message\":{\"content\":\"hello\"}}\n",
    )
    .unwrap();

    rename_transcript(&file, "sess-r", "my rename").expect("the append lands");
    let session = parse_session_file(&file).expect("a session");
    assert_eq!(session.custom_title.as_deref(), Some("my rename"));
    assert_eq!(session.title, "my rename");

    // Both records land, stamped with the session id, each a valid JSON
    // line — a torn or concatenated line would be skipped by every parser.
    let raw = std::fs::read_to_string(&file).unwrap();
    let records: Vec<serde_json::Value> = raw
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).expect("each appended line parses alone"))
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["type"], "custom-title");
    assert_eq!(records[0]["customTitle"], "my rename");
    assert_eq!(records[1]["type"], "agent-name");
    assert_eq!(records[1]["agentName"], "my rename");
    for record in &records {
        assert_eq!(record["sessionId"], "sess-r");
    }

    // A missing transcript is an error, not a new file.
    assert!(rename_session("/nowhere/forestui-rename-test", "ghost", "x").is_err());
    assert!(delete_session("/nowhere/forestui-rename-test", "ghost").is_err());
}

/// A transcript whose last line was torn mid-write (no trailing newline)
/// still takes the rename: appending blindly would fuse the first record
/// onto the torn line and lose it, leaving the pair half-applied.
#[test]
fn renaming_survives_a_transcript_with_a_torn_tail() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sess-t.jsonl");
    std::fs::write(
        &file,
        "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
              \"message\":{\"content\":\"hello\"}}\n\
             {\"type\":\"assistant\",\"truncated",
    )
    .unwrap();

    rename_transcript(&file, "sess-t", "after the tear").expect("the append lands");
    let session = parse_session_file(&file).expect("a session");
    assert_eq!(session.custom_title.as_deref(), Some("after the tear"));

    let raw = std::fs::read_to_string(&file).unwrap();
    let tail: Vec<&str> = raw.lines().rev().take(2).collect();
    assert!(
        tail[1].starts_with("{\"customTitle\""),
        "title on its own line"
    );
    assert!(
        tail[0].starts_with("{\"agentName\""),
        "agent name on its own line"
    );
}

/// Pins outrank recency, in pin order; everything else stays newest-first.
/// A pinned id that no longer parses to a session simply is not there.
#[test]
fn pinned_sessions_lead_in_pin_order() {
    let mk = |id: &str, hours_ago: i64| ClaudeSession {
        id: id.into(),
        title: id.into(),
        custom_title: None,
        recent_turns: Vec::new(),
        last_timestamp: chrono::Utc::now() - chrono::Duration::hours(hours_ago),
        message_count: 1,
        git_branch: None,
        tokens: TokenUsage::default(),
        model: None,
        continued_in: None,
    };
    let mut sessions = vec![mk("new", 1), mk("mid", 5), mk("old", 20), mk("ancient", 90)];
    let pinned = vec!["old".to_string(), "mid".to_string()];
    sort_sessions(&mut sessions, &pinned);
    let order: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(order, vec!["old", "mid", "new", "ancient"]);

    // No pins: pure recency, unchanged from before pins existed.
    sort_sessions(&mut sessions, &[]);
    let order: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(order, vec!["new", "mid", "old", "ancient"]);
}

/// The recency cap must not hide a pinned session: pinning is precisely a
/// claim against recency.
#[test]
fn a_pinned_session_survives_the_recency_cap() {
    let dir = tempfile::tempdir().unwrap();
    for (name, ts) in [
        ("a", "2026-08-05T10:00:00Z"),
        ("b", "2026-08-04T10:00:00Z"),
        ("c", "2026-08-03T10:00:00Z"),
    ] {
        std::fs::write(
            dir.path().join(format!("{name}.jsonl")),
            format!(
                "{{\"type\":\"user\",\"timestamp\":\"{ts}\",\"message\":{{\"content\":\"q\"}}}}\n"
            ),
        )
        .unwrap();
    }
    // Capped at 2, the oldest falls out…
    let ids: Vec<String> = read_sessions_dir(dir.path(), 2)
        .iter()
        .map(|s| s.id.clone())
        .collect();
    assert_eq!(ids, vec!["a", "b"]);
    // …but read_sessions_dir has no pin knowledge; the pin merge lives in
    // get_sessions_with_pins, which needs the real projects dir. Its logic
    // — append what refresh_one finds, then sort — is exercised through
    // sort order here: a pinned "c" would lead.
    let mut sessions = read_sessions_dir(dir.path(), 3);
    sort_sessions(&mut sessions, &["c".to_string()]);
    assert_eq!(sessions[0].id, "c");
}

#[test]
fn migration_moves_files_and_removes_empty_dir() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("old");
    let new = root.path().join("new");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("a.jsonl"), "{}").unwrap();

    migrate_dirs(&old, &new);
    assert!(new.join("a.jsonl").exists());
    assert!(!old.exists());
}

/// A hand-off to a background session (← in Claude Code) ends the old
/// transcript with a `continued-in` record naming the successor. A fork
/// copies its parent's records verbatim, so a `continued-in` stamped with
/// another session's id is the parent's hand-off, not this file's.
#[test]
fn a_transcript_names_its_own_continuation_only() {
    let dir = tempfile::tempdir().unwrap();
    let handed_off = dir.path().join("old-id.jsonl");
    std::fs::write(
        &handed_off,
        "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
              \"message\":{\"content\":\"keep going in the background\"}}\n\
             {\"type\":\"continued-in\",\"sessionId\":\"old-id\",\
              \"continuedInSessionId\":\"new-id\",\"timestamp\":\"2026-08-01T10:01:00Z\"}\n",
    )
    .unwrap();
    let session = parse_session_file(&handed_off).expect("a handed-off session");
    assert_eq!(session.continued_in.as_deref(), Some("new-id"));

    let fork = dir.path().join("the-fork.jsonl");
    std::fs::write(
        &fork,
        "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
              \"message\":{\"content\":\"carried over from the parent\"}}\n\
             {\"type\":\"continued-in\",\"sessionId\":\"the-parent\",\
              \"continuedInSessionId\":\"elsewhere\"}\n",
    )
    .unwrap();
    let forked = parse_session_file(&fork).expect("a forked session");
    assert_eq!(forked.continued_in, None);
}

/// The live scan asks on every sweep, while the transcript keeps growing.
/// A hand-off appended after an earlier read is still found, wherever the
/// transcript's project directory is, and a fork's copied record is not.
#[test]
fn continuations_follow_a_growing_transcript_in_any_project() {
    let projects = tempfile::tempdir().unwrap();
    let dir = projects.path().join("-work-wt");
    std::fs::create_dir(&dir).unwrap();
    let live = dir.join("old-id.jsonl");
    std::fs::write(&live, "{\"type\":\"user\",\"sessionId\":\"old-id\"}\n").unwrap();
    std::fs::write(
        dir.join("the-fork.jsonl"),
        "{\"type\":\"continued-in\",\"sessionId\":\"old-id\",\"continuedInSessionId\":\"new-id\"}\n",
    )
    .unwrap();
    let ids = [
        "old-id".to_string(),
        "the-fork".to_string(),
        "missing".to_string(),
    ];
    assert!(continuations_in(projects.path(), &ids).is_empty());

    let mut raw = std::fs::read_to_string(&live).unwrap();
    raw.push_str("{\"type\":\"continued-in\",\"sessionId\":\"old-id\",\"continuedInSessionId\":\"new-id\"}\n");
    std::fs::write(&live, raw).unwrap();
    assert_eq!(
        continuations_in(projects.path(), &ids),
        HashMap::from([("old-id".to_string(), "new-id".to_string())])
    );
}

/// A hand-off leaves two transcripts with the same name; only the successor
/// is the conversation now, so only it is a card.
#[test]
fn a_handed_off_session_gets_no_card() {
    let dir = tempfile::tempdir().unwrap();
    let user = "{\"type\":\"user\",\"timestamp\":\"2026-08-01T10:00:00Z\",\
                \"message\":{\"content\":\"the conversation\"}}\n";
    std::fs::write(
        dir.path().join("old-id.jsonl"),
        format!(
            "{user}{{\"type\":\"continued-in\",\"sessionId\":\"old-id\",\
             \"continuedInSessionId\":\"new-id\"}}\n"
        ),
    )
    .unwrap();
    std::fs::write(dir.path().join("new-id.jsonl"), user).unwrap();

    let ids: Vec<String> = read_sessions_dir(dir.path(), 5)
        .into_iter()
        .map(|session| session.id)
        .collect();
    assert_eq!(ids, vec!["new-id".to_string()]);
}
