// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! The themes, the accent palette, and the colours derived from the pair.
//!
//! The accent is used for text, not only for fills, so "does it look nice" is
//! not the bar -- each colour has to be readable on the window background and
//! telling them apart has to be possible. Both are measured rather than
//! eyeballed.
//!
//! Every guarantee here is checked against *each theme's own* surfaces. A
//! palette that clears the contrast floor on charcoal says nothing about how it
//! reads on near-black, and a theme that shipped without being measured would
//! be a theme that looks fine to whoever chose it and to nobody else.

use apricot_studio::theme::{self, Theme};
use apricot_studio::ui::chrome;

/// WCAG AA for normal text. The badge and the selection duration are rendered
/// in the accent, so this is the applicable threshold rather than the 3:1 for
/// large text.
const MIN_CONTRAST: f32 = 4.5;

/// CIE76 colour difference. Around 2 is "just noticeable"; 15 is comfortably
/// distinct at a glance, which is what a palette needs.
const MIN_DISTANCE: f32 = 15.0;

fn lab(colour: &str) -> (f32, f32, f32) {
    let channel = |value: u8| {
        let v = value as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = theme::rgb(colour);
    let (r, g, b) = (channel(r), channel(g), channel(b));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f32| {
        if t > 0.008856 {
            t.powf(1.0 / 3.0)
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
}

fn distance(first: &str, second: &str) -> f32 {
    let (a, b) = (lab(first), lab(second));
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt()
}

/// The colours a theme actually paints its interactive parts in.
///
/// A theme with its own fixed colour has an empty `accents` list, but that
/// colour still lands on the export button and in the selection duration, so it
/// faces exactly the same measurements as one of eleven choices would.
fn interactive(t: &Theme) -> Vec<(&'static str, &'static str)> {
    if t.accents.is_empty() {
        vec![(t.name, t.default_accent)]
    } else {
        t.accents.to_vec()
    }
}

fn every_theme() -> Vec<&'static Theme> {
    theme::THEMES.to_vec()
}

// ----- the palette --------------------------------------------------------

#[test]
fn offers_a_default_plus_ten() {
    assert_eq!(theme::ACCENTS.len(), 11);
}

#[test]
fn the_default_is_apricot() {
    assert_eq!(theme::DEFAULT_ACCENT, "#e27125");
    assert!(theme::ACCENTS.contains(&("Apricot", "#e27125")));
}

#[test]
fn every_entry_is_a_usable_colour() {
    for t in every_theme() {
        for (name, colour) in interactive(t) {
            assert_eq!(
                colour.to_lowercase(),
                colour,
                "{name} in {} is not lowercase hex",
                t.key
            );
            assert!(
                theme::is_valid(colour),
                "{name} in {} is not a colour",
                t.key
            );
        }
    }
}

#[test]
fn names_and_colours_are_unique() {
    for t in every_theme() {
        let entries = interactive(t);
        let mut names: Vec<&str> = entries.iter().map(|(n, _)| *n).collect();
        let mut colours: Vec<&str> = entries.iter().map(|(_, c)| *c).collect();
        let count = entries.len();
        names.sort_unstable();
        names.dedup();
        colours.sort_unstable();
        colours.dedup();
        assert_eq!(names.len(), count, "{} repeats an accent name", t.key);
        assert_eq!(colours.len(), count, "{} repeats an accent colour", t.key);
    }
}

#[test]
fn every_theme_defaults_to_a_colour_it_offers() {
    for t in every_theme() {
        assert!(
            interactive(t).iter().any(|(_, c)| *c == t.default_accent),
            "{}'s default accent is not one it offers",
            t.key
        );
    }
}

// ----- readability --------------------------------------------------------

#[test]
fn every_accent_is_readable_on_the_window() {
    for t in every_theme() {
        for (name, colour) in interactive(t) {
            let ratio = theme::contrast(colour, t.background);
            assert!(
                ratio >= MIN_CONTRAST,
                "{name} ({colour}) is only {ratio:.2}:1 on {}",
                t.key
            );
        }
    }
}

#[test]
fn every_accent_is_readable_on_the_sunken_surface() {
    // Text fields use a darker fill; the focus ring and selection sit there.
    for t in every_theme() {
        for (name, colour) in interactive(t) {
            let ratio = theme::contrast(colour, t.sunken);
            assert!(
                ratio >= MIN_CONTRAST,
                "{name} on {}'s sunken surface is {ratio:.2}:1",
                t.key
            );
        }
    }
}

#[test]
fn body_text_is_readable_on_every_surface() {
    for t in every_theme() {
        for (surface_name, surface) in [
            ("background", t.background),
            ("surface", t.surface),
            ("sunken", t.sunken),
            ("popup_bg", t.popup_bg),
        ] {
            for (ink_name, ink) in [("text", t.text), ("text_dim", t.text_dim)] {
                let ratio = theme::contrast(ink, surface);
                assert!(
                    ratio >= MIN_CONTRAST,
                    "{} {ink_name} on {surface_name} is {ratio:.2}:1",
                    t.key
                );
            }
        }
    }
}

#[test]
fn text_on_the_accent_is_readable() {
    // The export button puts text on top of the accent.
    for t in every_theme() {
        for (name, colour) in interactive(t) {
            let ratio = theme::contrast(colour, &theme::on_accent(colour));
            assert!(
                ratio >= MIN_CONTRAST,
                "button text on {name} is {ratio:.2}:1"
            );
        }
    }
}

#[test]
fn on_accent_chooses_dark_text_for_a_light_colour() {
    assert_ne!(theme::on_accent("#f5d76e"), "#ffffff");
}

#[test]
fn on_accent_chooses_light_text_for_a_dark_colour() {
    // Not in any shipped palette, but the helper must not assume.
    assert_eq!(theme::on_accent("#1b2a6b"), "#ffffff");
}

#[test]
fn accents_are_telling_apart_from_each_other() {
    for t in every_theme() {
        let entries = interactive(t);
        for i in 0..entries.len() {
            for j in i + 1..entries.len() {
                let (a, first) = entries[i];
                let (b, second) = entries[j];
                let gap = distance(first, second);
                assert!(
                    gap >= MIN_DISTANCE,
                    "{a} and {b} in {} are only dE {gap:.1} apart",
                    t.key
                );
            }
        }
    }
}

#[test]
fn accents_do_not_impersonate_the_semantic_colours() {
    // Green means "this export is a lossless copy" and red means "this will
    // delete your original". An accent close enough to be mistaken for either
    // would make the interface lie -- in any theme, against that theme's own
    // two colours.
    for t in every_theme() {
        for (name, colour) in interactive(t) {
            assert!(
                distance(colour, t.lossless) >= MIN_DISTANCE,
                "{name} could be mistaken for {}'s lossless badge",
                t.key
            );
            assert!(
                distance(colour, t.destructive_text) >= MIN_DISTANCE,
                "{name} could be mistaken for {}'s delete warning",
                t.key
            );
        }
    }
}

#[test]
fn the_two_statements_never_look_alike() {
    for t in every_theme() {
        assert!(
            distance(t.lossless, t.destructive) >= MIN_DISTANCE,
            "{}",
            t.key
        );
        assert!(
            distance(t.lossless, t.destructive_text) >= MIN_DISTANCE,
            "{}",
            t.key
        );
    }
}

// ----- the themes themselves ---------------------------------------------

#[test]
fn the_two_shipped_themes() {
    let keys: Vec<&str> = every_theme().iter().map(|t| t.key).collect();
    assert_eq!(keys, vec!["default", "cyberpunk"]);
}

#[test]
fn names_are_unique() {
    let mut names: Vec<&str> = every_theme().iter().map(|t| t.name).collect();
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count);
}

#[test]
fn get_falls_back_rather_than_crashing() {
    // Same contract as the accent: a settings file is user-editable and
    // survives upgrades, so anything can turn up here.
    for junk in [Some(""), Some("  "), Some("nonsense"), None, Some("42")] {
        assert_eq!(theme::get(junk).key, theme::DEFAULT.key, "{junk:?}");
    }
}

#[test]
fn get_is_case_insensitive_and_forgiving_of_space() {
    assert_eq!(theme::get(Some("CYBERPUNK")).key, "cyberpunk");
    assert_eq!(theme::get(Some(" cyberpunk ")).key, "cyberpunk");
}

#[test]
fn a_theme_with_no_palette_keeps_its_own_colour() {
    // Switching away from Default must not drag an apricot into Night City.
    for offered in [
        Some("#e27125"),
        Some("#3d92e0"),
        Some("#ffffff"),
        None,
        Some("junk"),
    ] {
        assert_eq!(
            theme::CYBERPUNK.normalise(offered),
            theme::CYBERPUNK.default_accent
        );
    }
}

#[test]
fn a_theme_with_a_palette_accepts_a_choice() {
    assert_eq!(theme::DEFAULT.normalise(Some("#3d92e0")), "#3d92e0");
}

#[test]
fn cyberpunk_is_the_crimson() {
    // Pinned deliberately: the rest of the suite follows whatever the theme
    // says, so without this the palette could drift to anything and still look
    // correct to every other test.
    assert_eq!(theme::CYBERPUNK.default_accent, "#ff0056");
}

#[test]
fn only_default_offers_a_picker() {
    assert!(!theme::DEFAULT.accents.is_empty());
    assert!(theme::CYBERPUNK.accents.is_empty());
}

// ----- normalising --------------------------------------------------------

#[test]
fn accepts_the_shipped_palette() {
    for (_, colour) in theme::ACCENTS {
        assert_eq!(theme::normalise(Some(colour)), *colour);
    }
}

#[test]
fn accepts_a_colour_of_the_users_own() {
    assert_eq!(theme::normalise(Some("#123456")), "#123456");
}

#[test]
fn rejects_anything_that_is_not_a_colour() {
    for junk in [Some(""), Some("red"), Some("#12345"), Some("#gggggg"), None] {
        assert_eq!(theme::normalise(junk), theme::DEFAULT_ACCENT, "{junk:?}");
    }
}

#[test]
fn normalising_lowercases_so_the_menu_can_tick_the_right_one() {
    assert_eq!(theme::normalise(Some("#E27125")), "#e27125");
}

// ----- colour maths -------------------------------------------------------

#[test]
fn contrast_spans_the_whole_range() {
    assert!((theme::contrast("#000000", "#ffffff") - 21.0).abs() < 0.01);
    assert!((theme::contrast("#123456", "#123456") - 1.0).abs() < 0.001);
}

#[test]
fn mixing_moves_between_the_two_ends() {
    assert_eq!(theme::mix("#000000", "#ffffff", 0.0), "#000000");
    assert_eq!(theme::mix("#000000", "#ffffff", 1.0), "#ffffff");
    assert_eq!(theme::mix("#000000", "#ffffff", 0.5), "#808080");
}

#[test]
fn lightening_never_leaves_the_range() {
    for colour in ["#000000", "#ffffff", "#e27125"] {
        let lighter = theme::lighten(colour, 0.9);
        assert!(theme::is_valid(&lighter), "{lighter}");
    }
}

#[test]
fn a_muted_accent_is_still_the_same_colour_family() {
    let muted = theme::DEFAULT.muted(theme::DEFAULT_ACCENT);
    assert!(theme::is_valid(&muted));
    // Closer to the background than the accent is, which is the whole point.
    assert!(
        theme::contrast(&muted, theme::DEFAULT.background)
            < theme::contrast(theme::DEFAULT_ACCENT, theme::DEFAULT.background)
    );
}

// ----- the scanline overlay ----------------------------------------------

#[test]
fn scanlines_leave_text_readable() {
    // A decoration that makes a timecode harder to read has cost more than it
    // gave. The lines cover one row in three, so everything behind them is
    // lifted towards the line colour -- including the near-black background,
    // which is what costs contrast.
    let t = theme::CYBERPUNK;
    for (name, ink) in [
        ("text", t.text),
        ("text_dim", t.text_dim),
        ("accent", t.default_accent),
        ("lossless", t.lossless),
        ("destructive", t.destructive_text),
    ] {
        for surface in [t.background, t.surface, t.sunken] {
            let ratio = chrome::overlay_contrast(ink, surface);
            assert!(
                ratio >= MIN_CONTRAST,
                "{name} under the scanlines is {ratio:.2}:1"
            );
        }
    }
}

#[test]
fn the_scanlines_are_drawn_light_not_dark() {
    // A darkening scanline over #010101 moves the pixel by one value in 255:
    // invisible on the surface that covers most of the window, and visible only
    // over the filmstrip. The light line reads as texture everywhere.
    let (r, g, b) = theme::rgb(chrome::SCANLINE_COLOUR);
    assert!(
        r > 200 && g > 200 && b > 200,
        "the overlay colour should be near-white"
    );
    assert!(u32::from(chrome::SCANLINE_ALPHA) < 40, "and faint");
}

#[test]
fn only_a_theme_that_asks_for_them_gets_the_extras() {
    assert!(!theme::DEFAULT.scanlines);
    assert!(!theme::DEFAULT.glitch);
    assert_eq!(theme::DEFAULT.chamfer, 0.0);
    assert!(theme::CYBERPUNK.scanlines);
    assert!(theme::CYBERPUNK.glitch);
    assert!(theme::CYBERPUNK.chamfer > 0.0);
}

#[test]
fn the_default_theme_asks_for_no_font_of_its_own() {
    // Which is what keeps it on the desktop's own face, exactly as before.
    assert_eq!(theme::DEFAULT.font_family, "");
    assert_eq!(theme::DEFAULT.letter_spacing, 100.0);
    assert_eq!(theme::CYBERPUNK.font_family, "Rajdhani");
}

#[test]
fn the_bundled_faces_are_really_bundled() {
    // A theme that loses its typeface depending on how the app was installed is
    // a theme that will be reported as broken, so the files are compiled in.
    for face in [
        theme::RAJDHANI_REGULAR,
        theme::RAJDHANI_MEDIUM,
        theme::RAJDHANI_SEMIBOLD,
        theme::RAJDHANI_BOLD,
    ] {
        assert!(
            face.len() > 10_000,
            "a TrueType face should not be this small"
        );
        assert_eq!(&face[..4], b"\x00\x01\x00\x00", "not a TrueType file");
    }
}
