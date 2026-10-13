use super::*;
use crate::modal::{BackKeyAnswer, BackKeyModal};
use crate::services::tmux::back_key::Claim;

fn offer_on_top(app: &App) -> Option<&BackKeyModal> {
    match app.modals.last() {
        Some(Modal::BackKey(m)) => Some(m),
        _ => None,
    }
}

/// A key the user bound themselves brings up the offer, naming their binding.
#[test]
fn a_conflict_opens_the_offer() {
    let (_dir, mut app) = app_with_fixture();
    app.on_back_key_claim(Claim::Conflict("previous-window".into()));
    let offer = offer_on_top(&app).expect("offer shown");
    assert_eq!(offer.key, "Left");
    assert_eq!(offer.binding, "previous-window");

    // Asked twice — a Settings save racing the startup check — it is still
    // one question.
    app.on_back_key_claim(Claim::Conflict("previous-window".into()));
    let offers = app
        .modals
        .iter()
        .filter(|m| matches!(m, Modal::BackKey(_)))
        .count();
    assert_eq!(offers, 1);
}

#[test]
fn a_bound_key_asks_nothing() {
    let (_dir, mut app) = app_with_fixture();
    app.on_back_key_claim(Claim::Bound);
    app.on_back_key_claim(Claim::Unavailable);
    assert!(app.modals.is_empty());
}

/// Answer the offer the way the user does: a button, then for "pick
/// another key" the key itself.
fn answer(app: &mut App, answer: BackKeyAnswer) {
    let mut modal = BackKeyModal::offer("Left", "previous-window");
    modal.disarm();
    app.modals.push(Modal::BackKey(modal));
    let focus = match &answer {
        BackKeyAnswer::Key(_) => BackKeyModal::FOCUS_PICK,
        BackKeyAnswer::UseAnyway { .. } => BackKeyModal::FOCUS_ANYWAY,
        BackKeyAnswer::Off => BackKeyModal::FOCUS_OFF,
    };
    if let Some(top) = app.modals.last_mut() {
        top.set_focus(focus);
    }
    app.handle_modal_key(key(KeyCode::Enter));
    if let BackKeyAnswer::Key(name) = answer {
        let c = name.chars().next().expect("a one-character key");
        app.handle_modal_key(key(KeyCode::Char(c)));
    }
    assert!(offer_on_top(app).is_none(), "answered");
}

#[test]
fn each_answer_lands_in_the_settings() {
    let (_dir, mut app) = app_with_fixture();
    answer(
        &mut app,
        BackKeyAnswer::UseAnyway {
            key: "Left".into(),
            binding: "previous-window".into(),
        },
    );
    assert_eq!(app.settings.back_key.as_deref(), Some("Left"));
    assert_eq!(
        app.settings.back_key_accepted.as_deref(),
        Some("previous-window")
    );

    answer(&mut app, BackKeyAnswer::Key("b".into()));
    assert_eq!(app.settings.back_key.as_deref(), Some("b"));
    assert_eq!(app.settings.back_key_accepted, None);

    answer(&mut app, BackKeyAnswer::Off);
    assert_eq!(app.settings.back_key, None);
    // Off never asks again: there is no key to conflict.
    app.on_back_key_claim(Claim::Conflict("previous-window".into()));
    assert!(!app.modals.iter().any(|m| matches!(m, Modal::BackKey(_))));
}
