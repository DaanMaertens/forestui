use super::*;
use crate::prune::{Outcome, Skip};
use std::collections::HashMap;

/// Mark `finished` as the fixture repository's finished worktrees, as a
/// check's result would.
fn mark_finished(app: &mut App, finished: &[Uuid]) {
    let repo_id = app.state.repositories()[0].id;
    let ticket = app.finished.ticket_for_test(repo_id);
    app.handle_event(AppEvent::WorktreesFinished {
        ticket,
        verdicts: finished
            .iter()
            .map(|id| (*id, true))
            .collect::<HashMap<_, _>>(),
    });
}

fn worktree_ids(app: &App) -> Vec<Uuid> {
    app.state.repositories()[0]
        .worktrees
        .iter()
        .map(|w| w.id)
        .collect()
}

fn name_of(app: &App, id: Uuid) -> String {
    app.state.find_worktree(id).expect("tracked").1.name.clone()
}

/// The pane's prune control, by label.
fn prune_control(app: &App) -> Option<String> {
    detail::content(app)
        .into_iter()
        .find_map(|node| match node {
            detail::DetailNode::Controls { controls, .. } => controls.into_iter().find_map(|c| {
                (c.item == detail::DetailItem::Action(detail::Action::PruneFinished))
                    .then_some(c.label)
            }),
            _ => None,
        })
}

fn confirm_modal(app: &App) -> &crate::modal::ConfirmModal {
    match app.modals.last() {
        Some(Modal::Confirm(confirm)) => confirm,
        other => panic!("a confirmation expected, found {other:?}"),
    }
}

fn confirm(app: &mut App) {
    confirm_modal(app);
    app.handle_key(key(KeyCode::Char('y')));
    assert!(app.modals.is_empty(), "the confirmation did not close");
}

#[tokio::test]
async fn the_control_is_absent_until_a_worktree_is_finished() {
    let (_dir, mut app) = app_with_fixture();
    assert_eq!(prune_control(&app), None);

    let ids = worktree_ids(&app);
    mark_finished(&mut app, &ids[..2]);
    assert_eq!(prune_control(&app).as_deref(), Some("Prune finished (2)"));
}

#[tokio::test]
async fn the_control_is_drawn_in_the_repository_pane() {
    let _guard = crate::theme::test_lock();
    let (_dir, mut app) = app_with_fixture();
    let ids = worktree_ids(&app);
    mark_finished(&mut app, &ids[..1]);
    assert!(rendered(&mut app).contains("Prune finished (1)"));
}

#[tokio::test]
async fn the_confirmation_lists_every_worktree_it_will_remove() {
    let (_dir, mut app) = app_with_fixture();
    let ids = worktree_ids(&app);
    mark_finished(&mut app, &ids[..2]);

    app.run_action(detail::Action::PruneFinished);
    let modal = confirm_modal(&app);
    for id in &ids[..2] {
        let name = name_of(&app, *id);
        assert!(
            modal.message.lines().any(|line| line.contains(&name)),
            "'{name}' is not listed: {:?}",
            modal.message
        );
    }
    if let Some(unfinished) = ids.get(2) {
        let name = name_of(&app, *unfinished);
        assert!(
            !modal.message.contains(&name),
            "an unfinished worktree is listed"
        );
    }
    assert_eq!(modal.action.confirm_label(), "Prune");
}

/// Confirming starts a removal on each listed worktree through
/// `WorktreeSync`, and nothing leaves state until the result folds.
#[tokio::test]
async fn confirming_starts_a_removal_on_each_worktree() {
    let (_dir, mut app) = app_with_fixture();
    let ids = worktree_ids(&app);
    mark_finished(&mut app, &ids[..2]);

    app.run_action(detail::Action::PruneFinished);
    confirm(&mut app);
    for id in &ids[..2] {
        assert!(app.worktree_sync.is_removing(*id));
        assert!(app.state.find_worktree(*id).is_some());
    }
}

/// A worktree with a change already running is refused by `WorktreeSync`;
/// prune leaves it alone and names it with the other skips.
#[tokio::test]
async fn a_worktree_with_a_change_in_flight_is_skipped_and_named() {
    let (_dir, mut app) = app_with_fixture();
    let ids = worktree_ids(&app);
    let (busy, free) = (ids[0], ids[1]);
    mark_finished(&mut app, &[busy, free]);
    let rename = begun(&mut app, busy, WorktreeSync::begin_rename);

    app.run_action(detail::Action::PruneFinished);
    confirm(&mut app);
    assert!(
        !app.worktree_sync.is_removing(busy),
        "the rename was overrun"
    );
    assert!(app.worktree_sync.is_removing(free));

    let change = app.worktree_sync.change_ticket_for_test(free);
    app.handle_event(AppEvent::WorktreesPruned {
        removals: vec![(
            change,
            name_of(&app, free),
            Outcome::Removed { branch_kept: None },
        )],
        busy: vec![name_of(&app, busy)],
    });
    let busy_name = name_of(&app, busy);
    assert!(
        notified(&app)
            .iter()
            .any(|n| n.contains(&format!("'{busy_name}' (still being changed)"))),
        "{:?}",
        notified(&app)
    );
    app.worktree_sync.finish(rename);
}

/// Every listed worktree busy: nothing to run, but the skips are still said.
#[tokio::test]
async fn all_busy_still_reports_the_skips() {
    let (_dir, mut app) = app_with_fixture();
    let ids = worktree_ids(&app);
    mark_finished(&mut app, &ids[..1]);
    let _rename = begun(&mut app, ids[0], WorktreeSync::begin_rename);

    app.run_action(detail::Action::PruneFinished);
    confirm(&mut app);
    let name = name_of(&app, ids[0]);
    assert!(
        notified(&app)
            .iter()
            .any(|n| n.contains(&format!("'{name}' (still being changed)"))),
        "{:?}",
        notified(&app)
    );
}

/// The fold: removed worktrees leave state and the sidebar, skipped ones
/// stay and are named, and every change is finished.
#[tokio::test]
async fn the_fold_removes_what_was_pruned_and_names_the_skips() {
    let (_dir, mut app) = app_with_fixture();
    let ids = worktree_ids(&app);
    let (gone, dirty) = (ids[0], ids[1]);
    mark_finished(&mut app, &[gone, dirty]);
    app.run_action(detail::Action::PruneFinished);
    confirm(&mut app);

    let removals = vec![
        (
            app.worktree_sync.change_ticket_for_test(gone),
            name_of(&app, gone),
            Outcome::Removed { branch_kept: None },
        ),
        (
            app.worktree_sync.change_ticket_for_test(dirty),
            name_of(&app, dirty),
            Outcome::Skipped(Skip::Uncommitted("1 modified".into())),
        ),
    ];
    let (gone_name, dirty_name) = (name_of(&app, gone), name_of(&app, dirty));
    app.handle_event(AppEvent::WorktreesPruned {
        removals,
        busy: Vec::new(),
    });

    assert!(app.state.find_worktree(gone).is_none());
    assert!(app.state.find_worktree(dirty).is_some());
    assert!(
        !app.rows
            .iter()
            .any(|row| matches!(row, SidebarRow::Worktree { id, .. } if *id == gone))
    );
    assert!(!app.worktree_sync.change_running(gone));
    assert!(!app.worktree_sync.change_running(dirty));
    let toast = notified(&app).join("\n");
    assert!(toast.contains(&format!("Pruned '{gone_name}'")), "{toast}");
    assert!(
        toast.contains(&format!("'{dirty_name}' (uncommitted changes: 1 modified)")),
        "{toast}"
    );
    assert_eq!(prune_control(&app).as_deref(), Some("Prune finished (1)"));
}
