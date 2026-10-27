// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! Interlaced sources, and the two ways of making them progressive.
//!
//! **BWDIF** is an ffmpeg filter, so it is always there and costs one more
//! entry in the filter chain. **QTGMC** is a VapourSynth script: far slower,
//! and visibly better on motion -- no shimmer on fine detail, no jaggies on
//! diagonals. It runs outside ffmpeg, as `vspipe` feeding raw frames into the
//! encoder, and it is only offered where VapourSynth with QTGMC can be found:
//! the Codex folder on Windows, the system's `vspipe` elsewhere.
//!
//! Everything here is text in, text out, so the whole of it is tested without
//! a decoder.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

/// Which field of an interlaced frame was captured first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parity {
    Tff,
    Bff,
}

impl Parity {
    fn ffmpeg(self) -> &'static str {
        match self {
            Parity::Tff => "tff",
            Parity::Bff => "bff",
        }
    }
}

/// What a source is, as far as fields go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scan {
    Progressive,
    Interlaced(Parity),
}

impl Scan {
    pub fn parity(self) -> Option<Parity> {
        match self {
            Scan::Progressive => None,
            Scan::Interlaced(parity) => Some(parity),
        }
    }
}

/// What the container says, when it says anything.
///
/// ffprobe's `field_order` is two letters, and by the letter of its
/// documentation the second is the field shown first. In practice the first
/// one is: ffmpeg itself writes `tb` for top-field-first material from every
/// encoder but MJPEG, so that is what most files in the wild carry, and a
/// `tb` capture read as bottom-first comes out juddering back and forth. So
/// the first letter decides. `None` means the stream does not say, which is
/// common -- a capture card's lossless AVI often does not -- and is the cue to
/// look at the pictures themselves with [`idet_args`].
pub fn scan_from_field_order(field_order: &str) -> Option<Scan> {
    match field_order.trim() {
        "progressive" => Some(Scan::Progressive),
        "tt" | "tb" => Some(Scan::Interlaced(Parity::Tff)),
        "bb" | "bt" => Some(Scan::Interlaced(Parity::Bff)),
        _ => None,
    }
}

/// How many frames the picture test looks at, from a tenth of the way in so a
/// black leader or a title card does not decide it.
pub const IDET_FRAMES: u32 = 300;

/// Arguments for ffmpeg's `idet` over a stretch of the file.
pub fn idet_args(path: &str, duration: f64) -> Vec<String> {
    let from = if duration.is_finite() && duration > 0.0 {
        duration * 0.1
    } else {
        0.0
    };
    [
        "-hide_banner",
        "-nostdin",
        "-threads",
        "1",
        "-ss",
        &format!("{from:.3}"),
        "-i",
        path,
        "-an",
        "-sn",
        "-frames:v",
        &IDET_FRAMES.to_string(),
        "-vf",
        "idet",
        "-f",
        "null",
        "-",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Read idet's verdict off ffmpeg's stderr.
///
/// The "Multi frame detection" line is the one to trust: it weighs each frame
/// against its neighbours, where the single-frame line is fooled by any still
/// shot. ffmpeg can print it more than once, and the last one is the total. Interlaced wins only on a clear majority of the frames it could decide
/// -- a progressive film has a scattering of false positives on fast motion,
/// and treating it as interlaced would halve its vertical detail for nothing.
pub fn scan_from_idet(stderr: &str) -> Option<Scan> {
    let line = stderr
        .lines()
        .rfind(|line| line.contains("Multi frame detection:"))?;
    let count = |key: &str| -> u64 {
        line.split_once(key)
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };
    let (tff, bff, progressive) = (count("TFF:"), count("BFF:"), count("Progressive:"));
    if tff + bff + progressive == 0 {
        return None;
    }
    if tff + bff > progressive * 2 {
        let parity = if bff > tff { Parity::Bff } else { Parity::Tff };
        Some(Scan::Interlaced(parity))
    } else {
        Some(Scan::Progressive)
    }
}

// --------------------------------------------------------------------------
// The choice
// --------------------------------------------------------------------------

/// BWDIF has no presets of its own; these are the two settings worth having.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BwdifProfile {
    /// Every field becomes a frame: 50i in, 50p out. The motion of the
    /// original, which is the point of interlaced video.
    Slowest,
    /// One frame per frame: 50i in, 25p out. Half the encode, and the motion
    /// of film.
    Fast,
}

pub const BWDIF_PROFILES: [BwdifProfile; 2] = [BwdifProfile::Slowest, BwdifProfile::Fast];

impl BwdifProfile {
    pub fn label(self) -> &'static str {
        match self {
            BwdifProfile::Slowest => "Slowest",
            BwdifProfile::Fast => "Fast",
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            BwdifProfile::Slowest => {
                "One frame per field: the full motion, at double the frame rate"
            }
            BwdifProfile::Fast => "One frame per frame: the original frame rate",
        }
    }
}

/// QTGMC's own presets, slowest first. Its two fastest ("Super Fast" and
/// above) give up the motion search that is the reason to use it at all, so
/// they are not offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QtgmcPreset {
    Placebo,
    VerySlow,
    Slower,
    Slow,
    Medium,
    Fast,
    Faster,
}

pub const QTGMC_PRESETS: [QtgmcPreset; 7] = [
    QtgmcPreset::Placebo,
    QtgmcPreset::VerySlow,
    QtgmcPreset::Slower,
    QtgmcPreset::Slow,
    QtgmcPreset::Medium,
    QtgmcPreset::Fast,
    QtgmcPreset::Faster,
];

impl QtgmcPreset {
    /// The name QTGMC itself takes.
    pub fn name(self) -> &'static str {
        match self {
            QtgmcPreset::Placebo => "Placebo",
            QtgmcPreset::VerySlow => "Very Slow",
            QtgmcPreset::Slower => "Slower",
            QtgmcPreset::Slow => "Slow",
            QtgmcPreset::Medium => "Medium",
            QtgmcPreset::Fast => "Fast",
            QtgmcPreset::Faster => "Faster",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            QtgmcPreset::Placebo => "Slowest (Placebo)",
            other => other.name(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Bwdif(BwdifProfile),
    Qtgmc(QtgmcPreset),
}

impl Method {
    /// Each one at its slowest, which is where both start.
    pub const BWDIF: Method = Method::Bwdif(BwdifProfile::Slowest);
    pub const QTGMC: Method = Method::Qtgmc(QtgmcPreset::Placebo);

    pub fn name(self) -> &'static str {
        match self {
            Method::Bwdif(_) => "BWDIF",
            Method::Qtgmc(_) => "QTGMC",
        }
    }

    pub fn profile_label(self) -> &'static str {
        match self {
            Method::Bwdif(profile) => profile.label(),
            Method::Qtgmc(preset) => preset.label(),
        }
    }

    /// How many output frames each source frame becomes.
    pub fn rate_multiplier(self) -> f64 {
        match self {
            Method::Bwdif(BwdifProfile::Slowest) | Method::Qtgmc(_) => 2.0,
            Method::Bwdif(BwdifProfile::Fast) => 1.0,
        }
    }
}

/// A deinterlacer, and the field order it is told to expect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deinterlace {
    pub method: Method,
    pub parity: Parity,
}

/// The ffmpeg filter for BWDIF.
///
/// `deint=all` rather than `interlaced`: the latter only touches frames
/// flagged as interlaced, and a source whose interlacing was found by looking
/// at it is exactly one whose frames are not flagged.
pub fn bwdif_filter(profile: BwdifProfile, parity: Parity) -> String {
    let mode = match profile {
        BwdifProfile::Slowest => "send_field",
        BwdifProfile::Fast => "send_frame",
    };
    format!("bwdif=mode={mode}:parity={}:deint=all", parity.ffmpeg())
}

/// The preview's stand-in: whatever was chosen, the preview runs BWDIF at the
/// source's own rate, because QTGMC is nowhere near real time and the preview
/// already paces itself to one frame per source frame.
pub fn preview_filter(parity: Parity) -> String {
    bwdif_filter(BwdifProfile::Fast, parity)
}

/// Frames either side of the cut that QTGMC is also given, so its temporal
/// search has neighbours at the edges rather than starting cold on the first
/// frame of the clip. Its widest radius at the slowest preset is 3.
const QTGMC_MARGIN: i64 = 8;

/// A VapourSynth script that deinterlaces `[first, last]` (source frames,
/// inclusive) with QTGMC and outputs exactly the frames that span them, at
/// double rate.
///
/// The source filter is whichever is installed: LSMASH (what the Codex set
/// carries), ffms2 or BestSource (what a Linux system is likelier to have).
/// The index the first two build goes to `cache`, not beside the source: the
/// source may be on read-only media, and a stray index file next to someone's
/// video is not the app's to leave.
pub fn qtgmc_script(
    source: &str,
    cache: &str,
    first: i64,
    last: i64,
    parity: Parity,
    preset: QtgmcPreset,
) -> String {
    // A JSON string is a valid Python string literal, escapes and all, which
    // is the whole of what quoting a Windows path for Python needs.
    let quote = |text: &str| serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    let tff = if parity == Parity::Tff {
        "True"
    } else {
        "False"
    };
    let first = first.max(0);
    let last = last.max(first);
    format!(
        "import vapoursynth as vs\n\
         from vapoursynth import core\n\
         import havsfunc\n\
         \n\
         if hasattr(core, 'lsmas'):\n    \
             clip = core.lsmas.LWLibavSource({source}, cachefile={cache})\n\
         elif hasattr(core, 'ffms2'):\n    \
             clip = core.ffms2.Source({source}, cachefile={cache})\n\
         else:\n    \
             clip = core.bs.VideoSource({source}, cachemode=0)\n\
         first, last = {first}, min({last}, clip.num_frames - 1)\n\
         lo = max(0, first - {QTGMC_MARGIN})\n\
         hi = min(clip.num_frames - 1, last + {QTGMC_MARGIN})\n\
         clip = core.std.Trim(clip, lo, hi)\n\
         clip = havsfunc.QTGMC(clip, Preset={preset}, TFF={tff})\n\
         clip = core.std.Trim(clip, (first - lo) * 2, (last - lo) * 2 + 1)\n\
         clip.set_output()\n",
        source = quote(source),
        cache = quote(cache),
        preset = quote(preset.name()),
    )
}

/// The arguments for `vspipe` to run a script and write YUV4MPEG to stdout.
pub fn vspipe_args(script: &str) -> Vec<String> {
    ["-c", "y4m", script, "-"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

// --------------------------------------------------------------------------
// Whether QTGMC can run here
// --------------------------------------------------------------------------

/// The script that proves the chain is whole: VapourSynth starts, a source
/// filter and the three plugins QTGMC calls are loaded, and havsfunc imports.
pub const QTGMC_CHECK: &str = "import vapoursynth as vs\n\
from vapoursynth import core\n\
import havsfunc\n\
assert any(hasattr(core, ns) for ns in ('lsmas', 'ffms2', 'bs'))\n\
for ns in ('mv', 'rgvs', 'znedi3'):\n    getattr(core, ns)\n\
assert hasattr(havsfunc, 'QTGMC')\n\
core.std.BlankClip(length=1).set_output()\n";

static AVAILABLE: OnceLock<std::sync::Mutex<Option<bool>>> = OnceLock::new();

/// Whether QTGMC can run, checked once by running [`QTGMC_CHECK`] through
/// `vspipe` and remembered until [`forget_availability`].
///
/// The check starts a Python interpreter and loads every plugin, which is
/// most of a second, so it runs on first need -- when an interlaced file is
/// opened -- and never at startup. In the Flatpak it is false: the sandbox
/// does not reach the host's VapourSynth, and the bundle does not carry one.
pub fn qtgmc_available() -> bool {
    let cell = AVAILABLE.get_or_init(Default::default);
    if let Some(known) = *cell.lock().unwrap() {
        return known;
    }
    let found = run_check();
    *cell.lock().unwrap() = Some(found);
    found
}

/// After a download, so the next question looks again.
pub fn forget_availability() {
    if let Some(cell) = AVAILABLE.get() {
        *cell.lock().unwrap() = None;
    }
}

fn run_check() -> bool {
    if crate::sandbox::is_sandboxed() {
        return false;
    }
    let Some(vspipe) = crate::ffmpeg::vspipe_path() else {
        return false;
    };
    let script = scratch_file("qtgmc-check.vpy");
    if std::fs::write(&script, QTGMC_CHECK).is_err() {
        return false;
    }
    let child = crate::ffmpeg::command(&vspipe)
        .args(["-i", &script.to_string_lossy(), "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let ok = match child {
        Ok(mut child) => wait_ok(&mut child, Duration::from_secs(30)),
        Err(_) => false,
    };
    let _ = std::fs::remove_file(&script);
    ok
}

fn wait_ok(child: &mut std::process::Child, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// A file in the system's temporary folder, named for this process so two
/// running copies cannot trip over each other's.
pub fn scratch_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("apricot-{}-{name}", std::process::id()))
}
