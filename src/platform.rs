// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! The three things that are a different call on every desktop: showing a file
//! in the file manager, moving one to the wastebasket, and deleting it.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::paths;
use crate::sandbox;

/// The command that opens a file manager with `path` selected.
///
/// On Linux this is the FileManager1 interface, which Dolphin, Nautilus and
/// Nemo all implement, and which selects the file rather than merely opening
/// its folder. The URI is built rather than interpolated: a clip named
/// "my clip #2.mp4" pasted straight into a file:// string is one a parser may
/// read as ending at the #.
pub fn reveal_args(path: &str) -> (String, Vec<String>) {
    #[cfg(target_os = "linux")]
    {
        let uri = file_uri(path);
        (
            "dbus-send".to_string(),
            vec![
                "--session".into(),
                "--dest=org.freedesktop.FileManager1".into(),
                "--type=method_call".into(),
                "/org/freedesktop/FileManager1".into(),
                "org.freedesktop.FileManager1.ShowItems".into(),
                format!("array:string:{uri}"),
                "string:".into(),
            ],
        )
    }
    #[cfg(target_os = "windows")]
    {
        // explorer wants exactly this comma and no space after it.
        (
            "explorer".to_string(),
            vec![format!("/select,{}", path.replace('/', "\\"))],
        )
    }
    #[cfg(target_os = "macos")]
    {
        ("open".to_string(), vec!["-R".into(), path.to_string()])
    }
}

/// A `file://` URI with every character a parser could trip over encoded.
pub fn file_uri(path: &str) -> String {
    let absolute = paths::absolute(path);
    #[cfg(windows)]
    let absolute = format!("/{}", absolute.replace('\\', "/"));
    let mut uri = String::from("file://");
    for byte in absolute.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || "/-_.~".contains(ch) {
            uri.push(ch);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// The paths in a `text/uri-list` payload -- the other end of [`file_uri`].
///
/// This is what a desktop hands over when a file is dropped on the window: CRLF
/// between entries, `#` for a comment, and everything a parser could trip over
/// percent-encoded, which is why "my clip #2.mp4" arrives as `my%20clip%20%232`
/// and has to come back out again.
///
/// Anything that is not a local file -- a URL dragged out of a browser, the
/// `x-special/` payloads some file managers add -- is dropped here rather than
/// further in, so the caller only ever sees paths.
pub fn paths_from_uri_list(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // A bare path is not to spec, but senders do it, and the alternative is
        // a drop that silently does nothing.
        let rest = match line.strip_prefix("file://") {
            Some(rest) => rest.strip_prefix("localhost").unwrap_or(rest),
            None if line.starts_with('/') => line,
            None => continue,
        };
        if !rest.starts_with('/') {
            continue;
        }
        let decoded = percent_decode(rest);
        // `file:///C:/clip.mp4` puts a slash in front of the drive letter, and
        // nothing on Windows opens a path that starts with one. Handled here
        // rather than behind a cfg, so the rule is compiled and tested on every
        // platform rather than only on the one that needs it. The slashes
        // themselves are left alone: Windows takes either kind.
        let decoded = match decoded.strip_prefix('/') {
            Some(rest) if drive_letter(rest) => rest.to_string(),
            _ => decoded,
        };
        if !decoded.is_empty() {
            paths.push(decoded);
        }
    }
    paths
}

/// Whether a path begins `C:`, which is a drive rather than a directory.
fn drive_letter(path: &str) -> bool {
    let mut chars = path.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.next() == Some(':')
}

/// `%20` back to a space, and every other escape back to its byte.
///
/// Byte-wise rather than char-wise: a percent escape encodes one byte of UTF-8,
/// so a non-ASCII filename arrives as several of them and only makes sense once
/// they have been put back together.
fn percent_decode(text: &str) -> String {
    let hex = |byte: u8| (byte as char).to_digit(16);
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // Indexed by byte rather than by char on purpose: slicing the string
        // two bytes past a % would land in the middle of an accented character
        // that was never escaped in the first place.
        let escape = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| Some(hex(bytes[i + 1])? * 16 + hex(bytes[i + 2])?))
            .flatten();
        match escape {
            Some(byte) => {
                out.push(byte as u8);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Open the file manager with `path` selected, falling back to its folder.
///
/// Detached rather than waited on: nothing here cares when a file manager
/// exits, and a child left unreaped is a zombie for every click.
pub fn reveal(path: &str) {
    let (program, args) = reveal_args(path);
    let launched = Command::new(&program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok();
    if !launched {
        open_folder(&paths::dirname(path));
    }
}

pub fn open_folder(folder: &str) {
    #[cfg(target_os = "linux")]
    let (program, args) = ("xdg-open", vec![folder.to_string()]);
    #[cfg(target_os = "windows")]
    let (program, args) = ("explorer", vec![folder.replace('/', "\\")]);
    #[cfg(target_os = "macos")]
    let (program, args) = ("open", vec![folder.to_string()]);

    let _ = Command::new(program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Hand the file to the desktop's wastebasket.
///
/// The `trash` crate is the right answer everywhere except inside a Flatpak.
/// There it would write to `XDG_DATA_HOME`, which the sandbox has redirected to
/// the app's own private directory -- a wastebasket no file manager will ever
/// look in, and no user will ever find. So a sandboxed build writes the
/// freedesktop trash entry into the real home wastebasket itself, which is what
/// `--filesystem=~/.local/share/Trash:create` in the manifest is for.
pub fn move_to_trash(path: &str) -> Result<(), String> {
    if sandbox::is_sandboxed() {
        return trash_by_hand(path);
    }
    match trash::delete(path) {
        Ok(()) => Ok(()),
        Err(error) => {
            // On Linux the fallback is the same spec by hand, which covers a
            // desktop whose trash implementation is missing or refusing.
            if cfg!(target_os = "linux") {
                trash_by_hand(path).map_err(|fallback| format!("{error}; {fallback}"))
            } else {
                Err(error.to_string())
            }
        }
    }
}

/// The freedesktop trash spec, done directly.
///
/// Targets the home wastebasket, which is where every location this app can
/// reach ends up. Deliberately does not pre-check the device: inside a sandbox
/// each granted path is a separate bind mount, so two directories on one
/// filesystem report the same device yet still refuse a rename. Whether the
/// move works is decided by attempting it.
fn trash_by_hand(path: &str) -> Result<(), String> {
    let trash = paths::home().join(".local/share/Trash");
    let files_dir = trash.join("files");
    let info_dir = trash.join("info");
    for dir in [&files_dir, &info_dir] {
        std::fs::create_dir_all(dir).map_err(|e| format!("no wastebasket available ({e})"))?;
    }

    let source = paths::absolute(path);
    let (stem, ext) = {
        let name = paths::basename(&source);
        match name.rfind('.') {
            Some(index) if index > 0 => (name[..index].to_string(), name[index..].to_string()),
            _ => (name, String::new()),
        }
    };

    // The spec wants the info file created first, and its name to be unique
    // against both directories.
    let mut name = format!("{stem}{ext}");
    let mut n = 1;
    let (info_path, mut handle) = loop {
        let candidate = info_dir.join(format!("{name}.trashinfo"));
        match std::fs::File::create_new(&candidate) {
            Ok(file) => break (candidate, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                name = format!("{stem}_{n}{ext}");
                n += 1;
            }
            Err(e) => return Err(e.to_string()),
        }
    };

    use std::io::Write;
    let record = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        percent_encode(&source),
        deletion_date()
    );
    handle
        .write_all(record.as_bytes())
        .map_err(|e| e.to_string())?;
    drop(handle);

    let destination = files_dir.join(&name);
    if std::fs::rename(&source, &destination).is_ok() {
        return Ok(());
    }
    // Different mount points, so a rename cannot work. Copy then unlink,
    // removing the source only once the copy is safely written.
    match std::fs::copy(&source, &destination).and_then(|_| std::fs::remove_file(&source)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&destination);
            let _ = std::fs::remove_file(&info_path); // don't leave a record with no file
            Err(error.to_string())
        }
    }
}

pub fn delete_forever(path: &str) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

/// Everything but the unreserved set, the way the trash spec wants the path.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || "/-_.~".contains(ch) {
            out.push(ch);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Local time as the trash spec spells it, seconds resolution.
///
/// Computed from the epoch rather than pulled from a date library: this is one
/// string in one file, and the only thing that reads it is a file manager
/// showing when something was deleted.
fn deletion_date() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let local = now + local_offset_seconds(now);
    let (days, seconds) = (local.div_euclid(86_400), local.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

/// How far local time runs ahead of UTC, asked of the system rather than
/// guessed. A wrong answer only misdates one line in a trashinfo file, so a
/// platform that will not say gets UTC.
fn local_offset_seconds(_now: i64) -> i64 {
    #[cfg(unix)]
    {
        // `date +%z` is on every unix and needs no dependency.
        if let Ok(out) = Command::new("date").arg("+%z").output() {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if text.len() == 5 {
                let sign = if text.starts_with('-') { -1 } else { 1 };
                let hours: i64 = text[1..3].parse().unwrap_or(0);
                let minutes: i64 = text[3..5].parse().unwrap_or(0);
                return sign * (hours * 3600 + minutes * 60);
            }
        }
    }
    0
}

/// Days since the epoch -> (year, month, day). Howard Hinnant's civil_from_days.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// Whether a path names a video this app will open.
pub fn is_video(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    VIDEO_EXTS.contains(&ext.as_str())
}

/// One list, two shapes. Kept separately they drifted: .m2ts, .3gp and .ogv
/// could be dropped onto the window but were invisible in the Open dialog.
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mkv", "mov", "webm", "avi", "m4v", "ts", "flv", "wmv", "mpg", "mpeg", "m2ts", "3gp",
    "ogv",
];
