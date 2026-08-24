// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! Dropping a file on the window, on Wayland, where the toolkit does not.
//!
//! Every other platform is already covered: winit turns XDND, OLE and
//! `performDragOperation` into `WindowEvent::DroppedFile`, egui collects those
//! into `dropped_files`, and the app reads them. Its Wayland backend has no
//! `wl_data_device` at all -- not a disabled one, none -- so on a Wayland
//! session that list is always empty however the window is configured, and the
//! app looks broken while being perfectly correct.
//!
//! The obvious repair is to open a data device of our own on the connection
//! eframe already holds. It does not work, and the reason is worth writing
//! down: a client may own several, but the compositor picks *one* to deliver a
//! drag to, and KWin 6.7 picks the first the client created. That one belongs
//! to `smithay-clipboard`, which egui-winit starts while it is still building
//! the window -- before any code here can run. Nothing this app does can put
//! itself first.
//!
//! So the drag is taken where it actually lands. `vendor/smithay-clipboard` is
//! that crate with its three empty drag-and-drop stubs filled in, and this is
//! the other end of the callback they call. Which leaves one data device for
//! the client, serving both the clipboard and the drops -- what should have
//! been true from the start, and what makes this work on every compositor
//! rather than on the ones that tolerate a second device.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// Files dropped on the window since the last look, and whether one is being
/// held over it right now.
pub struct Drops {
    paths: Receiver<String>,
    hovering: Arc<AtomicBool>,
}

impl Drops {
    /// Paths dropped since the last call. Named to match the media loaders,
    /// which the frame loop already drains this way.
    pub fn drain(&self) -> Vec<String> {
        self.paths.try_iter().collect()
    }

    /// Whether a drag is over the window right now, for the placeholder to say
    /// so. The difference between "not supported" and "you aimed badly" is the
    /// whole reason a drop target is ever visible.
    pub fn hovering(&self) -> bool {
        self.hovering.load(Ordering::Relaxed)
    }
}

/// Start listening, or `None` where there is nothing to listen for.
///
/// `None` on Windows and macOS, where egui already delivers drops and a second
/// route would open the same file twice. On an X11 session this returns a
/// [`Drops`] that stays empty, because the clipboard thread it listens to is
/// only started for a Wayland display -- harmless, and cheaper than working out
/// which display server is in use in order to answer the same way.
pub fn watch(ctx: &egui::Context) -> Option<Drops> {
    #[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
    {
        wayland::watch(ctx)
    }
    #[cfg(not(all(unix, not(any(target_os = "macos", target_os = "android")))))]
    {
        let _ = ctx;
        None
    }
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "android"))))]
mod wayland {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;

    use smithay_clipboard::DropEvent;

    use super::Drops;
    use crate::platform;

    pub fn watch(ctx: &egui::Context) -> Option<Drops> {
        let (sender, paths) = mpsc::channel();
        let hovering = Arc::new(AtomicBool::new(false));

        let held = hovering.clone();
        let ctx = ctx.clone();
        smithay_clipboard::watch_drops(Box::new(move |event| {
            match event {
                DropEvent::Entered => held.store(true, Ordering::Relaxed),
                DropEvent::Left => held.store(false, Ordering::Relaxed),
                DropEvent::Dropped(uri_list) => {
                    held.store(false, Ordering::Relaxed);
                    for path in platform::paths_from_uri_list(&uri_list) {
                        let _ = sender.send(path);
                    }
                }
            }
            // This runs on the clipboard thread, and the window is idle when
            // nothing is playing. Without a nudge the drop lands and no frame
            // is drawn to show for it.
            ctx.request_repaint();
        }));

        Some(Drops { paths, hovering })
    }
}
