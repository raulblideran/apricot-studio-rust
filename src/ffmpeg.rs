// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! Finding ffmpeg, and starting it the same way on every platform.
//!
//! The app is a front end for ffmpeg, so where that binary comes from differs
//! by how the app was installed:
//!
//! * Flatpak puts one in `/app/bin`, ahead of the runtime's on `PATH`.
//! * A Windows build is a folder the user unzipped, and the honest place to
//!   look first is beside the .exe -- in its `Codex` folder, which is where
//!   the app downloads them to on first run (see [`crate::codex`]).
//! * A macOS bundle keeps one in `Contents/MacOS` beside the binary, or the
//!   user has Homebrew's on `PATH`.
//!
//! So: an explicit override, then beside the executable, then `PATH`. The bare
//! name is the last resort -- it fails with a message naming ffmpeg, which is
//! the most useful thing a failure here can say.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::RwLock;

#[cfg(windows)]
const EXE: &str = ".exe";
#[cfg(not(windows))]
const EXE: &str = "";

/// Found once and kept, but not for good: a codec download lands new binaries
/// while the app is running, and [`refresh`] makes the next call look again.
static FFMPEG: RwLock<Option<PathBuf>> = RwLock::new(None);
static FFPROBE: RwLock<Option<PathBuf>> = RwLock::new(None);

/// The folder the running executable is in.
pub fn executable_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.parent().map(Path::to_path_buf)
}

fn beside_executable(name: &str) -> Option<PathBuf> {
    let dir = executable_dir()?;
    // A macOS bundle keeps helpers in Resources; a zip on Windows keeps them
    // in its Codex folder once downloaded, or beside the app when someone put
    // them there by hand.
    [
        dir.join(crate::codex::FOLDER).join(format!("{name}{EXE}")),
        dir.join(format!("{name}{EXE}")),
        dir.join("ffmpeg").join(format!("{name}{EXE}")),
        dir.join("../Resources").join(format!("{name}{EXE}")),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn on_path(name: &str) -> Option<PathBuf> {
    let filename = format!("{name}{EXE}");
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(&filename))
            .find(|candidate| candidate.is_file())
    })
}

fn locate(name: &str, override_var: &str) -> PathBuf {
    if let Some(explicit) = std::env::var_os(override_var) {
        let path = PathBuf::from(explicit);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    beside_executable(name)
        .or_else(|| on_path(name))
        .unwrap_or_else(|| PathBuf::from(name))
}

fn cached(cell: &RwLock<Option<PathBuf>>, name: &str, override_var: &str) -> PathBuf {
    if let Some(path) = cell.read().ok().and_then(|found| found.clone()) {
        return path;
    }
    let path = locate(name, override_var);
    if let Ok(mut found) = cell.write() {
        *found = Some(path.clone());
    }
    path
}

pub fn ffmpeg_path() -> PathBuf {
    cached(&FFMPEG, "ffmpeg", "APRICOT_FFMPEG")
}

pub fn ffprobe_path() -> PathBuf {
    cached(&FFPROBE, "ffprobe", "APRICOT_FFPROBE")
}

/// Forget where the binaries were, after a download put new ones in place.
pub fn refresh() {
    for cell in [&FFMPEG, &FFPROBE] {
        if let Ok(mut found) = cell.write() {
            *found = None;
        }
    }
    crate::deinterlace::forget_availability();
}

/// VapourSynth's `vspipe`, which QTGMC runs in: the Codex copy first, then
/// `PATH`. `None` when there is neither -- unlike ffmpeg, its absence is
/// ordinary, and only means one deinterlacer is not on offer.
pub fn vspipe_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("APRICOT_VSPIPE").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(explicit));
    }
    executable_dir()
        .map(|dir| crate::codex::vspipe_in(&dir.join(crate::codex::FOLDER)))
        .filter(|path| path.is_file())
        .or_else(|| on_path("vspipe"))
}

/// Whether both binaries are actually there, for the one-line complaint at
/// startup rather than a failure per action afterwards.
pub fn missing() -> Vec<&'static str> {
    let mut absent = Vec::new();
    for (name, path) in [("ffmpeg", ffmpeg_path()), ("ffprobe", ffprobe_path())] {
        // A bare name means neither the override, the executable's folder nor
        // PATH turned anything up.
        if path
            .parent()
            .map(|p| p.as_os_str().is_empty())
            .unwrap_or(true)
            && !path.is_file()
        {
            absent.push(name);
        }
    }
    absent
}

/// A [`Command`] that does not flash a console window on Windows.
///
/// Every ffmpeg call this app makes is a background job. Without this, each one
/// opens and closes a black window on top of the interface -- dozens of them
/// while a filmstrip loads.
pub fn command(program: &Path) -> Command {
    #[allow(unused_mut)] // only Windows mutates it
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
