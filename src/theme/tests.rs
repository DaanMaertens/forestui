use super::*;
use std::collections::HashSet;

/// The settings file stores the slug; a collision would make two themes
/// indistinguishable on disk, and an empty slug unselectable.
#[test]
fn every_theme_has_a_unique_slug() {
    let mut seen = HashSet::new();
    for theme in THEMES {
        assert!(!theme.slug.is_empty() && !theme.name.is_empty());
        assert!(
            theme
                .slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "slug '{}' is not kebab-case",
            theme.slug
        );
        assert!(seen.insert(theme.slug), "duplicate slug '{}'", theme.slug);
    }
}

#[test]
fn the_default_theme_keeps_the_textual_palette() {
    assert_eq!(THEMES[0].slug, "forest-dark");
    assert_eq!(THEMES[0].bg, Color::Rgb(0x1C, 0x1C, 0x1E));
    assert_eq!(THEMES[0].accent, Color::Rgb(0x52, 0xB7, 0x88));
    assert_eq!(THEMES[0].destructive_bg, Color::Rgb(0x3D, 0x20, 0x20));
}

#[test]
fn unknown_and_legacy_slugs_fall_back_to_the_default() {
    let _guard = test_lock();
    assert!(by_slug("forest-dark").is_some());
    assert!(by_slug("system").is_none());
    assert!(by_slug("dark").is_none());
    // set_active with garbage must leave a usable palette.
    set_active("definitely-not-a-theme");
    assert_eq!(active().slug, "forest-dark");
}

/// Each theme's text must be readable on its own backgrounds: a crude
/// luminance-distance check, but it catches a light-on-light or
/// dark-on-dark palette typo the moment the table grows.
#[test]
fn text_contrasts_with_backgrounds_in_every_theme() {
    fn luma(color: Color) -> i32 {
        let Color::Rgb(r, g, b) = color else {
            panic!("themes are all Rgb");
        };
        (r as i32 * 299 + g as i32 * 587 + b as i32 * 114) / 1000
    }
    for theme in THEMES {
        for (label, bg) in [
            ("bg", theme.bg),
            ("bg_elevated", theme.bg_elevated),
            ("accent_dark", theme.accent_dark),
        ] {
            let distance = (luma(theme.text_primary) - luma(bg)).abs();
            assert!(
                distance >= 60,
                "{}: text_primary vs {label} distance {distance}",
                theme.slug
            );
        }
    }
}

fn luma(color: Color) -> i32 {
    let Color::Rgb(r, g, b) = color else {
        panic!("themes are all Rgb");
    };
    (r as i32 * 299 + g as i32 * 587 + b as i32 * 114) / 1000
}

/// Busy, idle and needs-you must never share a colour: the sidebar name is
/// the one place a session's state shows at a glance, and five published
/// palettes use one value for both `accent` and `success`.
#[test]
fn session_states_have_distinct_colours_in_every_theme() {
    use crate::services::live::SessionState;
    let _guard = test_lock();
    for theme in THEMES {
        set_active(theme.slug);
        let fg = |state| session_state(state).fg.expect("state styles set fg");
        let (busy, idle, needs_you) = (
            fg(SessionState::Busy),
            fg(SessionState::Idle),
            fg(SessionState::NeedsYou),
        );
        assert_ne!(busy, idle, "{}: busy looks idle", theme.slug);
        assert_ne!(busy, needs_you, "{}: busy looks like needs-you", theme.slug);
        // A worktree with no session shows its name in text_primary.
        assert_ne!(
            busy, theme.text_primary,
            "{}: busy looks sessionless",
            theme.slug
        );
        if theme.accent != theme.success {
            assert_eq!(busy, theme.accent, "{}: busy left the accent", theme.slug);
        } else {
            // Not merely unequal: a fallback one step off idle would pass
            // `assert_ne!` and still look the same.
            for (label, other) in [("idle", idle), ("needs-you", needs_you)] {
                let distance = channel_distance(busy, other);
                assert!(
                    distance >= 80,
                    "{}: busy vs {label} channel distance {distance}",
                    theme.slug
                );
            }
            for (label, bg) in [("bg", theme.bg), ("bg_selected", theme.bg_selected)] {
                let distance = (luma(busy) - luma(bg)).abs();
                assert!(
                    distance >= 60,
                    "{}: busy vs {label} distance {distance}",
                    theme.slug
                );
            }
        }
    }
}

/// Sum of per-channel differences: crude, but enough to tell two colours
/// that only differ in a rounding step from two a reader can tell apart.
fn channel_distance(a: Color, b: Color) -> i32 {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) = (a, b) else {
        panic!("themes are all Rgb");
    };
    (i32::from(ar) - i32::from(br)).abs()
        + (i32::from(ag) - i32::from(bg)).abs()
        + (i32::from(ab) - i32::from(bb)).abs()
}
