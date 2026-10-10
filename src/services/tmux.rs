//! tmux integration.
//!
//! Every call shells out to the `tmux` binary. The commands are short-lived and
//! synchronous — the same blocking behaviour the libtmux-based build had.

use std::path::{Path, PathBuf};
use std::process::Command;

pub const TUI_EDITORS: [&str; 10] = [
    "vim", "nvim", "vi", "emacs", "nano", "helix", "hx", "micro", "kakoune", "kak",
];

/// Are we running inside a tmux session?
pub fn is_inside_tmux() -> bool {
    std::env::var("TMUX").is_ok_and(|v| !v.is_empty())
}

/// Is this editor a TUI editor that should be opened in a tmux window?
pub fn is_tui_editor(editor: &str) -> bool {
    let base = editor.split_whitespace().next().unwrap_or("");
    TUI_EDITORS.contains(&base)
}

fn tmux(args: &[&str]) -> Option<String> {
    // Unit tests must never reach a real tmux server — the developer working
    // on forestui is, almost by definition, running forestui, and the test
    // binary inherits their `TMUX_PANE`. One test whose action path reached
    // `new-window` opened a real window in a real session; this gate makes
    // that whole class impossible instead of relying on every future test to
    // stay away from the launch paths. Under the gate tmux reads and writes
    // both answer as they do outside tmux entirely.
    if cfg!(test) {
        return None;
    }
    let output = Command::new("tmux").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

/// The tmux session forestui creates its windows in.
///
/// Resolved from `TMUX_PANE`, which names this process's own pane and so is
/// exact. This used to ask `list-clients` for the most recently active client
/// on the server, filtered by session group — but a session `ensure_tmux`
/// creates on first launch is not in a group at all (the grouped session is
/// only made when the base session already existed), so `session_group` came
/// back empty, the filter went inert, and the scan returned whichever session
/// the user had touched last. A second forestui, opened on a different forest,
/// therefore created its windows in the first one's session.
///
/// Sessions in a group genuinely share their windows, so there any member is a
/// correct target; the activity scan still runs in that case, so the window
/// opens in whichever terminal the user is actually looking at.
///
/// There is deliberately no fallback for a missing `TMUX_PANE`. The previous
/// implementation ended in "first attached session, else any session", and
/// guessing is what put windows in the wrong forestui: a caller that cannot
/// identify its own pane cannot identify its own session either, and failing
/// the action visibly beats acting on somebody else's session.
pub fn current_session() -> Option<String> {
    if !is_inside_tmux() {
        return None;
    }

    let pane = std::env::var("TMUX_PANE").ok()?;
    let own = tmux(&["display-message", "-p", "-t", &pane, "#{session_id}"])?
        .trim()
        .to_string();
    if own.is_empty() {
        return None;
    }

    let group = tmux(&["display-message", "-p", "-t", &pane, "#{session_group}"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if group.is_empty() {
        return Some(own);
    }

    let clients = tmux(&[
        "list-clients",
        "-F",
        "#{client_activity} #{session_id} #{session_group}",
    ])
    .unwrap_or_default();
    Some(most_active_in_group(&clients, &group, &own))
}

/// Of the clients attached to our own session group, the session of the most
/// recently active one — falling back to our own session. Pure, for testing.
fn most_active_in_group(clients: &str, group: &str, own: &str) -> String {
    let mut best = own.to_string();
    let mut best_activity = i64::MIN;
    for line in clients.lines() {
        let parts: Vec<&str> = line.trim().splitn(3, ' ').collect();
        if parts.len() != 3 || parts[2] != group {
            continue;
        }
        let Ok(activity) = parts[0].parse::<i64>() else {
            continue;
        };
        if activity > best_activity {
            best_activity = activity;
            best = parts[1].to_string();
        }
    }
    best
}

/// The tmux window this process is running in.
///
/// Resolved from `TMUX_PANE` rather than the session's active window, which may
/// be a different window while forestui is starting up in the background.
pub fn current_window() -> Option<String> {
    if !is_inside_tmux() {
        return None;
    }
    let pane = std::env::var("TMUX_PANE").ok()?;
    tmux(&["display-message", "-p", "-t", &pane, "#{window_id}"]).map(|s| s.trim().to_string())
}

/// Rename the window forestui itself runs in.
pub fn rename_window(name: &str) -> bool {
    let Some(window) = current_window() else {
        return false;
    };
    tmux(&["rename-window", "-t", &window, name]).is_some()
}

/// Rename an arbitrary window by id.
///
/// This is how a *live* session is renamed: the title-sync plugin sees the tab
/// move and adopts the new name into the session on its next hook, exactly as
/// if the user had renamed the tab by hand. Nothing touches the transcript —
/// Claude holds it open, and the plugin path is the one that already works.
pub fn rename_window_by_id(window_id: &str, name: &str) -> bool {
    tmux(&["rename-window", "-t", window_id, name]).is_some()
}

/// Jump to a window, for the duplicate-open guard's "switch there" choice.
pub fn select_window(window_id: &str) -> bool {
    tmux(&["select-window", "-t", window_id]).is_some()
}

/// The window holding a pane, and the tmux session that owns it.
///
/// This is how a heartbeat's recorded `TMUX_PANE` resolves back to something
/// forestui can offer to switch to — but only when the pane's session is
/// forestui's own, which is why the session rides along for the caller to
/// compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneWindow {
    pub tmux_session: String,
    pub window_id: String,
    pub window_name: String,
}

pub fn pane_window(pane: &str) -> Option<PaneWindow> {
    let out = tmux(&[
        "display-message",
        "-p",
        "-t",
        pane,
        "#{session_id}\t#{window_id}\t#{window_name}",
    ])?;
    let line = out.trim_end_matches('\n');
    let mut parts = line.splitn(3, '\t');
    Some(PaneWindow {
        tmux_session: parts.next()?.to_string(),
        window_id: parts.next()?.to_string(),
        window_name: parts.next()?.to_string(),
    })
}

/// What `ensure_focus_events` found, and therefore what exit has to undo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusEvents {
    /// Not inside tmux, or tmux refused the option.
    Unavailable,
    /// Someone else's setting — leave it exactly as it is.
    AlreadyOn,
    /// Ours to undo, either because we just turned it on or because a previous
    /// forestui did and died before it could.
    Ours,
}

/// Marks `focus-events` as forestui's doing, so the option can be handed back
/// even when the run that set it never reached its cleanup.
///
/// Without it, one panic or `SIGTERM` would leave the option on forever: every
/// later run would find it already on, conclude it was the user's, and refuse
/// to touch it. A tmux user option is the natural place for the flag — it
/// lives exactly as long as the server whose setting it describes.
const FOCUS_EVENTS_MARKER: &str = "@forestui_focus_events";

/// Enable the tmux `focus-events` option so the app can refresh on focus.
///
/// It is a **server** option: turning it on reaches every session, window and
/// client on that tmux server, and nothing turns it off again on its own. That
/// is not free — while it is on, tmux asks the terminal for focus reporting,
/// and a focus report that arrives between the prefix key and the next key
/// cancels the tmux command (issue #51). So this reports whether the option is
/// ours to undo, and [`disable_focus_events`] hands it back.
pub fn ensure_focus_events() -> FocusEvents {
    if !is_inside_tmux() {
        return FocusEvents::Unavailable;
    }

    if tmux(&["show-options", "-gv", "focus-events"]).is_some_and(|v| v.trim() == "on") {
        return if marked_as_ours() {
            FocusEvents::Ours
        } else {
            FocusEvents::AlreadyOn
        };
    }

    if tmux(&["set-option", "-g", "focus-events", "on"]).is_some() {
        let _ = tmux(&["set-option", "-g", FOCUS_EVENTS_MARKER, "on"]);
        FocusEvents::Ours
    } else {
        FocusEvents::Unavailable
    }
}

/// Put `focus-events` back to off, for the run that owns it.
///
/// Only called for [`FocusEvents::Ours`], so a user who set the option
/// themselves keeps it. Two forestui instances can still disagree — the first
/// to exit turns it off under the second, which costs that instance its
/// refresh-on-return until it restarts. That is the cheaper of the two
/// mistakes: the other one is changing the user's server permanently and never
/// telling them.
pub fn disable_focus_events() {
    if !is_inside_tmux() {
        return;
    }
    let _ = tmux(&["set-option", "-g", "focus-events", "off"]);
    let _ = tmux(&["set-option", "-gu", FOCUS_EVENTS_MARKER]);
}

/// Hand back a `focus-events` a previous forestui stranded, for a run that
/// wants nothing to do with it.
///
/// `--no-focus-events` is what someone reaches for when focus reporting is
/// costing them a tmux prefix, so it has to clear the setting a crashed run
/// left behind rather than only declining to add one.
pub fn release_stranded_focus_events() {
    if is_inside_tmux() && marked_as_ours() {
        disable_focus_events();
    }
}

/// Whether a forestui — this one or one that died before cleaning up — is what
/// turned `focus-events` on.
fn marked_as_ours() -> bool {
    // An unset user option is an error, not an empty value, so a failed call
    // is the "not ours" answer rather than something to report.
    tmux(&["show-options", "-gv", FOCUS_EVENTS_MARKER]).is_some_and(|v| v.trim() == "on")
}

fn window_names() -> Vec<String> {
    let Some(session) = current_session() else {
        return Vec::new();
    };
    tmux(&["list-windows", "-t", &session, "-F", "#{window_name}"])
        .map(|out| out.lines().map(|l| l.trim().to_string()).collect())
        .unwrap_or_default()
}

/// Find a window by name in the current session.
pub fn find_window(name: &str) -> Option<String> {
    let session = current_session()?;
    let out = tmux(&[
        "list-windows",
        "-t",
        &session,
        "-F",
        "#{window_id} #{window_name}",
    ])?;
    for line in out.lines() {
        if let Some((id, wname)) = line.trim().split_once(' ')
            && wname == name
        {
            return Some(id.to_string());
        }
    }
    None
}

/// Record the name a window was born with.
///
/// The title-sync hook reads this to answer one question: did forestui open
/// this window? A window the user made by hand carries no stamp and is left
/// alone. Stamped whether or not the plugin is installed: it is one string on a
/// window that tmux discards when the window closes, and writing it eagerly
/// means installing the plugin later also works for windows already open.
pub fn stamp_birth_name(window_id: &str, window_name: &str) -> bool {
    tmux(&[
        "set-option",
        "-w",
        "-t",
        window_id,
        "@claude_birth_name",
        window_name,
    ])
    .is_some()
}

/// Record which Claude session a window holds.
///
/// Written for every Claude window forestui opens — fresh sessions carry the
/// pre-minted `--session-id`, resumed ones the id being resumed — so window ↔
/// session becomes a recorded fact instead of a guess from mtimes. Everything
/// the live badge, the duplicate-open guard and the pane peek know starts here.
pub fn stamp_session_id(window_id: &str, session_id: &str) -> bool {
    tmux(&[
        "set-option",
        "-w",
        "-t",
        window_id,
        "@claude_session_id",
        session_id,
    ])
    .is_some()
}

/// A window forestui opened for Claude, as the live scan sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeWindow {
    pub window_id: String,
    pub window_name: String,
    /// Whether the pane's foreground process is still Claude (or anything that
    /// is not a shell). `false` means the window is open but Claude exited or
    /// was suspended — the shell prompt is what is running.
    pub running: bool,
    pub session_id: String,
}

/// Shells a Claude window can drop back to. A pane whose foreground command is
/// one of these holds no running Claude — see [`ClaudeWindow::running`].
const SHELLS: [&str; 13] = [
    "zsh", "bash", "sh", "dash", "ash", "ksh", "ksh93", "mksh", "loksh", "yash", "fish", "nu",
    "nushell",
];

/// Every window on our session that carries a `@claude_session_id` stamp.
///
/// One tmux round trip for the whole session, not one per window: this runs on
/// the same cadence as the session refresh and has to stay that cheap.
pub fn list_claude_windows() -> Vec<ClaudeWindow> {
    let Some(session) = current_session() else {
        return Vec::new();
    };
    let Some(out) = tmux(&[
        "list-windows",
        "-t",
        &session,
        "-F",
        "#{window_id}\t#{pane_current_command}\t#{@claude_session_id}\t#{window_name}",
    ]) else {
        return Vec::new();
    };
    parse_claude_windows(&out)
}

/// Pure form of [`list_claude_windows`], for testing.
fn parse_claude_windows(out: &str) -> Vec<ClaudeWindow> {
    let mut windows = Vec::new();
    for line in out.lines() {
        let mut parts = line.splitn(4, '\t');
        let (Some(id), Some(command), Some(session_id), Some(name)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        // Windows without the stamp are not Claude windows forestui opened.
        if session_id.is_empty() {
            continue;
        }
        windows.push(ClaudeWindow {
            window_id: id.to_string(),
            window_name: name.to_string(),
            running: !SHELLS.contains(&command),
            session_id: session_id.to_string(),
        });
    }
    windows
}

/// Append `:2`, `:3`, … until the window name is unused.
pub fn find_unique_window_name(base_name: &str) -> String {
    unique_name_among(base_name, &window_names())
}

/// Pure form of [`find_unique_window_name`], for testing.
pub fn unique_name_among(base_name: &str, existing: &[String]) -> String {
    if !existing.iter().any(|n| n == base_name) {
        return base_name.to_string();
    }
    let mut counter = 2;
    loop {
        let candidate = format!("{base_name}:{counter}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        counter += 1;
    }
}

/// Create a window and select it. `shell_command` runs instead of a shell, so
/// the window closes when the command exits.
/// Create a window and return its id.
///
/// The id, not the name: a name is only unique at the moment it is chosen, and
/// the title-sync hook renames windows behind us, so anything that acts on the
/// window afterwards has to hold something that cannot come to mean a
/// different window.
fn new_window(name: &str, path: &str, shell_command: Option<&str>) -> Option<String> {
    let session = current_session()?;
    let mut args: Vec<&str> = vec![
        "new-window",
        "-t",
        &session,
        "-n",
        name,
        "-c",
        path,
        "-P",
        "-F",
        "#{window_id}",
    ];
    if let Some(cmd) = shell_command {
        args.push(cmd);
    }
    let id = tmux(&args)?.trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// Open the editor in a tmux window named `edit:<worktree_name>`, reusing an
/// existing window with that name when one is already open.
pub fn create_editor_window(worktree_name: &str, worktree_path: &str, editor: &str) -> bool {
    if current_session().is_none() {
        return false;
    }
    let window_name = format!("edit:{worktree_name}");

    if let Some(existing) = find_window(&window_name) {
        return tmux(&["select-window", "-t", &existing]).is_some();
    }
    new_window(&window_name, worktree_path, Some(&format!("{editor} ."))).is_some()
}

/// Open `file` in `editor` in a popup over forestui's own pane, returning once
/// the editor exits.
///
/// A popup rather than a window: `display-popup -E` blocks its caller until
/// the popup closes, which is the "editor closed, read the file" signal a
/// window does not give, and it keeps the confirm screen in view around the
/// edit. Call it off the UI thread.
pub fn edit_in_popup(editor: &str, file: &Path) -> Result<(), String> {
    let pane = std::env::var("TMUX_PANE")
        .ok()
        .filter(|pane| !pane.is_empty())
        .ok_or("forestui is not running inside tmux")?;
    // The editor is left unquoted, as for the editor window: it may carry
    // its own arguments (`code --wait`).
    let command = format!("{editor} {}", sh_quote(&file.to_string_lossy()));
    tmux(&[
        "display-popup",
        "-E",
        "-w",
        "90%",
        "-h",
        "90%",
        "-T",
        " Edit prompt ",
        "-t",
        &pane,
        &command,
    ])
    .map(|_| ())
    .ok_or_else(|| format!("the popup failed or '{editor}' exited with an error"))
}

/// Create a shell window named `term:<name>` (always a new window).
pub fn create_shell_window(name: &str, path: &str) -> bool {
    if current_session().is_none() {
        return false;
    }
    let window_name = find_unique_window_name(&format!("term:{name}"));
    new_window(&window_name, path, None).is_some()
}

/// Create a Midnight Commander window named `files:<name>` (always new).
pub fn create_mc_window(name: &str, path: &str) -> bool {
    if current_session().is_none() {
        return false;
    }
    let window_name = find_unique_window_name(&format!("files:{name}"));
    new_window(&window_name, path, Some("mc")).is_some()
}

/// A string the shell will read back verbatim, whatever is in it.
///
/// Single quotes are the only shell quoting with no escapes inside them: no
/// `$`, no backtick, and — since this is typed into the window's *interactive*
/// shell — no `!` history expansion either, which double quotes would not stop
/// in zsh. This matters because window names now come from session titles, and
/// a session title can be set by a `SessionStart` hook in a repository's own
/// `.claude/settings.json`, so it is not the user's own text by the time it
/// reaches here.
fn sh_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Everything that is not text, removed.
///
/// The line ends up as one line of a file the shell reads, so a control
/// character in it is not a character: a newline inside a window name would
/// end the line and leave the rest standing as a command of its own. Names
/// arrive from session titles, which are not the user's own text, so they are
/// stripped here as well as by the hook that adopts them.
fn single_line(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// Whether a custom command turns Remote Control on by itself — by the long
/// name or by Claude's `--rc` alias, bare or with `=name`. Adding ours on top
/// is not harmless: Claude reads `--remote-control` ahead of `--rc`, so a
/// button's `--rc 'Build box'` would lose its name to our bare flag.
///
/// Whole words, not substrings: `--remote-control-session-name-prefix` only
/// names remote sessions, and a button using it alone still needs the flag.
fn asks_for_remote_control(command: &str) -> bool {
    command.split_whitespace().any(|word| {
        ["--remote-control", "--rc"].iter().any(|flag| {
            word == *flag || word.strip_prefix(flag).is_some_and(|v| v.starts_with('='))
        })
    })
}

/// What a Claude launch asks for, apart from the window it lands in.
///
/// One value rather than a row of positional arguments: most of these are
/// `bool` or `Option<&str>`, and a row of them is one transposition away from
/// a YOLO launch nobody asked for.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeLaunch<'a> {
    /// The session to resume; `None` starts a fresh one.
    pub resume_session_id: Option<&'a str>,
    /// The built-in YOLO button. Ignored for a custom button.
    pub yolo: bool,
    /// A custom button's command, in place of `claude`.
    pub custom_command: Option<&'a str>,
    /// A custom button's window-name prefix.
    pub custom_prefix: Option<&'a str>,
    /// Start with Remote Control on (the Settings checkbox).
    pub remote_control: bool,
    /// The session's first message, sent as Claude's positional prompt.
    pub first_prompt: Option<&'a str>,
}

/// The command line a Claude window runs.
///
/// It runs as a job of the window's own interactive shell rather than as the
/// window's command, and that is the point. A window whose only process is
/// Claude has nothing underneath it: Ctrl-C leaves a dead pane, and Claude's
/// own Ctrl-Z suspend stops the process with no shell left to `fg` it back —
/// the session is unreachable and the window is closed to be rid of it.
/// Running Claude as a job makes both land at a prompt, where `fg` resumes and
/// the up arrow restarts.
pub fn claude_command_line(
    launch: &ClaudeLaunch<'_>,
    window_name: &str,
    new_session_id: Option<&str>,
) -> String {
    let mut cmd = launch.custom_command.unwrap_or("claude").to_string();
    // The YOLO flag belongs to the built-in YOLO button only, never to a custom one.
    if launch.yolo && launch.custom_prefix.is_none() {
        cmd.push_str(" --dangerously-skip-permissions");
    }
    // Bare on purpose: with no name, Claude titles the Remote Control session
    // after the session itself — the `-n` name on a fresh launch, the stored
    // title on a resume — and carries `/rename` across. Passing the window's
    // name here would put back the bug the missing `-n` on resume avoids: a
    // `:2` from uniquifying overwriting the real name, remotely this time.
    //
    // The value is optional, so the flag must be followed by another flag,
    // never by a word it could take as its name; `-r` or `-n` always follows.
    // A custom command that asks for it already is left alone.
    if launch.remote_control && !launch.custom_command.is_some_and(asks_for_remote_control) {
        cmd.push_str(" --remote-control");
    }
    if let Some(id) = launch.resume_session_id {
        // No -n here on purpose. A resumed session already carries the name it
        // was given, and passing one would overwrite it — with the *window's*
        // name, which may have picked up a `:2` from uniquifying against a
        // window still open for the same conversation. The hook adopts the
        // stored name onto the window instead.
        //
        // Quoted like the window name: a resume id is a transcript's filename
        // stem, which forestui read off disk rather than wrote.
        cmd.push_str(&format!(" -r {}", sh_quote(id)));
    } else {
        // Claude renders this in the prompt box from the first frame, which no
        // hook can do: a title set before the session's UI is live is stored
        // but never drawn.
        cmd.push_str(&format!(" -n {}", sh_quote(window_name)));
        // The pre-minted id, so the window knows which session it holds from
        // birth instead of forestui guessing later from transcript mtimes.
        if let Some(id) = new_session_id {
            cmd.push_str(&format!(" --session-id {}", sh_quote(id)));
        }
    }
    // Last, and after a flag that takes a value, so no optional-valued flag
    // ahead of it can take it as its own. Line breaks and tabs become spaces
    // here rather than being stripped by `single_line` below, so a template's
    // paragraphs do not run their words together.
    if let Some(prompt) = launch.first_prompt {
        let flat = prompt
            .split(char::is_control)
            .map(str::trim_end)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        cmd.push_str(&format!(" {}", sh_quote(&flat)));
    }
    single_line(&cmd)
}

/// The tail every generated startup file ends with.
///
/// `set -m` is the load-bearing line. Without it a command run from a startup
/// file is not a job: bash hands it the shell's own ignored `SIGTSTP`, so
/// Ctrl-Z does nothing at all and — because the shell is not waiting for a
/// stoppable child either — the window wedges with no way back. With it, the
/// command is a job in its own process group, which is what makes Ctrl-Z leave
/// a stopped job and a prompt, and `fg` bring it back. zsh already runs
/// interactive shells this way; saying so costs nothing and makes the file
/// mean the same thing everywhere.
///
/// The birth stamp is written here, from inside the window, for the reason it
/// always was: the shell starts Claude on its own, so a stamp forestui sends
/// after `new-window` returns is a separate tmux call that a fast-starting
/// Claude can beat to its first hook.
fn startup_tail(window_name: &str, session_id: &str, line: &str) -> String {
    // The session-id stamp rides along for the same reason as the birth name:
    // written from inside the window, ahead of the command, it cannot lose a
    // race to anything reading window options once Claude is up.
    format!(
        "tmux set-option -w @claude_birth_name {} >/dev/null 2>&1\n\
         tmux set-option -w @claude_session_id {} >/dev/null 2>&1\n\
         set -m\n{line}\n",
        sh_quote(window_name),
        sh_quote(session_id)
    )
}

/// The user's own startup files, as the generated ones have to name them.
struct ShellHome {
    home: String,
    zdotdir: Option<String>,
    env_file: Option<String>,
}

impl ShellHome {
    fn from_env() -> Self {
        Self {
            home: std::env::var("HOME").unwrap_or_default(),
            zdotdir: std::env::var("ZDOTDIR").ok().filter(|v| !v.is_empty()),
            env_file: std::env::var("ENV").ok().filter(|v| !v.is_empty()),
        }
    }
}

/// A shell that has been pointed at a startup file forestui wrote.
struct Startup {
    /// Files to write before the window is created, in order.
    files: Vec<(PathBuf, String)>,
    /// The command tmux runs for the window.
    command: String,
}

/// Point `shell` at a startup file that runs `line`, or `None` if this shell
/// has no hook we know.
///
/// Every one of these hooks *replaces* one of the user's own files rather than
/// adding to it — a new `ZDOTDIR` hides `.zshenv` and `.zshrc`, `--rcfile`
/// stands in for `.bashrc`, `ENV` for whatever `ENV` already named — so each
/// generated file sources the file it displaced. What the window ends up with
/// is the environment `$SHELL -ic` used to give it: the same interactive,
/// non-login startup, with the command running as a job at the end of it.
///
/// The generated files delete themselves as soon as the shell has read what it
/// needs, so a window that is never closed does not leave one behind.
fn startup_for(
    shell: &str,
    dir: &Path,
    home: &ShellHome,
    window_name: &str,
    session_id: &str,
    line: &str,
    remembered_line: &str,
) -> Option<Startup> {
    let base = Path::new(shell).file_name()?.to_str()?;
    let dir_str = dir.to_str()?;
    // Typing the command used to put it in the shell's history for free, and
    // that is worth keeping: after Ctrl-C the up arrow is the shortest way
    // back into a session. Only the shells with a builtin for it get one.
    //
    // What is remembered is not always what ran: a fresh session launches
    // with a pre-minted `--session-id`, and Claude refuses that flag once the
    // session exists ("Session ID is already in use") — which is exactly the
    // state an up-arrow rerun happens in. The caller hands the resume form
    // instead, so the up arrow goes back into the same conversation rather
    // than into an error.
    //
    // When it is remembered matters as much. zsh reads `HISTFILE` only after
    // its startup files, and Claude runs *inside* this one — so a line pushed
    // from here lands under the whole loaded file, and under `share_history`
    // the up arrow instead finds whatever line another window's shell wrote
    // last: a different conversation that looks like the right one. zsh
    // therefore pushes from a one-shot `precmd` hook, at the first prompt
    // after Claude exits or is suspended, once the file is loaded. bash reads
    // its history before the rc file, so its line is already on top.
    let remembered = match base {
        "zsh" => format!(
            "__forestui_remember() {{\n\
             \x20 print -s -- {}\n\
             \x20 precmd_functions=(${{precmd_functions:#__forestui_remember}})\n\
             \x20 unfunction __forestui_remember\n\
             }}\n\
             precmd_functions+=(__forestui_remember)\n",
            sh_quote(remembered_line)
        ),
        "bash" => format!("history -s {}\n", sh_quote(remembered_line)),
        _ => String::new(),
    };
    let tail = format!(
        "{remembered}{}",
        startup_tail(window_name, session_id, line)
    );
    let tail = tail.as_str();
    let sweep = format!("rm -rf {}\n", sh_quote(dir_str));
    let source = |path: String| {
        let quoted = sh_quote(&path);
        format!("[ -f {quoted} ] && . {quoted}\n")
    };

    match base {
        "zsh" => {
            // `.zshenv` is read for every shell and `.zshrc` for interactive
            // ones, so both are hidden by the new ZDOTDIR and both come back
            // here. ZDOTDIR itself is restored before the user's `.zshrc` runs
            // — a configuration that keys a cache or a plugin directory off it
            // must not see forestui's temporary one.
            let zdot = home.zdotdir.clone().unwrap_or_else(|| home.home.clone());
            let files = vec![
                (dir.join(".zshenv"), source(format!("{zdot}/.zshenv"))),
                (
                    dir.join(".zshrc"),
                    format!(
                        "ZDOTDIR={}\n{sweep}{}{tail}",
                        sh_quote(&zdot),
                        source(format!("{zdot}/.zshrc"))
                    ),
                ),
            ];
            Some(Startup {
                files,
                command: format!("env ZDOTDIR={} {} -i", sh_quote(dir_str), sh_quote(shell)),
            })
        }
        "bash" => {
            let rc = dir.join("rc");
            let rc_str = rc.to_str()?.to_string();
            Some(Startup {
                files: vec![(
                    rc,
                    format!("{sweep}{}{tail}", source(format!("{}/.bashrc", home.home))),
                )],
                command: format!("{} --rcfile {} -i", sh_quote(shell), sh_quote(&rc_str)),
            })
        }
        // The POSIX interactive startup file, which is whatever `ENV` names.
        "sh" | "dash" | "ash" | "ksh" | "ksh93" | "mksh" | "loksh" | "yash" => {
            let rc = dir.join("rc");
            let rc_str = rc.to_str()?.to_string();
            let inherited = home.env_file.clone().map(source).unwrap_or_default();
            Some(Startup {
                files: vec![(rc, format!("{sweep}{inherited}{tail}"))],
                command: format!("env ENV={} {} -i", sh_quote(&rc_str), sh_quote(shell)),
            })
        }
        // fish, nushell and friends: no hook here that has been tested, so the
        // line is typed into a plain shell instead. Nothing is lost but the
        // typing.
        _ => None,
    }
}

/// Write the startup files for this launch, or `None` to fall back to typing.
fn prepare_startup(
    window_name: &str,
    session_id: &str,
    line: &str,
    remembered_line: &str,
) -> Option<Startup> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty())?;
    let dir = std::env::temp_dir().join(format!(
        "forestui-launch-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let startup = startup_for(
        &shell,
        &dir,
        &ShellHome::from_env(),
        window_name,
        session_id,
        line,
        remembered_line,
    )?;

    // A launch that cannot write its files is not an error to show anyone:
    // typing the line still works, so fall back to it.
    std::fs::create_dir_all(&dir).ok()?;
    for (path, contents) in &startup.files {
        std::fs::write(path, contents).ok()?;
    }
    Some(startup)
}

/// Type a line into a window's shell and run it.
///
/// The fallback for a shell with no startup hook of its own. Two calls, because
/// `-l` sends its arguments as literal characters — the only way to keep a
/// command line from being read as key names — and `Enter` is precisely a key
/// name.
fn send_line(window_id: &str, line: &str) -> bool {
    tmux(&["send-keys", "-t", window_id, "-l", "--", line]).is_some()
        && tmux(&["send-keys", "-t", window_id, "Enter"]).is_some()
}

/// Window name for a Claude session, before uniquifying.
pub fn claude_base_window_name(name: &str, yolo: bool, custom_prefix: Option<&str>) -> String {
    match (custom_prefix, yolo) {
        (Some(prefix), _) => format!("{prefix}:{name}"),
        (None, true) => format!("yolo:{name}"),
        (None, false) => format!("claude:{name}"),
    }
}

/// The name a Claude window opens with.
///
/// A resumed session keeps its own name verbatim: the window name and the
/// session name are one string, so re-applying a prefix here would accrete
/// (`claude:yolo:thing`) a little more on every resume. A fresh session has no
/// name yet, so it gets forestui's opening name instead.
pub fn opening_window_name(
    seed: Option<&str>,
    worktree_name: &str,
    yolo: bool,
    custom_prefix: Option<&str>,
) -> String {
    match seed {
        Some(name) => name.to_string(),
        None => claude_base_window_name(worktree_name, yolo, custom_prefix),
    }
}

/// The line a Claude window runs, and the line its shell history remembers.
///
/// For a resume the two are the same line. For a fresh session the history
/// gets the resume form of the same id, because `--session-id` refuses to run
/// once the session exists and the up arrow exists for the second run — see
/// `startup_for`. Everything else rides along, Remote Control included: the up
/// arrow after a Ctrl-C should bring the session back as it was, reachable
/// from the phone.
fn launch_lines(
    launch: &ClaudeLaunch<'_>,
    window_name: &str,
    session_id: &str,
) -> (String, String) {
    let line = claude_command_line(
        launch,
        window_name,
        launch.resume_session_id.is_none().then_some(session_id),
    );
    let remembered = claude_command_line(
        &ClaudeLaunch {
            resume_session_id: Some(session_id),
            // The up arrow resumes the conversation; it must not send the
            // work a second time into the middle of it.
            first_prompt: None,
            ..*launch
        },
        window_name,
        None,
    );
    (line, remembered)
}

/// Create a tmux window running Claude Code. Returns the window name.
pub fn create_claude_window(
    base_name: &str,
    path: &str,
    launch: ClaudeLaunch<'_>,
) -> Option<String> {
    let resume_session_id = launch.resume_session_id;
    current_session()?;

    // The window knows its session from birth, both ways round: a fresh
    // session runs under an id forestui minted and passed as `--session-id`,
    // a resumed one under the id being resumed. Either way the stamp makes
    // "which window holds this conversation" a lookup, not a heuristic.
    let minted;
    let session_id = match resume_session_id {
        Some(id) => id,
        None => {
            minted = uuid::Uuid::new_v4().to_string();
            &minted
        }
    };

    let window_name = find_unique_window_name(base_name);
    let (line, remembered) = launch_lines(&launch, &window_name, session_id);

    match prepare_startup(&window_name, session_id, &line, &remembered) {
        // The window's command is the user's shell, interactive, reading a
        // startup file that ends in the Claude command. Nothing is typed, so
        // there is nothing to race the shell's startup.
        Some(startup) => {
            let id = new_window(&window_name, path, Some(&startup.command))?;
            // Belt and braces: the stamps inside the startup file are what
            // order correctly, this covers a shell that never read them.
            // Same values either way.
            stamp_birth_name(&id, &window_name);
            stamp_session_id(&id, session_id);
            Some(window_name)
        }
        // A shell with no startup hook we know. The window runs the plain
        // default shell and the line is typed into it — stamped first, which
        // orders it: Claude cannot reach its first hook until keys that have
        // not been sent yet arrive.
        None => {
            let id = new_window(&window_name, path, None)?;
            stamp_birth_name(&id, &window_name);
            stamp_session_id(&id, session_id);
            send_line(&id, &line).then_some(window_name)
        }
    }
}

#[cfg(test)]
mod tests;
