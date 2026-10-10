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
