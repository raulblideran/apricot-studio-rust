# Why this crate is vendored

This is `smithay-clipboard` 0.7.3, unmodified except for drag-and-drop.

Apricot Studio needs to know when a file is dropped on its window. On X11,
Windows and macOS the window toolkit reports that; on Wayland winit has no
drag-and-drop at all, so the application has to speak `wl_data_device`
itself. The obvious way to do that is to open a second data device on the
connection eframe already holds, and that is what `src/dnd.rs` in the
application originally did.

It does not work. A Wayland client may create several `wl_data_device`
objects, but a compositor decides which one a drag is delivered to, and
KWin 6.7.4 delivers it to the first one the client created -- which is this
crate's, opened while egui-winit is still building the window, before any
application code runs. Nothing the application does can make itself first.
KWin supports several devices on master; released Plasma does not. Mutter
picks one too, though it evicts the older device rather than the newer.

So the client gets one data device, this crate owns it, and the drag lands
here. Upstream's `DataDeviceHandler::enter`, `leave` and `drop_performed`
are empty stubs; they are filled in to accept `text/uri-list` and hand the
payload to a callback. Nothing else is changed, and nothing about the
clipboard behaviour is touched.

## The patch

- `src/dnd.rs` -- new. The callback registry, and `DropEvent`.
- `src/lib.rs` -- `mod dnd;` and the `watch_drops` / `DropEvent` re-export.
- `src/state.rs` -- the three `DataDeviceHandler` stubs, and one import.

The crates.io packaging that is not needed to build has been removed: the
example, the upstream CI workflows, and cargo's own bookkeeping files.
`Cargo.toml.orig` is kept because it, rather than the generated `Cargo.toml`,
is what a new upstream release should be diffed against.

## Reapplying it against a new upstream release

Diff this directory against the crates.io tarball for the version it claims
in Cargo.toml. The three stubs are the whole of it; if upstream has filled
them in itself, or grown a drag-and-drop API of its own, delete this
directory and the `[patch.crates-io]` entry in the root Cargo.toml instead.

Upstream is MIT (see LICENSE), which the application's GPL-3.0-or-later
permits; the licence file is kept alongside the source it covers.
