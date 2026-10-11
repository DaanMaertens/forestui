//! What the detail pane contains, independent of how it is drawn.
//!
//! This is the single source of truth for the pane. [`content`] walks the
//! current selection and produces the full sequence of nodes — headers, text,
//! cards, and every focusable control in order — and *both* consumers derive
//! from it: `App::detail_items` collects the focusable items for key handling,
//! and `ui/detail.rs` renders the nodes one by one, claiming a slot per
//! control as it goes. The two used to be written out twice, one list in each
//! file, with a comment demanding they be kept in the same order by hand;
//! deriving them from one walk makes that drift impossible rather than merely
//! tested against.

use super::App;
use crate::models::Speaker;
use crate::theme;
use crate::util::Noun;
use crate::work;
use ratatui::style::Style;

/// An actionable control in the detail pane — the immediate-mode stand-in for
/// Textual's buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    StartWork,
    Sync,
    Editor,
    Terminal,
    Files,
    ClaudeNew,
    ClaudeYolo,
    ClaudeCustom(usize),
    ResumeSession(usize),
    ResumeYolo(usize),
    ResumeCustom {
        button: usize,
        session: usize,
    },
    RenameSession(usize),
    TogglePinSession(usize),
    DeleteSession(usize),
    RefreshIssues,
    /// The issues header's "show blocked" checkbox, shared with Start work.
    ToggleBlockedIssues,
    /// An issue card's Start work: the confirm screen for that issue, by its
    /// position in the whole loaded list — hidden issues included.
    StartWorkOnIssue(usize),
    /// Choose one of the repository's settings files to open in an editor.
    RepositorySettings,
    /// Remove the repository's finished worktrees and their branches.
    PruneFinished,
    RemoveRepository,
    Archive,
    Unarchive,
    Delete,
}

/// An editable field in the detail pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    WorktreeName,
    BranchName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailItem {
    Action(Action),
    Field(Field),
}

/// One boxed control inside a [`DetailNode::Controls`] row.
#[derive(Debug, Clone)]
pub struct ControlSpec {
    /// What activating this control means — also its entry in `detail_items`.
    pub item: DetailItem,
    pub label: String,
    pub variant: theme::Variant,
    /// A control that cannot run still occupies its slot, so the layout and
    /// the item indices never shift.
    pub enabled: bool,
}

impl ControlSpec {
    fn new(action: Action, label: impl Into<String>, variant: theme::Variant) -> Self {
        Self {
            item: DetailItem::Action(action),
            label: label.into(),
            variant,
            enabled: true,
        }
    }

    fn disabled(action: Action, label: impl Into<String>) -> Self {
        Self {
            enabled: false,
            ..Self::new(action, label, theme::Variant::Normal)
        }
    }
}

/// The header line the issues section shares with its refresh control,
/// which says whose issues the list holds.
pub fn issues_header(scope: crate::models::Scope) -> &'static str {
    match scope {
        crate::models::Scope::Mine => "MY OPEN GITHUB ISSUES",
        crate::models::Scope::All => "OPEN GITHUB ISSUES",
    }
}

/// One piece of the pane, in rendering order.
#[derive(Debug, Clone)]
pub enum DetailNode {
    /// Section header, with the unconditional blank row above it that
    /// Textual's `.section-header { margin: 1 0 0 0 }` produced.
    Section(&'static str),
    Text {
        text: String,
        style: Style,
    },
    Blank,
    /// The horizontal rule between major sections, with its margin row above.
    Rule,
    /// A path in the elevated box that hugs its content.
    PathBox {
        path: String,
        style: Style,
    },
    /// Open a card; nodes until [`DetailNode::CardEnd`] land inside it.
    CardStart {
        padded: bool,
    },
    CardEnd,
    /// A row of boxed controls. `lead` is non-clickable text sharing the line.
    Controls {
        lead: Option<(String, Style)>,
        controls: Vec<ControlSpec>,
    },
    /// A session card's body: a column of text lines on the left and stacked
    /// button rows floating on the right, sharing the same screen rows. A
    /// full-width button band cost three rows that held nothing but buttons
    /// and read as a divider; here the text and the buttons interleave, and
    /// the renderer clips the text so the two never collide.
    CardBody {
        left: Vec<Vec<(String, Style)>>,
        rows: Vec<Vec<ControlSpec>>,
    },
    /// A rename field, rendered from the app's live input for `field`.
    Field {
        field: Field,
        label: &'static str,
    },
    /// The issues header with its inline refresh control, whose glyph doubles
    /// as the loading spinner, and the "show blocked" checkbox after it.
    IssuesHeader {
        title: &'static str,
        glyph: char,
        show_blocked: bool,
        /// The hidden-count note, while blocked issues are hidden.
        hidden: Option<String>,
    },
}

/// The full pane for the current selection. Empty when nothing is selected,
/// which is what the renderer shows the empty state for.
pub fn content(app: &App) -> Vec<DetailNode> {
    let mut nodes = Vec::new();
    if app.state.selection.is_worktree() {
        worktree(&mut nodes, app);
    } else if app.state.selection.is_repository() {
        repository(&mut nodes, app);
    }
    nodes
}

/// The focusable items a node sequence contains, in order. Test-only: the
/// running app consumes [`drawn`] via the `App::drawn_items` snapshot.
#[cfg(test)]
pub fn items(nodes: &[DetailNode]) -> Vec<DetailItem> {
    drawn(nodes).into_iter().map(|(item, _)| item).collect()
}

/// [`items`] paired with whether each control can actually run. This is what
/// the renderer snapshots onto `App::drawn_items`: activation resolves against
/// the frame the user saw, and a disabled control must not fire at all.
pub fn drawn(nodes: &[DetailNode]) -> Vec<(DetailItem, bool)> {
    let mut items = Vec::new();
    for node in nodes {
        match node {
            DetailNode::Controls { controls, .. } => {
                items.extend(
                    controls
                        .iter()
                        .map(|control| (control.item.clone(), control.enabled)),
                );
            }
            DetailNode::CardBody { rows, .. } => {
                for row in rows {
                    items.extend(
                        row.iter()
                            .map(|control| (control.item.clone(), control.enabled)),
                    );
                }
            }
            DetailNode::Field { field, .. } => items.push((DetailItem::Field(*field), true)),
            DetailNode::IssuesHeader { .. } => {
                items.push((DetailItem::Action(Action::RefreshIssues), true));
                items.push((DetailItem::Action(Action::ToggleBlockedIssues), true));
            }
            _ => {}
        }
    }
    items
}

fn text(nodes: &mut Vec<DetailNode>, text: impl Into<String>, style: Style) {
    nodes.push(DetailNode::Text {
        text: text.into(),
        style,
    });
}

fn controls(nodes: &mut Vec<DetailNode>, controls: Vec<ControlSpec>) {
    nodes.push(DetailNode::Controls {
        lead: None,
        controls,
    });
}

// --------------------------------------------------------------------- panes

fn repository(nodes: &mut Vec<DetailNode>, app: &App) {
    let repository = app.state.selected_repository();
    let name = repository.map(|r| r.name.as_str()).unwrap_or_default();
    let path = repository
        .map(|r| r.source_path.as_str())
        .unwrap_or_default();

    nodes.push(DetailNode::Section("MAIN REPOSITORY"));
    text(nodes, format!("Repository: {name}"), theme::title());
    if let Some(branch) = &app.meta.branch {
        text(nodes, format!("Branch:     {branch}"), theme::accent());
    }
    commit_line(nodes, app);

    let mut top = vec![
        ControlSpec::new(Action::StartWork, "Start work", theme::Variant::Primary),
        sync_control(app, false),
    ];
    // Only there when it has something to do: a control that is always
    // drawn, mostly disabled, is clutter on the pane's busiest row.
    let finished = repository.map_or(0, |repo| app.finished_worktrees(repo.id).len());
    if finished > 0 {
        top.push(ControlSpec::new(
            Action::PruneFinished,
            format!("Prune finished ({finished})"),
            theme::Variant::Normal,
        ));
    }
    controls(nodes, top);

    nodes.push(DetailNode::Rule);
    nodes.push(DetailNode::Section("LOCATION"));
    nodes.push(DetailNode::PathBox {
        path: path.to_string(),
        style: theme::secondary(),
    });

    nodes.push(DetailNode::Rule);
    open_in(nodes);
    // Textual ran CLAUDE straight into RECENT SESSIONS with no rule between them.
    nodes.push(DetailNode::Rule);
    claude(nodes, app);
    sessions(nodes, app);

    nodes.push(DetailNode::Rule);
    issues(nodes, app);

    nodes.push(DetailNode::Rule);
    nodes.push(DetailNode::Section("MANAGE"));
    // Settings are keyed by the GitHub repository, so without one the
    // control has nothing to open; it keeps its slot, drawn disabled, and
    // the line above says why.
    let settings = match app.repo_key(path) {
        Some(Some(key)) => {
            text(nodes, format!("Settings for {key}"), theme::muted());
            ControlSpec::new(
                Action::RepositorySettings,
                "Repository settings",
                theme::Variant::Normal,
            )
        }
        known => {
            let why = match known {
                Some(_) => "No GitHub repository: the global settings apply",
                None => "Looking up the GitHub repository...",
            };
            text(nodes, why, theme::muted());
            ControlSpec::disabled(Action::RepositorySettings, "Repository settings")
        }
    };
    controls(
        nodes,
        vec![
            settings,
            ControlSpec::new(
                Action::RemoveRepository,
                "Remove Repository",
                theme::Variant::Destructive,
            ),
        ],
    );
    bottom_padding(nodes);
}

/// A resting line under the pane's last control — its bottom padding. Without
/// it, a fully scrolled pane presses the final button straight against the
/// footer bar.
fn bottom_padding(nodes: &mut Vec<DetailNode>) {
    nodes.push(DetailNode::Blank);
}

fn worktree(nodes: &mut Vec<DetailNode>, app: &App) {
    // Read defensively: a selection can briefly outlive the worktree it points
    // at, and the sequence of focusable items must not change when it does.
    let selected = app.state.selected_worktree();
    let repository = selected
        .map(|(repo, _)| repo.name.as_str())
        .unwrap_or_default();
    let name = selected.map(|(_, w)| w.name.as_str()).unwrap_or_default();
    let branch = selected.map(|(_, w)| w.branch.as_str()).unwrap_or_default();
    let archived = selected.map(|(_, w)| w.is_archived).unwrap_or(false);
    let deleting = selected
        .map(|(_, w)| app.worktree_sync.is_removing(w.id))
        .unwrap_or(false);

    nodes.push(DetailNode::Section("WORKTREE"));
    text(nodes, format!("Repository: {repository}"), theme::title());
    text(nodes, format!("Worktree:   {name}"), theme::primary());
    text(nodes, format!("Branch:     {branch}"), theme::accent());
    if !app.meta.path_exists {
        text(
            nodes,
            "⚠ MISSING:   directory no longer exists on disk",
            theme::destructive(),
        );
    }
    if let Some(base) = selected.and_then(|(_, w)| w.base_branch.as_deref()) {
        let mut line = format!("Based on:   {base}");
        if let Some(reference) = selected.and_then(|(_, w)| w.created_from_ref.as_deref()) {
            line.push_str(&format!(" ({reference})"));
        }
        text(nodes, line, theme::muted());
    }
    commit_line(nodes, app);

    controls(nodes, vec![sync_control(app, !app.meta.path_exists)]);

    nodes.push(DetailNode::Rule);
    nodes.push(DetailNode::Section("LOCATION"));
    if app.meta.path_exists {
        nodes.push(DetailNode::PathBox {
            path: app.meta.path.clone(),
            style: theme::secondary(),
        });
    } else {
        nodes.push(DetailNode::PathBox {
            path: format!("{}  (missing)", app.meta.path),
            style: theme::destructive(),
        });
    }

    nodes.push(DetailNode::Rule);
    open_in(nodes);
    nodes.push(DetailNode::Rule);
    claude(nodes, app);
    sessions(nodes, app);

    nodes.push(DetailNode::Rule);
    nodes.push(DetailNode::Section("RENAME"));
    nodes.push(DetailNode::Field {
        field: Field::WorktreeName,
        label: "Worktree name",
    });
    nodes.push(DetailNode::Field {
        field: Field::BranchName,
        label: "Branch name",
    });

    nodes.push(DetailNode::Rule);
    nodes.push(DetailNode::Section("MANAGE"));
    controls(
        nodes,
        vec![
            if archived {
                ControlSpec::new(Action::Unarchive, "Unarchive", theme::Variant::Normal)
            } else {
                ControlSpec::new(Action::Archive, "Archive", theme::Variant::Normal)
            },
            if deleting {
                ControlSpec::disabled(Action::Delete, "Deleting…")
            } else {
                ControlSpec::new(Action::Delete, "Delete", theme::Variant::Destructive)
            },
        ],
    );
    bottom_padding(nodes);
}

// ------------------------------------------------------------------ sections

fn commit_line(nodes: &mut Vec<DetailNode>, app: &App) {
    let Some(hash) = &app.meta.commit_hash else {
        return;
    };
    let mut line = format!("Commit:     {hash}");
    if let Some(when) = app.meta.commit_time {
        line.push_str(&format!(" ({})", crate::util::naturaltime(when)));
    }
    text(nodes, line, theme::muted());
}

/// The `↓ Git Pull` control. Disabled when there is no remote to pull from, or
/// — for worktrees — when the directory is gone.
///
/// The Textual build used `⟳` (U+27F3). It is a poor choice for a terminal: no
/// monospace font checked here carries it — not DejaVu Sans Mono, Liberation
/// Mono, Noto Mono, Ubuntu Mono, nor the JetBrains Mono the screenshot tooling
/// embeds — so it lands as a `.notdef` box wherever the terminal has no font
/// fallback to lean on. `↓` is in every one of them, is single-width rather than
/// ambiguous, and says the same thing more plainly: a pull brings work down.
fn sync_control(app: &App, missing_directory: bool) -> ControlSpec {
    if missing_directory {
        ControlSpec::disabled(Action::Sync, "↓ Git Pull (Directory missing)")
    } else if app.meta.has_remote {
        ControlSpec::new(Action::Sync, "↓ Git Pull", theme::Variant::Normal)
    } else {
        ControlSpec::disabled(Action::Sync, "↓ Git Pull (No remote)")
    }
}

fn open_in(nodes: &mut Vec<DetailNode>) {
    nodes.push(DetailNode::Section("OPEN IN"));
    controls(
        nodes,
        vec![
            ControlSpec::new(Action::Editor, "Editor", theme::Variant::Normal),
            ControlSpec::new(Action::Terminal, "Terminal", theme::Variant::Normal),
            ControlSpec::new(Action::Files, "Files", theme::Variant::Normal),
        ],
    );
}

/// One launch vocabulary everywhere: the CLAUDE section and every session
/// card offer the same buttons under the same names in the same accent —
/// `Claude`, `YOLO`, then the user's own. The section header already says
/// these start something; "New Session" restated it on every button.
fn claude(nodes: &mut Vec<DetailNode>, app: &App) {
    nodes.push(DetailNode::Section("CLAUDE: NEW SESSION"));
    let mut row = vec![
        ControlSpec::new(Action::ClaudeNew, "Claude", theme::Variant::Accent),
        ControlSpec::new(Action::ClaudeYolo, "YOLO", theme::Variant::Accent),
    ];
    row.extend(custom_controls(app, Action::ClaudeCustom));
    controls(nodes, row);
}

/// The user's own Claude buttons, which follow both the new-session and the
/// resume controls. `action` maps a button index to the action it fires in
/// this particular row. Accent like the built-ins beside them — YOLO-style
/// or not, the launch rows are one standardized vocabulary.
fn custom_controls<'a>(
    app: &'a App,
    action: impl Fn(usize) -> Action + 'a,
) -> impl Iterator<Item = ControlSpec> + 'a {
    app.settings
        .custom_buttons
        .iter()
        .enumerate()
        .map(move |(index, custom)| {
            ControlSpec::new(action(index), custom.label.as_str(), theme::Variant::Accent)
        })
}

/// Shared by the issue header and the per-session refresh indicator.
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

fn sessions(nodes: &mut Vec<DetailNode>, app: &App) {
    nodes.push(DetailNode::Section("CLAUDE: RECENT SESSIONS"));
    let Some(list) = app.sessions.as_deref() else {
        text(nodes, "Loading...", theme::muted());
        return;
    };
    if list.is_empty() {
        text(nodes, "No sessions found", theme::muted());
        return;
    }
    let path = app.state.selected_path().unwrap_or_default();
    // Every card is spaced away from what precedes it — the header included,
    // which is Textual's `.session-item { margin: 0 2 1 0 }` seen from above.
    for (index, session) in list.iter().enumerate() {
        let live = app.live_sessions.get(&session.id);
        let pinned = app.state.is_pinned(&path, &session.id);
        nodes.push(DetailNode::Blank);
        // Unpadded: the card's own top and bottom blanks cost two rows per
        // session, and with five cards on screen that is ten rows of nothing.
        // The name carries the separation instead, where it does some work.
        nodes.push(DetailNode::CardStart { padded: false });

        // The left column, top to bottom: title, a blank, the turns, a blank,
        // the branch, the meta. The buttons float to its right, sharing these
        // rows — a full-width button band cost three rows that held nothing
        // but buttons and read as a divider through the card.
        let mut left: Vec<Vec<(String, Style)>> = Vec::new();

        // The title line carries everything that identifies the card: the pin
        // marker, the name, and the live badge — there is horizontal room to
        // spare, and a badge on its own line repeated the name for nothing.
        // `◆`, not `★`: the star is missing from JetBrains Mono and friends —
        // the same tofu problem that retired `⟳` — while the diamond is in
        // every monospace font the sync control's comment lists.
        let mut title_line: Vec<(String, Style)> = Vec::new();
        if pinned {
            title_line.push(("◆ ".to_string(), theme::accent()));
        }
        title_line.push((crate::util::truncate(&session.title, 60), theme::primary()));
        // A map hit means a claude process is running this session right now —
        // a window whose Claude exited never enters the map, because that
        // session is free again (see `services/live.rs`).
        if let Some(live) = live {
            title_line.push((" · ".to_string(), theme::muted()));
            match &live.place {
                crate::services::live::LivePlace::Window { window_name, .. } => {
                    // The window name and the session name are one string by
                    // design, so it is only worth printing when the two
                    // actually differ — a `:2`-uniquified window, or an
                    // unnamed session whose title is its first prompt while
                    // the window carries forestui's name.
                    let place = if window_name == &session.title {
                        String::new()
                    } else {
                        format!(" in {}", crate::util::truncate(window_name, 25))
                    };
                    title_line.push((format!("● live{place}"), theme::accent()));
                }
                // A heartbeat with no reachable window: another tmux session,
                // or no tmux at all. Live, but not somewhere to jump to.
                crate::services::live::LivePlace::Elsewhere { .. } => {
                    title_line.push(("● live elsewhere".to_string(), theme::accent()));
                }
            }
        }
        left.push(title_line);
        // The name is what you scan the list for, so it gets the whitespace —
        // everything below it reads as one block of detail about that name.
        left.push(Vec::new());
        // The exchange the conversation stopped on. A turn identical to the
        // name is not worth a line of its own — that happens on an unnamed
        // session, whose name *is* its first message.
        for turn in session
            .recent_turns
            .iter()
            .filter(|turn| turn.text != session.title)
        {
            let style = match turn.speaker {
                Speaker::User => theme::secondary(),
                Speaker::Claude => theme::muted(),
            };
            left.push(vec![(
                format!(
                    "{} {}",
                    turn.speaker.label(),
                    crate::util::truncate(&turn.text, 60)
                ),
                style,
            )]);
        }
        // Exactly one blank between the conversation and the closing lines.
        left.push(Vec::new());
        // The branch and the meta must clear the button block entirely: the
        // meta carries the spend, and beside the manage row's bottom border a
        // sparse card (a one-turn session) had it clipped mid-price. Six is
        // the two stacked button rows at three rows each.
        while left.len() < 6 {
            left.push(Vec::new());
        }
        if let Some(branch) = &session.git_branch {
            left.push(vec![
                ("on branch ".to_string(), theme::muted()),
                (crate::util::truncate(branch, 40), theme::accent()),
            ]);
        }
        // A card being re-read says so on its meta line. Without it a
        // conversation that moved on while forestui was in the background just
        // sits at its old turn count and then changes with no explanation.
        let msgs = Noun::new("msg", "msgs").count(session.message_count);
        let mut meta = if app.sessions_refreshing.contains(&session.id) {
            format!(
                "{} refreshing • {msgs}",
                SPINNER[app.spinner_index % SPINNER.len()],
            )
        } else {
            format!("{} • {msgs}", session.relative_time())
        };
        // The spend rides the meta line, next to the message count it
        // explains. Old minimal transcripts carry no usage and add nothing.
        if !session.tokens.is_zero() {
            use crate::services::claude_session as service;
            meta.push_str(&format!(
                " • {} in / {} out",
                service::fmt_tokens(session.tokens.total_in()),
                service::fmt_tokens(session.tokens.output),
            ));
            if let Some(cost) = service::cost_estimate(session.model.as_deref(), session.tokens) {
                meta.push_str(&format!(" • {}", service::fmt_cost(cost)));
            }
        }
        left.push(vec![(meta, theme::muted())]);

        // Two stacked button rows floating on the right: what launches Claude
        // on top, what manages the record below. Flattened top-then-bottom
        // they keep the item order the actions always had.
        //
        // The whole launch row wears the accent, YOLO-style customs included:
        // five cards of red YOLO buttons read as a wall of warnings, and the
        // CLAUDE section above already colours that choice at full strength.
        let mut launch = vec![
            ControlSpec::new(
                Action::ResumeSession(index),
                "Claude",
                theme::Variant::Accent,
            ),
            ControlSpec::new(Action::ResumeYolo(index), "YOLO", theme::Variant::Accent),
        ];
        launch.extend(custom_controls(app, move |button| Action::ResumeCustom {
            button,
            session: index,
        }));
        let manage = vec![
            ControlSpec::new(
                Action::RenameSession(index),
                "Rename",
                theme::Variant::Normal,
            ),
            ControlSpec::new(
                Action::TogglePinSession(index),
                if pinned { "Unpin" } else { "Pin" },
                theme::Variant::Normal,
            ),
            // Deleting a session that is open in a window would pull the
            // transcript out from under a running Claude; the control keeps
            // its slot so the row never shifts, but it cannot fire.
            if live.is_some() {
                ControlSpec::disabled(Action::DeleteSession(index), "Del")
            } else {
                ControlSpec::new(
                    Action::DeleteSession(index),
                    "Del",
                    theme::Variant::Destructive,
                )
            },
        ];
        nodes.push(DetailNode::CardBody {
            left,
            rows: vec![launch, manage],
        });
        nodes.push(DetailNode::CardEnd);
    }
}

fn issues(nodes: &mut Vec<DetailNode>, app: &App) {
    // The refresh control rides the header line and doubles as the loading
    // spinner — Textual's `.refresh-btn` was flat, `height: 1; border: none`.
    let show_blocked = app.settings.show_blocked_issues;
    nodes.push(DetailNode::Blank);
    nodes.push(DetailNode::IssuesHeader {
        title: issues_header(
            app.state
                .selected_repository()
                .map_or(app.settings.start_work.scope, |repo| {
                    app.start_work_for(&repo.source_path).scope
                }),
        ),
        glyph: match app.issues {
            Some(_) => '↻',
            None => SPINNER[app.spinner_index % SPINNER.len()],
        },
        show_blocked,
        hidden: app
            .issues
            .as_deref()
            .and_then(|list| work::hidden_blocked_text(work::hidden_blocked(list, show_blocked))),
    });

    let Some(list) = app.issues.as_deref() else {
        text(nodes, "Loading...", theme::muted());
        return;
    };
    if list.is_empty() {
        text(nodes, "No issues found", theme::muted());
        return;
    }
    // Enumerated before filtering: the index is into the whole list, which
    // is what Start work resolves it against.
    let shown: Vec<_> = list
        .iter()
        .enumerate()
        .filter(|(_, issue)| work::is_shown(issue, show_blocked))
        .collect();
    // The header already says how many are hidden; this says that is all
    // there is, rather than leaving a header over nothing.
    if shown.is_empty() {
        text(nodes, "No unblocked issues", theme::muted());
        return;
    }
    for (index, issue) in shown {
        nodes.push(DetailNode::Blank);
        nodes.push(DetailNode::CardStart { padded: true });

        let mut meta = issue.relative_time();
        let labels: Vec<&str> = issue
            .labels
            .iter()
            .take(2)
            .map(|label| label.name.as_str())
            .collect();
        if !labels.is_empty() {
            meta.push_str(&format!(" • {}", labels.join(", ")));
        }
        // The marker takes the spare line between title and meta, where the
        // labels cannot push it past the clip at the buttons.
        let marker = work::blocked_marker(issue)
            .map(|marker| vec![(marker, theme::warning())])
            .unwrap_or_default();
        // The button floats beside the text instead of taking a full-width
        // band, same as the session cards: title, a blank, the meta on the
        // left; Start work's three rows share those same lines on the right.
        nodes.push(DetailNode::CardBody {
            left: vec![
                vec![(
                    format!(
                        "#{} {}",
                        issue.number,
                        crate::util::truncate(&issue.title, 45)
                    ),
                    theme::primary(),
                )],
                marker,
                vec![(meta, theme::muted())],
            ],
            rows: vec![vec![ControlSpec::new(
                Action::StartWorkOnIssue(index),
                "Start work",
                theme::Variant::Normal,
            )]],
        });
        nodes.push(DetailNode::CardEnd);
    }
}
