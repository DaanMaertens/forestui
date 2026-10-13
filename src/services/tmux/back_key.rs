//! The back key: a tmux key, prefix + ← unless the user picks another, that
//! takes the viewer from a Claude window forestui opened back to forestui's
//! own window, leaving Claude running.
//!
//! It exists because the one key people knew was Ctrl-D, which quits Claude.
//! A binding is server-wide, so this module also owns putting things back:
//! the previous binding and `status-right` are restored exactly when the last
//! forestui using them goes away.
//!
//! Everything forestui needs to clean up lives *in tmux*, not in this process:
//!
//! - **`@forestui_home`**, a window option on each forestui's own window,
//!   holding that forestui's pid. It is the binding's target (found with a
//!   `#{W:…}` loop over the viewer's session, so grouped sessions and a
//!   renamed window both work) and the liveness record: "no mark with a live
//!   pid" means "nobody needs the key any more", whoever forgot to clean up.
//!   The pid matters because a forestui started from a shell inside tmux
//!   leaves its window — and the mark — behind when it is killed.
//! - **`@forestui_back_key`** and friends, global user options recording which
//!   key forestui bound and what it and `status-right` were before. They live
//!   exactly as long as the server whose settings they describe.
//!
//! The binding itself is the same text for every forestui on the server — the
//! target is found at press time — so two instances share one binding rather
//! than fighting over it.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{is_inside_tmux, tmux, tmux_with_input};

/// The key forestui binds when the setting has never been touched.
pub const DEFAULT_KEY: &str = "Left";

const HOME_MARK: &str = "@forestui_home";
const KEY_MARK: &str = "@forestui_back_key";
/// The full `bind-key` line the key had before, or empty when it was unbound.
const SAVED_BINDING: &str = "@forestui_back_key_saved";
const SAVED_STATUS: &str = "@forestui_status_right_saved";
const SAVED_STATUS_LENGTH: &str = "@forestui_status_right_length_saved";

/// True in a Claude window forestui opened, while the viewer's session holds
/// a forestui window to go back to. Shared by the binding and the hint, so the
/// hint is shown exactly where the key works.
const CONDITION: &str = "#{&&:#{@claude_birth_name},#{W:#{?#{@forestui_home},1,}}}";

/// Selects every marked window in the viewer's session, which in practice is
/// the one forestui window there. `run-shell -C` is what expands it: tmux
/// does not expand formats in a plain `-t` target, so the window id has to be
/// spliced into the command text at press time.
const GO_HOME: &str = "#{W:#{?#{@forestui_home},select-window -t #{window_id} ;,}}";

/// How much wider `status-right` may draw with the hint in front of it.
/// tmux's default length is 40, which the default `status-right` already
/// fills; without room the hint would push the clock off the end.
const HINT_ROOM: u32 = 24;

/// A key binding as `list-keys` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// `-r`: the key repeats without the prefix for `repeat-time`.
    pub repeat: bool,
    /// The key as tmux printed it, escapes included — the form its parser
    /// reads back.
    pub key_token: String,
    /// The bound command, in tmux's own syntax.
    pub command: String,
}

impl Binding {
    /// The command that binds this again, exactly as it was.
    pub fn line(&self) -> String {
        let repeat = if self.repeat { "-r " } else { "" };
        format!(
            "bind-key {repeat}-T prefix {} {}",
            self.key_token, self.command
        )
    }
}

/// Find `key`'s binding in a `list-keys -T prefix` listing.
///
/// The whole table is listed and searched because `list-keys -T prefix
/// <key>` prints nothing at all on tmux 3.7.
pub fn parse_binding(listing: &str, key: &str) -> Option<Binding> {
    listing.lines().find_map(|line| {
        let mut rest = line.trim_start().strip_prefix("bind-key")?.trim_start();
        // `-r` is the only flag printed ahead of the table.
        let repeat = match rest.strip_prefix("-r ") {
            Some(after) => {
                rest = after.trim_start();
                true
            }
            None => false,
        };
        let rest = rest.strip_prefix("-T ")?.trim_start();
        let rest = rest.strip_prefix("prefix")?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let rest = rest.trim_start();
        let key_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let token = &rest[..key_end];
        if unescape_key(token) != key {
            return None;
        }
        Some(Binding {
            repeat,
            key_token: token.to_string(),
            command: rest[key_end..].trim().to_string(),
        })
    })
}

/// `list-keys` writes `"` as `\"`, `#` as `\#` and so on.
fn unescape_key(token: &str) -> &str {
    match token.strip_prefix('\\') {
        Some(rest) if !rest.is_empty() => rest,
        _ => token,
    }
}

/// Whether a bound command is the back key — this run's, a killed run's, or
/// another forestui's, which all bind the same text.
pub fn is_ours(command: &str) -> bool {
    command.starts_with(&format!("if-shell -F \"{CONDITION}\""))
}

/// tmux's own binding for `key`, for the keys forestui knows it for. Anything
/// else bound counts as the user's, which only costs a question.
fn tmux_default(key: &str) -> Option<&'static str> {
    match key {
        "Left" => Some("select-pane -L"),
        "Right" => Some("select-pane -R"),
        "Up" => Some("select-pane -U"),
        "Down" => Some("select-pane -D"),
        _ => None,
    }
}

/// What to do with the key's current binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Already the back key; nothing to bind.
    Keep,
    /// Free, tmux's default, or a binding the user already said to use anyway.
    Bind,
    /// The user's own binding: ask before taking it.
    Conflict,
}

pub fn decide(key: &str, current: Option<&Binding>, accepted: Option<&str>) -> Decision {
    let Some(current) = current else {
        return Decision::Bind;
    };
    if is_ours(&current.command) {
        Decision::Keep
    } else if tmux_default(key) == Some(current.command.as_str())
        || accepted == Some(current.command.as_str())
    {
        Decision::Bind
    } else {
        Decision::Conflict
    }
}

/// The `bind-key` line for the back key on `key`, falling back to
/// `previous` in every window that is not a Claude window forestui opened.
pub fn bind_line(key: &str, previous: Option<&Binding>) -> String {
    let repeat = if previous.is_some_and(|p| p.repeat) {
        "-r "
    } else {
        ""
    };
    // `switch-client -T root` ends a repeating key's repeat: otherwise an
    // arrow pressed just after landing on forestui runs the binding again
    // instead of reaching forestui.
    let mut line = format!(
        "bind-key {repeat}-T prefix {} if-shell -F \"{CONDITION}\" \
         {{ run-shell -C \"{GO_HOME}\" ; switch-client -T root }}",
        quote_key(key)
    );
    if let Some(previous) = previous {
        line.push_str(&format!(" {{ {} }}", previous.command));
    }
    line
}

/// Quote a key name for tmux's parser.
fn quote_key(key: &str) -> String {
    if key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        key.to_string()
    } else if key.contains('\'') {
        format!("\"{}\"", key.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{key}'")
    }
}

/// `status-right` with the hint in front of `previous`.
///
/// `#{prefix}` is resolved by tmux as it draws, so the hint names the
/// prefix the user has *now*, including one changed while forestui runs.
pub fn hinted_status(previous: &str, key: &str) -> String {
    let glyph = glyph(key)
        .replace('#', "##")
        .replace(',', "#,")
        .replace('}', "#}");
    format!("{}{glyph} forestui │ ,}}{previous}", hint_opening())
}

fn hint_opening() -> String {
    format!("#{{?{CONDITION},#{{prefix}} ")
}

/// Whether `status-right` carries the hint — ours to put back.
pub fn is_hinted(status: &str) -> bool {
    status.starts_with(&hint_opening())
}

/// How a key reads on screen.
pub fn glyph(key: &str) -> String {
    match key {
        "Left" => "←",
        "Right" => "→",
        "Up" => "↑",
        "Down" => "↓",
        other => other,
    }
    .to_string()
}

/// The tmux name of a pressed key, or `None` for one that cannot be the back
/// key (Esc, which cancels the question that asked for it).
pub fn key_name(key: KeyEvent) -> Option<String> {
    let base = match key.code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PPage".into(),
        KeyCode::PageDown => "NPage".into(),
        KeyCode::Insert => "IC".into(),
        KeyCode::Delete => "DC".into(),
        KeyCode::Backspace => "BSpace".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => "BTab".into(),
        KeyCode::F(n) => format!("F{n}"),
        _ => return None,
    };
    let mods = key.modifiers;
    let mut name = String::new();
    if mods.contains(KeyModifiers::CONTROL) {
        name.push_str("C-");
    }
    if mods.contains(KeyModifiers::ALT) {
        name.push_str("M-");
    }
    // A character's case already says whether Shift was held.
    if mods.contains(KeyModifiers::SHIFT)
        && !matches!(key.code, KeyCode::Char(_) | KeyCode::BackTab)
    {
        name.push_str("S-");
    }
    name.push_str(&base);
    Some(name)
}

/// The marked windows other than `own` in a
/// `list-windows -a -F '#{window_id}\t#{@forestui_home}'`, split by whether
/// the forestui that marked it is still running: `(live, stale)`.
pub fn homes<'a>(
    listing: &'a str,
    own: Option<&str>,
    alive: impl Fn(u32) -> bool,
) -> (usize, Vec<&'a str>) {
    let mut live = 0;
    let mut stale = Vec::new();
    for line in listing.lines() {
        let Some((id, mark)) = line.split_once('\t') else {
            continue;
        };
        if mark.is_empty() || Some(id) == own {
            continue;
        }
        match mark.parse::<u32>() {
            Ok(pid) if alive(pid) => live += 1,
            _ => stale.push(id),
        }
    }
    (live, stale)
}

/// Whether a process with this pid exists. tmux and forestui share a
/// machine, so a pid in a mark is one this process can ask about.
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 delivers nothing; `kill` only checks the pid exists
    // and that we may signal it. EPERM still means it exists.
    let status = unsafe { libc::kill(pid, 0) };
    status == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

// ------------------------------------------------------------- side effects

/// What [`claim`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// The back key is live on this window.
    Bound,
    /// The key is the user's own; the command it runs, for the offer to show
    /// and, if they say "use it anyway", to remember.
    Conflict(String),
    /// Not inside tmux, or tmux would not answer.
    Unavailable,
}

/// Bind the back key on `key` for the forestui running in `own_window`,
/// unless that would take a binding the user made (and did not accept).
pub fn claim(key: &str, accepted: Option<&str>, own_window: &str) -> Claim {
    if !is_inside_tmux() {
        return Claim::Unavailable;
    }
    // Another key from an earlier setting, here or in another forestui: the
    // server has one back key, so it goes back first.
    if let Some(bound) = global(KEY_MARK)
        && bound != key
    {
        restore_binding();
    }
    let Some(listing) = tmux(&["list-keys", "-T", "prefix"]) else {
        return Claim::Unavailable;
    };
    let current = parse_binding(&listing, key);
    match decide(key, current.as_ref(), accepted) {
        Decision::Keep => {}
        Decision::Conflict => {
            return Claim::Conflict(current.map(|c| c.command).unwrap_or_default());
        }
        Decision::Bind => {
            let saved = current.as_ref().map(Binding::line).unwrap_or_default();
            if tmux_with_input(
                &["source-file", "-"],
                &format!("{}\n", bind_line(key, current.as_ref())),
            )
            .is_none()
            {
                return Claim::Unavailable;
            }
            set_global(KEY_MARK, key);
            set_global(SAVED_BINDING, &saved);
        }
    }
    let _ = sweep_homes(Some(own_window));
    let pid = std::process::id().to_string();
    let _ = tmux(&["set-option", "-w", "-t", own_window, HOME_MARK, &pid]);
    install_hint(key);
    Claim::Bound
}

/// Stop using the back key from `own_window` (when there is one), and hand
/// the binding and `status-right` back once no live forestui needs them.
///
/// Also what heals a killed run: its window took its mark with it, so the
/// next forestui finds nobody and restores what the dead one left bound.
pub fn release(own_window: Option<&str>) {
    if !is_inside_tmux() {
        return;
    }
    if let Some(own) = own_window {
        let _ = tmux(&["set-option", "-wu", "-t", own, HOME_MARK]);
    }
    if sweep_homes(own_window) > 0 {
        return;
    }
    restore_binding();
    restore_hint();
}

/// Clear the marks of forestuis that are gone, and count the live ones
/// other than `own`.
///
/// A dead forestui's mark would keep sending the key to a window that is only
/// a shell now — and, counted as live, would keep the binding forever.
fn sweep_homes(own: Option<&str>) -> usize {
    let windows = tmux(&[
        "list-windows",
        "-a",
        "-F",
        "#{window_id}\t#{@forestui_home}",
    ])
    .unwrap_or_default();
    let (live, stale) = homes(&windows, own, process_alive);
    for window in stale {
        let _ = tmux(&["set-option", "-wu", "-t", window, HOME_MARK]);
    }
    live
}

/// Put the bound key back as it was — unless it is no longer ours, in which
/// case whoever rebound it keeps their binding.
fn restore_binding() {
    let Some(key) = global(KEY_MARK) else {
        return;
    };
    let listing = tmux(&["list-keys", "-T", "prefix"]).unwrap_or_default();
    if parse_binding(&listing, &key).is_some_and(|b| is_ours(&b.command)) {
        match global(SAVED_BINDING).filter(|line| !line.is_empty()) {
            Some(line) => {
                let _ = tmux_with_input(&["source-file", "-"], &format!("{line}\n"));
            }
            None => {
                let _ = tmux(&["unbind-key", "-T", "prefix", &key]);
            }
        }
    }
    unset_global(KEY_MARK);
    unset_global(SAVED_BINDING);
}

fn install_hint(key: &str) {
    let Some(current) = global("status-right") else {
        return;
    };
    let previous = match global(SAVED_STATUS) {
        // Ours already, maybe for another key: rebuild from the original.
        Some(saved) if is_hinted(&current) => saved,
        // Never hinted, or the user replaced ours since: theirs is the one
        // to keep and to put back.
        _ => {
            set_global(SAVED_STATUS, &current);
            current
        }
    };
    set_global("status-right", &hinted_status(&previous, key));
    if global(SAVED_STATUS_LENGTH).is_none()
        && let Some(length) = global("status-right-length")
        && let Ok(length) = length.parse::<u32>()
    {
        set_global(SAVED_STATUS_LENGTH, &length.to_string());
        set_global("status-right-length", &(length + HINT_ROOM).to_string());
    }
}

fn restore_hint() {
    if let Some(saved) = global(SAVED_STATUS)
        && global("status-right").is_some_and(|current| is_hinted(&current))
    {
        set_global("status-right", &saved);
    }
    if let Some(saved) = global(SAVED_STATUS_LENGTH)
        && let Ok(saved_length) = saved.parse::<u32>()
        && global("status-right-length")
            .is_some_and(|current| current == (saved_length + HINT_ROOM).to_string())
    {
        set_global("status-right-length", &saved);
    }
    unset_global(SAVED_STATUS);
    unset_global(SAVED_STATUS_LENGTH);
}

/// A global option's value, or `None` when it is unset — tmux answers an
/// unset user option with an error, not an empty value.
fn global(name: &str) -> Option<String> {
    tmux(&["show-options", "-gv", name]).map(|v| v.strip_suffix('\n').unwrap_or(&v).to_string())
}

fn set_global(name: &str, value: &str) {
    // No `--` before the value: tmux rejects it here, and stops reading
    // flags at the option name anyway, so a value starting with `-` is safe.
    let _ = tmux(&["set-option", "-g", name, value]);
}

fn unset_global(name: &str) {
    let _ = tmux(&["set-option", "-gu", name]);
}

#[cfg(test)]
mod tests;
