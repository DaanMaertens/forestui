use super::*;

#[tokio::test]
async fn hit_at_prefers_the_last_recorded_region() {
    let (_dir, mut app) = app_with_fixture();
    app.push_hit(rect(0, 0, 10, 1), HitTarget::SidebarRow(3));
    // A modal drawn afterwards covers the same cell and must win.
    app.push_hit(
        rect(0, 0, 10, 1),
        HitTarget::ModalControl {
            index: 1,
            click: ModalClick::Activate,
        },
    );

    assert_eq!(
        app.hit_at(5, 0),
        Some(HitTarget::ModalControl {
            index: 1,
            click: ModalClick::Activate
        })
    );
    assert_eq!(app.hit_at(50, 0), None, "outside every region");
    assert_eq!(app.hit_at(5, 9), None, "wrong row");
}

fn wheel(column: u16, row: u16) -> ratatui::crossterm::event::MouseEvent {
    use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
    MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// The footer is a row of buttons, and a click on one says which binding
/// was meant. Replaying it as a keystroke let a focused rename field eat it
/// as text — every footer button was dead while renaming.
#[tokio::test]
async fn a_footer_click_works_while_a_rename_field_has_focus() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    app.sessions = Some(Vec::new());
    snapshot_drawn(&mut app);

    app.focus = Focus::Detail;
    app.detail_index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Field(Field::WorktreeName))
        .expect("rename field present");
    let name = app.name_input.value().to_string();

    app.push_hit(rect(0, 40, 10, 1), HitTarget::FooterKey('s'));
    app.handle_mouse(click(2, 40));

    assert_eq!(app.modals.len(), 1, "the settings dialog never opened");
    assert_eq!(
        app.name_input.value(),
        name,
        "the click was typed into the rename field"
    );
}

#[tokio::test]
async fn a_toast_swallows_the_clicks_it_covers() {
    let (_dir, mut app) = app_with_fixture();
    // A control is recorded first, then a toast is drawn over the same cell.
    app.push_hit(rect(10, 5, 20, 3), HitTarget::DetailItem(0));
    app.push_hit(rect(10, 5, 20, 3), HitTarget::Notification);

    assert_eq!(
        app.hit_at(12, 6),
        Some(HitTarget::Notification),
        "the toast must win the cells it covers, or the control under it runs"
    );
}

#[tokio::test]
async fn an_open_modal_owns_the_wheel_as_well_as_the_click() {
    let (_dir, mut app) = app_with_fixture();
    app.sidebar_rect = rect(0, 0, 30, 20);
    let before = app.state.selection;

    // The panes are still drawn behind a modal, so they are still
    // hit-testable; only the guard stops the wheel reaching them.
    app.handle_key(key(KeyCode::Char('S')));
    assert_eq!(app.modals.len(), 1);
    app.handle_mouse(wheel(4, 5));

    assert_eq!(
        app.state.selection, before,
        "the wheel reselected the sidebar behind an open modal"
    );
}

#[tokio::test]
async fn clicking_a_sidebar_row_selects_it() {
    let (_dir, mut app) = app_with_fixture();
    assert!(app.state.selection.is_repository());

    app.push_hit(rect(0, 1, 30, 1), HitTarget::SidebarRow(1));
    app.handle_mouse(click(4, 1));

    assert_eq!(app.sidebar_index, 1);
    assert!(app.state.selection.is_worktree());
    assert_eq!(app.focus, Focus::Sidebar);
}

#[tokio::test]
async fn clicking_a_detail_control_focuses_and_runs_it() {
    let (_dir, mut app) = app_with_fixture();
    app.sessions = Some(Vec::new());
    snapshot_drawn(&mut app);

    let index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Action(Action::Terminal))
        .expect("terminal control present");
    app.push_hit(rect(40, 5, 12, 1), HitTarget::DetailItem(index));
    app.handle_mouse(click(45, 5));

    assert_eq!(app.focus, Focus::Detail);
    assert_eq!(app.detail_index, index);
    // The fixture's directories do not exist, so the action refuses loudly
    // rather than silently opening a window somewhere else.
    assert_eq!(app.notifications.len(), 1);
    assert!(app.notifications[0].text.contains("no longer exists"));
}

/// A control the frame drew as disabled must not fire — from the mouse or
/// from Enter. The greyed-out `↓ Git Pull (No remote)` used to run `git
/// pull` anyway and answer with an error toast. A click must not take
/// focus either: a focused control renders with the full accent style, so
/// moving the cursor there made a disabled `[Del]` light up as if it ran.
#[tokio::test]
async fn a_disabled_control_does_not_activate() {
    let (_dir, mut app) = app_with_fixture();
    app.sessions = Some(Vec::new());
    // The fixture has no remote, so the sync control renders disabled.
    assert!(!app.meta.has_remote);
    snapshot_drawn(&mut app);

    let index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Action(Action::Sync))
        .expect("sync control present");
    assert!(!app.drawn_items[index].1, "sync should be drawn disabled");

    app.push_hit(rect(40, 5, 12, 1), HitTarget::DetailItem(index));
    app.handle_mouse(click(45, 5));
    assert!(app.notifications.is_empty(), "the click ran the action");
    assert_eq!(app.focus, Focus::Sidebar, "the click focused the control");

    app.focus = Focus::Detail;
    app.detail_index = index;
    app.handle_key(key(KeyCode::Enter));
    assert!(app.notifications.is_empty(), "Enter ran the action");
}

/// A background event drained in the same batch as a click must not remap
/// the clicked index onto a different control: activation resolves against
/// the snapshot of the frame the user saw.
#[tokio::test]
async fn a_click_resolves_against_the_drawn_frame_not_the_live_list() {
    let (_dir, mut app) = app_with_fixture();
    app.sessions = Some(Vec::new());
    snapshot_drawn(&mut app);

    // On the drawn frame (no sessions), this index is Remove Repository.
    let index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Action(Action::RemoveRepository))
        .expect("remove control present");
    app.push_hit(rect(40, 5, 20, 1), HitTarget::DetailItem(index));

    // Sessions finish loading before the click is handled; their card's
    // controls land ahead of MANAGE, so `index` now points at another one.
    app.handle_event(AppEvent::Sessions {
        path: app.meta.path.clone(),
        sessions: Some(vec![a_session("loaded")]),
    });
    assert_eq!(
        app.detail_items().get(index),
        Some(&DetailItem::Action(Action::ResumeYolo(0))),
        "precondition: the live list did remap the index"
    );

    app.handle_mouse(click(45, 5));
    // Remove Repository opens the confirm modal; resuming the session
    // would instead have refused over the missing directory.
    assert!(
        matches!(app.modals.last(), Some(Modal::Confirm(_))),
        "the click fired the remapped control instead of the drawn one"
    );
}

#[tokio::test]
async fn clicking_a_detail_field_only_moves_focus() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    app.sessions = Some(Vec::new());
    snapshot_drawn(&mut app);

    let index = app
        .detail_items()
        .iter()
        .position(|item| *item == DetailItem::Field(Field::BranchName))
        .expect("rename field present");
    app.push_hit(rect(40, 9, 20, 1), HitTarget::DetailItem(index));
    app.handle_mouse(click(42, 9));

    assert_eq!(app.detail_index, index);
    assert!(app.notifications.is_empty(), "a field is not an action");
}

#[tokio::test]
async fn a_modal_swallows_clicks_on_the_panes_behind_it() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Char('a')));
    assert_eq!(app.modals.len(), 1);

    let before = app.sidebar_index;
    app.push_hit(rect(0, 1, 30, 1), HitTarget::SidebarRow(1));
    app.handle_mouse(click(4, 1));

    assert_eq!(app.sidebar_index, before, "the modal keeps the click");
    assert_eq!(app.modals.len(), 1);
}

#[tokio::test]
async fn clicking_confirm_delete_runs_the_deletion() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Down));
    let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
    app.handle_key(key(KeyCode::Char('d')));
    assert!(matches!(app.modals.last(), Some(Modal::Confirm(_))));

    // Index 1 is Delete; index 0 would be Cancel.
    app.push_hit(
        rect(10, 10, 10, 1),
        HitTarget::ModalControl {
            index: 1,
            click: ModalClick::Activate,
        },
    );
    app.handle_mouse(click(12, 10));

    assert!(app.modals.is_empty());
    // Removal is asynchronous now; the click only dispatches it. The entry
    // goes when the git result is folded.
    assert!(app.state.find_worktree(worktree_id).is_some());
    let change = app.worktree_sync.change_ticket_for_test(worktree_id);
    app.handle_event(AppEvent::WorktreeRemoveResult {
        change,
        outcome: Ok(git::RemoveOutcome::Removed),
    });
    assert!(app.state.find_worktree(worktree_id).is_none());
}

#[tokio::test]
async fn clicking_a_field_or_cycle_does_not_submit_the_modal() {
    let (_dir, mut app) = app_with_fixture();
    app.handle_key(key(KeyCode::Char('S')));
    assert_eq!(app.modals.len(), 1);

    // Focus index 1 is the branch-prefix input. Clicking it must focus, not save.
    app.push_hit(
        rect(0, 0, 10, 3),
        HitTarget::ModalControl {
            index: 1,
            click: ModalClick::Focus,
        },
    );
    app.handle_mouse(click(2, 1));
    assert_eq!(app.modals.len(), 1, "clicking a field closed the dialog");

    // Index 0 is the editor cycle: clicking advances it, still without saving.
    let before = match app.modals.last() {
        Some(Modal::Settings(m)) => m.editor_index,
        _ => panic!("settings modal expected"),
    };
    app.push_hit(
        rect(0, 5, 20, 1),
        HitTarget::ModalControl {
            index: 0,
            click: ModalClick::Cycle(Direction::Next),
        },
    );
    app.handle_mouse(click(3, 5));
    assert_eq!(app.modals.len(), 1, "clicking a cycle closed the dialog");
    let advanced = match app.modals.last() {
        Some(Modal::Settings(m)) => {
            assert_ne!(m.editor_index, before, "cycle did not advance");
            m.editor_index
        }
        _ => panic!("settings modal expected"),
    };

    // `Previous` steps back to exactly where it started — not merely
    // "somewhere else", which a second forward step would also satisfy.
    app.push_hit(
        rect(0, 5, 20, 1),
        HitTarget::ModalControl {
            index: 0,
            click: ModalClick::Cycle(Direction::Previous),
        },
    );
    app.handle_mouse(click(3, 5));
    match app.modals.last() {
        Some(Modal::Settings(m)) => assert_eq!(
            m.editor_index, before,
            "stepping back from {advanced} landed on {} instead of {before}",
            m.editor_index
        ),
        _ => panic!("settings modal expected"),
    }
}

#[tokio::test]
async fn pointer_motion_is_ignored() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    app.push_hit(rect(0, 1, 30, 1), HitTarget::SidebarRow(1));

    let at = |kind| MouseEvent {
        kind,
        column: 4,
        row: 1,
        modifiers: KeyModifiers::NONE,
    };
    // Motion and release must not act; only the press does.
    app.handle_mouse(at(MouseEventKind::Moved));
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left)));
    app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left)));
    assert_eq!(app.sidebar_index, 0, "motion changed the selection");

    app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left)));
    assert_eq!(app.sidebar_index, 1);
}

/// The wheel follows the pointer, not the keyboard focus. Scrolling the
/// sidebar because the mouse happened to be over the detail pane was the
/// reported bug.
#[tokio::test]
async fn scroll_wheel_follows_the_pointer_not_the_focus() {
    use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    app.sidebar_rect = rect(0, 0, 35, 40);
    app.detail_rect = rect(35, 0, 115, 40);
    let wheel = |kind, column| MouseEvent {
        kind,
        column,
        row: 10,
        modifiers: KeyModifiers::NONE,
    };

    // Over the sidebar: the sidebar moves.
    app.handle_mouse(wheel(MouseEventKind::ScrollDown, 5));
    assert_eq!(app.sidebar_index, 1);
    app.handle_mouse(wheel(MouseEventKind::ScrollUp, 5));
    assert_eq!(app.sidebar_index, 0);

    // Over the detail pane, while the sidebar still has focus: the detail
    // pane scrolls and the selection is left alone.
    assert_eq!(app.focus, Focus::Sidebar);
    app.handle_mouse(wheel(MouseEventKind::ScrollDown, 90));
    assert_eq!(app.sidebar_index, 0, "the wheel moved the wrong pane");
    assert_eq!(app.detail_scroll, SCROLL_STEP as u16);

    // And it never scrolls above the top.
    app.handle_mouse(wheel(MouseEventKind::ScrollUp, 90));
    app.handle_mouse(wheel(MouseEventKind::ScrollUp, 90));
    assert_eq!(app.detail_scroll, 0);
}

/// Dragging the scrollbar thumb scrolls the pane, and picking the thumb up
/// part-way down must not snap it to the pointer.
#[tokio::test]
async fn dragging_the_scrollbar_thumb_scrolls_the_pane() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    // A 20-row track over 120 lines of content: thumb 3 rows, travel 17,
    // max offset 100.
    let geom = ScrollbarGeom::new(rect(148, 2, 2, 20), 120).expect("content overflows");
    assert_eq!((geom.thumb, geom.travel, geom.max_offset), (3, 17, 100));
    app.scrollbar = Some(geom);
    app.detail_scroll = 0;

    let at = |kind, row| MouseEvent {
        kind,
        column: 148,
        row,
        modifiers: KeyModifiers::NONE,
    };

    // Grab the thumb one row below its top; the offset must not move yet.
    app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), 3));
    assert_eq!(app.scroll_drag, Some(1));
    assert_eq!(app.detail_scroll, 0, "grabbing the thumb moved the pane");

    // Drag down ten rows: the thumb top follows the grab point, not the
    // pointer, so it lands at row 10 of the track.
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 13));
    assert_eq!(app.detail_scroll, geom.offset_at(10));

    // Past the bottom of the track it pins to the end rather than running on.
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 200));
    assert_eq!(app.detail_scroll, geom.max_offset);

    // Above the top it pins to the start.
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 0));
    assert_eq!(app.detail_scroll, 0);

    // The release applies its own position before ending the gesture, so a
    // flick that ends past the last drag report is not thrown away.
    app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left), 9));
    assert_eq!(app.detail_scroll, geom.offset_at(9 - 2 - 1));
    assert_eq!(app.scroll_drag, None);
    app.detail_scroll = 0;
    app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 18));
    assert_eq!(app.detail_scroll, 0, "the pane scrolled after the release");
}

/// Pressing the bare track pages to the pointer instead of doing nothing.
#[tokio::test]
async fn pressing_the_scrollbar_track_pages_to_the_pointer() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    let geom = ScrollbarGeom::new(rect(148, 2, 2, 20), 120).expect("content overflows");
    app.scrollbar = Some(geom);
    app.detail_scroll = 0;
    let focus_before = app.focus;

    // Row 14 of the track, well below the resting thumb.
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 148,
        row: 16,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(app.detail_scroll, geom.offset_at(14 - geom.thumb / 2));
    assert!(app.scroll_drag.is_some(), "the press did not begin a drag");
    assert_eq!(app.focus, focus_before, "the scrollbar stole focus");
}

/// A drag that began somewhere else must not move the pane.
#[tokio::test]
async fn a_drag_outside_the_scrollbar_does_not_scroll() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    app.scrollbar = Some(ScrollbarGeom::new(rect(148, 2, 2, 20), 120).expect("overflows"));
    app.detail_scroll = 7;

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 40,
        row: 10,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(app.detail_scroll, 7);
}

/// The thumb the renderer draws and the offset a drag computes have to be
/// inverses, or the bar drifts under the pointer over a long drag.
#[tokio::test]
async fn thumb_position_and_offset_round_trip() {
    let geom = ScrollbarGeom::new(rect(0, 0, 2, 20), 120).expect("content overflows");
    for top in 0..=geom.travel {
        let offset = geom.offset_at(top);
        assert_eq!(
            geom.thumb_top(offset),
            top,
            "thumb top {top} did not survive"
        );
    }
}

/// Clicking a footer entry is the same as pressing its key.
#[tokio::test]
async fn clicking_the_footer_runs_the_binding() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let (_dir, mut app) = app_with_fixture();
    app.push_hit(rect(0, 39, 10, 1), HitTarget::FooterKey('a'));

    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 3,
        row: 39,
        modifiers: KeyModifiers::NONE,
    });

    assert!(
        matches!(app.modals.last(), Some(Modal::AddRepository(_))),
        "clicking `a` in the footer did not open Add Repository"
    );
}
