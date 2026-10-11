//! Application state and the event loop's core: startup, the sidebar, the
//! background loads a selection kicks off, and event folding.
//!
//! The input handling lives beside it in focused modules: [`keys`] for the
//! keyboard, [`mouse`] for hit testing and scrolling, [`actions`] for what a
//! control does when activated, and [`detail`] for what the detail pane
//! contains.

mod actions;
pub mod detail;
pub mod finished;
mod keys;
mod mouse;
mod repo_settings;
pub mod worktree_sync;

pub use detail::{Action, DetailItem, Field};
pub use keys::BINDINGS;
pub use mouse::{Direction, Hit, HitTarget, ModalClick, ScrollbarGeom};

use crate::event::{AppEvent, DetailMeta, EventTx, Severity};
use crate::modal::Modal;
use crate::models::{ClaudeSession, GitHubIssue, Settings};
use crate::services::{claude_session, git, github, settings as settings_service, tmux};
use crate::state::AppState;
use crate::ui::widgets::TextInput;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Textual's `App.NOTIFICATION_TIMEOUT`, which every `notify()` here inherited.
pub const NOTIFICATION_TTL: Duration = Duration::from_secs(5);
pub const ISSUE_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
/// How often every repository is re-checked against `git worktree list`, so
/// worktrees created or removed outside forestui (an agent's `git worktree
/// add`, a manual remove) converge into the sidebar without being asked for.
pub const WORKTREE_SCAN_INTERVAL: Duration = Duration::from_secs(30);
/// How often the session list is re-scanned while forestui is just sitting
/// there. Frequent because a conversation in another window moves constantly,
/// and affordable because an unchanged transcript is never re-parsed. The
/// per-card refresh does *not* run on this timer — see `App::on_tick`.
pub const SESSION_REFRESH_INTERVAL: Duration = Duration::from_secs(10);
pub const SESSION_LIMIT: usize = 5;
pub const ISSUE_LIMIT: usize = 10;
/// Lines the detail pane moves per wheel notch, matching Textual's
/// `scroll_sensitivity_y` of 3.
pub(super) const SCROLL_STEP: isize = 3;
/// Rows `PageUp` / `PageDown` move.
pub(super) const PAGE_STEP: isize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Detail,
}

/// The branch to show beside a worktree name, or `None` when it only repeats it.
///
/// `w` creates `<branch_prefix><name>`, which is the overwhelmingly common case,
/// so showing it back costs a third of the sidebar and tells the user nothing. A
/// branch that genuinely differs — an existing branch checked out under another
/// name — is worth the space.
fn informative_branch(branch: &str, name: &str, prefix: &str) -> Option<String> {
    if branch == name || branch == format!("{prefix}{name}") {
        return None;
    }
    Some(branch.to_string())
}

/// One visible line in the sidebar tree.
#[derive(Debug, Clone)]
pub enum SidebarRow {
    Repository {
        id: Uuid,
        name: String,
        /// Whether the row has anything to fold away, so a leaf repository does
        /// not grow a twisty that would do nothing.
        has_worktrees: bool,
        collapsed: bool,
    },
    Worktree {
        repo_id: Uuid,
        id: Uuid,
        name: String,
        /// The branch, when it says something the name does not. A worktree
        /// created here is `<branch_prefix><name>`, so printing that beside the
        /// name is pure repetition — and in a 35-column sidebar the repetition
        /// is what gets truncated, leaving every row ending in `[feat/doc-impro`.
        branch: Option<String>,
        is_last: bool,
        /// Its work item is done; see [`finished`].
        finished: bool,
    },
    ArchivedHeader,
    ArchivedWorktree {
        repo_id: Uuid,
        id: Uuid,
        name: String,
        repo_name: String,
    },
}

impl SidebarRow {
    pub fn ids(&self) -> Option<(Uuid, Option<Uuid>)> {
        match self {
            SidebarRow::Repository { id, .. } => Some((*id, None)),
            SidebarRow::Worktree { repo_id, id, .. }
            | SidebarRow::ArchivedWorktree { repo_id, id, .. } => Some((*repo_id, Some(*id))),
            SidebarRow::ArchivedHeader => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub text: String,
    pub severity: Severity,
    pub created: Instant,
}

pub struct App {
    pub state: AppState,
    pub settings: Settings,
    pub tx: EventTx,
    pub version: String,

    pub focus: Focus,
    pub rows: Vec<SidebarRow>,
    pub sidebar_index: usize,
    pub detail_index: usize,

    /// `None` while the background load is still running.
    pub sessions: Option<Vec<ClaudeSession>>,
    pub issues: Option<Vec<GitHubIssue>>,
    pub meta: DetailMeta,

    pub gh_status: String,
    /// A newer forestui this run has learned about — installed and waiting
    /// for a restart, or available to `cargo install`. Set once by the startup
    /// check and never cleared: the running process stays the old build until
    /// it exits, so the title bar says so for as long as that is true. (A
    /// cargo install's notice comes back on the next launch too, until the
    /// user runs `cargo install` — that is still true as well.)
    pub pending_update: Option<crate::version_check::UpdateStatus>,
    pub modals: Vec<Modal>,
    pub notifications: Vec<Notification>,
    pub spinner_index: usize,
    pub last_issue_refresh: Instant,
    pub should_quit: bool,
    /// Paths with an issue fetch / session scan already running. Selection
    /// changes arrive far faster than the subprocess chains behind these
    /// panels; without the guard, bouncing across the sidebar stacks
    /// concurrent duplicate fetches for identical results.
    issues_in_flight: std::collections::HashSet<(String, crate::models::Scope)>,
    /// What is known of each checkout's repository settings, by path.
    repos: std::collections::HashMap<String, repo_settings::RepoLookup>,
    /// Paths a selection has asked `gh` about and not had an answer for.
    repo_probes: std::collections::HashSet<String>,
    sessions_in_flight: std::collections::HashSet<String>,
    /// Worktree scans and the changes they must not undo; see
    /// [`worktree_sync`].
    worktree_sync: worktree_sync::WorktreeSync,
    /// Which worktrees are finished, from the check that rides on the scans.
    finished: finished::FinishedWorktrees,
    /// When the last periodic worktree sweep started.
    last_worktree_scan: Instant,
    /// When the last periodic session refresh started.
    last_session_refresh: Instant,
    /// Session ids being re-read right now. Each one draws a spinner on its own
    /// card, so a slow transcript is visibly working rather than silently old.
    pub sessions_refreshing: std::collections::HashSet<String>,
    /// Where each Claude session is running, from the last live scan — a
    /// stamped tmux window or, via the plugin's heartbeat, anywhere else on
    /// the machine. A card whose id is here has a running Claude somewhere.
    pub live_sessions: std::collections::HashMap<String, crate::services::live::LiveSession>,
    /// One live scan at a time — it shells out to tmux once per sweep.
    live_scan_in_flight: bool,
    /// Quit was requested while removals were in flight; honoured when the
    /// last one folds. A second request quits immediately.
    pub quit_pending: bool,

    /// Clickable regions recorded by the renderers for the current frame.
    pub hits: Vec<Hit>,
    /// What the pointer is currently over, so controls can light up under it.
    pub hovered: Option<HitTarget>,
    /// Set by anything that changes what the next frame looks like. Pointer
    /// motion that does not change `hovered` leaves it alone, which is what
    /// keeps a moving mouse from repainting the app on every report.
    pub redraw: bool,

    /// Where the two panes were drawn, so the wheel can be routed to whichever
    /// one the pointer is over rather than to whichever one has focus.
    pub sidebar_rect: ratatui::layout::Rect,
    pub detail_rect: ratatui::layout::Rect,

    /// First visible line of the detail pane.
    pub detail_scroll: u16,
    /// Set when the cursor moves by keyboard, so the next frame scrolls the
    /// focused control back into view. The wheel and the scrollbar deliberately
    /// do not set it.
    pub detail_follow_focus: bool,
    /// The scrollbar as the last frame drew it, or `None` when the content fits.
    pub scrollbar: Option<ScrollbarGeom>,
    /// Rows between the top of the thumb and where the pointer grabbed it, held
    /// for the duration of a drag so the thumb does not jump under the cursor.
    pub scroll_drag: Option<u16>,
    /// Repositories folded shut in the sidebar.
    pub collapsed: std::collections::HashSet<Uuid>,
    /// Whether to check for a newer release at startup; `--no-self-update`
    /// clears it.
    pub self_update: bool,
    /// Which optional terminal input modes this run asked for; `--no-hover`
    /// and `--no-focus-events` narrow it.
    pub input_modes: crate::terminal::InputModes,
    /// Whether *this* run turned tmux's server-wide `focus-events` on, and so
    /// has to turn it off again on the way out.
    focus_events_owned: bool,

    /// The detail items as the last frame drew them, each with whether it was
    /// enabled. Activation — a click or Enter — resolves against this snapshot
    /// rather than re-deriving the list, because a background event drained in
    /// the same batch (issues finishing, sessions landing) can grow or shrink
    /// the live list before the click is handled, silently remapping the index
    /// the user aimed at onto a different control. The snapshot is what was on
    /// screen, which is what the user acted on.
    pub drawn_items: Vec<(DetailItem, bool)>,

    pub name_input: TextInput,
    pub branch_input: TextInput,
    /// Worktree the rename inputs belong to, so a selection change resets them.
    pub(crate) rename_target: Option<Uuid>,
    /// The work item Start work picked, held while the branch list its
    /// confirm screen needs is loading.
    pub(crate) pending_start: Option<PendingStart>,
    /// A jump to an already-started worktree, waiting to learn whether a
    /// Claude session is running there. See [`App::finish_jump`].
    pub(crate) pending_jump: Option<PendingJump>,
}

/// What Start work's confirm screen opens on once the branches load. The
/// names travel separately from the item: a free-text feature's may carry the
/// suffix its clash question chose.
#[derive(Debug)]
pub(crate) struct PendingStart {
    pub mode: crate::work::WorkMode,
    pub item: crate::work::WorkItem,
    pub names: crate::work::WorktreeNames,
}

/// What a jump still waits for: the jumped-to worktree's session list, and
/// the next live scan. Only together do they answer whether a
/// Claude session is running there.
#[derive(Debug)]
pub(crate) struct PendingJump {
    pub path: String,
    pub sessions: bool,
    pub live: bool,
}

impl App {
    pub fn new(tx: EventTx, version: String) -> Self {
        let state = AppState::load();
        let settings = settings_service::load_settings();
        let mut app = Self::with_state(tx, state, settings);
        app.version = version;
        app
    }

    /// Build an app around explicit state instead of reading disk.
    pub fn with_state(tx: EventTx, state: AppState, settings: Settings) -> Self {
        // The renderers read the active theme from the global; activate the
        // saved one before the first frame so launch never flashes the default.
        // `theme_name` carries the chosen slug — not `theme`, the inert
        // System/Dark/Light key older files still carry, which once silently
        // reset every launch to the default palette.
        crate::theme::set_active(&settings.theme_name);
        let version = crate::cli::VERSION.to_string();
        let mut app = Self {
            state,
            settings,
            tx,
            version,
            focus: Focus::Sidebar,
            rows: Vec::new(),
            sidebar_index: 0,
            detail_index: 0,
            sessions: None,
            issues: None,
            meta: DetailMeta::default(),
            gh_status: "...".to_string(),
            pending_update: None,
            modals: Vec::new(),
            notifications: Vec::new(),
            spinner_index: 0,
            last_issue_refresh: Instant::now(),
            should_quit: false,
            issues_in_flight: std::collections::HashSet::new(),
            repos: std::collections::HashMap::new(),
            repo_probes: std::collections::HashSet::new(),
            sessions_in_flight: std::collections::HashSet::new(),
            worktree_sync: worktree_sync::WorktreeSync::default(),
            finished: finished::FinishedWorktrees::default(),
            last_worktree_scan: Instant::now(),
            last_session_refresh: Instant::now(),
            sessions_refreshing: std::collections::HashSet::new(),
            live_sessions: std::collections::HashMap::new(),
            live_scan_in_flight: false,
            quit_pending: false,
            hits: Vec::new(),
            hovered: None,
            redraw: true,
            sidebar_rect: ratatui::layout::Rect::default(),
            detail_rect: ratatui::layout::Rect::default(),
            detail_scroll: 0,
            detail_follow_focus: true,
            scrollbar: None,
            scroll_drag: None,
            collapsed: std::collections::HashSet::new(),
            self_update: true,
            input_modes: crate::terminal::InputModes::default(),
            focus_events_owned: false,
            drawn_items: Vec::new(),
            name_input: TextInput::new(""),
            branch_input: TextInput::new(""),
            rename_target: None,
            pending_start: None,
            pending_jump: None,
        };
        app.rebuild_rows();
        app
    }

    pub fn title(&self) -> String {
        format!("forestui v{}", self.version)
    }

    /// What follows the title while a newer version is pending, in the full or
    /// the `compact` wording; `None` when there is nothing to say.
    pub fn title_suffix(&self, compact: bool) -> Option<String> {
        self.pending_update
            .as_ref()
            .and_then(|status| status.title_suffix(compact))
    }

    /// Quit, unless a removal is still in flight — quitting then would orphan
    /// the git child: it finishes removing the tree on disk, but the result is
    /// never folded, so the saved config keeps a phantom entry. A second
    /// request forces the quit anyway.
    pub(super) fn request_quit(&mut self) {
        if !self.worktree_sync.any_removal_running() || self.quit_pending {
            self.should_quit = true;
        } else {
            self.quit_pending = true;
            self.notify(
                "Waiting for a worktree deletion to finish — press again to force quit",
                Severity::Information,
            );
        }
    }

    /// Undo the global tmux state this run turned on.
    ///
    /// Called from the main loop's exit; a `SIGTERM` still skips it, which is
    /// the same gap every tmux command has and not one a signal handler can
    /// close (spawning a process from one is not async-signal-safe).
    pub fn release_tmux_options(&mut self) {
        if std::mem::take(&mut self.focus_events_owned) {
            tmux::disable_focus_events();
        }
    }

    // ------------------------------------------------------------------ startup

    /// Work kicked off once the terminal is up.
    pub fn on_start(&mut self) {
        if self.input_modes.focus {
            match tmux::ensure_focus_events() {
                tmux::FocusEvents::Ours => self.focus_events_owned = true,
                tmux::FocusEvents::AlreadyOn => {}
                tmux::FocusEvents::Unavailable => {
                    self.notify("Could not enable focus events", Severity::Warning);
                }
            }
        } else {
            // Declining the mode also means clearing one a killed run left on:
            // the flag exists for people whose prefix key it is eating.
            tmux::release_stranded_focus_events();
        }
        if self.state.selection.repository_id.is_none()
            && let Some(first) = self.state.repositories().first()
        {
            let id = first.id;
            self.state.select_repository(id);
            self.rebuild_rows();
            self.sync_sidebar_index();
        }
        self.reload_detail();
        self.scan_all_worktrees();
        self.load_gh_status();
        if self.self_update {
            self.check_for_update();
        }
        self.check_plugin_version();
        self.probe_claude_cli();
    }

    /// Ask the installed Claude, once, whether it knows `--remote-control`.
    /// Until it answers, launches keep the flag; a definite no drops it from
    /// every later launch, and says so while the setting is on — otherwise the
    /// checkbox would read ticked over sessions that are not reachable.
    fn probe_claude_cli(&self) {
        let tx = self.tx.clone();
        let wanted = self.settings.remote_control;
        tokio::spawn(async move {
            let answer = crate::services::claude_cli::probe_remote_control().await;
            if wanted && answer == Some(false) {
                tx.notify(
                    "This Claude Code has no --remote-control; sessions start without \
                     Remote Control until Claude is updated",
                    Severity::Warning,
                );
            }
        });
    }

    /// Nag once per launch when the installed Claude integration is behind
    /// this build. Off the loop like every other startup check: the status
    /// read hashes the installed files, and a slow home directory must not
    /// stall the first frame.
    fn check_plugin_version(&self) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let notice = tokio::task::spawn_blocking(|| {
                crate::services::claude_plugin::upgrade_notice(
                    &crate::services::claude_plugin::status(),
                )
            })
            .await
            .ok()
            .flatten();
            if let Some(notice) = notice {
                tx.notify(notice, Severity::Warning);
            }
        });
    }

    /// Kick a directory walk for one path's sessions. This is what finds
    /// sessions that appeared or disappeared; [`Self::refresh_visible_sessions`]
    /// is what keeps the ones already on screen current.
    fn scan_sessions(&mut self, path: &str) {
        // One scan per path at a time: bouncing across the sidebar must not
        // stack duplicate directory walks for content the cache already shows.
        if !self.sessions_in_flight.insert(path.to_string()) {
            return;
        }
        let tx = self.tx.clone();
        let session_path = path.to_string();
        let event_path = path.to_string();
        // Cloned here so the task needs nothing from state: pins are the app's
        // to own, the scan only honours them.
        let pinned = self.state.pinned_for(path);
        tokio::spawn(async move {
            let sessions = tokio::task::spawn_blocking(move || {
                claude_session::get_sessions_with_pins(&session_path, SESSION_LIMIT, &pinned)
            })
            .await
            .ok();
            tx.send(AppEvent::Sessions {
                path: event_path,
                sessions,
            });
        });
    }

    /// Ask where every Claude session is running — stamped tmux windows plus
    /// plugin heartbeats — off the loop.
    pub(super) fn scan_live_sessions(&mut self) {
        if self.live_scan_in_flight {
            return;
        }
        self.live_scan_in_flight = true;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let sessions = tokio::task::spawn_blocking(crate::services::live::snapshot)
                .await
                .unwrap_or_default();
            tx.send(AppEvent::LiveSessions { sessions });
        });
    }

    /// Which session card the focused detail control belongs to, if any.
    /// Read from the drawn snapshot, like every other act on the focus.
    fn focused_session_index(&self) -> Option<usize> {
        match self.drawn_items.get(self.detail_index)? {
            (
                DetailItem::Action(
                    Action::ResumeSession(index)
                    | Action::ResumeYolo(index)
                    | Action::RenameSession(index)
                    | Action::TogglePinSession(index)
                    | Action::DeleteSession(index)
                    | Action::ResumeCustom { session: index, .. },
                ),
                _,
            ) => Some(*index),
            _ => None,
        }
    }

    /// Move the pinned session under the cursor one slot up or down within
    /// the pinned block, keeping the cursor on the card it moved with.
    /// Returns false when the cursor is not on a pinned card, so the caller
    /// can let the key fall through.
    pub(super) fn move_focused_pin(&mut self, delta: isize) -> bool {
        let Some(path) = self.state.selected_path() else {
            return false;
        };
        let Some(index) = self.focused_session_index() else {
            return false;
        };
        let Some(id) = self
            .sessions
            .as_ref()
            .and_then(|list| list.get(index))
            .map(|s| s.id.clone())
        else {
            return false;
        };
        if !self.state.is_pinned(&path, &id) {
            return false;
        }
        if !self.state.move_pin(&path, &id, delta) {
            // Pinned but already at the end of the pinned block: the key was
            // still for us, it just has nowhere to go.
            return true;
        }
        self.sort_visible_sessions();
        // Every card carries the same controls, so the moved card's slots sit
        // exactly one card-width away and the cursor can ride along.
        let per_card = 5 + self.settings.custom_buttons.len();
        let target = self.detail_index as isize + delta * per_card as isize;
        if target >= 0 {
            self.detail_index = target as usize;
            self.detail_follow_focus = true;
        }
        self.report_save_error();
        self.redraw = true;
        true
    }

    /// Re-sort the visible session list around the current pin order.
    pub(super) fn sort_visible_sessions(&mut self) {
        let Some(path) = self.state.selected_path() else {
            return;
        };
        let pinned = self.state.pinned_for(&path);
        if let Some(list) = self.sessions.as_mut() {
            claude_session::sort_sessions(list, &pinned);
        }
    }

    /// Re-read every session card on screen, each in its own task.
    ///
    /// Coming back to forestui is exactly when the conversations it lists have
    /// moved on: turns were taken in other windows while it sat in the
    /// background. One task per card means each answers as soon as its own
    /// transcript is read, and the card carries a spinner until it does — so
    /// the pane fills in rather than sitting still and then blinking.
    pub fn refresh_visible_sessions(&mut self) {
        let Some(path) = self.state.selected_path() else {
            return;
        };
        // A directory walk alongside, for sessions that were started or removed
        // while we were away; the per-card reads cannot see those.
        self.scan_sessions(&path);

        let Some(sessions) = self.sessions.as_ref() else {
            return;
        };
        for id in sessions.iter().map(|s| s.id.clone()).collect::<Vec<_>>() {
            if !self.sessions_refreshing.insert(id.clone()) {
                continue;
            }
            let tx = self.tx.clone();
            let read_path = path.clone();
            let read_id = id.clone();
            let event_path = path.clone();
            tokio::spawn(async move {
                let session = tokio::task::spawn_blocking(move || {
                    claude_session::refresh_one(&read_path, &read_id)
                })
                .await
                .ok()
                .flatten();
                tx.send(AppEvent::SessionRefreshed {
                    path: event_path,
                    id,
                    session: Box::new(session),
                });
            });
        }
    }

    /// Reconcile every repository against `git worktree list`. One cheap git
    /// subprocess per repository, so this runs on startup, on tmux focus
    /// return, and on the periodic sweep.
    fn scan_all_worktrees(&mut self) {
        let ids: Vec<Uuid> = self.state.repositories().iter().map(|r| r.id).collect();
        for id in ids {
            self.scan_worktrees(id);
        }
    }

    /// Kick a background `git worktree list` for one repository. The listing
    /// comes back as [`AppEvent::WorktreesScanned`] and is reconciled against
    /// the config on the main loop — never here, where state may move while
    /// git runs.
    pub(super) fn scan_worktrees(&mut self, repo_id: Uuid) {
        let Some(repo) = self.state.find_repository(repo_id) else {
            return;
        };
        let source = repo.source_path.clone();
        let Some(scan) = self.worktree_sync.start_scan(repo_id) else {
            return;
        };
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let listed = git::list_worktrees(&source).await.ok();
            tx.send(AppEvent::WorktreesScanned { scan, listed });
        });
        self.check_finished(repo_id);
    }

    /// Kick a background check of which of a repository's linked worktrees
    /// are finished. Rides on every worktree scan, throttled per repository
    /// by [`finished::MIN_INTERVAL`] since a selection change rescans too.
    fn check_finished(&mut self, repo_id: Uuid) {
        let Some(repo) = self.state.find_repository(repo_id) else {
            return;
        };
        let Some(ticket) = self.finished.start(repo_id, Instant::now()) else {
            return;
        };
        let source = repo.source_path.clone();
        let linked = finished::linked(repo);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let verdicts = finished::check(&source, &linked).await;
            tx.send(AppEvent::WorktreesFinished { ticket, verdicts });
        });
    }

    /// Keep the binary current, the way the Python build did on every launch.
    ///
    /// Deliberately fire-and-forget on a background task: the UI is already up
    /// by the time this runs, so a slow or unreachable GitHub costs nothing but
    /// a title that never changes. Being offline stays silent — it is the
    /// common case and not the user's problem to solve mid-session — but a
    /// *persistent* install failure (an unwritable install dir) is theirs to
    /// fix and surfaces once per launch.
    fn check_for_update(&self) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let status = crate::version_check::update_if_stale().await;
            tx.send(AppEvent::UpdateChecked(status));
        });
    }

    /// Fold the update check's verdict. A failure is a toast; a newer version
    /// marks the title bar for the rest of the run, the way the Python build
    /// (and Claude Code) did — a toast times out while the user is looking at
    /// something else, and "restart to update" stays true until they do.
    fn apply_update_status(&mut self, status: crate::version_check::UpdateStatus) {
        if let Some((message, severity)) = status.notification() {
            self.notify(message, severity);
        }
        if status.title_suffix(false).is_some() {
            self.pending_update = Some(status);
        }
    }

    fn load_gh_status(&self) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let (status, username) = github::get_auth_status().await;
            tx.send(AppEvent::GhStatus(status, username));
        });
    }

    // ------------------------------------------------------------------ sidebar

    pub fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        for repo in self.state.repositories() {
            let active = repo.active_worktrees();
            rows.push(SidebarRow::Repository {
                id: repo.id,
                name: repo.name.clone(),
                has_worktrees: !active.is_empty(),
                collapsed: self.collapsed.contains(&repo.id),
            });
            if self.collapsed.contains(&repo.id) {
                continue;
            }
            let last_index = active.len().saturating_sub(1);
            for (index, worktree) in active.iter().enumerate() {
                rows.push(SidebarRow::Worktree {
                    repo_id: repo.id,
                    id: worktree.id,
                    name: worktree.name.clone(),
                    branch: informative_branch(
                        &worktree.branch,
                        &worktree.name,
                        &self.settings.branch_prefix,
                    ),
                    is_last: index == last_index,
                    finished: self.finished.contains(worktree.id),
                });
            }
        }

        if self.state.show_archived && self.state.has_archived_worktrees() {
            rows.push(SidebarRow::ArchivedHeader);
            for repo in self.state.repositories() {
                for worktree in repo.archived_worktrees() {
                    rows.push(SidebarRow::ArchivedWorktree {
                        repo_id: repo.id,
                        id: worktree.id,
                        name: worktree.name.clone(),
                        repo_name: repo.name.clone(),
                    });
                }
            }
        }

        self.rows = rows;
        if self.sidebar_index >= self.rows.len() {
            self.sidebar_index = self.rows.len().saturating_sub(1);
        }
    }

    /// The repository's finished worktrees still tracked, in sidebar order:
    /// what Prune would remove. Archived ones count — finished is about the
    /// work item, not about where the worktree is shown.
    pub fn finished_worktrees(&self, repo_id: Uuid) -> Vec<(Uuid, String)> {
        self.state
            .find_repository(repo_id)
            .map(|repo| {
                repo.worktrees
                    .iter()
                    .filter(|worktree| self.finished.contains(worktree.id))
                    .map(|worktree| (worktree.id, worktree.name.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Move the sidebar cursor onto whatever is currently selected.
    fn sync_sidebar_index(&mut self) {
        let selection = self.state.selection;
        if let Some(index) = self.rows.iter().position(|row| match row.ids() {
            Some((repo_id, worktree_id)) => {
                selection.repository_id == Some(repo_id) && selection.worktree_id == worktree_id
            }
            None => false,
        }) {
            self.sidebar_index = index;
        }
    }

    fn move_sidebar(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as isize;
        let mut next = self.sidebar_index as isize + delta;
        next = next.clamp(0, len - 1);
        if next == self.sidebar_index as isize {
            return;
        }
        self.sidebar_index = next as usize;
        self.select_current_row();
    }

    /// Selecting follows the cursor, matching the Textual tree's highlight
    /// behaviour where moving the cursor loads the detail pane.
    fn select_current_row(&mut self) {
        let Some(row) = self.rows.get(self.sidebar_index).cloned() else {
            return;
        };
        match row.ids() {
            Some((repo_id, Some(worktree_id))) => self.state.select_worktree(repo_id, worktree_id),
            Some((repo_id, None)) => self.state.select_repository(repo_id),
            None => return,
        }
        self.reload_detail();
        // Landing on a repository (or one of its worktrees) is the moment its
        // tree should be fresh — an agent may have added one since last look.
        if let Some(repo_id) = self.state.selection.repository_id {
            self.scan_worktrees(repo_id);
        }
    }

    // ------------------------------------------------------------------- detail

    /// The list of focusable items the detail pane would render right now.
    ///
    /// Derived from the same [`detail::content`] walk the renderer draws, so
    /// item N here is control N on screen by construction. The running app
    /// acts on the [`App::drawn_items`] snapshot instead; this live view
    /// exists for the tests that assert on pane structure.
    #[cfg(test)]
    pub fn detail_items(&self) -> Vec<DetailItem> {
        detail::items(&detail::content(self))
    }

    /// Reload everything the detail pane shows for the current selection.
    pub fn reload_detail(&mut self) {
        self.detail_index = 0;
        // A new selection starts at the top; keeping the old offset would open
        // the pane part-way down a different repository's content.
        self.detail_scroll = 0;
        self.detail_follow_focus = true;
        self.sessions = None;
        self.issues = None;
        // Nothing of the new selection is on screen yet, and activating a
        // control from the previous pane against the new selection would act
        // on the wrong thing entirely.
        self.drawn_items.clear();
        // A Start work confirm screen belongs to the selection that started it,
        // and a jump to the worktree that was selected.
        self.pending_start = None;
        self.pending_jump = None;

        let Some(path) = self.state.selected_path() else {
            self.meta = DetailMeta::default();
            return;
        };
        let is_repository = self.state.selection.is_repository();

        // Reset the rename fields whenever the selected worktree changes.
        if let Some((_repo, worktree)) = self.state.selected_worktree() {
            if self.rename_target != Some(worktree.id) {
                self.rename_target = Some(worktree.id);
                self.name_input = TextInput::new(worktree.name.clone());
                self.branch_input = TextInput::new(worktree.branch.clone());
            }
        } else {
            self.rename_target = None;
        }

        self.meta = DetailMeta {
            path: path.clone(),
            path_exists: crate::util::path_exists(&path),
            ..DetailMeta::default()
        };

        let tx = self.tx.clone();
        let meta_path = path.clone();
        tokio::spawn(async move {
            let mut meta = DetailMeta {
                path: meta_path.clone(),
                path_exists: crate::util::path_exists(&meta_path),
                ..DetailMeta::default()
            };
            // Three independent subprocesses; run them concurrently so the
            // header appears after one git round-trip rather than three. The
            // branch answer goes unused for a worktree, which costs one spawn
            // and saves a second await chain.
            let (branch, commit, has_remote) = tokio::join!(
                git::get_current_branch(&meta_path),
                git::get_latest_commit(&meta_path),
                git::has_remote_tracking(&meta_path),
            );
            if is_repository {
                meta.branch = branch.ok();
            }
            if let Ok(commit) = commit {
                meta.commit_hash = Some(commit.short_hash);
                meta.commit_time = Some(commit.timestamp);
                meta.has_remote = has_remote.unwrap_or(false);
            }
            tx.send(AppEvent::Meta(Box::new(meta)));
        });

        // Cache-first: what was shown for this selection before renders again
        // synchronously, and the fresh scan lands behind it. The alternative —
        // dropping to "Loading…" on every switch with a warm cache in hand —
        // is the flash issue #29 is about.
        self.sessions = claude_session::peek_sessions(&path, SESSION_LIMIT);
        // The cache holds plain recency order; the pane shows pins first.
        self.sort_visible_sessions();
        // A new selection has nothing of the old one's refreshes to wait for.
        self.sessions_refreshing.clear();
        self.scan_sessions(&path);
        self.scan_live_sessions();

        if is_repository {
            // Read before the issues, whose scope they may set.
            self.reread_repository_settings(&path);
            // A key, once known, stands until Start work asks again; "no
            // GitHub repository" is asked again on every selection, since a
            // remote may have been added since.
            if !matches!(self.repo_key(&path), Some(Some(_))) {
                self.probe_github(path.clone(), false);
            }
            self.load_issues(&path);
        }
    }

    /// Show the repository pane's issues for `path`: the cached list for the
    /// scope in effect, refreshed behind it when stale or missing.
    fn load_issues(&mut self, path: &str) {
        match github::peek_issues(path, self.start_work_for(path).scope) {
            // Fresh enough to stand on its own; the 300s tick refreshes.
            Some((issues, true)) => self.issues = Some(issues),
            // Stale content beats a spinner: render it and refresh behind.
            Some((issues, false)) => {
                self.issues = Some(issues);
                self.fetch_issues_for(path.to_string());
            }
            None => {
                self.issues = None;
                self.fetch_issues_for(path.to_string());
            }
        }
    }

    fn fetch_issues(&mut self) {
        let Some(repo) = self.state.selected_repository() else {
            return;
        };
        self.fetch_issues_for(repo.source_path.clone());
    }

    /// Fetch the issues for one repository's checkout. The result folds into
    /// whichever of the pane and an open Start work still want that path.
    pub(super) fn fetch_issues_for(&mut self, path: String) {
        // One fetch per path at a time — each one is a chain of gh
        // subprocesses and possibly a network call for an identical result.
        let scope = self.start_work_for(&path).scope;
        if !self.issues_in_flight.insert((path.clone(), scope)) {
            return;
        }
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let issues = github::list_issues(&path, ISSUE_LIMIT, scope).await;
            tx.send(AppEvent::Issues {
                path,
                scope,
                issues,
            });
        });
    }

    // ------------------------------------------------------------- notifications

    pub fn notify(&mut self, text: impl Into<String>, severity: Severity) {
        self.notifications.push(Notification {
            text: text.into(),
            severity,
            created: Instant::now(),
        });
        if self.notifications.len() > 4 {
            self.notifications.remove(0);
        }
    }

    /// Announce a state change that has just been persisted — unless it wasn't.
    ///
    /// The success toast is only true if the config write landed. A dropped
    /// write is silent otherwise, and the config deliberately loads as empty
    /// state, so the user would be told a worktree exists that vanishes on the
    /// next launch.
    pub fn notify_saved(&mut self, text: impl Into<String>) {
        match self.state.take_save_error() {
            Some(error) => self.notify(
                format!("Could not save the config: {error}"),
                Severity::Error,
            ),
            None => self.notify(text, Severity::Information),
        }
    }

    /// Report a failed write for a change that announces nothing when it works.
    fn report_save_error(&mut self) {
        if let Some(error) = self.state.take_save_error() {
            self.notify(
                format!("Could not save the config: {error}"),
                Severity::Error,
            );
        }
    }

    fn expire_notifications(&mut self) {
        self.notifications
            .retain(|n| n.created.elapsed() < NOTIFICATION_TTL);
    }

    /// The 100ms heartbeat. It earns a repaint only when something on screen
    /// actually moved — a spinner frame, a toast expiring — so an idle app
    /// paints nothing at all rather than ten frames a second.
    fn on_tick(&mut self) {
        self.spinner_index = self.spinner_index.wrapping_add(1);
        if self.issue_spinner_visible() || !self.sessions_refreshing.is_empty() {
            self.redraw = true;
        }

        // The relative times on screen ("11 seconds ago") have second
        // granularity, so once a second is exactly as often as they can
        // change. Repainting every tick — ten times a second — kept the
        // terminal busy for content that mostly cannot differ; repainting
        // never leaves them frozen at whatever the last keypress showed.
        if self.spinner_index.is_multiple_of(10) {
            self.redraw = true;
        }

        let before = self.notifications.len();
        self.expire_notifications();
        if self.notifications.len() != before {
            self.redraw = true;
        }

        if let Some(modal) = self.modals.last_mut()
            && modal.tick()
        {
            self.redraw = true;
        }

        if self.last_worktree_scan.elapsed() >= WORKTREE_SCAN_INTERVAL {
            self.last_worktree_scan = Instant::now();
            // No repaint here: a sweep that changes nothing stays invisible,
            // and one that does pays for its own when the fold lands.
            self.scan_all_worktrees();
        }

        if self.last_session_refresh.elapsed() >= SESSION_REFRESH_INTERVAL {
            self.last_session_refresh = Instant::now();
            // The directory walk only — not the per-card refresh. A cached scan
            // costs a fraction of a millisecond and repaints nothing when it
            // finds nothing, whereas the per-card pass raises a spinner on
            // every card, and one that clears before the next frame is a
            // flicker every ten seconds with nothing to wait for.
            if let Some(path) = self.state.selected_path() {
                self.scan_sessions(&path);
            }
            // The live map ages the same way: a window closed in another tab
            // should drop its badge without being asked. Its fold repaints
            // only on change, so the common no-change sweep stays free.
            self.scan_live_sessions();
        }

        if self.last_issue_refresh.elapsed() >= ISSUE_REFRESH_INTERVAL {
            self.last_issue_refresh = Instant::now();
            // Only while a repository pane is actually showing its issues —
            // refreshing under a worktree pane would churn subprocesses (and
            // destroy a warm cache) for content that is not on screen.
            if self.state.selection.is_repository()
                && let Some(repo) = self.state.selected_repository()
            {
                // Invalidate only the repository being refreshed. Nuking the
                // whole cache here made the next selection change on every
                // other repository a guaranteed cold miss — spinner included.
                let path = repo.source_path.clone();
                github::invalidate_cache(&path);
                // No repaint yet: nothing changes on screen until the result
                // arrives, and that event pays for its own.
                self.fetch_issues();
            }
        }
    }

    /// Whether the issue-refresh spinner — the only consumer of
    /// `spinner_index` — is on screen this frame.
    fn issue_spinner_visible(&self) -> bool {
        self.modals.is_empty() && self.state.selection.is_repository() && self.issues.is_none()
    }

    // ------------------------------------------------------------------- events

    pub fn handle_event(&mut self, event: AppEvent) {
        // Everything except a bare pointer move or a tick changes the frame.
        // Motion is asked about rather than cleared afterwards so that a batch
        // of [Notify, Moved] still repaints: a no-op move must never cancel a
        // repaint some other event in the same drain already earned. The tick
        // decides for itself in `on_tick`: at ten a second, repainting an idle
        // frame on every one keeps the terminal permanently busy for nothing.
        // Scan results opt out for the same reason as the tick — the periodic
        // sweep almost always finds nothing, and its fold claims a repaint
        // itself when it changes something.
        if !mouse::is_pointer_motion(&event)
            && !matches!(
                event,
                AppEvent::Tick
                    | AppEvent::WorktreesScanned { .. }
                    | AppEvent::WorktreesFinished { .. }
                    | AppEvent::Sessions { .. }
                    | AppEvent::LiveSessions { .. }
            )
        {
            self.redraw = true;
        }
        match event {
            AppEvent::Term(term_event) => self.handle_term_event(term_event),
            AppEvent::Tick => self.on_tick(),
            AppEvent::GhStatus(status, username) => {
                self.gh_status = status.display(username.as_deref());
            }
            AppEvent::SessionRefreshed { path, id, session } => {
                self.sessions_refreshing.remove(&id);
                // A result for a selection the user has already left is not
                // wrong, just irrelevant — and folding it would put another
                // path's conversation into this pane. Keyed on the pane's own
                // path, as the directory scan is: two notions of "current"
                // eventually disagree.
                if path != self.meta.path {
                    return;
                }
                if let Some(list) = self.sessions.as_mut() {
                    match *session {
                        // Gone from disk: drop the card rather than leave a
                        // Resume button pointing at nothing.
                        None => list.retain(|s| s.id != id),
                        Some(fresh) => {
                            if let Some(slot) = list.iter_mut().find(|s| s.id == id) {
                                *slot = fresh;
                            }
                        }
                    }
                    // A turn taken while we were away makes a session the most
                    // recent one — within the recency half of the list, which
                    // pins always precede.
                    self.sort_visible_sessions();
                }
            }
            AppEvent::Sessions { path, sessions } => {
                // Release the guard even for a result the checks below drop.
                self.sessions_in_flight.remove(&path);
                // A failed scan still answers a jump: the cache is all there is.
                if let Some(jump) = self.pending_jump.as_mut()
                    && jump.path == path
                {
                    jump.sessions = true;
                }
                // A failed scan (None) keeps whatever the cache painted; an
                // empty *successful* scan is real content and lands normally.
                if let Some(mut sessions) = sessions
                    && path == self.meta.path
                {
                    // The scan already sorted around the pins it was handed,
                    // but a pin toggled while it ran wins here.
                    claude_session::sort_sessions(&mut sessions, &self.state.pinned_for(&path));
                    if self.sessions.as_deref() != Some(sessions.as_slice()) {
                        // Claims its own repaint, like the worktree sweep: this
                        // runs on a timer and almost always finds exactly what
                        // is already on screen.
                        self.sessions = Some(sessions);
                        self.redraw = true;
                    }
                }
                self.finish_jump();
            }
            AppEvent::LiveSessions { sessions } => {
                self.live_scan_in_flight = false;
                // The snapshot already decided what counts as live: a window
                // whose Claude exited stays out (that session is free again —
                // holding it hostage to a shell prompt helped nobody), and a
                // heartbeat only survives while its claude process does.
                let fresh: std::collections::HashMap<String, crate::services::live::LiveSession> =
                    sessions
                        .into_iter()
                        .map(|session| (session.session_id.clone(), session))
                        .collect();
                // Claims its own repaint: this runs on the ten-second sweep
                // and usually finds exactly the map it replaced.
                if fresh != self.live_sessions {
                    self.live_sessions = fresh;
                    self.redraw = true;
                }
                if let Some(jump) = self.pending_jump.as_mut() {
                    jump.live = true;
                }
                self.finish_jump();
            }
            AppEvent::SessionDeleted {
                path,
                session_id,
                title,
                result,
            } => match result {
                Ok(()) => {
                    // The pin dies with the transcript, whichever path it was
                    // pinned under — this is the one the card was deleted from.
                    self.state.remove_pin(&path, &session_id);
                    if path == self.meta.path
                        && let Some(list) = self.sessions.as_mut()
                    {
                        list.retain(|s| s.id != session_id);
                    }
                    self.notify(
                        format!("Deleted session '{}'", crate::util::truncate(&title, 40)),
                        Severity::Information,
                    );
                }
                Err(error) => self.notify(
                    format!("Could not delete the session: {error}"),
                    Severity::Error,
                ),
            },
            AppEvent::SessionRenamed {
                path,
                session_id,
                session,
                result,
            } => match result {
                Ok(()) => {
                    if path == self.meta.path
                        && let Some(list) = self.sessions.as_mut()
                        && let Some(fresh) = *session
                        && let Some(slot) = list.iter_mut().find(|s| s.id == session_id)
                    {
                        *slot = fresh;
                    }
                    self.notify("Session renamed", Severity::Information);
                }
                Err(error) => self.notify(
                    format!("Could not rename the session: {error}"),
                    Severity::Error,
                ),
            },
            AppEvent::Issues {
                path,
                scope,
                issues,
            } => {
                self.issues_in_flight.remove(&(path.clone(), scope));
                // Listed for a scope the repository's settings have since
                // replaced; the refetch for the new one is on its way.
                if scope != self.start_work_for(&path).scope {
                    return;
                }
                if let Some(Modal::StartWork(flow)) = self.modals.last_mut()
                    && flow.repo_path == path
                {
                    flow.set_issues(issues.clone());
                }
                if self.state.selection.is_repository()
                    && Some(path.as_str())
                        == self
                            .state
                            .selected_repository()
                            .map(|r| r.source_path.as_str())
                {
                    self.issues = Some(issues);
                }
            }
            AppEvent::PullRequests { path, prs, viewer } => {
                // The flow may be beneath its confirm screen by now.
                for modal in &mut self.modals {
                    if let Modal::StartWork(flow) = modal
                        && flow.repo_path == path
                    {
                        flow.set_pull_requests(prs.clone(), viewer.clone());
                    }
                }
            }
            AppEvent::GitHubChecked {
                path,
                repo,
                for_start_work,
            } => self.on_github_checked(path, repo, for_start_work),
            AppEvent::Meta(meta) => {
                if meta.path == self.meta.path {
                    self.meta = *meta;
                }
            }
            AppEvent::Branches {
                repo_id,
                list,
                target,
            } => self.on_branches(repo_id, list, target),
            AppEvent::FetchFailed(error) => {
                if let Some(Modal::StartWorkConfirm(modal)) = self.modals.last_mut() {
                    modal.fetch_failed();
                }
                self.notify(format!("Fetch failed: {error}"), Severity::Error);
            }
            AppEvent::Notify(text, severity) => self.notify(text, severity),
            AppEvent::UpdateChecked(status) => self.apply_update_status(status),
            AppEvent::WorktreeAdded {
                create,
                worktree,
                first_prompt,
                yolo,
            } => {
                let repo_id = create.repo_id();
                self.worktree_sync.finish_create(create);
                // The repository can be removed while git runs; dropping the
                // result beats resurrecting an entry nothing owns.
                if self.state.find_repository(repo_id).is_none() {
                    return;
                }
                let worktree_id = worktree.id;
                let name = worktree.name.clone();
                let path = worktree.path.clone();
                self.state.add_worktree(repo_id, *worktree);
                // Selecting a row a collapsed repository hides would leave the
                // cursor pointing at nothing, so the fold opens to reveal what
                // the user just created.
                self.collapsed.remove(&repo_id);
                self.state.select_worktree(repo_id, worktree_id);
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
                // Announced here rather than from the task, so a result that
                // was dropped above never produces a success toast for a
                // worktree that is not in the config — and `notify_saved`
                // extends that to a write that was attempted and failed.
                self.notify_saved(format!("Created worktree '{name}'"));
                // Named after the worktree itself, with no prefix: the window,
                // the session and the worktree carry one recognisable name.
                if let Some(prompt) = first_prompt {
                    self.start_claude(&path, None, yolo, None, Some(name), Some(&prompt));
                }
            }
            AppEvent::PromptEdited {
                repo_id,
                link,
                result,
            } => {
                // The screen the editor was opened from, still on top; any
                // other has its own prompt, which this text is not.
                let Some(Modal::StartWorkConfirm(confirm)) = self.modals.last_mut() else {
                    return;
                };
                if confirm.repo_id != repo_id || confirm.item.link() != link {
                    return;
                }
                match result {
                    Ok(text) => {
                        if !confirm.finish_edit(&text) {
                            self.notify("Prompt left empty; kept as it was", Severity::Warning);
                        }
                    }
                    Err(error) => {
                        confirm.is_editing = false;
                        self.notify(
                            format!("Could not edit the prompt: {error}"),
                            Severity::Error,
                        );
                    }
                }
            }
            AppEvent::WorktreesFinished { ticket, verdicts } => {
                // Only the worktrees the repository has now: one removed
                // while the check ran, or another repository's, is not
                // this result's to mark.
                let current = self
                    .state
                    .find_repository(ticket.repo_id())
                    .map(|repo| repo.worktrees.iter().map(|w| w.id).collect());
                if self.finished.fold(ticket, &verdicts, current.as_ref()) {
                    self.redraw = true;
                    self.rebuild_rows();
                }
            }
            AppEvent::WorktreesScanned { scan, listed } => {
                let repo_id = scan.repo_id();
                let worktree_sync::ScanVerdict::Reconcile { listed, protected } =
                    self.worktree_sync.judge_scan(scan, listed)
                else {
                    return;
                };
                // The repository can be removed while git runs.
                if self.state.find_repository(repo_id).is_none() {
                    return;
                }
                let selected = self.state.selection;
                let Some(outcome) = self.state.reconcile_worktrees(repo_id, &listed, &protected)
                else {
                    return;
                };
                self.redraw = true;
                // Changed worktrees are *sidebar rows*; no detail pane shows
                // them, so the pane is left alone — reloading it would reset
                // the scroll of whatever the user is reading, twice a minute.
                self.rebuild_rows();
                self.sync_sidebar_index();
                // …unless the reconcile disturbed the selection itself. A
                // pruned selection reloads the pane outright. A branch that
                // drifted on the selected worktree reseeds only the rename
                // inputs — Enter would otherwise rename the branch straight
                // back to the stale value they still show — but must not
                // reload the pane: the path is unchanged, so its sessions and
                // scroll position are not stale, and a full reload on every
                // external `git checkout` would yank them out from under the
                // reader twice a minute.
                if self.state.selection != selected {
                    self.rename_target = None;
                    self.reload_detail();
                } else if selected
                    .worktree_id
                    .is_some_and(|id| outcome.branch_updated.contains(&id))
                    && let Some((_, worktree)) = self.state.selected_worktree()
                {
                    self.rename_target = Some(worktree.id);
                    self.name_input = TextInput::new(worktree.name.clone());
                    self.branch_input = TextInput::new(worktree.branch.clone());
                }
                let mut parts: Vec<String> = Vec::new();
                match outcome.added.as_slice() {
                    [] => {}
                    [name] => parts.push(format!("Detected worktree '{name}'")),
                    added => parts.push(format!("Detected {} worktrees", added.len())),
                }
                match outcome.removed.as_slice() {
                    [] => {}
                    [name] => parts.push(format!("Worktree '{name}' was removed outside forestui")),
                    removed => parts.push(format!(
                        "{} worktrees were removed outside forestui",
                        removed.len()
                    )),
                }
                if parts.is_empty() {
                    // A branch-only correction saved, but is not worth a toast.
                    self.report_save_error();
                } else {
                    self.notify_saved(parts.join("; "));
                }
            }
            AppEvent::WorktreeRenamed { change, name, path } => {
                let worktree_id = change.worktree_id();
                self.worktree_sync.finish(change);
                self.state.update_worktree(worktree_id, |worktree| {
                    worktree.name = name;
                    worktree.path = path;
                });
                // The rename fields are only re-seeded when the *target*
                // changes, so without this they keep whatever the pane last
                // showed — the pre-rename name, if the user navigated away and
                // back while the task ran. Pressing Enter would then rename the
                // worktree straight back.
                self.rename_target = None;
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
                self.report_save_error();
            }
            AppEvent::WorktreeBranchRenamed { change, branch } => {
                let worktree_id = change.worktree_id();
                self.worktree_sync.finish(change);
                self.state
                    .update_worktree(worktree_id, |worktree| worktree.branch = branch);
                self.rename_target = None;
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
                self.report_save_error();
            }
            AppEvent::WorktreeRemoveResult { change, outcome } => {
                self.on_worktree_remove_result(change, outcome)
            }
            AppEvent::WorktreesPruned { removals, busy } => {
                self.on_worktrees_pruned(removals, busy)
            }
            AppEvent::WorktreeRenameAborted { change } => self.worktree_sync.finish(change),
            AppEvent::WorktreeAddFailed { create } => self.worktree_sync.finish_create(create),
            AppEvent::ReloadDetail => self.reload_detail(),
        }
    }

    /// Fold a finished removal attempt. State changes only here — the confirm
    /// handler just spawns git, so a dirty refusal arrives with the entry (and
    /// the user's uncommitted work) fully intact.
    fn on_worktree_remove_result(
        &mut self,
        change: worktree_sync::ChangeTicket,
        outcome: Result<git::RemoveOutcome, String>,
    ) {
        let worktree_id = change.worktree_id();
        self.worktree_sync.finish(change);
        // A quit that waited on this removal can go through now.
        if self.quit_pending && !self.worktree_sync.any_removal_running() {
            self.should_quit = true;
        }
        // The entry may have been removed by other means while git ran.
        let Some((_, worktree)) = self.state.find_worktree(worktree_id) else {
            return;
        };
        let name = worktree.name.clone();
        match outcome {
            Ok(git::RemoveOutcome::Removed) => {
                self.state.remove_worktree(worktree_id);
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
                self.report_save_error();
            }
            Ok(git::RemoveOutcome::Dirty(summary)) => {
                // The name is capped so the summary — the one thing the user
                // needs before confirming — never clips off the dialog edge.
                let display = crate::util::truncate(&name, 40);
                self.modals.push(Modal::Confirm(
                    crate::modal::ConfirmModal::new(
                        "Uncommitted Changes",
                        format!(
                            "'{display}' has uncommitted changes:\n{summary}\nDeleting will discard them permanently."
                        ),
                        crate::modal::ConfirmAction::ForceDeleteWorktree(worktree_id),
                    )
                    // Pushed by a background event, not a user action — a
                    // keystroke already in the queue must not confirm it.
                    .with_arm_delay(),
                ));
            }
            Err(error) => {
                self.notify(
                    format!("Could not delete '{name}': {error}"),
                    Severity::Error,
                );
            }
        }
    }

    /// Fold a prune: finish every removal it started, drop the worktrees
    /// git removed, and say in one notification what was pruned and what
    /// was skipped.
    fn on_worktrees_pruned(
        &mut self,
        removals: Vec<(worktree_sync::ChangeTicket, String, crate::prune::Outcome)>,
        busy: Vec<String>,
    ) {
        let mut outcomes = Vec::with_capacity(removals.len() + busy.len());
        let mut removed_any = false;
        for (change, name, outcome) in removals {
            let worktree_id = change.worktree_id();
            self.worktree_sync.finish(change);
            if matches!(outcome, crate::prune::Outcome::Removed { .. })
                && self.state.find_worktree(worktree_id).is_some()
            {
                self.state.remove_worktree(worktree_id);
                removed_any = true;
            }
            outcomes.push((name, outcome));
        }
        outcomes.extend(busy.into_iter().map(|name| {
            (
                name,
                crate::prune::Outcome::Skipped(crate::prune::Skip::Busy),
            )
        }));
        if self.quit_pending && !self.worktree_sync.any_removal_running() {
            self.should_quit = true;
        }
        if removed_any {
            self.rebuild_rows();
            self.sync_sidebar_index();
            self.reload_detail();
            self.report_save_error();
        }
        let report = crate::prune::report(&outcomes);
        self.notify(report.lines.join(" "), report.severity);
    }

    fn handle_term_event(&mut self, event: ratatui::crossterm::event::Event) {
        use ratatui::crossterm::event::{Event, KeyEventKind};
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            // tmux focus-events let the app refresh when the user comes back —
            // which is exactly when an agent in another window may have
            // created or removed a worktree.
            Event::FocusGained => {
                // Not `reload_detail`: that is for a *new* selection and resets
                // the focus index and scroll, which would yank the pane out
                // from under someone who just switched back to the window.
                self.last_session_refresh = Instant::now();
                self.refresh_visible_sessions();
                self.scan_live_sessions();
                self.scan_all_worktrees();
                // The editor may have just saved this repository's settings.
                if self.state.selection.is_repository()
                    && let Some(path) = self.state.selected_path()
                {
                    self.reread_repository_settings(&path);
                }
            }
            _ => {}
        }
    }

    /// Handle everything already queued, and report how many events that was.
    ///
    /// The loop draws once per iteration, so without this a burst — a tick plus
    /// a click's press and release, or several background results landing
    /// together — repaints the screen once per event. That reads as the whole
    /// app flickering for a single user action.
    pub fn drain(&mut self, rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) -> usize {
        let mut handled = 0;
        while let Ok(queued) = rx.try_recv() {
            self.handle_event(queued);
            handled += 1;
            if self.should_quit {
                break;
            }
        }
        handled
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::models::{Repository, Selection, Worktree};

    /// An app backed by a throwaway forest directory, with one repository that
    /// has two active worktrees and one archived worktree.
    fn fixture(settings: Settings) -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = AppState::load_from(dir.path().join(".forestui-config.json"));

        let repo = Repository::new(
            "demo".into(),
            dir.path().join("demo").to_string_lossy().to_string(),
        );
        let repo_id = repo.id;
        state.add_repository(repo);

        for name in ["alpha", "beta"] {
            let worktree = Worktree::new(
                name.into(),
                format!("feat/{name}"),
                dir.path()
                    .join("forest")
                    .join(name)
                    .to_string_lossy()
                    .to_string(),
            );
            state.add_worktree(repo_id, worktree);
        }
        let archived = Worktree::new(
            "old".into(),
            "feat/old".into(),
            dir.path()
                .join("forest")
                .join("old")
                .to_string_lossy()
                .to_string(),
        );
        let archived_id = archived.id;
        state.add_worktree(repo_id, archived);
        state.set_archived(archived_id, true);
        state.selection = Selection {
            repository_id: Some(repo_id),
            worktree_id: None,
        };

        // A bare channel: `event::start` would add a detached stdin-polling
        // thread per test. Dropping the receiver is fine — sends are
        // best-effort and no test reads events back.
        let (tx, _rx) = crate::event::test_channel();
        let app = App::with_state(tx, state, settings);
        (dir, app)
    }

    /// [`app_with_fixture`] with explicit settings, for tests that need a
    /// non-default configuration (a saved theme, a branch prefix).
    pub fn app_with_settings(settings: Settings) -> (tempfile::TempDir, App) {
        fixture(settings)
    }

    pub fn app_with_fixture() -> (tempfile::TempDir, App) {
        fixture(Settings::default())
    }

    pub fn key(code: ratatui::crossterm::event::KeyCode) -> ratatui::crossterm::event::KeyEvent {
        use ratatui::crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    /// The same key with `Ctrl` held — what a pane is handed when tmux sends
    /// the prefix through, and what the app must not act on.
    pub fn ctrl(code: ratatui::crossterm::event::KeyCode) -> ratatui::crossterm::event::KeyEvent {
        use ratatui::crossterm::event::KeyModifiers;
        ratatui::crossterm::event::KeyEvent {
            modifiers: KeyModifiers::CONTROL,
            ..key(code)
        }
    }
}

#[cfg(test)]
mod tests;
