// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! The modal boxes.
//!
//! A dialog is data: a headline, a detail and a list of buttons, each carrying
//! what it means rather than what it says. The wording of the two that matter
//! -- the blocked folder and the offer to delete the original -- is built in
//! [`crate::sandbox`] and [`crate::app`] respectively, so both can be tested
//! without putting a window on screen.

use egui::Key;

use crate::theme;
use crate::ui::chrome::{self, Look};

/// What pressing a button means. The window decides what to do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Dismiss, having chosen nothing.
    Close,
    /// Reopen the file chooser at a folder the sandbox is hiding.
    Locate(String),
    /// Put the flatpak override command on the clipboard.
    CopyCommand(String),
    /// Go ahead with an export that would replace an existing clip.
    Overwrite,
    /// Move the file that was cut to the wastebasket.
    Trash,
    /// Ask again before removing it for good.
    ConfirmForever,
    /// Remove it for good, having asked.
    DeleteForever,
    /// Leave the original alone.
    Keep,
    /// Fetch these codecs into the Codex folder.
    Download(Vec<crate::codex::Part>),
    /// Stop a codec download that is under way.
    CancelDownload,
}

#[derive(Clone, Debug)]
pub struct Button {
    pub label: String,
    pub act: Act,
    /// Painted in the accent: the recoverable option, and the one Return picks.
    pub default: bool,
    /// Painted in the theme's warning colour.
    pub destructive: bool,
    /// What Escape picks. Escape always means "change nothing".
    pub escape: bool,
}

impl Button {
    pub fn new(label: &str, act: Act) -> Self {
        Self {
            label: label.to_string(),
            act,
            default: false,
            destructive: false,
            escape: false,
        }
    }

    pub fn default_button(mut self) -> Self {
        self.default = true;
        self
    }

    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    pub fn escape(mut self) -> Self {
        self.escape = true;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Dialog {
    pub title: String,
    pub text: String,
    pub detail: String,
    pub buttons: Vec<Button>,
    pub warning: bool,
    /// A bar under the text: `Some(fraction)` for work that can be measured,
    /// `Some(-1.0)` for work that can only be said to be going on.
    pub progress: Option<f32>,
}

impl Dialog {
    /// A plain statement with nothing to decide.
    pub fn message(title: &str, text: &str, detail: &str) -> Self {
        Self {
            title: title.to_string(),
            text: text.to_string(),
            detail: detail.to_string(),
            buttons: vec![Button::new("OK", Act::Close).default_button().escape()],
            warning: false,
            progress: None,
        }
    }

    pub fn warning(title: &str, text: &str, detail: &str) -> Self {
        Self {
            warning: true,
            ..Self::message(title, text, detail)
        }
    }

    pub fn with_buttons(mut self, buttons: Vec<Button>) -> Self {
        self.buttons = buttons;
        self
    }
}

/// Show the dialog. Returns what the user chose, or None while it is still up.
///
/// [`egui::Modal`] provides the backdrop and blocks everything behind it, so
/// the window underneath keeps drawing but cannot be reached -- which is what
/// makes a dialog offering to delete a file safe to leave on screen.
pub fn show(ctx: &egui::Context, look: &Look, dialog: &Dialog) -> Option<Act> {
    let t = look.theme;
    let mut chosen: Option<Act> = None;

    // The keyboard answers before the pointer does: Escape always means "change
    // nothing", and Return takes the safe option, which is what makes the
    // recoverable button the default one everywhere below.
    let (escape, enter) = ctx.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::Enter)));
    if escape {
        if let Some(button) = dialog.buttons.iter().find(|b| b.escape) {
            chosen = Some(button.act.clone());
        }
    } else if enter {
        if let Some(button) = dialog.buttons.iter().find(|b| b.default) {
            chosen = Some(button.act.clone());
        }
    }

    let frame = egui::Frame::NONE
        .fill(theme::c32(t.popup_bg))
        .stroke(egui::Stroke::new(
            1.0,
            if dialog.warning {
                theme::c32(t.destructive)
            } else {
                theme::c32(t.border)
            },
        ))
        .corner_radius(t.radius)
        .inner_margin(egui::Margin::same(16));

    egui::Modal::new(egui::Id::new("dialog"))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(150))
        .show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.vertical(|ui| {
                chrome::text(ui, look, &dialog.title, theme::c32(t.text_dim), false);
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(&dialog.text)
                        .color(theme::c32(t.text))
                        .size(theme::base_size(t) + 2.0)
                        .strong(),
                );
                if !dialog.detail.is_empty() {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new(&dialog.detail).color(theme::c32(t.text_dim)));
                }
                if let Some(fraction) = dialog.progress {
                    ui.add_space(10.0);
                    let bar = if fraction < 0.0 {
                        egui::ProgressBar::new(0.0).animate(true)
                    } else {
                        egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage()
                    };
                    ui.add(bar.fill(look.accent()).desired_width(480.0));
                }
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Right to left, so the first button in the list is the
                    // rightmost -- where the safe answer belongs.
                    for button in &dialog.buttons {
                        let kind = if button.default {
                            chrome::Kind::Primary
                        } else {
                            chrome::Kind::Normal
                        };
                        // A destructive button borrows the theme's warning
                        // colour for its own accent, so it cannot read as the
                        // ordinary one.
                        let look = if button.destructive {
                            Look {
                                accent: t.destructive.to_string(),
                                ..look.clone()
                            }
                        } else {
                            look.clone()
                        };
                        if chrome::button(ui, &look, &button.label, kind, true).clicked() {
                            chosen = Some(button.act.clone());
                        }
                    }
                });
            });
        });

    chosen
}
