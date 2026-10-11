use super::*;
use crate::modal::{ConfirmAction, CustomButtonsModal};
use crate::models::{CustomClaudeButton, GitHubIssue, Repository, Settings};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::path::PathBuf;
use uuid::Uuid;

/// Render a modal into a fixed buffer, returning it as text (one row per
/// line) alongside the clickable regions it recorded.
fn render_with_hits(modal: &Modal, width: u16, height: u16) -> (String, Hits) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let mut hits = Hits::new();
    terminal
        .draw(|frame| hits = render_modal(frame, modal, frame.area()))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let screen = (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (screen, hits)
}

fn render(modal: &Modal, width: u16, height: u16) -> String {
    render_with_hits(modal, width, height).0
}

/// The cell where `needle` starts on screen, as (column, row).
fn cell_of(screen: &str, needle: &str) -> (u16, u16) {
    let (row, line) = screen
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains(needle))
        .unwrap_or_else(|| panic!("{needle} missing from:\n{screen}"));
    let byte = line.find(needle).unwrap_or_default();
    // Columns are cells, not bytes: the pills and borders are multi-byte.
    let column = line[..byte].chars().count() as u16;
    (column, row as u16)
}

/// What a click on a cell does, resolved the way `App::hit_at` does.
fn index_click(hits: &Hits, (column, row): (u16, u16)) -> Option<ModalClick> {
    hits.iter()
        .rev()
        .find(|(rect, _, _)| {
            column >= rect.x
                && column < rect.x + rect.width
                && row >= rect.y
                && row < rect.y + rect.height
        })
        .map(|(_, _, click)| *click)
}

/// The focus index recorded for a cell, resolved the way `App::hit_at` does.
fn index_at(hits: &Hits, (column, row): (u16, u16)) -> Option<usize> {
    hits.iter()
        .rev()
        .find(|(rect, _, _)| {
            column >= rect.x
                && column < rect.x + rect.width
                && row >= rect.y
                && row < rect.y + rect.height
        })
        .map(|(_, index, _)| *index)
}

fn repo() -> Repository {
    Repository::new("demo".into(), "/tmp/demo".into())
}

fn worktree_modal(new_branch: bool) -> Modal {
    let mut modal = AddWorktreeModal::new(
        &repo(),
        vec!["main".into(), "feat/login".into(), "origin/main".into()],
        vec!["origin".into()],
        PathBuf::from("/forest"),
        "feat/".into(),
    );
    modal.new_branch = new_branch;
    Modal::AddWorktree(Box::new(modal))
}

fn issue() -> GitHubIssue {
    serde_json::from_str(
        r#"{"number":42,"title":"Fix login bug","state":"OPEN","url":"u",
                "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z",
                "author":{"login":"me"},"assignees":[],"labels":[]}"#,
    )
    .expect("issue fixture")
}

fn start_work_modal(step: StartWorkStep) -> Modal {
    let mut modal = StartWorkModal::new(&repo(), Some(vec![issue()]), Default::default());
    modal.step = step;
    Modal::StartWork(Box::new(modal))
}

fn confirm_modal() -> Modal {
    let branches = crate::event::BranchList {
        branches: vec!["main".into(), "origin/main".into()],
        remotes: vec!["origin".into()],
        remote_default: Some("origin/main".into()),
        current_branch: "main".into(),
    };
    let issue = issue();
    Modal::StartWorkConfirm(Box::new(StartWorkConfirmModal::new(
        &repo(),
        WorkMode::Issue,
        crate::work::WorkItem::from_issue(&issue),
        crate::work::names(
            &Default::default(),
            WorkMode::Issue,
            issue.number,
            &issue.title,
        ),
        branches,
        PathBuf::from("/forest"),
        true,
    )))
}

fn button(label: &str) -> CustomClaudeButton {
    CustomClaudeButton {
        label: label.into(),
        prefix: label.to_lowercase(),
        command: "claude --dangerously-skip-permissions".into(),
    }
}

fn every_modal() -> Vec<(Modal, &'static str)> {
    vec![
        (
            Modal::AddRepository(AddRepositoryModal::new()),
            "Add Repository",
        ),
        (worktree_modal(true), "Add Worktree"),
        (start_work_modal(StartWorkStep::Mode), "Start Work"),
        (start_work_modal(StartWorkStep::Item), "Start Work: issue"),
        (confirm_modal(), "Start Work: Issue #42"),
        (
            Modal::Settings(Box::new(SettingsModal::new(&Settings::default()))),
            "Settings",
        ),
        (
            Modal::ClaudeIntegration(ClaudeIntegrationModal::new()),
            "Claude Integration",
        ),
        (
            Modal::CustomButtons(CustomButtonsModal::new(vec![button("Opus")])),
            "Custom Claude Buttons",
        ),
        (
            Modal::EditButton(Box::new(EditButtonModal::new(None, &[], None))),
            "Add Button",
        ),
        (
            Modal::ThemePicker(crate::modal::ThemePickerModal::new()),
            "Theme",
        ),
        (
            Modal::Confirm(ConfirmModal::new(
                "Delete Worktree",
                "This cannot be undone.",
                ConfirmAction::DeleteWorktree(Uuid::new_v4()),
            )),
            "Delete Worktree",
        ),
    ]
}

/// The prompt is what Claude will be told, so a terminal with room shows
/// all of it — down to the template's last instruction — and a short one
/// says it cut the rest rather than ending mid-sentence silently.
#[test]
fn the_confirm_screen_shows_as_much_prompt_as_fits() {
    let tall = render(&confirm_modal(), 120, 60);
    assert!(tall.contains("then implement it."), "{tall}");
    assert!(!tall.contains('…'), "{tall}");

    let short = render(&confirm_modal(), 120, 36);
    assert!(short.contains("gh issue view 42"), "{short}");
    assert!(short.contains('…'), "{short}");
    assert!(
        short.contains("Launch"),
        "the buttons stay on screen: {short}"
    );
}

#[test]
fn every_modal_renders_its_title() {
    for (modal, title) in every_modal() {
        let screen = render(&modal, 160, 40);
        assert!(screen.contains(title), "missing {title} in:\n{screen}");
    }
}

/// A dialog taller than the terminal must still show the row you act on.
/// Boxing the buttons made every modal 6-10 rows taller, and an 80x24
/// terminal is not unusual — dropping the button row there would leave a
/// dialog you can only dismiss from the keyboard, with nothing on screen
/// saying so.
#[test]
fn buttons_survive_a_short_terminal() {
    let mut checked = 0;
    for (modal, title) in every_modal() {
        // Not every dialog has a button row — the list editors drive
        // themselves from a key hint instead. Ask a roomy render which ones
        // do, so this cannot quietly pass by checking nothing. The dismiss
        // button is Cancel everywhere except the integration dialog, whose
        // actions apply as they are taken, leaving nothing to cancel.
        let roomy = render(&modal, 160, 40);
        let Some(dismiss) = ["Cancel", "Close"]
            .into_iter()
            .find(|label| roomy.contains(label))
        else {
            continue;
        };
        checked += 1;
        let (screen, hits) = render_with_hits(&modal, 80, 24);
        assert!(
            screen.contains(dismiss),
            "{title}: button row fell off an 80x24 terminal:\n{screen}"
        );
        let cell = cell_of(&screen, dismiss);
        assert!(
            index_at(&hits, cell).is_some(),
            "{title}: {dismiss} drawn but not clickable at 80x24:\n{screen}"
        );
    }
    assert!(checked >= 4, "expected several modals to have buttons");
}

/// The other half of the short-terminal contract: what is *not* drawn must
/// not be clickable.
///
/// Every row that overflows the dialog is pinned to the same three rows at
/// the bottom, so each one is painted over by whatever is drawn after it.
/// Their hit regions used to survive that, stacked on top of each other —
/// an invisible control answering clicks from the margin around the row
/// that replaced it. Two controls claiming one cell is the signature, and
/// no dialog that fits has any business producing it either.
#[test]
fn controls_never_claim_the_same_cell_twice() {
    let mut checked = 0;
    for (modal, title) in every_modal() {
        for height in [8, 10, 12, 14, 24] {
            let (screen, hits) = render_with_hits(&modal, 80, height);
            checked += 1;

            for (a, first) in hits.iter().enumerate() {
                for second in hits.iter().skip(a + 1) {
                    assert!(
                        !first.0.intersects(second.0),
                        "{title} at 80x{height}: focus {} and focus {} both \
                             claim {:?} ∩ {:?} — one of them is not on screen:\n{screen}",
                        first.1,
                        second.1,
                        first.0,
                        second.0
                    );
                }
            }
        }
    }
    assert!(checked >= 20, "expected every modal at every height");
}

/// Clicking the mode option that is already active must not switch away
/// from it. Both segments used to carry the row's Enter, which toggles.
#[test]
fn clicking_the_active_mode_option_keeps_it() {
    let modal = worktree_modal(true);
    let (screen, hits) = render_with_hits(&modal, 100, 30);

    let new_branch = index_click(&hits, cell_of(&screen, "New Branch"));
    let existing = index_click(&hits, cell_of(&screen, "Existing"));

    // Left selects "New Branch", Right selects "Existing" — the same key
    // whichever mode is current, so re-clicking the active one is a no-op.
    assert_eq!(
        new_branch,
        Some(ModalClick::Cycle(Direction::Previous)),
        "the New Branch pill does not select New Branch"
    );
    assert_eq!(
        existing,
        Some(ModalClick::Cycle(Direction::Next)),
        "the Existing pill does not select Existing"
    );
}

#[test]
fn add_repository_shows_the_validation_message() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut modal = AddRepositoryModal::new();
    modal
        .path
        .set_value(dir.path().to_string_lossy().to_string());
    let screen = render(&Modal::AddRepository(modal), 100, 20);
    assert!(screen.contains("Not a git repository"), "{screen}");
}

#[test]
fn add_worktree_dropdown_lists_matching_branches() {
    let Modal::AddWorktree(mut modal) = worktree_modal(false) else {
        panic!("wrong modal");
    };
    modal.picker.set_query("main");
    modal.focus = AddWorktreeModal::FOCUS_RESULTS;
    let screen = render(&Modal::AddWorktree(modal), 100, 40);
    // Two of the three branches match "main", so the count line says so.
    assert!(screen.contains("2 matches"), "{screen}");
    assert!(screen.contains("origin/main"), "{screen}");
    assert!(screen.contains("Existing"), "{screen}");
}

#[test]
fn add_worktree_counts_every_branch_with_an_empty_query() {
    let screen = render(&worktree_modal(false), 100, 40);
    assert!(screen.contains("3 branches"), "{screen}");
}

#[test]
fn confirm_shows_the_message_and_both_choices() {
    let modal = Modal::Confirm(ConfirmModal::new(
        "Delete Worktree",
        "Remove wt-one?\nThis cannot be undone.",
        ConfirmAction::DeleteWorktree(Uuid::new_v4()),
    ));
    let screen = render(&modal, 100, 20);
    assert!(screen.contains("Remove wt-one?"), "{screen}");
    assert!(screen.contains("This cannot be undone."), "{screen}");
    assert!(screen.contains("Cancel"), "{screen}");
    assert!(screen.contains("Delete"), "{screen}");
    assert!(screen.contains("y confirm"), "{screen}");
}

#[test]
fn custom_buttons_scroll_to_keep_the_selection_visible() {
    let mut modal = CustomButtonsModal::new(vec![
        button("Alpha"),
        button("Bravo"),
        button("Charlie"),
        button("Delta"),
    ]);
    modal.selected = 3;
    let screen = render(&Modal::CustomButtons(modal), 100, 14);
    assert!(screen.contains("Delta"), "{screen}");
    assert!(screen.contains("(YOLO)"), "{screen}");
    assert!(screen.contains("a add"), "{screen}");
}

#[test]
fn a_tiny_area_clamps_instead_of_panicking() {
    for (modal, _) in every_modal() {
        render(&modal, 20, 6);
        render(&modal, 4, 2);
    }
}

#[test]
fn clicking_a_modal_control_maps_to_its_focus_index() {
    // Every index below is the constant the modal in `src/modal.rs` acts on;
    // a click that focused anything else would act on the wrong control.
    let settings = Modal::Settings(Box::new(SettingsModal::new(&Settings::default())));
    let (screen, hits) = render_with_hits(&settings, 160, 40);
    for (needle, index) in [
        ("│ ◂ Vim (tmux) ▸ │", SettingsModal::FOCUS_EDITOR),
        ("feat/", SettingsModal::FOCUS_PREFIX),
        ("│ Forest Dark… │", SettingsModal::FOCUS_THEME),
        ("│ Manage Custom Buttons... │", SettingsModal::FOCUS_MANAGE),
        (
            "│ Manage Integration... │",
            SettingsModal::FOCUS_INTEGRATION,
        ),
        (
            "│ [x] Start sessions with Remote Control │",
            SettingsModal::FOCUS_REMOTE_CONTROL,
        ),
        // Short labels are padded out to Textual's `min-width: 10`.
        ("│  Save  │", SettingsModal::FOCUS_SAVE),
        ("│ Cancel │", SettingsModal::FOCUS_CANCEL),
    ] {
        assert_eq!(
            index_at(&hits, cell_of(&screen, needle)),
            Some(index),
            "{needle} in:\n{screen}"
        );
    }

    // The checkbox shares the integration row, so it must still fit whole —
    // and still answer the mouse — on a plain 80-column terminal.
    let (narrow, narrow_hits) = render_with_hits(&settings, 80, 40);
    assert_eq!(
        index_at(
            &narrow_hits,
            cell_of(&narrow, "│ [x] Start sessions with Remote Control │")
        ),
        Some(SettingsModal::FOCUS_REMOTE_CONTROL),
        "{narrow}"
    );

    // A button is three rows tall, and every one of them has to answer the
    // mouse — its border is part of the button.
    let (column, row) = cell_of(&screen, "│  Save  │");
    for offset in [-1i16, 0, 1] {
        let y = row.saturating_add_signed(offset);
        assert_eq!(
            index_at(&hits, (column, y)),
            Some(SettingsModal::FOCUS_SAVE),
            "row {y}:\n{screen}"
        );
    }

    let add = Modal::AddRepository(AddRepositoryModal::new());
    let (screen, hits) = render_with_hits(&add, 100, 20);
    for (needle, index) in [
        ("Enter path or paste", AddRepositoryModal::FOCUS_PATH),
        ("│ Add Repository │", AddRepositoryModal::FOCUS_ADD),
        ("│ Cancel │", AddRepositoryModal::FOCUS_CANCEL),
    ] {
        assert_eq!(
            index_at(&hits, cell_of(&screen, needle)),
            Some(index),
            "{needle} in:\n{screen}"
        );
    }
}

#[test]
fn confirm_modal_records_cancel_and_delete() {
    let modal = Modal::Confirm(ConfirmModal::new(
        "Delete Worktree",
        "Remove wt-one?",
        ConfirmAction::DeleteWorktree(Uuid::new_v4()),
    ));
    let (screen, hits) = render_with_hits(&modal, 100, 20);
    // `Modal::set_focus` reads 0 as Cancel and 1 as the destructive choice.
    assert_eq!(
        index_at(&hits, cell_of(&screen, "│ Cancel │")),
        Some(0),
        "{screen}"
    );
    assert_eq!(
        index_at(&hits, cell_of(&screen, "│ Delete │")),
        Some(1),
        "{screen}"
    );
}

#[test]
fn branch_rows_are_clickable() {
    let (screen, hits) = render_with_hits(&worktree_modal(false), 100, 40);
    // The dropdown is a single focus stop, so every visible row records a
    // region against the results-list index.
    let results = AddWorktreeModal::FOCUS_RESULTS;
    let rows = hits
        .iter()
        .filter(|(_, index, _)| *index == results)
        .count();
    assert_eq!(rows, 3, "one region per visible branch:\n{screen}");
    assert_eq!(
        index_at(&hits, cell_of(&screen, "origin/main")),
        Some(results),
        "{screen}"
    );
}
