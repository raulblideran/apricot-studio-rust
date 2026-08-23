// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! Sandbox reasoning tests.
//!
//! No window and no ffmpeg: a written-out /.flatpak-info stands in for the real
//! one, and a temporary directory stands in for the filesystem the sandbox
//! either does or does not mount. Both halves of the question -- confined or
//! not -- are forced explicitly, so the suite gives the same answer whether it
//! is run from a shell or from inside the flatpak.

use apricot_studio::sandbox::{self, Kind};

/// What the app's own /.flatpak-info actually looks like, runtime defaults and
/// all. The awkward parts are deliberate: an access mode on two entries, a
/// trailing separator, and plumbing nobody wants named in a dialog.
const FLATPAK_INFO: &str = "\
[Application]
name=io.github.raulblideran.ApricotStudio

[Instance]
flatpak-version=1.16.1
session-bus-proxy=true

[Context]
shared=ipc;
sockets=fallback-x11;pulseaudio;wayland;
devices=dri;
filesystems=xdg-download;xdg-pictures;xdg-videos;xdg-config/kdeglobals:ro;~/.local/share/Trash:create;

[Session Bus Policy]
org.freedesktop.FileManager1=talk
";

struct Tree {
    dir: tempfile::TempDir,
}

impl Tree {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("a scratch directory"),
        }
    }

    fn path(&self, name: &str) -> String {
        self.dir.path().join(name).to_string_lossy().into_owned()
    }

    fn write(&self, name: &str, body: &str) -> String {
        let path = self.path(name);
        std::fs::write(&path, body).expect("write");
        path
    }

    fn info_file(&self) -> String {
        self.write("flatpak-info", FLATPAK_INFO)
    }
}

// ----- reading /.flatpak-info --------------------------------------------

#[test]
fn reports_the_application_id() {
    let tree = Tree::new();
    assert_eq!(
        sandbox::app_id(&tree.info_file()),
        "io.github.raulblideran.ApricotStudio"
    );
}

#[test]
fn lists_granted_filesystems_without_their_access_modes() {
    let tree = Tree::new();
    assert_eq!(
        sandbox::granted_folders(&tree.info_file()),
        vec![
            "xdg-download",
            "xdg-pictures",
            "xdg-videos",
            "xdg-config/kdeglobals",
            "~/.local/share/Trash",
        ]
    );
}

#[test]
fn names_only_folders_a_person_would_recognise() {
    let tree = Tree::new();
    let names = sandbox::reachable_names(&tree.info_file());
    assert_eq!(names, vec!["Downloads", "Pictures", "Videos"]);
    // The wastebasket and a config file are plumbing, not places the user put a
    // video in.
    assert!(!names.join(" ").contains("Trash"));
}

#[test]
fn an_absent_file_means_no_sandbox() {
    let tree = Tree::new();
    let missing = tree.path("nothing-here");
    assert!(sandbox::info(&missing).is_none());
    assert!(!sandbox::is_sandboxed_at(&missing));
}

#[test]
fn a_corrupt_file_still_means_confined() {
    // The file's presence is the signal, not its contents.
    let tree = Tree::new();
    let broken = tree.write("broken-info", "this is not an ini file at all");
    assert!(sandbox::is_sandboxed_at(&broken));
    // And the app id falls back rather than coming back empty.
    assert_eq!(sandbox::app_id(&broken), sandbox::APP_ID);
    assert!(sandbox::granted_folders(&broken).is_empty());
}

#[test]
fn an_empty_file_says_nothing() {
    let tree = Tree::new();
    let empty = tree.write("empty-info", "");
    assert!(sandbox::info(&empty).is_none());
    assert_eq!(sandbox::app_id(&empty), sandbox::APP_ID);
}

// ----- telling a hidden folder from a missing file ------------------------

#[test]
fn a_file_that_is_there_is_readable() {
    let tree = Tree::new();
    let path = tree.write("clip.mp4", "x");
    assert_eq!(sandbox::diagnose_with(&path, true).kind, Kind::Readable);
}

#[test]
fn a_file_missing_from_a_visible_folder_is_missing() {
    // We can see where it belongs, so it really is gone -- confined or not.
    let tree = Tree::new();
    let path = tree.path("deleted.mp4");
    for sandboxed in [true, false] {
        assert_eq!(
            sandbox::diagnose_with(&path, sandboxed).kind,
            Kind::Missing,
            "sandboxed={sandboxed}"
        );
    }
}

#[test]
fn a_file_in_an_unmounted_folder_is_blocked_only_inside_the_sandbox() {
    // Inside the sandbox an ungranted location is simply not mounted: a folder
    // that does not exist here is precisely the folder that was never allowed.
    // Outside, that same missing folder means what it says.
    // A path chosen so it cannot exist on the machine running the tests: the
    // point is a folder the sandbox never mounted, and a real second drive
    // would make this assert the opposite of what it means.
    let path = "/var/mnt/apricot-not-mounted/Videos/replay.mp4";
    assert_eq!(sandbox::diagnose_with(path, true).kind, Kind::Blocked);
    assert_eq!(sandbox::diagnose_with(path, false).kind, Kind::Missing);
}

#[test]
fn the_folder_it_names_is_the_one_to_grant() {
    let trouble = sandbox::diagnose_with("/var/mnt/apricot-not-mounted/clips/replay.mp4", true);
    assert_eq!(trouble.folder, "/var/mnt/apricot-not-mounted/clips");
}

// ----- what to say about it ----------------------------------------------

#[test]
fn the_command_names_the_folder_and_the_app() {
    let tree = Tree::new();
    let command = sandbox::override_command_at("/var/mnt/Videos", &tree.info_file());
    assert!(command.starts_with("flatpak override --user"), "{command}");
    assert!(
        command.contains("--filesystem=/var/mnt/Videos"),
        "{command}"
    );
    assert!(
        command.ends_with("io.github.raulblideran.ApricotStudio"),
        "{command}"
    );
}

#[test]
fn a_folder_with_spaces_is_quoted() {
    // This is meant to be pasted into a shell, and plenty of folders have
    // spaces in them.
    let tree = Tree::new();
    let command = sandbox::override_command_at("/var/mnt/My Videos", &tree.info_file());
    assert!(command.contains("'/var/mnt/My Videos'"), "{command}");
}

#[test]
fn a_folder_with_a_quote_in_it_cannot_break_out() {
    let tree = Tree::new();
    let command = sandbox::override_command_at("/tmp/it's here", &tree.info_file());
    assert!(command.contains(r"'\''"), "{command}");
    assert!(!command.contains("; rm"), "{command}");
}

#[test]
fn the_explanation_says_hidden_not_missing() {
    let tree = Tree::new();
    let trouble = sandbox::diagnose_with("/var/mnt/apricot-not-mounted/replay.mp4", true);
    let (text, detail) = sandbox::explain_at(&trouble, &tree.info_file());
    assert!(text.contains("replay.mp4"), "{text}");
    assert!(text.contains("not allowed"), "{text}");
    assert!(detail.contains("hidden, not missing"), "{detail}");
    // It lists what the app *can* read, so the user can tell where to put it.
    assert!(
        detail.contains("Downloads, Pictures and Videos"),
        "{detail}"
    );
    // And carries the command, because the button that copies it closes the box.
    assert!(detail.contains("flatpak override --user"), "{detail}");
}

#[test]
fn the_explanation_survives_a_sandbox_that_granted_nothing() {
    let tree = Tree::new();
    let bare = tree.write(
        "bare-info",
        "[Application]\nname=io.github.raulblideran.ApricotStudio\n",
    );
    let trouble = sandbox::diagnose_with("/var/mnt/apricot-not-mounted/replay.mp4", true);
    let (_, detail) = sandbox::explain_at(&trouble, &bare);
    assert!(!detail.contains("It can read"), "{detail}");
    assert!(detail.contains("flatpak override"), "{detail}");
}

// ----- where a clip may be written ---------------------------------------

#[test]
fn a_writable_folder_is_usable() {
    let tree = Tree::new();
    let folder = tree.dir.path().to_string_lossy().into_owned();
    assert_eq!(sandbox::usable_output_dir(&folder), Some(folder.clone()));
}

#[test]
fn a_folder_that_is_not_there_is_not_usable() {
    let tree = Tree::new();
    assert_eq!(sandbox::usable_output_dir(&tree.path("nope")), None);
    assert_eq!(sandbox::usable_output_dir(""), None);
}

#[test]
fn the_document_portal_is_never_a_destination() {
    // A file located through the portal is exposed in a one-file directory
    // where siblings cannot be created, so a clip written beside it would fail.
    let root = sandbox::document_portal_root();
    if root.is_empty() {
        return; // no runtime directory on this machine; nothing to assert
    }
    let exposed = format!("{root}/12345");
    assert_eq!(sandbox::usable_output_dir(&exposed), None);
}

#[test]
fn the_fallback_is_somewhere_that_exists() {
    let fallback = sandbox::fallback_output_dir();
    assert!(!fallback.is_empty());
    assert!(std::path::Path::new(&fallback).is_dir(), "{fallback}");
}

#[test]
fn a_clip_goes_beside_its_source_when_that_is_possible() {
    let tree = Tree::new();
    let source = tree.write("replay.mp4", "x");
    assert_eq!(
        sandbox::output_dir_for(&source),
        tree.dir.path().to_string_lossy()
    );
}

#[test]
fn a_clip_falls_back_when_the_source_folder_will_not_take_it() {
    let source = "/var/mnt/apricot-not-mounted/Videos/replay.mp4";
    assert_eq!(
        sandbox::output_dir_for(source),
        sandbox::fallback_output_dir()
    );
}
