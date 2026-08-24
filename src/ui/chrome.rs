// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! The parts of a theme a style cannot express.
//!
//! Colours and rounding come out of [`crate::theme`], which is where a theme
//! that only recolours things ends. A theme that wants notched corners,
//! scanlines or a glitch has to paint them, and this is where that happens.
//!
//! Everything here is inert unless the active theme asks for it. [`button`]
//! falls straight through to egui's own rendering when `chamfer` is 0, which is
//! what keeps the Default theme identical to a plain one: it is not a
//! reimplementation of the ordinary look, it is the ordinary code path.

use egui::{
    Align2, Color32, FontId, Id, Pos2, Rect, Response, Sense, Shape, Stroke, TextFormat, Ui, Vec2,
};

use crate::theme::{self, Theme};

/// Cyan and magenta, split either side of the text for a frame or two. Fixed
/// rather than themed: chromatic aberration is an artefact of a bad signal, and
/// a signal does not know what colour scheme it is being displayed in.
pub const GLITCH_A: Color32 = Color32::from_rgb(0, 240, 255);
pub const GLITCH_B: Color32 = Color32::from_rgb(255, 0, 90);
/// How long a hover flickers before it settles.
pub const GLITCH_SECONDS: f64 = 0.190;
/// Seconds between redraws while it does.
pub const GLITCH_TICK: f64 = 0.024;

/// Room a menu button leaves for its painted caret.
const CARET_W: f32 = 14.0;

/// px between lines.
pub const SCANLINE_GAP: f32 = 3.0;
/// Out of 255, on every third row only.
pub const SCANLINE_ALPHA: u8 = 26;

/// The lines are drawn *light*, not dark. A darkening scanline is the obvious
/// implementation and it is the wrong one here: over a #0a0d0e window it moves
/// the pixel by a single value out of 255, so the effect is invisible on the
/// surface that covers most of the interface, and shows up only over the
/// filmstrip. A cool white at the same alpha reads as texture everywhere.
pub const SCANLINE_COLOUR: &str = "#d6fbff";

// There is no vignette, and that is a decision rather than an omission.
// Darkening the foot of the window is the other half of the CRT look, but the
// foot of this window is where the export button, the delete checkbox and the
// size estimate live. Measured: even the gentlest useful vignette (alpha 8)
// drops the delete red from 4.67:1 to 4.44:1, under the floor everything else
// here is held to, and it does it precisely on the control that destroys a
// file. The scanlines carry the effect on their own.

/// The theme, the accent it is painting with, and whether decoration is being
/// held still.
///
/// Passed to every painted widget rather than kept as module state: a value
/// threaded through the call is one that cannot be left behind by a theme
/// switch, which is the failure this arrangement exists to prevent.
#[derive(Clone)]
pub struct Look {
    pub theme: &'static Theme,
    pub accent: String,
    /// Set while an export is running or the preview is playing. A glitch is a
    /// joke the first time and an obstruction while someone is reading a
    /// timecode off a moving picture, and the repaints are not free either.
    pub quiet: bool,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            theme: &theme::DEFAULT,
            accent: theme::DEFAULT_ACCENT.to_string(),
            quiet: false,
        }
    }
}

impl Look {
    pub fn colour(&self, hex: &str) -> Color32 {
        theme::c32(hex)
    }

    pub fn accent(&self) -> Color32 {
        theme::c32(&self.accent)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Normal,
    /// The export button: filled in the accent.
    Primary,
    /// The one-frame nudges, which are narrow and set in a mono face.
    Step,
    /// A button that opens a menu, so it carries a caret.
    ///
    /// The caret is painted rather than typed. A glyph like U+25BE is missing
    /// from most of the faces this runs against, and a missing glyph is not a
    /// slightly wrong arrow -- it is an empty box in the middle of the header.
    Menu,
    /// The accent picker, which is a colour rather than a label.
    Swatch,
}

/// The notched rectangle, corners cut top-left and bottom-right.
///
/// Kept as a function so the shape can be tested without a window. The cut is
/// clamped to half the shorter side, because a notch larger than the button
/// stops being a notch and starts being a triangle.
pub fn chamfer_polygon(width: f32, height: f32, cut: f32) -> Vec<Pos2> {
    let c = cut.clamp(0.0, width.min(height) / 2.0);
    vec![
        Pos2::new(c, 0.0),
        Pos2::new(width, 0.0),
        Pos2::new(width, height - c),
        Pos2::new(width - c, height),
        Pos2::new(0.0, height),
        Pos2::new(0.0, c),
    ]
}

fn shifted(points: &[Pos2], origin: Pos2) -> Vec<Pos2> {
    points
        .iter()
        .map(|p| Pos2::new(origin.x + p.x, origin.y + p.y))
        .collect()
}

/// The label as a theme wants it spelled.
fn label_for(look: &Look, text: &str) -> String {
    if look.theme.chamfer > 0.0 {
        text.to_uppercase()
    } else {
        text.to_string()
    }
}

fn font_for(look: &Look, kind: Kind) -> FontId {
    let size = theme::base_size(look.theme);
    match kind {
        Kind::Step => FontId::new(size - 0.5, egui::FontFamily::Monospace),
        Kind::Primary => FontId::new(size, theme::strong_family(look.theme)),
        _ => FontId::new(size, theme::body_family(look.theme)),
    }
}

fn padding_for(look: &Look, kind: Kind) -> Vec2 {
    let chamfered = look.theme.chamfer > 0.0;
    match kind {
        Kind::Primary if chamfered => Vec2::new(24.0, 7.0),
        Kind::Primary => Vec2::new(22.0, 7.0),
        Kind::Step if chamfered => Vec2::new(8.0, 4.0),
        Kind::Step => Vec2::new(7.0, 4.0),
        Kind::Menu if chamfered => Vec2::new(13.0, 5.0),
        Kind::Menu => Vec2::new(11.0, 5.0),
        Kind::Swatch => Vec2::ZERO,
        _ if chamfered => Vec2::new(13.0, 5.0),
        _ => Vec2::new(11.0, 5.0),
    }
}

/// Letter spacing in points, from the theme's percentage.
fn letter_spacing(look: &Look, size: f32) -> f32 {
    size * (look.theme.letter_spacing - 100.0) / 100.0
}

fn galley(
    ui: &Ui,
    look: &Look,
    text: &str,
    kind: Kind,
    colour: Color32,
) -> std::sync::Arc<egui::Galley> {
    let font = font_for(look, kind);
    let spacing = letter_spacing(look, font.size);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font,
            color: colour,
            extra_letter_spacing: spacing,
            ..Default::default()
        },
    );
    ui.painter().layout_job(job)
}

/// One button, painted the way the active theme wants it.
pub fn button(ui: &mut Ui, look: &Look, text: &str, kind: Kind, enabled: bool) -> Response {
    let label = label_for(look, text);
    let text_size = if kind == Kind::Swatch {
        Vec2::ZERO
    } else {
        galley(ui, look, &label, kind, Color32::WHITE).size()
    };
    let padding = padding_for(look, kind);
    let caret = if kind == Kind::Menu { CARET_W } else { 0.0 };
    let size = if kind == Kind::Swatch {
        Vec2::splat(18.0)
    } else {
        Vec2::new(
            text_size.x + padding.x * 2.0 + caret,
            (text_size.y + padding.y * 2.0).max(ui.spacing().interact_size.y),
        )
    };

    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    // These are painted rather than built out of egui's own button, so nothing
    // else would tell a screen reader -- or a test -- what they are.
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            enabled,
            if kind == Kind::Swatch {
                "Accent colour"
            } else {
                text
            },
        )
    });
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let t = look.theme;
    let accent = look.accent.as_str();
    let hovered = enabled && response.hovered();
    let down = enabled && response.is_pointer_button_down_on();

    if kind == Kind::Swatch {
        // A filled square: the choice is shown, not described.
        let stroke_colour = if hovered {
            theme::c32(t.text)
        } else {
            theme::c32(&theme::lighten(accent, 0.2))
        };
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(t.radius.max(2.0) as u8),
            theme::c32(accent),
            Stroke::new(1.0, stroke_colour),
            egui::StrokeKind::Inside,
        );
        return response;
    }

    let primary = kind == Kind::Primary;
    let (fill, edge, ink) = if !enabled {
        (
            theme::c32(t.disabled_bg),
            theme::c32(t.disabled_bg),
            theme::c32(t.disabled_text),
        )
    } else if primary {
        (
            if hovered {
                theme::c32(&theme::lighten(accent, 0.14))
            } else {
                theme::c32(accent)
            },
            theme::c32(&theme::lighten(accent, 0.3)),
            theme::c32(&theme::on_accent(accent)),
        )
    } else {
        (
            if down {
                theme::c32(t.pressed)
            } else if hovered {
                theme::c32(&theme::lighten(t.surface, 0.08))
            } else {
                theme::c32(t.surface)
            },
            if hovered {
                theme::c32(accent)
            } else {
                theme::c32(t.border)
            },
            theme::c32(t.text),
        )
    };

    if t.chamfer > 0.0 {
        let shape = shifted(
            &chamfer_polygon(rect.width() - 1.0, rect.height() - 1.0, t.chamfer),
            rect.min,
        );
        ui.painter().add(Shape::convex_polygon(
            shape.clone(),
            fill,
            Stroke::new(1.0, edge),
        ));
        // A tick in the cut corner, the way the game marks a live control.
        if enabled && !primary {
            let c = t.chamfer;
            ui.painter().line_segment(
                [
                    Pos2::new(rect.left(), rect.bottom() - 1.0),
                    Pos2::new(rect.left() + c, rect.bottom() - 1.0 - c),
                ],
                Stroke::new(1.0, theme::c32(accent)),
            );
        }
    } else {
        ui.painter().rect(
            rect,
            egui::CornerRadius::same(t.radius as u8),
            fill,
            Stroke::new(1.0, if primary { fill } else { edge }),
            egui::StrokeKind::Inside,
        );
    }

    let centre = rect.center() - Vec2::new(caret / 2.0, 0.0);
    if kind == Kind::Menu {
        // A filled triangle, in the same ink as the label.
        let x = rect.right() - padding.x - CARET_W / 2.0;
        let y = rect.center().y;
        ui.painter().add(Shape::convex_polygon(
            vec![
                Pos2::new(x - 4.0, y - 2.0),
                Pos2::new(x + 4.0, y - 2.0),
                Pos2::new(x, y + 3.0),
            ],
            ink,
            Stroke::NONE,
        ));
    }
    if let Some(offset) = glitch_offset(ui, look, &response, enabled) {
        // Split the channels a pixel apart. The offset walks with the countdown
        // so it settles rather than stopping mid-flicker.
        for (colour, dx) in [(GLITCH_A, -offset), (GLITCH_B, offset)] {
            let ghost = Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), 150);
            let text = galley(ui, look, &label, kind, ghost);
            ui.painter().galley(
                Pos2::new(
                    centre.x - text.size().x / 2.0 + dx,
                    centre.y - text.size().y / 2.0,
                ),
                text,
                ghost,
            );
        }
    }
    let text = galley(ui, look, &label, kind, ink);
    ui.painter().galley(
        Pos2::new(
            centre.x - text.size().x / 2.0,
            centre.y - text.size().y / 2.0,
        ),
        text,
        ink,
    );
    response
}

/// How far the channels are split this instant, or None if nothing is glitching.
///
/// The countdown lives in egui's own per-widget memory, so a button that is
/// rebuilt every frame still remembers when the pointer arrived.
fn glitch_offset(ui: &Ui, look: &Look, response: &Response, enabled: bool) -> Option<f32> {
    if !look.theme.glitch || look.quiet || !enabled {
        return None;
    }
    let id = response.id.with("glitch");
    let now = ui.input(|i| i.time);
    let started: Option<f64> = ui.data(|d| d.get_temp(id));

    if response.hovered() {
        let started = match started {
            Some(previous) if now - previous < GLITCH_SECONDS => previous,
            Some(_) => return None, // it has settled; a fresh hover restarts it
            None => {
                ui.data_mut(|d| d.insert_temp(id, now));
                now
            }
        };
        let left = GLITCH_SECONDS - (now - started);
        if left <= 0.0 {
            return None;
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(GLITCH_TICK));
        let step = 1 + ((left / GLITCH_TICK) as i64) % 2;
        Some(step as f32)
    } else {
        if started.is_some() {
            ui.data_mut(|d| d.remove::<f64>(id));
        }
        None
    }
}

/// The overlay's line segments, with `clear` cut out of them.
///
/// Separated from the painting so the one rule that matters here -- that no
/// segment ever crosses the picture -- is a test rather than a look at the
/// screen.
pub fn scanline_segments(screen: Rect, clear: Option<Rect>) -> Vec<[Pos2; 2]> {
    let mut segments = Vec::new();
    let mut push = |y: f32, from: f32, to: f32| {
        // Anything under a pixel wide is not a line, it is a dot on the edge of
        // the picture. The usual case is a video as wide as the window, where
        // both side pieces vanish and the row is skipped entirely.
        if to - from >= 1.0 {
            segments.push([Pos2::new(from, y), Pos2::new(to, y)]);
        }
    };
    let mut y = screen.top();
    while y < screen.bottom() {
        match clear {
            Some(clear) if y >= clear.top() && y <= clear.bottom() => {
                push(y, screen.left(), clear.left());
                push(y, clear.right(), screen.right());
            }
            _ => push(y, screen.left(), screen.right()),
        }
        y += SCANLINE_GAP;
    }
    segments
}

/// Horizontal lines over the whole window, except `clear`.
///
/// Drawn into a layer above every control, so nothing has to know it is there,
/// and with no input sense at all, so it cannot swallow a click on the timeline
/// underneath.
///
/// `clear` is the picture, when there is one. It is the single surface the
/// overlay has to leave alone: everywhere else the lines are texture over
/// something the app drew, but over a decoded frame they are texture over
/// someone's footage, which is not decoration any more, it is dirt.
///
/// The alpha is deliberately low. tests/theme.rs checks that text still clears
/// the 4.5:1 contrast floor with the overlay on top, because a decoration that
/// makes a timecode harder to read has cost more than it gave.
pub fn scanlines(ctx: &egui::Context, look: &Look, clear: Option<Rect>) {
    if !look.theme.scanlines {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        Id::new("scanlines"),
    ));
    let colour = theme::c32a(SCANLINE_COLOUR, SCANLINE_ALPHA);
    let stroke = Stroke::new(1.0, colour);
    for segment in scanline_segments(ctx.content_rect(), clear) {
        painter.line_segment(segment, stroke);
    }
}

/// Contrast between two colours once the scanline overlay is over both.
///
/// The lines cover one row in `SCANLINE_GAP`, so on average they lift
/// everything by that fraction of their alpha towards `SCANLINE_COLOUR`.
/// Lifting a near-black background is what costs contrast here -- both the text
/// and what it sits on are behind the overlay, which is why this is not simply
/// "the background got lighter".
pub fn overlay_contrast(text: &str, background: &str) -> f32 {
    let lift = (SCANLINE_ALPHA as f32 / 255.0) / SCANLINE_GAP;
    let under = |colour: &str| theme::mix(colour, SCANLINE_COLOUR, lift);
    theme::contrast(&under(text), &under(background))
}

/// A line of text in a theme's own face and spacing.
pub fn text(ui: &mut Ui, look: &Look, value: &str, colour: Color32, strong: bool) -> Response {
    let kind = if strong { Kind::Primary } else { Kind::Normal };
    let galley = galley(ui, look, value, kind, colour);
    let (rect, response) = ui.allocate_exact_size(galley.size(), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.min, galley, colour);
    }
    response
}

/// Draw a string centred in a rectangle, in the theme's face.
pub fn centred_text(
    painter: &egui::Painter,
    look: &Look,
    rect: Rect,
    value: &str,
    colour: Color32,
    size: f32,
) {
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        value,
        FontId::new(size, theme::body_family(look.theme)),
        colour,
    );
}
