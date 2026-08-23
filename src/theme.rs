// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! Themes, accent colours, and the interface style derived from the pair.
//!
//! A theme is the whole shell: surfaces, borders, typography, the timeline's
//! palette, and whether the chrome is drawn with square corners or notched
//! ones. An accent is the interactive colour inside that shell -- the export
//! button, the in/out handles, selection, focus rings.
//!
//! The two axes are not equal, and deliberately so. Default offers eleven
//! accents because its shell is neutral enough to carry any of them. Cyberpunk
//! offers none: the crimson *is* the theme, and a user-chosen teal inside it
//! would only be a worse Default. A theme whose `accents` list is empty hides
//! the picker.
//!
//! Two colours do not follow the accent in either theme, because they are
//! statements rather than decoration -- green for "this export is a lossless
//! copy" and red for "this will delete your original". If those tracked the
//! accent, picking a green accent would make the delete checkbox look
//! reassuring. They are per-theme rather than global, because neon on
//! near-black and muted on charcoal are different colours saying the same two
//! things.
//!
//! Every accent clears 4.5:1 against its own theme's window background, because
//! the accent is used for text (the badge, the selection duration) and not only
//! for fills. tests/theme.rs measures that for every theme rather than trusting
//! the eye.

use egui::Color32;

// --------------------------------------------------------------------------
// Colour maths. None of this knows about themes.
// --------------------------------------------------------------------------

/// The three channels of a `#rrggbb` string. Anything unparseable reads as
/// black, which is a visible wrong rather than a panic in a paint routine.
pub fn rgb(colour: &str) -> (u8, u8, u8) {
    let hex = colour.trim_start_matches('#');
    if hex.len() != 6 {
        return (0, 0, 0);
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
    (byte(0), byte(2), byte(4))
}

pub fn hex(rgb: (f32, f32, f32)) -> String {
    let clamp = |c: f32| c.round().clamp(0.0, 255.0) as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        clamp(rgb.0),
        clamp(rgb.1),
        clamp(rgb.2)
    )
}

/// Whether a stored preference is still something we can paint with.
pub fn is_valid(colour: &str) -> bool {
    colour.len() == 7
        && colour.starts_with('#')
        && colour[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// WCAG relative luminance.
pub fn luminance(colour: &str) -> f32 {
    let channel = |value: u8| {
        let v = value as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = rgb(colour);
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// WCAG contrast ratio, 1.0 (identical) to 21.0 (black on white).
pub fn contrast(first: &str, second: &str) -> f32 {
    let (a, b) = (luminance(first), luminance(second));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `first` blended `amount` of the way towards `second`.
pub fn mix(first: &str, second: &str, amount: f32) -> String {
    let (a, b) = (rgb(first), rgb(second));
    let blend = |x: u8, y: u8| x as f32 + (y as f32 - x as f32) * amount;
    hex((blend(a.0, b.0), blend(a.1, b.1), blend(a.2, b.2)))
}

pub fn lighten(colour: &str, amount: f32) -> String {
    mix(colour, "#ffffff", amount)
}

/// Readable text to sit on top of the accent.
///
/// Computed rather than hardcoded so a dark accent stays legible -- the
/// original palette happened to be light enough for dark text throughout, and
/// hardcoding that would break the first time someone picked a deep colour.
pub fn on_accent(colour: &str) -> String {
    let dark = mix(colour, "#000000", 0.85);
    if contrast(colour, &dark) >= contrast(colour, "#ffffff") {
        dark
    } else {
        "#ffffff".to_string()
    }
}

/// A hex string as something egui can paint with.
pub fn c32(colour: &str) -> Color32 {
    let (r, g, b) = rgb(colour);
    Color32::from_rgb(r, g, b)
}

pub fn c32a(colour: &str, alpha: u8) -> Color32 {
    let (r, g, b) = rgb(colour);
    Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

// --------------------------------------------------------------------------
// The timeline paints itself, so it takes colours rather than a style.
// --------------------------------------------------------------------------

/// What the timeline paints with, minus the accent, which it is handed.
///
/// These are held as values rather than derived from the surfaces because the
/// Default set was tuned by hand and several sit a single step off the nearest
/// surface colour. Deriving them would change today's look by a shade for no
/// gain, and the whole point of the Default theme is that it does not move.
#[derive(Clone, Copy, Debug)]
pub struct TimelineColours {
    pub bg: &'static str,
    pub film_bg: &'static str,
    pub wave_bg: &'static str,
    pub wave_fg: &'static str,
    pub ruler_bg: &'static str,
    pub ruler_fg: &'static str,
    pub keyframe: &'static str,
    pub keyframe_hot: &'static str,
    pub dim: &'static str,
    pub dim_alpha: u8,
    pub playhead: &'static str,
    pub popup_bg: &'static str,
    pub popup_alpha: u8,
    pub popup_edge: &'static str,
    pub text: &'static str,
    /// A border drawn round the whole widget in every state, empty video
    /// included. "" means no border, which is what keeps Default as it was.
    pub outline: &'static str,
}

// --------------------------------------------------------------------------
// Themes
// --------------------------------------------------------------------------

/// One complete look. `key` is what lands in the settings file.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub key: &'static str,
    pub name: &'static str,

    // Surfaces. Accents are layered over these, so contrast is measured
    // against `background` rather than against black.
    pub background: &'static str,
    pub surface: &'static str,
    pub sunken: &'static str,
    pub border: &'static str,
    pub text: &'static str,
    pub text_dim: &'static str,

    // Meaning, not preference -- see the module docs.
    pub lossless: &'static str,
    pub destructive: &'static str,
    pub destructive_text: &'static str,

    // State colours the style needs but nothing derives well.
    pub pressed: &'static str,
    pub disabled_text: &'static str,
    pub disabled_bg: &'static str,
    pub popup_bg: &'static str,
    pub indicator_border: &'static str,
    pub checkbox_disabled: &'static str,

    pub default_accent: &'static str,
    pub timeline: TimelineColours,

    /// Empty means this theme owns its colour and the picker is hidden.
    pub accents: &'static [(&'static str, &'static str)],

    /// "" leaves the default font alone.
    pub font_family: &'static str,
    /// Percentage; 100 is the font's own spacing.
    pub letter_spacing: f32,
    /// Body text size, in points. A theme carrying its own face sets its own,
    /// because two faces at the same nominal size are not the same size on
    /// screen.
    pub font_size: f32,

    /// Corner rounding for ordinary controls.
    pub radius: f32,
    /// >0 cuts the corners off buttons, in px.
    pub chamfer: f32,
    pub scanlines: bool,
    pub glitch: bool,
}

impl Theme {
    pub fn accent_name(&self, colour: &str) -> Option<&'static str> {
        self.accents
            .iter()
            .find(|(_, value)| *value == colour)
            .map(|(name, _)| *name)
    }

    /// A usable accent for this theme.
    ///
    /// A theme with no palette of its own always answers with its own colour,
    /// so a value left behind by another theme cannot leak through a switch.
    pub fn normalise(&self, colour: Option<&str>) -> String {
        if self.accents.is_empty() {
            return self.default_accent.to_string();
        }
        match colour {
            Some(c) if is_valid(c) => c.to_lowercase(),
            _ => self.default_accent.to_string(),
        }
    }

    /// The accent for a disabled control: present, clearly not clickable.
    pub fn muted(&self, colour: &str) -> String {
        mix(colour, self.background, 0.68)
    }
}

pub static DEFAULT: Theme = Theme {
    key: "default",
    name: "Default",
    background: "#1c1e22",
    surface: "#2a2e35",
    sunken: "#14161a",
    border: "#383d47",
    text: "#e6e8ec",
    text_dim: "#9aa2b1",
    lossless: "#6ee7a0",
    destructive: "#e05252",
    destructive_text: "#ff8080",
    pressed: "#23262c",
    disabled_text: "#6a7181",
    disabled_bg: "#24272d",
    popup_bg: "#22252b",
    indicator_border: "#4a505d",
    checkbox_disabled: "#5a616f",
    default_accent: "#e27125",
    // Eleven choices spanning the hue wheel at roughly even lightness, so
    // switching accent changes the colour without changing how heavy the
    // interface looks. Verified: worst contrast on the background is 4.51:1,
    // closest perceptual pair is dE 17.1, both comfortable.
    accents: &[
        ("Apricot", "#e27125"),
        ("Amber", "#e8a33d"),
        ("Coral", "#e2564d"),
        ("Rose", "#e06189"),
        ("Purple", "#a87ae8"),
        ("Indigo", "#6f7ce8"),
        ("Azure", "#3d92e0"),
        ("Teal", "#26a69a"),
        ("Green", "#5aab4f"),
        ("Lime", "#9bbf3c"),
        ("Slate", "#8b93a6"),
    ],
    font_family: "",
    letter_spacing: 100.0,
    font_size: 14.0,
    radius: 5.0,
    chamfer: 0.0,
    scanlines: false,
    glitch: false,
    timeline: TimelineColours {
        bg: "#1c1e22",
        film_bg: "#141619",
        wave_bg: "#181a1e",
        wave_fg: "#60aaff",
        ruler_bg: "#16181c",
        ruler_fg: "#969ca8",
        keyframe: "#586070",
        keyframe_hot: "#78dc96",
        dim: "#0a0b0d",
        dim_alpha: 168,
        playhead: "#ffffff",
        popup_bg: "#101215",
        popup_alpha: 245,
        popup_edge: "#464c58",
        text: "#e6e8ec",
        outline: "",
    },
};

// NetWatch: crimson chrome and teal readout over near-black, sampled from the
// Netrunner terminal rather than invented -- #ff0056 for the accent, #a10036 for
// the filled bars and borders, #24d3d0 for the readout, #010101 for the ground.
//
// One deliberate departure from the reference. There, the body text is crimson
// too and teal is only for descriptions; here the accent has to stay tellable
// apart from ordinary text, because it is what marks the badge and the selection
// duration. So the two swap roles: crimson takes the chrome, the borders and the
// accent, and the teal readout -- which the reference already uses for every
// button label -- becomes the body text. It reads as the same terminal and the
// accent still means something.
//
// The other departure is forced. Red already had a job in this interface, and it
// was "this will delete your original"; a crimson accent leaves that signal
// nowhere to sit. The warning is therefore hazard orange, dE 49 from the accent
// and the only warm colour in the theme that is not crimson. Measured, not
// eyeballed -- tests/theme.rs runs the same checks over every theme.
pub static CYBERPUNK: Theme = Theme {
    key: "cyberpunk",
    name: "Cyberpunk",
    background: "#010101",
    surface: "#12040a",
    sunken: "#000000",
    border: "#a10036", // the filled-bar crimson; the NetWatch tell
    text: "#2ee0dd",
    text_dim: "#1a9b9d",
    lossless: "#00ff9f",
    destructive: "#ff5c00",
    destructive_text: "#ff8c3d",
    pressed: "#1e0610",
    disabled_text: "#5a2a3a",
    disabled_bg: "#0a0206",
    popup_bg: "#0a0206",
    indicator_border: "#a10036",
    checkbox_disabled: "#4a2430",
    default_accent: "#ff0056",
    accents: &[], // the crimson is the theme; see above
    font_family: "Rajdhani",
    letter_spacing: 103.0,
    // Rajdhani is condensed and reads a shade small at a given size, so it asks
    // for a little more than Default's 14 -- but only a little. At 15.5 the
    // longer labels ("Smallest worth keeping", "Set to playhead") crowded the
    // rows they sit in.
    font_size: 14.5,
    radius: 0.0,
    chamfer: 7.0,
    scanlines: true,
    glitch: true,
    timeline: TimelineColours {
        bg: "#010101",
        film_bg: "#000000",
        wave_bg: "#070208",
        wave_fg: "#24d3d0",
        ruler_bg: "#050205",
        ruler_fg: "#1a9b9d",
        keyframe: "#a10036",
        keyframe_hot: "#00ff9f",
        dim: "#000000",
        dim_alpha: 190,
        // Not crimson. The in/out handles are painted in the accent, and a
        // crimson playhead between two crimson handles is three of the same.
        playhead: "#ffd6e0",
        popup_bg: "#0a0206",
        popup_alpha: 248,
        popup_edge: "#a10036",
        text: "#2ee0dd",
        // Set, because the timeline is otherwise a dark rectangle on a dark
        // window with nothing to say it is there. Matches `border` above, so
        // the timeline is edged like every other framed control in the theme.
        outline: "#a10036",
    },
};

pub static THEMES: &[&Theme] = &[&DEFAULT, &CYBERPUNK];

/// The named theme, falling back to Default for anything unrecognised.
///
/// Same forgiving contract as `normalise` has for accents: a preference file
/// written by a newer version, or edited by hand, must not stop the app opening
/// a window.
pub fn get(key: Option<&str>) -> &'static Theme {
    let key = key.unwrap_or_default().trim().to_lowercase();
    THEMES
        .iter()
        .copied()
        .find(|t| t.key == key)
        .unwrap_or(&DEFAULT)
}

/// Default's accent list, which is what the picker offers.
pub const ACCENTS: &[(&str, &str)] = DEFAULT.accents;
pub const DEFAULT_ACCENT: &str = DEFAULT.default_accent;

/// A usable accent under Default's rules -- the user's stored preference,
/// which a theme owning its own colour must not overwrite.
pub fn normalise(colour: Option<&str>) -> String {
    DEFAULT.normalise(colour)
}

// --------------------------------------------------------------------------
// Fonts
// --------------------------------------------------------------------------

/// Every weight the interface asks for. A renderer synthesises a missing weight
/// by smearing the outline, which on a condensed face looks like a different
/// font rather than a bolder one, so the real files are shipped.
pub const RAJDHANI_REGULAR: &[u8] = include_bytes!("../assets/fonts/Rajdhani-Regular.ttf");
pub const RAJDHANI_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Rajdhani-Medium.ttf");
pub const RAJDHANI_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Rajdhani-SemiBold.ttf");
pub const RAJDHANI_BOLD: &[u8] = include_bytes!("../assets/fonts/Rajdhani-Bold.ttf");

/// Register the bundled faces.
///
/// They are compiled into the binary rather than read from beside it: a Windows
/// build is one file, a macOS bundle moves its resources around, and a theme
/// that loses its typeface depending on how the app was installed is a theme
/// that will be reported as broken.
pub fn font_definitions() -> egui::FontDefinitions {
    use egui::{FontData, FontFamily};
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [
        ("Rajdhani", RAJDHANI_REGULAR),
        ("Rajdhani-Medium", RAJDHANI_MEDIUM),
        ("Rajdhani-SemiBold", RAJDHANI_SEMIBOLD),
        ("Rajdhani-Bold", RAJDHANI_BOLD),
    ] {
        fonts.font_data.insert(
            name.to_owned(),
            std::sync::Arc::new(FontData::from_static(bytes)),
        );
    }
    // Named families the interface asks for by name, so a theme can switch the
    // whole window to Rajdhani without disturbing the proportional default.
    //
    // Each list ends with the default stack. Rajdhani is a Latin text face and
    // has none of the transport glyphs the buttons are labelled with -- without
    // a fallback behind it, "▶ Play" renders as a box and the word Play.
    let fallback = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for (name, faces) in [
        ("rajdhani", ["Rajdhani", "Rajdhani-Medium"]),
        ("rajdhani-bold", ["Rajdhani-Bold", "Rajdhani-SemiBold"]),
    ] {
        let mut stack: Vec<String> = faces.iter().map(|f| (*f).to_owned()).collect();
        stack.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(name.into()), stack);
    }
    fonts
}

/// The family a theme's ordinary text is set in.
pub fn body_family(theme: &Theme) -> egui::FontFamily {
    if theme.font_family.is_empty() {
        egui::FontFamily::Proportional
    } else {
        egui::FontFamily::Name("rajdhani".into())
    }
}

/// The family a theme's bold text is set in.
pub fn strong_family(theme: &Theme) -> egui::FontFamily {
    if theme.font_family.is_empty() {
        egui::FontFamily::Proportional
    } else {
        egui::FontFamily::Name("rajdhani-bold".into())
    }
}

// --------------------------------------------------------------------------
// The style
// --------------------------------------------------------------------------

/// Body text size for a theme, in points. Everything else in the interface is
/// derived from it, so one number moves the whole scale.
pub fn base_size(theme: &Theme) -> f32 {
    theme.font_size
}

/// Point the whole interface at a theme and accent.
///
/// The single choke point for anything an egui widget draws by itself. Whatever
/// this does not cover is painted by `ui::chrome` and the timeline, which read
/// the theme directly.
pub fn apply(ctx: &egui::Context, theme: &Theme, accent: &str) {
    use egui::{style::Selection, CornerRadius, FontId, Stroke, TextStyle};

    let radius = CornerRadius::same(theme.radius as u8);
    let body = body_family(theme);
    let strong = strong_family(theme);
    let size = base_size(theme);
    let accent = accent.to_string();
    let accent = accent.as_str();

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::new(size - 3.0, body.clone())),
            (TextStyle::Body, FontId::new(size, body.clone())),
            (TextStyle::Button, FontId::new(size, body.clone())),
            (
                TextStyle::Monospace,
                FontId::new(size - 0.5, egui::FontFamily::Monospace),
            ),
            (TextStyle::Heading, FontId::new(size + 2.0, strong.clone())),
        ]
        .into();

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = c32(theme.background);
        v.window_fill = c32(theme.popup_bg);
        v.extreme_bg_color = c32(theme.sunken);
        v.faint_bg_color = c32(theme.surface);
        v.code_bg_color = c32(theme.sunken);
        v.window_stroke = Stroke::new(1.0, c32(theme.border));
        v.window_corner_radius = radius;
        v.menu_corner_radius = radius;
        v.popup_shadow = egui::epaint::Shadow::NONE;
        v.window_shadow = egui::epaint::Shadow::NONE;
        v.selection = Selection {
            bg_fill: c32(accent),
            stroke: Stroke::new(1.0, c32(&on_accent(accent))),
        };
        v.hyperlink_color = c32(accent);
        v.warn_fg_color = c32(theme.destructive_text);
        v.error_fg_color = c32(theme.destructive);

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = c32(theme.background);
        w.noninteractive.weak_bg_fill = c32(theme.background);
        w.noninteractive.bg_stroke = Stroke::new(1.0, c32(theme.border));
        w.noninteractive.fg_stroke = Stroke::new(1.0, c32(theme.text));
        w.noninteractive.corner_radius = radius;

        w.inactive.bg_fill = c32(theme.surface);
        w.inactive.weak_bg_fill = c32(theme.surface);
        w.inactive.bg_stroke = Stroke::new(1.0, c32(theme.border));
        w.inactive.fg_stroke = Stroke::new(1.0, c32(theme.text));
        w.inactive.corner_radius = radius;

        w.hovered.bg_fill = c32(&lighten(theme.surface, 0.06));
        w.hovered.weak_bg_fill = c32(&lighten(theme.surface, 0.06));
        w.hovered.bg_stroke = Stroke::new(1.0, c32(accent));
        w.hovered.fg_stroke = Stroke::new(1.0, c32(theme.text));
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = c32(theme.pressed);
        w.active.weak_bg_fill = c32(theme.pressed);
        w.active.bg_stroke = Stroke::new(1.0, c32(accent));
        w.active.fg_stroke = Stroke::new(1.0, c32(theme.text));
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;

        w.open.bg_fill = c32(theme.surface);
        w.open.weak_bg_fill = c32(theme.surface);
        w.open.bg_stroke = Stroke::new(1.0, c32(accent));
        w.open.fg_stroke = Stroke::new(1.0, c32(theme.text));
        w.open.corner_radius = radius;

        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        style.spacing.button_padding = egui::vec2(11.0, 5.0);
        style.spacing.menu_margin = egui::Margin::same(4);
        style.spacing.interact_size.y = 26.0;
        style.interaction.selectable_labels = false;
    });
}
