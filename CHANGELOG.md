# Changelog

Versions follow `MAJOR.MINOR.PATCH`. Each section is what a release's notes on GitHub
say: `build/release-notes.sh` prints it, and the Release workflow publishes it.

## 1.3.0 — 2026-10-27

- **Deinterlacing.** An interlaced video (a camcorder's, a broadcast capture's) is
  recognised when it opens, from the file's own flag or, when it has none, by looking at
  the pictures. A **Deinterlace** menu then appears beside Quality: **BWDIF**, ffmpeg's
  own, or **QTGMC**, slower and the best there is on motion. Both start at their
  **Slowest** profile, which turns every field into a frame (50i becomes 50p). Off by
  default, so a cut on a keyframe is still a lossless copy. The preview shows the
  picture deinterlaced too
- QTGMC runs in VapourSynth. On Windows it downloads with the codecs; on Linux it is
  offered when VapourSynth with QTGMC is installed on the system
- The timeline's filmstrip no longer pins every core while it loads. It used to decode
  every keyframe in the file, which for some formats (ProRes, DV, MJPEG) is every frame;
  it now takes at most 160 frames on a single core, spread across the file, and fills in
  coarse to fine. Finding the keyframes reads packet headers instead of decoding
- **Windows:** the zip now carries the app alone. On first run, with no codecs installed,
  it asks whether to download them: ffmpeg's most complete build (from BtbN's GitHub
  releases) and the QTGMC set, into a `Codex` folder beside the app. Every file is
  checked against its published or pinned checksum before it is installed, and a
  cancelled download leaves nothing behind
- Releases now publish from this changelog, and the Linux Flatpak's build cache is kept
  on main so a release does not rebuild ffmpeg

## 1.2.1 — 2026-10-26

- No change to the app. The first release built entirely by CI: Windows ships as one zip,
  with ffmpeg and ffprobe beside the program

## 1.2.0 — 2026-10-26

Four things reported from real use: two repairs to the transport, one to the timeline,
and a click that used to do nothing.

- The Pause button reads "Pause" again — its symbol was a character none of the bundled
  typefaces contained, so it drew as two empty boxes
- Hold either one-frame button, beside the playhead or beside the In and Out timecodes,
  and it keeps stepping
- The timeline's hover preview steps aside near the in and out handles, instead of
  covering the handle you are reaching for
- Clicking the empty picture opens a video, the same as the Open button

## 1.1.0 — 2026-08-24

Dropping a video onto the window now works on Wayland. It never had: the window toolkit
implements drag-and-drop on X11, Windows and macOS and not on Wayland at all, so the app
was reading a list that was always empty. It now speaks that part of the Wayland protocol
itself, over the connection eframe already holds.

- Drag-and-drop on Wayland, alongside the X11, Windows and macOS support that already
  worked
- The picture says "Drop to open" while a file is held over the window
- A dropped file that is not a video is named and refused, rather than silently ignored
- A drop landing while a dialog is up is discarded rather than queued
- The Cyberpunk theme's scanlines stop at the picture: over the interface they are
  texture, over your footage they were dirt

## 1.0.0 — 2026-08-23

- First release. Apricot Studio takes a video, lets you mark a start and an end, and
  writes out that section, with no export settings to choose
