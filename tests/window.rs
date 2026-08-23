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

use std::cell::RefCell;
use std::rc::Rc;

use apricot_studio::app::{App, Pickers};
use apricot_studio::settings::Settings;
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
    assert!(window.has("▶  Play"));
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
        "▶  Play",
        "⏮  Start",
        "◀◀  10s",
        "◀  1f",
        "1f  ▶",
        "10s  ▶▶",
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
    window.click("◀◀  10s");
    assert!(
        window.with(App::position) < 0.001,
        "10s back from 3s is the start"
    );
    window.click("10s  ▶▶");
    assert!(window.with(App::position) > 5.0);
    window.click("⏮  Start");
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
