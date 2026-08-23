// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! What the Flatpak sandbox can and cannot reach.
//!
//! Inside the Flatpak only a few locations are mounted, so a video on a second
//! drive does not merely fail to open -- it does not appear to exist at all,
//! and ffprobe says so in exactly the words it would use for a deleted file.
//! That is the one failure the user can fix, and the one they cannot diagnose,
//! so this module works out which case it is and what to tell them.
//!
//! Outside a Flatpak -- which is every Windows and macOS build, and a Linux one
//! installed from source -- every function answers "there is no sandbox", so
//! the rest of the app can call it unconditionally.

use std::collections::BTreeMap;
use std::path::Path;

use crate::paths;

pub const FLATPAK_INFO: &str = "/.flatpak-info";

/// Only used when /.flatpak-info cannot say: the command the dialog offers
/// still has to name the right application.
pub const APP_ID: &str = "io.github.raulblideran.ApricotStudio";

/// What [`diagnose`] concluded about a file that would not open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// It is right there -- ffprobe failed for its own reasons.
    Readable,
    /// The folder is visible and the file is not in it.
    Missing,
    /// It is there but its permissions say no.
    Unreadable,
    /// The sandbox is not mounting the folder it lives in.
    Blocked,
}

/// Why a file would not open, and the folder to talk about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trouble {
    pub kind: Kind,
    pub path: String,
    pub folder: String,
}

/// A filesystem entry may carry the access it was granted with.
const ACCESS_MODES: [&str; 3] = [":ro", ":rw", ":create"];

/// Manifest names for places a person would recognise. Everything else a
/// manifest lists is plumbing -- the wastebasket, a config file -- and saying
/// it out loud in a dialog about videos would only confuse.
fn friendly(entry: &str) -> Option<&'static str> {
    Some(match entry {
        "home" => "Home",
        "host" => "the whole system",
        "xdg-desktop" => "Desktop",
        "xdg-documents" => "Documents",
        "xdg-download" => "Downloads",
        "xdg-music" => "Music",
        "xdg-pictures" => "Pictures",
        "xdg-public-share" => "Public",
        "xdg-templates" => "Templates",
        "xdg-videos" => "Videos",
        _ => return None,
    })
}

/// /.flatpak-info, parsed, or None if there is nothing to parse.
///
/// Never fails loudly. A missing file and a malformed one mean the same thing
/// to every caller: no reliable answer about what this sandbox allows.
pub fn info(source: &str) -> Option<BTreeMap<String, BTreeMap<String, String>>> {
    let text = std::fs::read_to_string(source).ok()?;
    let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.to_string();
            sections.entry(current.clone()).or_default();
        } else if let Some((key, value)) = line.split_once('=') {
            if !current.is_empty() {
                sections
                    .entry(current.clone())
                    .or_default()
                    .insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }
    // An empty file parses cleanly and says nothing, which is the same answer.
    if sections.is_empty() {
        None
    } else {
        Some(sections)
    }
}

/// Whether this process is running inside a Flatpak.
///
/// The file's presence is the signal, not its contents -- a corrupt one still
/// means the app is confined.
pub fn is_sandboxed_at(source: &str) -> bool {
    Path::new(source).exists()
}

pub fn is_sandboxed() -> bool {
    cfg!(target_os = "linux") && is_sandboxed_at(FLATPAK_INFO)
}

pub fn app_id(source: &str) -> String {
    info(source)
        .and_then(|parsed| {
            parsed
                .get("Application")
                .and_then(|section| section.get("name"))
                .map(|name| name.trim().to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| APP_ID.to_string())
}

/// The locations this sandbox was given, as the manifest words them.
///
/// For telling the user what is already allowed, nothing more. Whether a path
/// is reachable is settled by looking at the filesystem: names like
/// `xdg-videos` and `host` do not resolve to a directory without knowing the
/// user's own layout, and a wrong guess here would misreport the one thing this
/// module exists to get right.
pub fn granted_folders(source: &str) -> Vec<String> {
    let Some(parsed) = info(source) else {
        return Vec::new();
    };
    let raw = parsed
        .get("Context")
        .and_then(|section| section.get("filesystems"))
        .cloned()
        .unwrap_or_default();
    let mut folders: Vec<String> = Vec::new();
    for entry in raw.split(';') {
        let mut entry = entry.trim();
        for mode in ACCESS_MODES {
            if let Some(stripped) = entry.strip_suffix(mode) {
                entry = stripped;
                break;
            }
        }
        if !entry.is_empty() && !folders.iter().any(|f| f == entry) {
            folders.push(entry.to_string());
        }
    }
    folders
}

/// The granted folders a person would recognise, in reading order.
pub fn reachable_names(source: &str) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for entry in granted_folders(source) {
        if let Some(label) = friendly(&entry) {
            if !names.contains(&label) {
                names.push(label);
            }
        }
    }
    names
}

/// Why `path` would not open, in terms the user can act on.
///
/// The test is the filesystem itself rather than the granted list, because
/// inside the sandbox an ungranted location is simply not mounted: a folder
/// that does not exist here is precisely the folder that was never allowed.
/// Outside a sandbox that same missing folder means what it says, so this stays
/// quiet and lets the ordinary message through.
pub fn diagnose_with(path: &str, sandboxed: bool) -> Trouble {
    let folder = {
        let parent = paths::dirname(path);
        if parent.is_empty() {
            ".".to_string()
        } else {
            parent
        }
    };
    if paths::exists(path) {
        let kind = if readable(path) {
            Kind::Readable
        } else {
            Kind::Unreadable
        };
        return Trouble {
            kind,
            path: path.to_string(),
            folder,
        };
    }
    if paths::is_dir(&folder) {
        // We can see where it belongs, so it really is gone.
        return Trouble {
            kind: Kind::Missing,
            path: path.to_string(),
            folder,
        };
    }
    Trouble {
        kind: if sandboxed {
            Kind::Blocked
        } else {
            Kind::Missing
        },
        path: path.to_string(),
        folder,
    }
}

pub fn diagnose(path: &str) -> Trouble {
    diagnose_with(path, is_sandboxed())
}

fn readable(path: &str) -> bool {
    std::fs::File::open(path).is_ok()
}

/// Quoted, because this is meant to be pasted into a shell and plenty of
/// folders have spaces in them.
pub fn shell_quote(value: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c);
    if !value.is_empty() && value.chars().all(safe) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The host command that allows `folder` from now on.
pub fn override_command_at(folder: &str, source: &str) -> String {
    format!(
        "flatpak override --user --filesystem={} {}",
        shell_quote(folder),
        app_id(source)
    )
}

pub fn override_command(folder: &str) -> String {
    override_command_at(folder, FLATPAK_INFO)
}

/// Headline and detail for a folder the sandbox is hiding.
///
/// The wording lives here rather than in the dialog so it can be tested without
/// putting a window on screen.
pub fn explain_at(trouble: &Trouble, source: &str) -> (String, String) {
    let reachable = reachable_names(source);
    let allowed = match reachable.len() {
        0 => String::new(),
        1 => format!(" It can read {}.", reachable[0]),
        n => format!(
            " It can read {} and {}.",
            reachable[..n - 1].join(", "),
            reachable[n - 1]
        ),
    };
    let text = format!(
        "Apricot Studio is not allowed to open “{}”.",
        paths::basename(&trouble.path)
    );
    let detail = format!(
        "{} is not one of the folders this Flatpak may read. The file is hidden, \
not missing.{allowed}\n\nLocate the file to open it once. To allow the folder \
permanently, run this and restart Apricot Studio:\n\n{}",
        trouble.folder,
        override_command_at(&trouble.folder, source)
    );
    (text, detail)
}

pub fn explain(trouble: &Trouble) -> (String, String) {
    explain_at(trouble, FLATPAK_INFO)
}

/// Where the file chooser portal hands over a file it has just granted.
pub fn document_portal_root() -> String {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| {
        #[cfg(unix)]
        {
            format!("/run/user/{}", users_uid())
        }
        #[cfg(not(unix))]
        {
            String::new()
        }
    });
    if runtime.is_empty() {
        return String::new();
    }
    paths::join(&runtime, "doc")
}

#[cfg(unix)]
fn users_uid() -> u32 {
    // No libc dependency for one number: the runtime dir is almost always set,
    // and where it is not, /proc knows.
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("Uid:")
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|uid| uid.parse().ok())
            })
        })
        .unwrap_or(1000)
}

/// `folder` if a clip can be written into it, else None.
///
/// Two ways it cannot be. A file located through the portal is exposed in a
/// one-file directory under the document portal, where siblings cannot be
/// created; and a source may simply sit somewhere this app cannot write.
pub fn usable_output_dir(folder: &str) -> Option<String> {
    if folder.is_empty() {
        return None;
    }
    let folder = paths::absolute(folder);
    let root = document_portal_root();
    if !root.is_empty() && (folder == root || folder.starts_with(&format!("{root}/"))) {
        return None;
    }
    if !paths::is_dir(&folder) || !writable(&folder) {
        return None;
    }
    Some(folder)
}

/// Whether a directory will take a new file, answered by trying it.
///
/// Permission bits are not the whole story -- a read-only mount, an ACL, or a
/// sandbox bind can all refuse a write that the mode bits allow.
fn writable(folder: &str) -> bool {
    let probe = Path::new(folder).join(format!(".apricot-write-test-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Where the clip goes when the source's own folder will not take it.
pub fn fallback_output_dir() -> String {
    let videos = paths::videos_dir();
    if videos.is_dir() {
        videos.to_string_lossy().into_owned()
    } else {
        paths::home().to_string_lossy().into_owned()
    }
}

/// The folder to suggest for the clip cut out of `source`.
pub fn output_dir_for(source: &str) -> String {
    usable_output_dir(&paths::dirname(source)).unwrap_or_else(fallback_output_dir)
}
