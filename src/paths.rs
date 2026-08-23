// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! Path arithmetic on plain strings.
//!
//! Everything the interface holds -- the folder box, the name box, the recent
//! list -- is text the user can edit, so it is handled as text and only becomes
//! a [`std::path::Path`] at the point something touches the filesystem.

use std::path::{Path, PathBuf};

pub fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

pub fn dirname(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `("/tmp/clip", ".mp4")`. Splits the basename only, so a dot in a directory
/// name cannot become the extension.
pub fn split_ext(path: &str) -> (String, String) {
    let name = basename(path);
    match name.rfind('.') {
        // A leading dot is a hidden file, not an extension.
        Some(index) if index > 0 => {
            let cut = path.len() - (name.len() - index);
            (path[..cut].to_string(), path[cut..].to_string())
        }
        _ => (path.to_string(), String::new()),
    }
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        return name.to_string();
    }
    Path::new(dir).join(name).to_string_lossy().into_owned()
}

/// `~` and `~/...` against the user's home. Left alone if there is no home to
/// expand against, which is what every caller wants: the text stays as typed.
pub fn expand_user(path: &str) -> String {
    if path == "~" {
        return home().to_string_lossy().into_owned();
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        return home().join(rest).to_string_lossy().into_owned();
    }
    path.to_string()
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// The user's videos folder, or their home if the desktop has no such thing.
pub fn videos_dir() -> PathBuf {
    dirs::video_dir().unwrap_or_else(|| home().join("Videos"))
}

/// An absolute path, without requiring the file to exist.
///
/// `std::fs::canonicalize` refuses a path that is not there and resolves
/// symlinks besides, neither of which suits comparing two names the user typed.
pub fn absolute(path: &str) -> String {
    let expanded = expand_user(path);
    let candidate = Path::new(&expanded);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(candidate)
    };

    // Fold away "." and "..", which is all the normalising two typed paths
    // need before they can be compared.
    let mut parts: Vec<std::path::Component> = Vec::new();
    for part in joined.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if matches!(parts.last(), Some(std::path::Component::Normal(_))) {
                    parts.pop();
                } else {
                    parts.push(part);
                }
            }
            other => parts.push(other),
        }
    }
    parts
        .iter()
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

/// Whether two paths name the same file, as far as text can tell.
pub fn same_file(first: &str, second: &str) -> bool {
    absolute(first) == absolute(second)
}

pub fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

pub fn is_file(path: &str) -> bool {
    Path::new(path).is_file()
}

pub fn is_dir(path: &str) -> bool {
    Path::new(path).is_dir()
}

pub fn size_of(path: &str) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}
