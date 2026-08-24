//! Dropped files.
//!
//! Not part of upstream smithay-clipboard. See VENDORING.md: a Wayland client
//! may only usefully own one `wl_data_device`, this crate owns the one, and
//! drag-and-drop arrives on it. Upstream discards those events; here they are
//! handed to whoever asked for them.
//!
//! Deliberately dumb. It knows nothing about what a drop means, only that some
//! text arrived under a mime type someone asked for, so the patch stays small
//! enough to reapply against a new upstream release.

use std::sync::OnceLock;

/// The mime type a file manager offers when dropping files.
pub const URI_LIST: &str = "text/uri-list";

/// What happened to a drag over the surface.
pub enum DropEvent {
    /// A drag carrying [`URI_LIST`] arrived, and has been accepted.
    Entered,
    /// It left, or was cancelled, without dropping.
    Left,
    /// It was dropped. The payload is the raw `text/uri-list` it carried.
    Dropped(String),
}

type Sink = Box<dyn Fn(DropEvent) + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();

/// Ask to be told about files dropped on this application's surfaces.
///
/// Callable once; later calls are ignored. The callback runs on the clipboard
/// thread, so it must not block -- send the event somewhere and return.
pub fn watch_drops(sink: Sink) {
    let _ = SINK.set(sink);
}

/// Whether anyone is listening, so the drag can be refused when nobody is.
pub(crate) fn watched() -> bool {
    SINK.get().is_some()
}

pub(crate) fn emit(event: DropEvent) {
    if let Some(sink) = SINK.get() {
        sink(event);
    }
}
