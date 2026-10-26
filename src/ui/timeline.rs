// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! The timeline: filmstrip, waveform, keyframe ticks, in/out handles, playhead.
//!
//! The widget owns the visible time range and the selection; everything else it
//! is handed. What the pointer did comes back out as an [`Action`], because the
//! window is what knows how to seek a player and what to write in a timecode
//! box -- the timeline only knows where things are.

use egui::{Color32, FontId, Pos2, Rect, Response, Sense, Stroke, TextureHandle, Ui, Vec2};

use crate::media::{fmt_tc, THUMB_H, THUMB_W};
use crate::theme::{self, TimelineColours};
use crate::ui::chrome::Look;

pub const FILM_H: f32 = 58.0;
pub const WAVE_H: f32 = 34.0;
pub const RULER_H: f32 = 20.0;
/// So handles at 0 and at the end stay fully visible.
pub const PAD: f32 = 6.0;
pub const HANDLE_W: f32 = 9.0;
/// How close the pointer must be to grab a handle.
pub const GRAB_PX: f32 = 10.0;
/// In-point snaps to a keyframe within this many pixels.
pub const SNAP_PX: f32 = 7.0;
/// Don't zoom in past a quarter second.
pub const MIN_SPAN: f64 = 0.25;
/// How many filmstrip frames are worth holding.
pub const MAX_THUMBS: usize = 512;

pub const HEIGHT: f32 = FILM_H + WAVE_H + RULER_H;

/// Label steps that read naturally; the first one giving <= ~9 labels wins.
const STEPS: [f64; 16] = [
    0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0,
    3600.0,
];

/// Whether a pointer at `x` is close enough to grab one of the two handles.
///
/// One predicate for two things: it is what turns the cursor into the resize
/// arrow, and it is what holds the hover preview back. They have to agree --
/// a thumbnail that pops up over the handle you are reaching for hides the
/// thing you are reaching for, and it appears exactly as you get close enough
/// to grab it.
pub fn over_handle(x: f32, x_in: f32, x_out: f32) -> bool {
    (x - x_in).abs() <= GRAB_PX || (x - x_out).abs() <= GRAB_PX
}

fn short_tc(seconds: f64, fine: bool) -> String {
    let total = seconds as i64;
    let (h, rem) = (total / 3600, total % 3600);
    let (m, s) = (rem / 60, rem % 60);
    let frac = if fine {
        format!(".{}", ((seconds - total as f64) * 10.0).round() as i64)
    } else {
        String::new()
    };
    if h > 0 {
        format!("{h}:{m:02}:{s:02}{frac}")
    } else {
        format!("{m}:{s:02}{frac}")
    }
}

/// The timeline's colours for one theme, ready to paint with.
///
/// The timeline paints itself rather than being styled, so it has to be handed
/// its colours explicitly. That makes it the half that would otherwise keep the
/// old palette silently through a theme switch, which is why the whole set is
/// rebuilt from the theme on every frame rather than cached anywhere.
struct Palette {
    bg: Color32,
    film_bg: Color32,
    wave_bg: Color32,
    wave_fg: Color32,
    ruler_bg: Color32,
    ruler_fg: Color32,
    keyframe: Color32,
    keyframe_hot: Color32,
    dim: Color32,
    playhead: Color32,
    popup_bg: Color32,
    popup_edge: Color32,
    text: Color32,
    outline: Option<Color32>,
}

impl Palette {
    fn new(colours: &TimelineColours) -> Self {
        Self {
            bg: theme::c32(colours.bg),
            film_bg: theme::c32(colours.film_bg),
            wave_bg: theme::c32(colours.wave_bg),
            wave_fg: theme::c32(colours.wave_fg),
            ruler_bg: theme::c32(colours.ruler_bg),
            ruler_fg: theme::c32(colours.ruler_fg),
            keyframe: theme::c32(colours.keyframe),
            keyframe_hot: theme::c32(colours.keyframe_hot),
            // The two translucent ones: the mask over untrimmed footage, and
            // the hover popup, which sits over the filmstrip and has to stay
            // readable.
            dim: theme::c32a(colours.dim, colours.dim_alpha),
            playhead: theme::c32(colours.playhead),
            popup_bg: theme::c32a(colours.popup_bg, colours.popup_alpha),
            popup_edge: theme::c32(colours.popup_edge),
            text: theme::c32(colours.text),
            outline: (!colours.outline.is_empty()).then(|| theme::c32(colours.outline)),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    In,
    Out,
    Seek,
}

/// What the pointer did to the timeline this frame.
#[derive(Default, Clone, Copy)]
pub struct Action {
    pub seeked: Option<f64>,
    pub in_changed: bool,
    pub out_changed: bool,
    pub scrub_started: bool,
    pub scrub_ended: bool,
}

pub struct Timeline {
    duration: f64,
    position: f64,
    in_point: f64,
    out_point: f64,

    thumbs: Vec<TextureHandle>,
    /// How many frames arrived, and how many of them one kept frame stands for.
    /// Both are needed to keep a thinned strip lined up with keyframes.
    thumbs_seen: usize,
    thumb_stride: usize,
    peaks: Vec<f32>,
    keyframes: Vec<f64>,

    drag: Option<Drag>,
    snapped: bool,

    /// Visible range. Starts as the whole file.
    view_start: f64,
    view_end: f64,

    /// Geometry from the last frame, so input can be read in the same
    /// coordinates the painting used.
    rect: Rect,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            duration: 0.0,
            position: 0.0,
            in_point: 0.0,
            out_point: 0.0,
            thumbs: Vec::new(),
            thumbs_seen: 0,
            thumb_stride: 1,
            peaks: Vec::new(),
            keyframes: Vec::new(),
            drag: None,
            snapped: false,
            view_start: 0.0,
            view_end: 0.0,
            rect: Rect::ZERO,
        }
    }
}

impl Timeline {
    pub fn reset(&mut self, duration: f64) {
        self.duration = duration.max(0.0);
        self.position = 0.0;
        self.in_point = 0.0;
        self.out_point = self.duration;
        self.thumbs.clear();
        self.thumbs_seen = 0;
        self.thumb_stride = 1;
        self.peaks.clear();
        self.keyframes.clear();
        self.view_start = 0.0;
        self.view_end = self.duration;
        self.drag = None;
        self.snapped = false;
    }

    // ----- data ---------------------------------------------------------

    /// Take a frame into the strip, thinning it out when there are too many.
    ///
    /// The strip decodes keyframes, and a two-hour recording has one every
    /// couple of seconds -- thousands of images held in memory and a draw call
    /// each per repaint. Once the strip is full, dropping every other frame
    /// halves both and doubles the interval it samples at, which is what a long
    /// file zoomed out wants regardless.
    pub fn add_thumbnail(&mut self, texture: TextureHandle) {
        if self.thumb_stride == 0 {
            self.thumb_stride = 1;
        }
        if self.thumbs_seen % self.thumb_stride == 0 {
            self.thumbs.push(texture);
        }
        self.thumbs_seen += 1;
        if self.thumbs.len() > MAX_THUMBS {
            let thinned: Vec<TextureHandle> = self.thumbs.iter().step_by(2).cloned().collect();
            self.thumbs = thinned;
            self.thumb_stride *= 2;
        }
    }

    pub fn set_peaks(&mut self, peaks: Vec<f32>) {
        self.peaks = peaks;
    }

    pub fn set_keyframes(&mut self, times: Vec<f64>) {
        self.keyframes = times;
    }

    pub fn keyframes(&self) -> &[f64] {
        &self.keyframes
    }

    pub fn in_point(&self) -> f64 {
        self.in_point
    }

    pub fn out_point(&self) -> f64 {
        self.out_point
    }

    /// Whether the in-point currently sits on a keyframe.
    pub fn snapped(&self) -> bool {
        self.snapped
    }

    pub fn set_in(&mut self, value: f64) {
        let snapped = self
            .keyframes
            .iter()
            .any(|k| (k - value.clamp(0.0, self.duration)).abs() <= 0.002);
        self.set_in_marked(value, snapped);
    }

    fn set_in_marked(&mut self, value: f64, snapped: bool) {
        self.in_point = value.clamp(0.0, self.duration);
        self.out_point = self.out_point.max(self.in_point);
        self.snapped = snapped;
    }

    pub fn set_out(&mut self, value: f64) {
        self.out_point = value.clamp(0.0, self.duration);
        self.in_point = self.in_point.min(self.out_point);
    }

    pub fn set_position(&mut self, value: f64) {
        self.position = value.clamp(0.0, self.duration);
        self.follow_playhead();
    }

    /// Page the view along when playback leaves it.
    ///
    /// Zoomed in, the playhead used to walk off the edge and keep going while
    /// the view stood still. This pages rather than scrolls, which is what an
    /// editor does: a view that slides under a moving playhead is unreadable.
    fn follow_playhead(&mut self) {
        if self.drag.is_some() || !self.zoomed() {
            return;
        }
        if self.view_start <= self.position && self.position <= self.view_end {
            return;
        }
        // Land the playhead near the left edge so there is somewhere to play
        // into -- unless the jump was backwards, where the reverse is true.
        let span = self.view_span();
        let lead = if self.position > self.view_end {
            span * 0.15
        } else {
            span * 0.85
        };
        self.set_view(self.position - lead, self.position - lead + span);
    }

    // ----- zoom ---------------------------------------------------------

    pub fn view_span(&self) -> f64 {
        (self.view_end - self.view_start).max(MIN_SPAN)
    }

    pub fn zoomed(&self) -> bool {
        self.duration > 0.0 && self.view_span() < self.duration - 1e-6
    }

    /// Frame the current in/out with a little air on each side.
    pub fn zoom_to_selection(&mut self) {
        if self.duration <= 0.0 {
            return;
        }
        let span = (self.out_point - self.in_point).max(MIN_SPAN);
        let pad = span * 0.15;
        self.set_view(self.in_point - pad, self.out_point + pad);
    }

    pub fn zoom_all(&mut self) {
        self.set_view(0.0, self.duration);
    }

    pub fn zoom_in(&mut self) {
        self.zoom_at(0.6, self.position);
    }

    pub fn zoom_out(&mut self) {
        self.zoom_at(1.0 / 0.6, self.position);
    }

    fn set_view(&mut self, start: f64, end: f64) {
        let limit = if self.duration > 0.0 {
            self.duration
        } else {
            MIN_SPAN
        };
        let span = (end - start).max(MIN_SPAN).min(limit);
        let start = start.clamp(0.0, (self.duration - span).max(0.0));
        self.view_start = start;
        self.view_end = start + span;
    }

    /// Zoom keeping `anchor` under the same pixel.
    fn zoom_at(&mut self, factor: f64, anchor: f64) {
        let span = self.view_span();
        let new_span = (span * factor).clamp(MIN_SPAN, self.duration.max(MIN_SPAN));
        let frac = if span > 0.0 {
            (anchor - self.view_start) / span
        } else {
            0.5
        };
        self.set_view(
            anchor - frac * new_span,
            anchor - frac * new_span + new_span,
        );
    }

    // ----- geometry -----------------------------------------------------

    fn track_width(&self) -> f32 {
        (self.rect.width() - 2.0 * PAD).max(1.0)
    }

    fn x_of(&self, seconds: f64) -> f32 {
        self.rect.left()
            + PAD
            + self.track_width() * ((seconds - self.view_start) / self.view_span()) as f32
    }

    fn time_at(&self, x: f32) -> f64 {
        let frac = ((x - self.rect.left() - PAD) / self.track_width()) as f64;
        (self.view_start + frac * self.view_span()).clamp(0.0, self.duration)
    }

    /// When thumbnail `i` was taken.
    ///
    /// The filmstrip decodes keyframes only, so the i-th frame kept is keyframe
    /// `i * stride` -- the stride being 1 until the strip fills up and starts
    /// thinning itself. The count compared is frames *seen*, not frames kept,
    /// for the same reason. If the two loaders disagree, fall back to even
    /// spacing.
    fn thumb_time(&self, i: usize) -> f64 {
        if !self.keyframes.is_empty() && self.keyframes.len() == self.thumbs_seen {
            let index = (i * self.thumb_stride).min(self.keyframes.len() - 1);
            return self.keyframes[index];
        }
        if self.thumbs.len() > 1 {
            return self.duration * i as f64 / (self.thumbs.len() - 1) as f64;
        }
        0.0
    }

    /// Where the two handles are on screen this frame.
    fn handle_xs(&self) -> (f32, f32) {
        (self.x_of(self.in_point), self.x_of(self.out_point))
    }

    fn hit(&self, x: f32) -> Option<Drag> {
        let (x_in, x_out) = self.handle_xs();
        if !over_handle(x, x_in, x_out) {
            return None;
        }
        // Both are in reach at once only when the selection is a sliver, and
        // there the in-point wins -- which is the order this always had.
        Some(if (x - x_in).abs() <= GRAB_PX {
            Drag::In
        } else {
            Drag::Out
        })
    }

    /// Pull the in-point onto a nearby keyframe unless overridden.
    ///
    /// Landing on one turns the export into a stream copy, so it is worth a
    /// little magnetism -- but never silently, and never without an escape.
    fn snap_in(&self, t: f64, disable: bool) -> (f64, bool) {
        if disable || self.keyframes.is_empty() {
            return (t, false);
        }
        let tolerance = SNAP_PX as f64 * self.view_span() / self.track_width() as f64;
        let nearest = crate::export::nearest_keyframe(t, &self.keyframes);
        match nearest {
            Some(k) if (k - t).abs() <= tolerance => (k, true),
            _ => (t, false),
        }
    }

    // ----- the widget ---------------------------------------------------

    pub fn show(&mut self, ui: &mut Ui, look: &Look) -> Action {
        let width = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(width, HEIGHT), Sense::click_and_drag());
        self.rect = rect;
        let palette = Palette::new(&look.theme.timeline);
        let mut action = Action::default();

        self.handle_input(ui, &response, &mut action);
        self.paint(ui, look, &palette, &response);
        action
    }

    fn handle_input(&mut self, ui: &Ui, response: &Response, action: &mut Action) {
        if self.duration <= 0.0 {
            return;
        }
        let modifiers = ui.input(|i| i.modifiers);

        // The wheel zooms; shift-wheel pans.
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                if modifiers.shift {
                    let shift = self.view_span() * if scroll > 0.0 { -0.20 } else { 0.20 };
                    self.set_view(self.view_start + shift, self.view_end + shift);
                } else {
                    let anchor = ui
                        .input(|i| i.pointer.hover_pos())
                        .map(|p| self.time_at(p.x))
                        .unwrap_or(self.position);
                    self.zoom_at(if scroll > 0.0 { 0.8 } else { 1.25 }, anchor);
                }
            }
        }

        if response.double_clicked() {
            if self.zoomed() {
                self.zoom_all();
            } else {
                self.zoom_to_selection();
            }
            return;
        }

        if response.drag_started() || (response.clicked() && self.drag.is_none()) {
            if let Some(pos) = response.interact_pointer_pos() {
                self.drag = Some(self.hit(pos.x).unwrap_or(Drag::Seek));
                if self.drag == Some(Drag::Seek) {
                    action.scrub_started = true;
                }
                self.apply_drag(pos.x, modifiers, action);
            }
        } else if response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                self.apply_drag(pos.x, modifiers, action);
            }
        }

        if response.drag_stopped() || (response.clicked() && self.drag.is_some()) {
            if self.drag == Some(Drag::Seek) {
                action.scrub_ended = true;
            }
            self.drag = None;
        }

        // The pointer changes shape over a handle, which is the only clue that
        // it can be grabbed at all.
        if self.drag.is_none() && response.hovered() {
            if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                if self.hit(pos.x).is_some() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
            }
        }
    }

    fn apply_drag(&mut self, x: f32, modifiers: egui::Modifiers, action: &mut Action) {
        let t = self.time_at(x);
        match self.drag {
            Some(Drag::In) => {
                // Alt is the usual "ignore snapping" modifier in editors.
                let (snapped_t, snapped) = self.snap_in(t, modifiers.alt);
                let value = snapped_t.min(self.out_point);
                self.set_in_marked(value, snapped);
                action.in_changed = true;
            }
            Some(Drag::Out) => {
                self.set_out(t.max(self.in_point));
                action.out_changed = true;
            }
            Some(Drag::Seek) => {
                self.set_position(t);
                action.seeked = Some(t);
            }
            None => {}
        }
    }

    fn paint(&self, ui: &mut Ui, look: &Look, palette: &Palette, response: &Response) {
        let rect = self.rect;
        if !ui.is_rect_visible(rect) {
            return;
        }
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, palette.bg);

        let track = self.track_width();
        let film = Rect::from_min_size(
            Pos2::new(rect.left() + PAD, rect.top()),
            Vec2::new(track, FILM_H),
        );
        let wave = Rect::from_min_size(
            Pos2::new(rect.left() + PAD, rect.top() + FILM_H),
            Vec2::new(track, WAVE_H),
        );
        let ruler = Rect::from_min_size(
            Pos2::new(rect.left() + PAD, rect.top() + FILM_H + WAVE_H),
            Vec2::new(track, RULER_H),
        );
        painter.rect_filled(film, 0.0, palette.film_bg);
        painter.rect_filled(wave, 0.0, palette.wave_bg);
        painter.rect_filled(ruler, 0.0, palette.ruler_bg);

        self.paint_film(&painter, film);
        self.paint_wave(&painter, palette, wave);
        self.paint_ruler(&painter, look, palette, ruler);

        if self.duration <= 0.0 {
            self.paint_outline(&painter, palette);
            return;
        }

        let h = rect.height();
        let clamp = |x: f32| x.clamp(rect.left() - 50.0, rect.right() + 50.0);
        let x_in = clamp(self.x_of(self.in_point));
        let x_out = clamp(self.x_of(self.out_point));
        let accent = look.accent();

        // Everything outside the selection recedes.
        if x_in > rect.left() {
            painter.rect_filled(
                Rect::from_min_max(rect.min, Pos2::new(x_in, rect.bottom())),
                0.0,
                palette.dim,
            );
        }
        if x_out < rect.right() {
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(x_out, rect.top()), rect.max),
                0.0,
                palette.dim,
            );
        }

        painter.line_segment(
            [Pos2::new(x_in, rect.top()), Pos2::new(x_in, rect.top() + h)],
            Stroke::new(1.0, accent),
        );
        painter.line_segment(
            [
                Pos2::new(x_out, rect.top()),
                Pos2::new(x_out, rect.top() + h),
            ],
            Stroke::new(1.0, accent),
        );
        painter.rect_filled(
            Rect::from_min_size(Pos2::new(x_in, rect.top()), Vec2::new(x_out - x_in, 2.0)),
            0.0,
            accent,
        );
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(x_in, rect.bottom() - 2.0),
                Vec2::new(x_out - x_in, 2.0),
            ),
            0.0,
            accent,
        );

        self.paint_handle(&painter, look, x_in, true);
        self.paint_handle(&painter, look, x_out, false);

        // Playhead last so it always reads on top.
        let x_pos = self.x_of(self.position);
        if x_pos >= rect.left() - 1.0 && x_pos <= rect.right() + 1.0 {
            painter.line_segment(
                [
                    Pos2::new(x_pos, rect.top()),
                    Pos2::new(x_pos, rect.bottom()),
                ],
                Stroke::new(1.0, palette.playhead),
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(x_pos - 5.0, rect.top()),
                    Pos2::new(x_pos + 5.0, rect.top()),
                    Pos2::new(x_pos, rect.top() + 7.0),
                ],
                palette.playhead,
                Stroke::NONE,
            ));
        }

        if self.drag.is_none() {
            if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                let (x_in, x_out) = self.handle_xs();
                if response.hovered() && !over_handle(pos.x, x_in, x_out) {
                    self.paint_hover(&painter, look, palette, pos.x);
                }
            }
        }
        self.paint_outline(&painter, palette);
    }

    /// A border round the widget, drawn in every state including empty.
    ///
    /// Loaded, the filmstrip gives the timeline an edge of its own. Empty, it
    /// is a dark rectangle on a dark window, and on a theme built out of
    /// outlines there was nothing to say it was there at all.
    ///
    /// Drawn last so the dimming over the untrimmed ends cannot eat it.
    fn paint_outline(&self, painter: &egui::Painter, palette: &Palette) {
        let Some(colour) = palette.outline else {
            return;
        };
        painter.rect_stroke(
            self.rect,
            0.0,
            Stroke::new(1.0, colour),
            egui::StrokeKind::Inside,
        );
    }

    /// Each frame is drawn from its own timestamp to the next one's.
    ///
    /// That keeps the strip correct when zoomed: frames spread apart rather
    /// than stretching, exactly like an NLE.
    fn paint_film(&self, painter: &egui::Painter, film: Rect) {
        if self.thumbs.is_empty() {
            return;
        }
        let painter = painter.with_clip_rect(film);
        for (i, texture) in self.thumbs.iter().enumerate() {
            let t0 = self.thumb_time(i);
            let t1 = if i + 1 < self.thumbs.len() {
                self.thumb_time(i + 1)
            } else {
                self.duration
            };
            let (x0, x1) = (self.x_of(t0), self.x_of(t1));
            if x1 < film.left() || x0 > film.right() {
                continue;
            }
            let width = (x1 - x0).max(1.0);
            // Tile the frame across its slice rather than stretching one copy.
            let mut drawn = 0.0;
            while drawn < width {
                let slice_w = (THUMB_W as f32).min(width - drawn);
                let src_w = (THUMB_W as f32).min(slice_w * THUMB_H as f32 / FILM_H);
                let u0 = (THUMB_W as f32 - src_w) / 2.0 / THUMB_W as f32;
                let u1 = u0 + src_w / THUMB_W as f32;
                painter.image(
                    texture.id(),
                    Rect::from_min_size(
                        Pos2::new(x0 + drawn, film.top()),
                        Vec2::new(slice_w, FILM_H),
                    ),
                    Rect::from_min_max(Pos2::new(u0, 0.0), Pos2::new(u1, 1.0)),
                    Color32::WHITE,
                );
                drawn += slice_w;
            }
        }
    }

    fn paint_wave(&self, painter: &egui::Painter, palette: &Palette, wave: Rect) {
        if self.peaks.is_empty() || self.duration <= 0.0 {
            return;
        }
        let mid = wave.center().y + 0.5;
        let half = wave.height() / 2.0 - 1.0;
        let n = self.peaks.len();
        let track = self.track_width() as i32;
        for px in 0..track {
            let x = wave.left() + px as f32;
            let t = self.time_at(x);
            let index = ((t / self.duration * n as f64) as usize).min(n - 1);
            let h = (self.peaks[index] * half).max(0.5);
            painter.line_segment(
                [Pos2::new(x, mid - h), Pos2::new(x, mid + h)],
                Stroke::new(1.0, palette.wave_fg),
            );
        }
    }

    fn paint_ruler(&self, painter: &egui::Painter, look: &Look, palette: &Palette, ruler: Rect) {
        if self.duration <= 0.0 {
            return;
        }
        // Keyframe ticks. Zoomed in far enough that they are individually
        // meaningful, they brighten -- those are the free, lossless cut points.
        if !self.keyframes.is_empty() {
            let spacing = if self.keyframes.len() > 1 {
                self.x_of(self.keyframes[1]) - self.x_of(self.keyframes[0])
            } else {
                0.0
            };
            let hot = spacing > 14.0;
            let colour = if hot {
                palette.keyframe_hot
            } else {
                palette.keyframe
            };
            let length = if hot { 6.0 } else { 4.0 };
            let mut last = -99.0_f32;
            for t in &self.keyframes {
                let x = self.x_of(*t);
                if x < self.rect.left() + PAD - 1.0
                    || x > self.rect.right() - PAD + 1.0
                    || x - last < 3.0
                {
                    continue;
                }
                last = x;
                painter.line_segment(
                    [
                        Pos2::new(x, ruler.top()),
                        Pos2::new(x, ruler.top() + length),
                    ],
                    Stroke::new(1.0, colour),
                );
            }
        }

        let size = (theme::base_size(look.theme) - 3.0).max(7.5);
        let font = FontId::new(size, theme::body_family(look.theme));
        let span = self.view_span();
        let step = STEPS
            .iter()
            .copied()
            .find(|s| span / s <= 9.0)
            .unwrap_or(STEPS[STEPS.len() - 1]);

        let mut t = (self.view_start / step).floor() * step;
        while t <= self.view_end {
            if t >= self.view_start {
                let x = self.x_of(t);
                let label = short_tc(t, step < 1.0);
                let galley = painter.layout_no_wrap(label, font.clone(), palette.ruler_fg);
                let tx = (x + 3.0).clamp(
                    self.rect.left() + PAD,
                    (self.rect.right() - PAD - galley.size().x).max(self.rect.left() + PAD),
                );
                painter.line_segment(
                    [
                        Pos2::new(x, ruler.top() + 7.0),
                        Pos2::new(x, ruler.bottom()),
                    ],
                    Stroke::new(1.0, palette.ruler_fg),
                );
                painter.galley(
                    Pos2::new(tx, ruler.bottom() - 4.0 - galley.size().y),
                    galley,
                    palette.ruler_fg,
                );
            }
            t += step;
        }
    }

    fn paint_handle(&self, painter: &egui::Painter, look: &Look, x: f32, left: bool) {
        let h = self.rect.height();
        let rect = Rect::from_min_size(
            Pos2::new(
                if left { x - HANDLE_W } else { x },
                self.rect.top() + (h - 30.0) / 2.0,
            ),
            Vec2::new(HANDLE_W, 30.0),
        );
        painter.rect_filled(rect, 2.5, look.accent());
        let grip = theme::c32(&theme::on_accent(&look.accent));
        for offset in [-2.0_f32, 1.0] {
            let gx = rect.center().x + offset;
            painter.line_segment(
                [
                    Pos2::new(gx, rect.top() + 8.0),
                    Pos2::new(gx, rect.bottom() - 8.0),
                ],
                Stroke::new(1.0, grip),
            );
        }
    }

    /// Floating frame + timecode following the pointer.
    fn paint_hover(&self, painter: &egui::Painter, look: &Look, palette: &Palette, hover_x: f32) {
        let t = self.time_at(hover_x);
        let texture = self.nearest_thumb(t);
        let pad = 4.0;
        let (tw, th) = match texture {
            Some(_) => (THUMB_W as f32, THUMB_H as f32),
            None => (86.0, 0.0),
        };
        let label = fmt_tc(t);
        let font = FontId::new(
            theme::base_size(look.theme) - 2.0,
            theme::body_family(look.theme),
        );
        let galley = painter.layout_no_wrap(label, font, palette.text);
        let text_h = galley.size().y;
        let box_w = tw.max(galley.size().x) + pad * 2.0;
        let box_h = th + text_h + pad * 2.0 + if texture.is_some() { 2.0 } else { 0.0 };

        let x = (hover_x - box_w / 2.0).clamp(
            self.rect.left() + 2.0,
            (self.rect.right() - box_w - 2.0).max(self.rect.left() + 2.0),
        );
        let y = (self.rect.top() + FILM_H - box_h - 6.0).max(self.rect.top() + 2.0);
        let box_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(box_w, box_h));

        painter.rect(
            box_rect,
            4.0,
            palette.popup_bg,
            Stroke::new(1.0, palette.popup_edge),
            egui::StrokeKind::Inside,
        );
        if let Some(texture) = texture {
            painter.image(
                texture.id(),
                Rect::from_min_size(
                    Pos2::new(x + (box_w - tw) / 2.0, y + pad),
                    Vec2::new(tw, th),
                ),
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        painter.galley(
            Pos2::new(
                box_rect.center().x - galley.size().x / 2.0,
                box_rect.bottom() - text_h - pad,
            ),
            galley,
            palette.text,
        );
    }

    fn nearest_thumb(&self, t: f64) -> Option<&TextureHandle> {
        if self.thumbs.is_empty() {
            return None;
        }
        let best = (0..self.thumbs.len()).min_by(|a, b| {
            (self.thumb_time(*a) - t)
                .abs()
                .partial_cmp(&(self.thumb_time(*b) - t).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        self.thumbs.get(best)
    }
}
