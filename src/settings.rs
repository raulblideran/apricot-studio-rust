// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! What the app remembers between launches.
//!
//! One JSON file in the platform's config directory. Deliberately small: the
//! theme, the accent, the recent list and the size of the window.
//!
//! The delete-original checkbox is deliberately *not* remembered. It arms
//! something irreversible, so it is chosen again every launch or not at all.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::paths;

pub const MAX_RECENT: usize = 10;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    pub accent: String,
    pub recent: Vec<String>,
    pub window: [f32; 2],

    #[serde(skip)]
    path: PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: crate::theme::DEFAULT.key.to_string(),
            accent: crate::theme::DEFAULT_ACCENT.to_string(),
            recent: Vec::new(),
            window: [1180.0, 900.0],
            path: PathBuf::new(),
        }
    }
}

fn config_dir() -> PathBuf {
    // An explicit override, so the test suite never writes the real settings
    // file -- and so a portable install can keep its settings beside itself.
    if let Some(explicit) = std::env::var_os("APRICOT_CONFIG_DIR") {
        if !explicit.is_empty() {
            return PathBuf::from(explicit);
        }
    }
    dirs::config_dir()
        .unwrap_or_else(|| paths::home().join(".config"))
        .join("apricot-studio")
}

fn config_file() -> PathBuf {
    config_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        let path = config_file();
        let mut settings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
            .or_else(inherited_from_pyqt)
            .unwrap_or_default();
        settings.path = path;
        settings
    }

    pub fn save(&self) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&self.path, text);
        }
    }

    /// The recent list, minus anything that is no longer there.
    pub fn recent(&self) -> Vec<String> {
        self.recent
            .iter()
            .filter(|path| paths::exists(path))
            .cloned()
            .collect()
    }

    pub fn push_recent(&mut self, path: &str) {
        let mut items: Vec<String> = self.recent().into_iter().filter(|p| p != path).collect();
        items.truncate(MAX_RECENT - 1);
        self.recent = std::iter::once(path.to_string()).chain(items).collect();
        self.save();
    }

    pub fn forget(&mut self, path: &str) {
        self.recent = self.recent().into_iter().filter(|p| p != path).collect();
        self.save();
    }

    pub fn clear_recent(&mut self) {
        self.recent.clear();
        self.save();
    }
}

/// Carry settings over from the PyQt version of this app.
///
/// Same reasoning the Python app had for migrating from its own older name: a
/// rewrite that silently loses the recent list and the chosen theme looks like
/// a bug to the only person who would notice. Runs once -- after the first save
/// there is a JSON file and this is never consulted again.
fn inherited_from_pyqt() -> Option<Settings> {
    if std::env::var_os("APRICOT_CONFIG_DIR").is_some() {
        return None; // a redirected config directory has nothing to inherit
    }
    let legacy = dirs::config_dir()?
        .join("ApricotStudio")
        .join("ApricotStudio.conf");
    let text = std::fs::read_to_string(legacy).ok()?;

    let mut settings = Settings::default();
    let mut found = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "theme" => {
                settings.theme = value.to_string();
                found = true;
            }
            "accent" => {
                settings.accent = value.to_string();
                found = true;
            }
            "recent" => {
                settings.recent = parse_qt_list(value);
                found = !settings.recent.is_empty() || found;
            }
            _ => {}
        }
    }
    found.then_some(settings)
}

/// QSettings writes a string list as a comma-separated line, quoting any entry
/// that contains a comma or a quote.
fn parse_qt_list(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                }
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                items.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(ch),
        }
    }
    items.push(current.trim().to_string());
    items.into_iter().filter(|item| !item.is_empty()).collect()
}
