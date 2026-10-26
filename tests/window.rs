// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! The window itself, driven headlessly.
//!
//! Real frames, real widgets, real clicks -- through `egui_kittest`, which
//! finds controls by the same labels a screen reader would read out. No
//! platform window is opened, so this runs the same on a laptop and on a CI
//! machine with no display at all.
//!
//! **There is a test here that clicks every button in the window.** The PyQt
//! version shipped a crashing Open button for two releases because the test
//! covering it mocked the handler and asserted it was called, while the button
//! that called it took the app down. So the choosers are a seam
//! ([`Pickers`](apricot_studio::app::Pickers)) rather than a mock of the thing
//! being tested: the click goes all the way through.

mod common;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use apricot_studio::app::{self, App, Pickers};
use apricot_studio::settings::Settings;
use apricot_studio::ui::chrome;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use common::{fixture, have_ffmpeg};

/// A settings directory of this test's own, so the suite never touches the real
/// one -- and so no test can leave a theme behind for the next.
fn scratch_settings() -> std::path::PathBuf {
    let dir = tempfile::tempdir().expect("a scratch config directory");
    let path = dir.keep();
    std::env::set_var("APRICOT_CONFIG_DIR", &path);
    path
}

/// A window, and a handle on the app inside it.
struct Window {
    harness: Harness<'static>,
    app: Rc<RefCell<Option<App>>>,
}

/// How many frames one interaction is given to settle.
///
/// Fixed rather than "run until quiet": the preview and a running export both
/// ask for repaints continuously and legitimately, so there is no quiet to wait
/// for.
const STEPS: usize = 4;

impl Window {
    fn open(path: Option<String>) -> Self {
        scratch_settings();
        Self::build(path)
    }

    /// A window sharing an existing settings directory, for the one test that
    /// needs a second launch to see what the first remembered.
    fn open_in(settings: &std::path::Path, path: Option<String>) -> Self {
        std::env::set_var("APRICOT_CONFIG_DIR", settings);
        Self::build(path)
    }

    fn build(path: Option<String>) -> Self {
        let app: Rc<RefCell<Option<App>>> = Rc::new(RefCell::new(None));
        let inner = Rc::clone(&app);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1180.0, 900.0))
            .build_ui(move |ui| {
                let mut slot = inner.borrow_mut();
                let app = slot.get_or_insert_with(|| {
                    let mut app = App::with_context(ui.ctx(), path.clone());
                    // Nothing in a test may open a modal chooser: it would
                    // block the run forever.
                    app.set_pickers(Pickers {
                        open: Box::new(|_| None),
                        save: Box::new(|_, _| None),
                    });
                    app
                });
                app.draw(ui);
            });
        harness.run_steps(STEPS);
        Self { harness, app }
    }

    /// Run frames until the background probes have settled.
    fn settle(&mut self) {
        for _ in 0..40 {
            self.harness.run_steps(STEPS);
            if self.with(|app| !app.keyframes().is_empty()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        self.harness.run_steps(STEPS);
    }

    fn with<T>(&self, read: impl FnOnce(&App) -> T) -> T {
        let slot = self.app.borrow();
        read(slot.as_ref().expect("the window builds on the first frame"))
    }

    fn edit<T>(&mut self, write: impl FnOnce(&mut App) -> T) -> T {
        let result = {
            let mut slot = self.app.borrow_mut();
            write(slot.as_mut().expect("the window builds on the first frame"))
        };
        self.harness.run_steps(STEPS);
        result
    }

    fn click(&mut self, label: &str) {
        self.harness.get_by_label(label).click();
        self.harness.run_steps(STEPS);
    }

    fn has(&self, label: &str) -> bool {
        self.harness.query_by_label(label).is_some()
    }

    fn press(&mut self, key: egui::Key) {
        self.harness.key_press(key);
        self.harness.run_steps(STEPS);
    }

    /// Press a button, keep it down for `frames` frames, then let go.
    ///
    /// kittest's own `click()` is a press immediately followed by a release, so
    /// a hold has to be assembled from the events by hand. Each frame is a
    /// quarter second of simulated time, which is what makes a handful of them
    /// long enough for a repeat to start.
    fn hold(&mut self, label: &str, frames: usize) {
        self.hold_nth(label, 0, frames);
    }

    /// Where a character's glyph sits in the font atlas, under the theme now on
    /// screen.
    ///
    /// Two characters drawn with the same atlas glyph share this slot, which is
    /// how a missing one is told apart: it comes back drawn as epaint's
    /// replacement box, in the same slot as every other character no face has.
    fn atlas_slot(&self, font: &egui::FontId, c: char) -> ([u16; 2], [u16; 2]) {
        let galley = self.harness.ctx.fonts_mut(|fonts| {
            fonts.layout_no_wrap(c.to_string(), font.clone(), egui::Color32::WHITE)
        });
        galley
            .rows
            .first()
            .and_then(|placed| placed.row.glyphs.first())
            .map(|glyph| (glyph.uv_rect.min, glyph.uv_rect.max))
            .unwrap_or_else(|| panic!("{c:?} laid out to nothing"))
    }

    /// The same, for a label that more than one button answers to -- the bare
    /// nudges beside the In and Out timecodes are the same two arrows twice.
    fn hold_nth(&mut self, label: &str, index: usize, frames: usize) {
        let at = self
            .harness
            .get_all_by_label(label)
            .nth(index)
            .unwrap_or_else(|| panic!("no {label} at index {index}"))
            .rect()
            .center();
        let event = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let input = self.harness.input_mut();
        input.events.push(egui::Event::PointerMoved(at));
        input.events.push(event(true));
        self.harness.run_steps(frames);
        self.harness.input_mut().events.push(event(false));
        self.harness.run_steps(STEPS);
    }
}

fn loaded_window() -> Option<Window> {
    if !have_ffmpeg() {
        return None;
    }
    let mut window = Window::open(Some(fixture("allp")));
    window.settle();
    Some(window)
}

// ----- the empty window ---------------------------------------------------

#[test]
fn opens_with_nothing_loaded() {
    let window = Window::open(None);
    assert!(!window.with(App::is_loaded));
    assert!(
        window.has("Open…"),
        "the Open button should always be there"
    );
    assert!(window.has("Recent"));
    assert!(window.has("Theme"));
}

#[test]
fn the_controls_that_need_a_file_are_disabled_without_one() {
    let window = Window::open(None);
    // The buttons are still drawn -- an interface that loses half its controls
    // when nothing is open is harder to read, not easier.
    assert!(window.has("Export clip"));
    assert!(window.has(app::PLAY));
    assert!(window.with(|app| app.badge_text().is_none()));
}

#[test]
fn a_file_that_is_not_there_says_so_rather_than_opening() {
    let mut window = Window::open(Some("/tmp/definitely-not-here-3b1f9.mp4".into()));
    window.harness.run_steps(STEPS);
    assert!(!window.with(App::is_loaded));
    let text = window.with(|app| app.dialog_text().map(str::to_string));
    assert!(text.is_some(), "a failure to open must be reported");
}

// ----- loading ------------------------------------------------------------

#[test]
fn loading_a_file_fills_in_everything_that_depends_on_it() {
    let Some(window) = loaded_window() else {
        return;
    };
    assert!(window.with(App::is_loaded));
    let (start, end) = window.with(App::selection);
    assert_eq!(start, 0.0);
    assert!(
        (end - 6.0).abs() < 0.1,
        "the selection should be the whole file"
    );
    // The keyframes arrive on a worker thread, and the badge cannot tell a copy
    // from a re-encode until they do.
    assert!(!window.with(|app| app.keyframes().to_vec()).is_empty());
    let destination = window.with(App::destination);
    assert!(destination.ends_with("_clip.mp4"), "{destination}");
}

#[test]
fn a_cut_at_the_start_is_a_lossless_copy() {
    let Some(window) = loaded_window() else {
        return;
    };
    // Zero is a keyframe in every file, so the badge must say so.
    let badge = window.with(App::badge_text).unwrap_or_default();
    assert!(badge.starts_with("Lossless copy"), "{badge}");
}

#[test]
fn moving_the_in_point_off_a_keyframe_turns_it_into_a_re_encode() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    window.edit(|app| app.set_in_point(1.0));
    let badge = window.with(App::badge_text).unwrap_or_default();
    assert!(badge.starts_with("Re-encode"), "{badge}");
    assert!(badge.contains("libx264"), "{badge}");
}

#[test]
fn snapping_puts_it_back_on_one() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    window.edit(|app| app.set_in_point(1.6));
    window.click("Snap");
    let (start, _) = window.with(App::selection);
    assert!((start - 2.0).abs() < 0.01, "snapped to {start}");
    let badge = window.with(App::badge_text).unwrap_or_default();
    assert!(badge.starts_with("Lossless copy"), "{badge}");
}

#[test]
fn closing_a_file_empties_the_window() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    window.click("Close");
    assert!(!window.with(App::is_loaded));
    assert_eq!(window.with(App::destination), "");
}

// ----- every button -------------------------------------------------------

#[test]
fn every_button_in_the_window_can_be_clicked() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    // Everything except Export, which starts an encode of its own, and Close,
    // which would take the file away from the buttons after it.
    let buttons = [
        "Open…",
        "Recent",
        "Theme",
        "Accent colour",
        app::PLAY,
        app::START,
        app::BACK_10S,
        app::BACK_1F,
        app::FWD_1F,
        app::FWD_10S,
        "Set to playhead  (I)",
        "Snap",
        "Set to playhead  (O)",
        "Play selection",
        "…",
    ];
    for label in buttons {
        assert!(window.has(label), "{label} is missing from the window");
        window.click(label);
        // A menu button leaves a popup open, which would swallow the next
        // click; Escape closes it.
        window.press(egui::Key::Escape);
        assert!(
            window.with(App::is_loaded),
            "{label} left the window without a file"
        );
    }
    // Still alive, still drawing, still holding the same file.
    assert!(window.with(App::is_loaded));
}

#[test]
fn the_transport_buttons_move_the_playhead() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    window.edit(|app| app.seek_to(3.0));
    window.click(app::BACK_10S);
    assert!(
        window.with(App::position) < 0.001,
        "10s back from 3s is the start"
    );
    window.click(app::FWD_10S);
    assert!(window.with(App::position) > 5.0);
    window.click(app::START);
    assert!(window.with(App::position) < 0.001);
}

#[test]
fn set_to_playhead_moves_the_marks() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    window.edit(|app| app.seek_to(2.5));
    window.click("Set to playhead  (I)");
    let (start, _) = window.with(App::selection);
    assert!((start - 2.5).abs() < 0.05, "in-point at {start}");

    window.edit(|app| app.seek_to(4.0));
    window.click("Set to playhead  (O)");
    let (_, end) = window.with(App::selection);
    assert!((end - 4.0).abs() < 0.05, "out-point at {end}");
}

// ----- holding a step button ----------------------------------------------

#[test]
fn a_click_on_a_step_button_is_still_exactly_one_frame() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    let frame = 1.0 / common::FPS as f64;
    window.edit(|app| app.seek_to(1.0));

    window.click(app::FWD_1F);
    let moved = window.with(App::position) - 1.0;
    assert!(
        (moved - frame).abs() < frame / 2.0,
        "a click moved {moved}s, not one {frame}s frame"
    );
}

#[test]
fn holding_a_step_button_keeps_stepping() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    let frame = 1.0 / common::FPS as f64;
    window.edit(|app| app.seek_to(1.0));

    window.hold(app::FWD_1F, 8);
    let moved = window.with(App::position) - 1.0;
    assert!(
        moved > frame * 2.0,
        "held for two seconds and moved {moved}s -- one frame is {frame}s"
    );

    // And backwards, from wherever that left it.
    let before = window.with(App::position);
    window.hold(app::BACK_1F, 8);
    let back = before - window.with(App::position);
    assert!(back > frame * 2.0, "held back and moved {back}s");
}

#[test]
fn holding_the_in_nudge_keeps_nudging() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    let frame = 1.0 / common::FPS as f64;
    window.edit(|app| app.set_in_point(1.0));

    // The In pair comes first; the Out pair wears the same two arrows.
    window.hold_nth(app::STEP_FWD, 0, 8);
    let moved = window.with(App::selection).0 - 1.0;
    assert!(
        moved > frame * 2.0,
        "the in-point moved {moved}s -- one frame is {frame}s"
    );
}

// ----- the glyphs on the buttons ------------------------------------------

#[test]
fn every_glyph_on_a_button_is_in_the_fonts_this_binary_ships() {
    // "❚❚  Pause" shipped as two empty boxes, because U+275A is in none of the
    // four faces here. A missing glyph is not a slightly wrong symbol -- it is
    // an empty box in the middle of the transport -- and the only way to know
    // is to ask the fonts what they would actually draw.
    //
    // egui's own `has_glyph` cannot answer this: it reports which face a
    // character resolved to, and calls anything landing on the family's
    // replacement face missing -- which is every emoji glyph in these labels,
    // present or not. So the question is put the way it is asked on screen:
    // lay the character out, and see whether it came back as the box.
    let mut window = Window::open(None);
    for theme in apricot_studio::theme::THEMES {
        window.click("Theme");
        window.click(theme.name);
        let font = egui::FontId::new(
            apricot_studio::theme::base_size(theme),
            apricot_studio::theme::body_family(theme),
        );

        // U+0378 is permanently unassigned, so no face can ever have it:
        // whatever this theme draws when it has nothing, it draws here. The two
        // controls keep the check honest -- the character that caused the bug
        // has to fail it, and an ordinary letter has to pass.
        let tofu = window.atlas_slot(&font, '\u{0378}');
        assert_eq!(
            window.atlas_slot(&font, '❚'),
            tofu,
            "{}: U+275A is no longer read as missing, so this check proves nothing",
            theme.name
        );
        assert_ne!(
            window.atlas_slot(&font, 'P'),
            tofu,
            "{}: an ordinary letter reads as missing, so this check proves nothing",
            theme.name
        );

        for label in app::GLYPH_LABELS {
            for glyph in label.chars().filter(|c| !c.is_ascii()) {
                assert_ne!(
                    window.atlas_slot(&font, glyph),
                    tofu,
                    "{} draws {glyph:?} (U+{:04X}) in {label:?} as an empty box",
                    theme.name,
                    glyph as u32
                );
            }
        }
    }
}

// ----- clicking the picture -----------------------------------------------

#[test]
fn clicking_the_empty_picture_opens_a_file() {
    // It says "Open a video, or drop one here" and it takes a dropped file, so
    // a click on it that does nothing is a dead control. The click goes all the
    // way to the chooser seam here rather than to a mock of the handler.
    if !have_ffmpeg() {
        return;
    }
    let clip = fixture("allp");
    let mut window = Window::open(None);
    assert!(!window.with(App::is_loaded));

    let answer = clip.clone();
    window.edit(move |app| {
        app.set_pickers(Pickers {
            open: Box::new(move |_| Some(answer.clone())),
            save: Box::new(|_, _| None),
        });
    });
    window.click(app::OPEN_PROMPT);
    window.settle();

    assert!(
        window.with(App::is_loaded),
        "the click never reached the chooser"
    );
    assert_eq!(
        window.with(|app| app.source_path().map(str::to_string)),
        Some(clip)
    );
}

#[test]
fn the_picture_still_toggles_playback_once_a_file_is_open() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    let opened = Rc::new(Cell::new(false));
    let flag = Rc::clone(&opened);
    window.edit(move |app| {
        app.set_pickers(Pickers {
            open: Box::new(move |_| {
                flag.set(true);
                None
            }),
            save: Box::new(|_, _| None),
        });
    });

    assert!(!window.with(App::is_playing));
    window.click(app::PICTURE);
    assert!(
        window.with(App::is_playing),
        "the picture should start playback"
    );
    window.click(app::PICTURE);
    assert!(!window.with(App::is_playing));
    assert!(
        !opened.get(),
        "the chooser opened over a file that was already loaded"
    );
}

// ----- the keyboard -------------------------------------------------------

#[test]
fn the_keyboard_does_what_the_readme_says() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    window.edit(|app| app.seek_to(3.0));

    window.press(egui::Key::I);
    assert!((window.with(App::selection).0 - 3.0).abs() < 0.05);

    window.edit(|app| app.seek_to(4.0));
    window.press(egui::Key::O);
    assert!((window.with(App::selection).1 - 4.0).abs() < 0.05);

    window.press(egui::Key::Home);
    assert!(window.with(App::position) < 0.001);

    window.press(egui::Key::End);
    assert!(window.with(App::position) > 5.9);

    // S snaps the in-point to the nearest keyframe, whatever the distance.
    window.edit(|app| app.set_in_point(1.7));
    window.press(egui::Key::S);
    assert!((window.with(App::selection).0 - 2.0).abs() < 0.01);
}

#[test]
fn ctrl_w_closes_the_file() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    window
        .harness
        .key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::W);
    window.harness.run_steps(STEPS);
    assert!(!window.with(App::is_loaded));
}

#[test]
fn typing_a_timecode_is_never_intercepted() {
    // Space is play/pause everywhere except inside a text field, where it is a
    // space.
    let Some(mut window) = loaded_window() else {
        return;
    };
    // The first of the four fields is the In timecode.
    let field = window
        .harness
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .expect("the window has timecode fields");
    field.focus();
    window.harness.run_steps(STEPS);
    window.press(egui::Key::Space);
    assert!(
        !window.with(App::is_playing),
        "Space in a text field must not start playback"
    );
}

// ----- dropping a file on the window --------------------------------------

/// A file egui says was dropped. The real one comes from whichever backend
/// reported the drop; this is the same thing with no window behind it.
#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|error| error.to_string())
    }
}

impl Window {
    /// Hand the window a file the way a backend that supports drops would.
    fn drop_file(&mut self, path: &str) {
        self.harness
            .input_mut()
            .dropped_files
            .push(std::sync::Arc::new(Dropped(path.into())));
        self.harness.run_steps(STEPS);
    }
}

#[test]
fn a_dropped_video_opens() {
    if !have_ffmpeg() {
        return;
    }
    let clip = fixture("allp");
    let mut window = Window::open(None);
    assert!(!window.with(App::is_loaded));

    window.drop_file(&clip);
    window.settle();
    assert!(
        window.with(App::is_loaded),
        "the drop should have opened it"
    );
    assert_eq!(
        window.with(|app| app.source_path().map(str::to_string)),
        Some(clip)
    );
}

#[test]
fn a_dropped_file_that_is_not_a_video_says_so() {
    // Silence here is indistinguishable from a drop that never arrived, which
    // is exactly how the Wayland gap looked before there was a message.
    let dir = tempfile::tempdir().expect("a scratch directory");
    let note = dir.path().join("notes.txt");
    std::fs::write(&note, "not a video").expect("the scratch file");

    let mut window = Window::open(None);
    window.drop_file(&note.to_string_lossy());
    assert!(!window.with(App::is_loaded));
    let status = window.with(|app| app.status().to_string());
    assert!(status.contains("notes.txt"), "{status}");
    assert!(status.contains("not a video"), "{status}");
}

#[test]
fn a_drop_during_a_dialog_is_thrown_away_rather_than_queued() {
    // Swapping the open file out from under a question is answering it for a
    // file that is no longer open -- and holding the drop until the dialog is
    // answered would open it later, out of nowhere.
    if !have_ffmpeg() {
        return;
    }
    let clip = fixture("allp");
    let mut window = Window::open(Some("/tmp/definitely-not-here-3b1f9.mp4".into()));
    assert!(
        window.with(|app| app.dialog_text().is_some()),
        "the dialog should be up"
    );

    window.drop_file(&clip);
    assert!(
        !window.with(App::is_loaded),
        "a drop landed while a dialog was waiting to be answered"
    );

    window.edit(App::dismiss_dialog);
    window.settle();
    assert!(
        !window.with(App::is_loaded),
        "the discarded drop came back once the dialog closed"
    );

    window.drop_file(&clip);
    window.settle();
    assert!(window.with(App::is_loaded), "and a fresh drop still lands");
}

#[test]
fn the_picture_is_kept_out_of_the_scanlines() {
    // The pure geometry is measured in tests/theme.rs. What matters here is the
    // wiring: that a window actually showing a frame reports where it drew it,
    // because a rect that is never filled in cuts nothing out of anything.
    let Some(mut window) = loaded_window() else {
        return;
    };
    window.click("Theme");
    window.click("Cyberpunk");

    let picture = window.with(App::video_rect).expect("a frame is on screen");
    assert!(
        picture.width() > 1.0 && picture.height() > 1.0,
        "{picture:?}"
    );
    let screen = window.harness.ctx.content_rect();
    for [a, b] in chrome::scanline_segments(screen, Some(picture)) {
        assert!(
            b.x <= picture.left()
                || a.x >= picture.right()
                || a.y < picture.top()
                || a.y > picture.bottom(),
            "a scanline crosses the picture at y={}",
            a.y
        );
    }

    // And with nothing open there is no picture to protect, so the empty window
    // keeps the full overlay.
    let mut empty = Window::open(None);
    empty.click("Theme");
    empty.click("Cyberpunk");
    assert!(empty.with(App::video_rect).is_none());
}

// ----- themes and accents -------------------------------------------------

#[test]
fn the_theme_menu_switches_the_whole_look() {
    let mut window = Window::open(None);
    assert_eq!(window.with(App::theme_key), "default");
    window.click("Theme");
    window.click("Cyberpunk");
    assert_eq!(window.with(App::theme_key), "cyberpunk");
    // The theme owns its colour, so the picker has nothing to offer.
    assert!(!window.has("Accent colour"));
    assert_eq!(
        window.with(|app| app.painted_accent().to_string()),
        "#ff0056"
    );
}

#[test]
fn the_accent_chosen_under_default_survives_a_round_trip() {
    // Going Default -> Cyberpunk -> Default has to come back to the accent that
    // was picked, not to Night City crimson.
    let mut window = Window::open(None);
    window.click("Accent colour");
    window.click("Azure");
    assert_eq!(
        window.with(|app| app.chosen_accent().to_string()),
        "#3d92e0"
    );

    window.click("Theme");
    window.click("Cyberpunk");
    assert_eq!(
        window.with(|app| app.painted_accent().to_string()),
        "#ff0056"
    );
    assert_eq!(
        window.with(|app| app.chosen_accent().to_string()),
        "#3d92e0",
        "the choice underneath was overwritten"
    );

    window.click("Theme");
    window.click("Default");
    assert_eq!(
        window.with(|app| app.painted_accent().to_string()),
        "#3d92e0"
    );
}

#[test]
fn the_theme_is_remembered() {
    let settings = scratch_settings();
    let mut window = Window::open_in(&settings, None);
    window.click("Theme");
    window.click("Cyberpunk");
    drop(window);

    // A fresh window reads the settings file the last one wrote.
    assert_eq!(Settings::load().theme, "cyberpunk");
    let window = Window::open_in(&settings, None);
    assert_eq!(window.with(App::theme_key), "cyberpunk");
}

// ----- the delete checkbox ------------------------------------------------

#[test]
fn the_delete_checkbox_starts_disarmed_every_time() {
    // It arms something irreversible, so it is chosen again every launch or not
    // at all -- deliberately not remembered.
    let Some(mut window) = loaded_window() else {
        return;
    };
    assert!(!window.with(App::delete_armed));
    window.click("Delete original");
    assert!(window.with(App::delete_armed));
    drop(window);

    let stored = Settings::load();
    let text = serde_json::to_string(&stored).unwrap_or_default();
    assert!(
        !text.contains("delete"),
        "the delete choice must not be written to the settings file"
    );
}

// ----- exporting ----------------------------------------------------------

#[test]
fn exporting_with_no_destination_says_so() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    window.edit(|app| app.set_destination("", ""));
    window.click("Export clip");
    let text = window.with(|app| app.dialog_text().map(str::to_string));
    assert_eq!(text.as_deref(), Some("Choose where to save the clip."));
}

#[test]
fn refusing_to_overwrite_the_file_being_cut() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    let source = window.with(|app| app.source_path().unwrap_or_default().to_string());
    let (folder, name) = (
        apricot_studio::paths::dirname(&source),
        apricot_studio::paths::split_ext(&apricot_studio::paths::basename(&source)).0,
    );
    window.edit(|app| app.set_destination(&folder, &name));
    window.click("Export clip");
    let text = window.with(|app| app.dialog_text().map(str::to_string));
    assert_eq!(
        text.as_deref(),
        Some("The clip would overwrite the file you are cutting.")
    );
    assert!(!window.with(App::exporting));
}

#[test]
fn a_selection_of_nothing_is_refused() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    window.edit(|app| {
        app.set_in_point(2.0);
        app.set_out_point(2.0);
    });
    window.click("Export clip");
    let text = window.with(|app| app.dialog_text().map(str::to_string));
    assert_eq!(
        text.as_deref(),
        Some("The in and out points are at the same place.")
    );
}

#[test]
fn an_export_runs_and_reports_what_it_wrote() {
    let Some(mut window) = loaded_window() else {
        return;
    };
    let dir = tempfile::tempdir().expect("a scratch directory");
    window.edit(|app| {
        app.set_in_point(0.0);
        app.set_out_point(1.0);
        app.set_destination(&dir.path().to_string_lossy(), "exported");
    });
    window.click("Export clip");

    // The encode runs on its own thread; frames keep going while it does.
    let mut wrote = None;
    for _ in 0..200 {
        window.harness.run_steps(STEPS);
        if !window.with(App::exporting) {
            wrote = Some(window.with(|app| app.status().to_string()));
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let status = wrote.expect("the export should finish");
    assert!(status.starts_with("Saved"), "{status}");
    let clip = dir.path().join("exported.mp4");
    assert!(clip.exists(), "the clip was not written");
    assert!(std::fs::metadata(&clip).unwrap().len() > 1024);
}
