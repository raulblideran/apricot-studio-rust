// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! Apricot Studio -- open a video, mark in and out, export the clip.
//!
//! The export inherits every setting from the source file, so there is nothing
//! to configure. Run with an optional file path:
//!
//!     apricot-studio ~/Videos/Replay_2026-02-23_22-28-36.mp4

// No console window behind the app on Windows. The CLI flags below still work
// from a terminal, which attaches to the parent console when there is one.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use apricot_studio::app::{App, VERSION};
use apricot_studio::{paths, sandbox, settings::Settings};

const USAGE: &str = "\
Apricot Studio -- open a video, mark in and out, export the clip.

    apricot-studio [FILE]

  -h, --help     show this and exit
  -V, --version  show the version and exit

Space play/pause · I / O set in and out · S snap the in-point to a keyframe
Z zoom · Ctrl+Return export · Esc cancel
";

/// Answer the flags that need no window. `Some(code)` means "we are done".
///
/// Runs before anything graphical is built, so --version answers over ssh and
/// on a machine with no display -- and so it stays testable without a window.
fn cli(argv: &[String]) -> Option<i32> {
    for arg in argv {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Some(0);
            }
            "-V" | "--version" => {
                println!("apricot-studio {VERSION}");
                return Some(0);
            }
            _ => {}
        }
    }
    None
}

fn main() -> eframe::Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli(&argv) {
        std::process::exit(code);
    }

    let path = argv.iter().find(|arg| !arg.starts_with('-')).cloned();
    if let Some(path) = path.as_deref() {
        let expanded = paths::expand_user(path);
        if sandbox::diagnose(&expanded).kind == sandbox::Kind::Missing {
            // A file that is genuinely not there is a typo, and a terminal
            // wants an exit code rather than a window. A file the sandbox is
            // hiding is the opposite: "Open With" from the file manager comes
            // through here, and exiting silently looks like the app failed to
            // start, so let the window open and explain itself.
            eprintln!("apricot-studio: no such file: {path}");
            std::process::exit(1);
        }
    }

    // Windows only: the zip carries the app alone, so a first run has no
    // codecs. Ask before the window opens -- with the system's own message
    // box, the one place a question can be answered before anything else
    // happens -- and let the window carry out the answer.
    #[cfg(windows)]
    let download_codecs = !apricot_studio::ffmpeg::missing().is_empty() && {
        let (title, text) = apricot_studio::app::codecs_prompt();
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title(title)
            .set_description(text)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes
    };
    #[cfg(not(windows))]
    let download_codecs = false;

    let size = Settings::load().window;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Apricot Studio")
            // Lets Wayland match the window to the installed .desktop entry, so
            // the task manager shows the right name and icon.
            .with_app_id("io.github.raulblideran.ApricotStudio")
            .with_inner_size(size)
            .with_min_inner_size([900.0, 620.0])
            .with_icon(icon()),
        ..Default::default()
    };

    eframe::run_native(
        "Apricot Studio",
        options,
        Box::new(move |cc| Ok(Box::new(App::with_download(cc, path, download_codecs)))),
    )
}

/// The window icon, decoded from the PNG built into the binary.
fn icon() -> egui::IconData {
    const BYTES: &[u8] = include_bytes!("../assets/icon-256.png");
    match image::load_from_memory(BYTES) {
        Ok(image) => {
            let image = image.into_rgba8();
            let (width, height) = image.dimensions();
            egui::IconData {
                rgba: image.into_raw(),
                width,
                height,
            }
        }
        // An icon is decoration; a window without one still works.
        Err(_) => egui::IconData::default(),
    }
}
