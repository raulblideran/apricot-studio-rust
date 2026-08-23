// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! Turning an in/out selection into an ffmpeg command, and running it.
//!
//! Every encoding parameter here is derived from the source file. There is
//! deliberately nothing for the user to configure.

use std::io::Read;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread;

use crate::ffmpeg::{command, ffmpeg_path};
use crate::media::MediaInfo;
use crate::paths;

pub const VAAPI_DEVICE: &str = "/dev/dri/renderD128";

/// Capped CRF: quality drives the encode, but a one-second VBV buffer pinned to
/// the source's own bitrate stops it running away. Easy footage comes in under
/// the source's size, hard footage lands right on it -- so the output inherits
/// the source's bitrate without ever being told a target.
///
/// Measured on a 1440p60 replay (source 9.51 Mb/s): this lands at 9.58 Mb/s,
/// 1.00x the source, at 37.5 dB PSNR against it. Dropping to CRF 24 saves
/// nothing on size (both hit the cap) and costs 1.8 dB, which is why the floor
/// is 20.
pub fn crf_for(codec: &str) -> i64 {
    match codec {
        "h264" => 20,
        "hevc" => 22,
        "vp9" | "av1" => 30,
        _ => 20,
    }
}

/// VA-API takes a quantiser rather than a CRF, on its own scale.
pub const AV1_QP: i64 = 28;

/// Converting to WebM is a deliberate change of format rather than a match to
/// the source, so it starts from its own baseline instead of the source
/// codec's.
pub const WEBM_CRF: i64 = 32;

/// VP9 at the settings used here lands well under an equivalent H.264 bitrate;
/// only used to estimate a size before encoding.
pub const WEBM_RATE_FACTOR: f64 = 0.6;

/// Matching the source's GOP structure means giving up B-frames on
/// screen-recorder footage, which costs efficiency; a slower preset buys it
/// back. Measured on a 14s 1440p60 clip: veryfast 5.0s/36.41 dB, fast
/// 9.3s/37.35 dB, medium 11.2s/37.45 dB -- medium adds 2s for 0.1 dB, so
/// `fast` is where it stops paying. x265 is far slower at the same preset name,
/// so it keeps veryfast.
pub fn preset_for(codec: &str) -> &'static str {
    match codec {
        "hevc" => "veryfast",
        _ => "fast",
    }
}

/// Fast-seek to this many seconds before the cut, then seek the rest of the way
/// accurately. Stream-copied audio can only start on a whole packet, so seeking
/// the whole distance on the input would start the audio at the preceding
/// keyframe while video started on the exact frame -- a half-second of desync.
pub const SEEK_MARGIN: f64 = 10.0;

/// A cut can be stream-copied only if it starts exactly on a keyframe. The
/// interface snaps to them, so this compares against the same value it snapped
/// to.
pub const KEYFRAME_EPS: f64 = 0.002;

pub const GIF_WIDTH: i64 = 480;
pub const GIF_FPS: i64 = 15;

/// A size target is usually an upload limit, so overshooting is a failure, not
/// a rounding error. Single-pass ABR overshoots by a few percent on its own, so
/// aim under and cap hard (measured: 0.97 with a loose cap landed 10.3 MB on a
/// 10 MB target -- rejected by Discord).
pub const SIZE_MARGIN: f64 = 0.90;

/// Sanity bounds for arithmetic that divides by a caller-supplied duration.
pub const MIN_DURATION: f64 = 0.001;
/// Far above any real source; catches divide-by-tiny.
pub const MAX_BITRATE: i64 = 800_000_000;

// --------------------------------------------------------------------------
// The few things that are allowed to deviate from the source
// --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Source,
    WebM,
    Gif,
}

/// Quality presets. Each pairs a CRF offset with a ceiling expressed as a share
/// of the source's own bitrate, so "smaller" means smaller *than this file*
/// rather than some absolute number that would mean different things for a 720p
/// capture and a 4K one. The cap does most of the work; the CRF offset stops
/// easy footage from spending the whole budget on nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Source,
    Smaller,
    Smallest,
}

impl Quality {
    pub fn crf_offset(self) -> i64 {
        match self {
            Quality::Source => 0,
            Quality::Smaller => 4,
            Quality::Smallest => 8,
        }
    }

    pub fn rate_share(self) -> f64 {
        match self {
            Quality::Source => 1.00,
            Quality::Smaller => 0.50,
            Quality::Smallest => 0.30,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audio {
    All,
    None,
    Track(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub fmt: Format,
    pub audio: Audio,
    /// 0 = quality-driven rather than size-driven.
    pub target_bytes: i64,
    pub quality: Quality,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            fmt: Format::Source,
            audio: Audio::All,
            target_bytes: 0,
            quality: Quality::Source,
        }
    }
}

impl Options {
    pub fn ext_for(&self, info: &MediaInfo) -> String {
        match self.fmt {
            Format::WebM => "webm".to_string(),
            Format::Gif => "gif".to_string(),
            Format::Source => info.ext.clone(),
        }
    }

    /// Whether this reproduces the source rather than deviating from it.
    pub fn inherits_everything(&self) -> bool {
        self.fmt == Format::Source && self.target_bytes == 0 && self.quality == Quality::Source
    }
}

/// A fully resolved ffmpeg invocation, plus what to tell the user about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub args: Vec<String>,
    pub output: String,
    pub duration: f64,
    pub label: String,
    pub lossless: bool,
}

// --------------------------------------------------------------------------
// Naming and keyframes
// --------------------------------------------------------------------------

/// `clip.mp4` -> `clip_clip.mp4`, avoiding names that already exist.
pub fn default_output(path: &str, ext: Option<&str>) -> String {
    let (stem, source_ext) = paths::split_ext(path);
    // A source with no extension would otherwise produce a destination with
    // none either, leaving ffmpeg no way to pick a muxer.
    let ext = match ext {
        Some(ext) => format!(".{}", ext.trim_start_matches('.')),
        None if !source_ext.is_empty() => source_ext,
        None => ".mp4".to_string(),
    };
    let mut candidate = format!("{stem}_clip{ext}");
    let mut n = 2;
    while paths::exists(&candidate) {
        candidate = format!("{stem}_clip{n}{ext}");
        n += 1;
    }
    candidate
}

/// The exact keyframe this cut starts on, or None if it starts between two.
///
/// The exact stored timestamp matters: ffmpeg seeks to the keyframe at or
/// *before* the time it is given, so asking for 44.0 when the keyframe really
/// sits at 44.000004 rewinds a whole GOP and the clip comes out too long.
pub fn on_keyframe(start: f64, keyframes: &[f64]) -> Option<f64> {
    keyframes
        .iter()
        .copied()
        .find(|k| (k - start).abs() <= KEYFRAME_EPS)
}

/// The nearest keyframe within `tolerance`, else `t` unchanged.
pub fn snap_to_keyframe(t: f64, keyframes: &[f64], tolerance: f64) -> f64 {
    let Some(nearest) = nearest_keyframe(t, keyframes) else {
        return t;
    };
    if (nearest - t).abs() <= tolerance {
        nearest
    } else {
        t
    }
}

pub fn nearest_keyframe(t: f64, keyframes: &[f64]) -> Option<f64> {
    keyframes.iter().copied().min_by(|a, b| {
        (a - t)
            .abs()
            .partial_cmp(&(b - t).abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

// --------------------------------------------------------------------------
// Size arithmetic
// --------------------------------------------------------------------------

fn audio_maps(info: &MediaInfo, options: &Options) -> Vec<String> {
    if !info.has_audio() || options.audio == Audio::None || options.fmt == Format::Gif {
        return Vec::new();
    }
    match options.audio {
        // Capture setups often record mic and desktop to separate tracks, and
        // dropping one is silent.
        Audio::All => vec!["-map".into(), "0:a".into()],
        Audio::Track(index) => vec!["-map".into(), format!("0:a:{index}")],
        Audio::None => Vec::new(),
    }
}

/// Bitrate of whichever tracks survive, for size-target arithmetic.
fn selected_audio_bitrate(info: &MediaInfo, options: &Options) -> i64 {
    if !info.has_audio() || options.audio == Audio::None || options.fmt == Format::Gif {
        return 0;
    }
    let tracks: Vec<&crate::media::AudioTrack> = match options.audio {
        Audio::All => info.audio.iter().collect(),
        Audio::Track(index) => info.audio.iter().filter(|t| t.index == index).collect(),
        Audio::None => Vec::new(),
    };
    // Matroska often omits per-stream bitrate; assume a typical stereo stream.
    tracks
        .iter()
        .map(|t| if t.bitrate > 0 { t.bitrate } else { 128_000 })
        .sum()
}

/// Solve backwards from a wanted file size to a video bitrate.
pub fn target_video_bitrate(info: &MediaInfo, options: &Options, duration: f64) -> i64 {
    let budget_bits = options.target_bytes as f64 * 8.0 * SIZE_MARGIN;
    let audio_bits = selected_audio_bitrate(info, options) as f64 * duration;
    let video_bps = (budget_bits - audio_bits) / duration.max(MIN_DURATION);
    // Floor: below this the clip is unwatchable anyway. Ceiling: a near-zero
    // duration would otherwise solve to an absurd rate that no encoder accepts.
    (video_bps as i64).clamp(100_000, MAX_BITRATE)
}

/// Roughly how large the export will be, for showing before it runs.
///
/// An estimate, not a promise: quality-driven encodes come in under their
/// ceiling on easy footage. A size target is the one case that is close to
/// exact, because the bitrate was solved from it.
pub fn estimate_bytes(info: &MediaInfo, options: &Options, duration: f64) -> i64 {
    if duration <= 0.0 {
        return 0;
    }
    if options.fmt == Format::Gif {
        return 0; // palette output is not predictable enough
    }
    if options.target_bytes > 0 {
        return (options.target_bytes as f64 * SIZE_MARGIN) as i64;
    }
    let audio_bps = selected_audio_bitrate(info, options) as f64;
    let mut video_bps = info.v_bitrate as f64 * options.quality.rate_share();
    if options.fmt == Format::WebM {
        // VP9 at these settings lands well under an equivalent H.264 bitrate,
        // and the preset still applies on top -- it moves the CRF, so a smaller
        // preset really does produce a smaller WebM.
        video_bps *= WEBM_RATE_FACTOR;
    }
    ((video_bps + audio_bps) * duration / 8.0) as i64
}

/// The source's keyframe interval in frames, so the clip behaves like it.
fn gop_size(info: &MediaInfo, keyframes: &[f64]) -> i64 {
    if keyframes.len() >= 3 {
        let mut gaps: Vec<f64> = keyframes.windows(2).map(|pair| pair[1] - pair[0]).collect();
        gaps.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = gaps[gaps.len() / 2];
        if median > 0.1 && median < 30.0 {
            return ((median * info.fps).round() as i64).max(1);
        }
    }
    ((2.0 * info.fps).round() as i64).max(1)
}

// --------------------------------------------------------------------------
// The encoder arguments
// --------------------------------------------------------------------------

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// Average bitrate with a hard ceiling, for a size-targeted encode.
///
/// The cap is what decides whether the file fits: single-pass ABR overshoots on
/// hard content and sits at -maxrate, and a buffer no larger than one second of
/// the rate stops it banking that overshoot across the clip.
fn abr_args(rate: i64) -> Vec<String> {
    vec![
        "-b:v".into(),
        rate.to_string(),
        "-maxrate".into(),
        ((rate as f64 * 1.05) as i64).to_string(),
        "-bufsize".into(),
        rate.to_string(),
    ]
}

/// VP9 in software -- no GPU here encodes it, so it is genuinely slow.
///
/// Deliberately no VBV ceiling in the quality case: libvpx reads `-maxrate`
/// alongside `-b:v 0` as nothing at all, so a cap there would be decoration.
fn vp9_args(rate: Vec<String>, cpu_used: &str) -> Vec<String> {
    let mut out = args(&["-c:v", "libvpx-vp9"]);
    out.extend(rate);
    out.extend(args(&[
        "-row-mt",
        "1",
        "-deadline",
        "good",
        "-cpu-used",
        cpu_used,
    ]));
    out
}

/// AV1 stays AV1, on the GPU when there is one.
///
/// AV1 in software is far too slow for the 4K sources that tend to arrive in
/// this codec; a VA-API card encodes it natively. There is no such device on
/// Windows or macOS, so those fall through to SVT-AV1 on the processor -- which
/// is the same answer this gives a Linux box with no render node.
fn av1_args(info: &MediaInfo, abr: &[String], offset: i64) -> (Vec<String>, Vec<String>, String) {
    if cfg!(target_os = "linux") && std::path::Path::new(VAAPI_DEVICE).exists() {
        let surface = if info.pix_fmt.contains("10") {
            "p010"
        } else {
            "nv12"
        };
        let mut video = vec![
            "-vf".into(),
            format!("format={surface},hwupload"),
            "-c:v".into(),
            "av1_vaapi".into(),
        ];
        if abr.is_empty() {
            video.extend(vec!["-qp".into(), (AV1_QP + offset).to_string()]);
        } else {
            video.extend(abr.to_vec());
        }
        return (
            args(&["-vaapi_device", VAAPI_DEVICE]),
            video,
            "av1_vaapi".into(),
        );
    }
    let mut video = args(&["-c:v", "libsvtav1", "-preset", "8"]);
    if abr.is_empty() {
        video.extend(vec!["-crf".into(), (crf_for("av1") + offset).to_string()]);
    } else {
        video.extend(abr.to_vec());
    }
    (Vec::new(), video, "libsvtav1".into())
}

/// Returns (args before -i, args after -i, human label).
fn video_args(
    info: &MediaInfo,
    options: &Options,
    duration: f64,
) -> (Vec<String>, Vec<String>, String) {
    if options.fmt == Format::Gif {
        // One pass: build a palette from the clip and apply it, which is the
        // difference between a GIF that looks like the video and one that
        // doesn't.
        let chain = format!(
            "fps={GIF_FPS},scale={GIF_WIDTH}:-1:flags=lanczos,\
split[a][b];[a]palettegen=stats_mode=diff[p];\
[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle"
        );
        return (
            Vec::new(),
            vec!["-filter_complex".into(), chain, "-loop".into(), "0".into()],
            "gif".into(),
        );
    }

    // Two ways to drive the encoder, and every codec below picks one of them. A
    // size target solves an average bitrate from the budget; otherwise quality
    // leads, capped by a ceiling expressed as a share of the source's own
    // bitrate so that a preset means the same thing whatever the file is.
    let abr = if options.target_bytes > 0 {
        abr_args(target_video_bitrate(info, options, duration))
    } else {
        Vec::new()
    };
    let offset = options.quality.crf_offset();
    let ceiling = (info.v_bitrate as f64 * options.quality.rate_share()) as i64;
    let vbv = if ceiling > 0 {
        vec![
            "-maxrate".into(),
            ceiling.to_string(),
            "-bufsize".into(),
            ceiling.to_string(),
        ]
    } else {
        Vec::new()
    };

    if options.fmt == Format::WebM {
        // Under a target, -crf must be absent rather than sit alongside the
        // bitrate: libvpx reads a CRF next to `-b:v 0` as "constant quality,
        // ignore the bitrate", which is how a size target asked for here used
        // to be discarded without a word.
        let rate = if abr.is_empty() {
            vec![
                "-crf".into(),
                (WEBM_CRF + offset).to_string(),
                "-b:v".into(),
                "0".into(),
            ]
        } else {
            abr
        };
        return (Vec::new(), vp9_args(rate, "4"), "libvpx-vp9".into());
    }

    let quality_or = |quality: Vec<String>| -> Vec<String> {
        if abr.is_empty() {
            quality
        } else {
            abr.clone()
        }
    };

    match info.v_codec.as_str() {
        "h264" => {
            let mut video = args(&["-c:v", "libx264", "-preset", preset_for("h264"), "-bf"]);
            video.push(info.has_b_frames.to_string());
            let mut quality = vec!["-crf".into(), (crf_for("h264") + offset).to_string()];
            quality.extend(vbv);
            video.extend(quality_or(quality));
            if !info.encoder_profile().is_empty() {
                video.extend(vec!["-profile:v".into(), info.encoder_profile().into()]);
            }
            (Vec::new(), video, "libx264".into())
        }
        "hevc" => {
            // x265 is loud on stderr by default, and its CRF scale sits a
            // couple of points above x264's for equivalent quality. It takes
            // its B-frame count through x265-params rather than -bf.
            let mut video = args(&[
                "-c:v",
                "libx265",
                "-preset",
                preset_for("hevc"),
                "-x265-params",
            ]);
            video.push(format!("log-level=error:bframes={}", info.has_b_frames));
            let mut quality = vec!["-crf".into(), (crf_for("hevc") + offset).to_string()];
            quality.extend(vbv);
            video.extend(quality_or(quality));
            if !info.encoder_profile().is_empty() {
                video.extend(vec!["-profile:v".into(), info.encoder_profile().into()]);
            }
            (Vec::new(), video, "libx265".into())
        }
        // These two used to fall through to libx264 whenever a size target was
        // set, which put H.264 inside whatever container the source arrived in
        // -- for a VP9 .webm that is not a silent codec swap but a muxer
        // refusing the file, so the export failed outright.
        "vp9" => {
            let rate = if abr.is_empty() {
                vec![
                    "-crf".into(),
                    (crf_for("vp9") + offset).to_string(),
                    "-b:v".into(),
                    "0".into(),
                ]
            } else {
                abr
            };
            (Vec::new(), vp9_args(rate, "2"), "libvpx-vp9".into())
        }
        "av1" => av1_args(info, &abr, offset),
        // Unknown codec: fall back to H.264, which every player handles.
        _ => {
            let mut video = args(&["-c:v", "libx264", "-preset", preset_for("h264"), "-bf"]);
            video.push(info.has_b_frames.to_string());
            let mut quality = vec!["-crf".into(), (crf_for("h264") + offset).to_string()];
            quality.extend(vbv);
            video.extend(quality_or(quality));
            (Vec::new(), video, "libx264".into())
        }
    }
}

/// Which encoder this file will be re-encoded with.
pub fn encoder_label(info: &MediaInfo, options: &Options) -> String {
    video_args(info, options, 1.0).2
}

fn faststart_container(ext: &str) -> bool {
    matches!(ext, "mp4" | "mov" | "m4v")
}

/// A pure stream copy. Only valid when the cut starts on a keyframe.
fn build_lossless(
    info: &MediaInfo,
    start: f64,
    duration: f64,
    output: &str,
    options: &Options,
) -> Plan {
    // The same floor the re-encode path applies. Without it a reversed or
    // zero-length range reaches ffmpeg as a negative -t, and Plan.duration goes
    // negative with it -- which is then the divisor for the progress fraction.
    let duration = duration.max(info.frame_duration());
    let mut out = args(&["-hide_banner", "-nostdin", "-y"]);
    // Safe to seek the whole way on the input here: the start is a keyframe, so
    // there is no partial GOP for audio to disagree about.
    out.extend(vec![
        "-ss".into(),
        format!("{start:.6}"),
        "-i".into(),
        info.path.clone(),
        "-t".into(),
        format!("{duration:.6}"),
        "-map".into(),
        "0:v:0".into(),
    ]);
    out.extend(audio_maps(info, options));
    out.extend(args(&["-c", "copy", "-avoid_negative_ts", "make_zero"]));
    if faststart_container(&info.ext) {
        out.extend(args(&["-movflags", "+faststart"]));
    }
    out.extend(args(&[
        "-stats_period",
        "0.25",
        "-progress",
        "pipe:1",
        "-nostats",
        output,
    ]));
    Plan {
        args: out,
        output: output.to_string(),
        duration,
        label: "stream copy".into(),
        lossless: true,
    }
}

/// Assemble the cut command for [start, end).
pub fn build(
    info: &MediaInfo,
    start: f64,
    end: f64,
    output: &str,
    keyframes: &[f64],
    options: &Options,
) -> Plan {
    // A caller can hand over anything; keep the arithmetic inside the file.
    let start = start.clamp(0.0, info.duration);
    let duration = (end - start).max(info.frame_duration());

    // Starting exactly on a keyframe means nothing has to be decoded at all.
    // A copy reproduces the source exactly, so it is only valid when nothing
    // was asked to differ from it -- a smaller-quality preset rules it out too.
    if options.inherits_everything() {
        if let Some(exact) = on_keyframe(start, keyframes) {
            return build_lossless(info, exact, end - exact, output, options);
        }
    }

    let gop = gop_size(info, keyframes);
    let (pre, video, label) = video_args(info, options, duration);

    let mut out = args(&["-hide_banner", "-nostdin", "-y"]);
    out.extend(pre);

    if options.fmt == Format::Gif {
        // GIF seeks entirely on the input, and must. palettegen holds its
        // palette back until end of stream, and paletteuse buffers every frame
        // waiting for it, so whatever the graph is fed is decoded and kept in
        // memory. An output-side seek trims *after* the filters, which would
        // feed it everything from the seek to the end of the file: measured on
        // a 60s source, cutting 2s took 1.56s and 300 MB against 0.37s and 121
        // MB, and the palette came out built from footage the clip does not
        // contain. Seeking the input the whole way is safe here for the reason
        // it is not elsewhere -- a GIF carries no audio, so no stream copy has
        // to stay aligned with the video.
        out.extend(vec![
            "-ss".into(),
            format!("{start:.6}"),
            "-t".into(),
            format!("{duration:.6}"),
            "-i".into(),
            info.path.clone(),
        ]);
    } else {
        // Split the seek: cheap keyframe seek on the input, exact seek on the
        // output. The output-side seek trims every stream to the same instant,
        // which is what keeps copied audio lined up with the re-encoded video.
        let coarse = (start - SEEK_MARGIN).max(0.0);
        if coarse > 0.0 {
            out.extend(vec!["-ss".into(), format!("{coarse:.6}")]);
        }
        out.extend(vec![
            "-i".into(),
            info.path.clone(),
            "-ss".into(),
            format!("{:.6}", start - coarse),
            "-t".into(),
            format!("{duration:.6}"),
        ]);
        // filter_complex does its own stream selection, so -map would clash.
        out.extend(args(&["-map", "0:v:0"]));
        out.extend(audio_maps(info, options));
    }

    let uses_hwupload = video.iter().any(|arg| arg.contains("hwupload"));
    out.extend(video);

    if options.fmt == Format::Gif {
        // paletteuse emits pal8 and the fps filter sets the rate, so pinning
        // pixel format, GOP, colour or frame timing here would fight the chain.
        out.push("-an".into());
    } else {
        // Hardware frames live on the GPU in their own format, so -pix_fmt
        // would fight the hwupload filter.
        if !uses_hwupload && !info.pix_fmt.is_empty() {
            out.push("-pix_fmt".into());
            out.push(if options.fmt == Format::WebM {
                "yuv420p".to_string()
            } else {
                info.pix_fmt.clone()
            });
        }

        if gop > 0 && options.fmt != Format::WebM {
            out.extend(vec!["-g".into(), gop.to_string()]);
        }

        for (flag, value) in [
            ("-color_primaries", &info.color_primaries),
            ("-color_trc", &info.color_transfer),
            ("-colorspace", &info.color_space),
            ("-color_range", &info.color_range),
        ] {
            if !value.is_empty() && value != "unknown" {
                out.extend(vec![flag.to_string(), value.clone()]);
            }
        }

        // Sources from screen recorders are variable-frame-rate; passthrough
        // keeps their original timestamps instead of resampling to a constant
        // rate.
        out.extend(args(&[
            "-fps_mode",
            "passthrough",
            "-avoid_negative_ts",
            "make_zero",
        ]));

        if !audio_maps(info, options).is_empty() {
            if options.fmt == Format::WebM && info.a_codec() != "opus" {
                // WebM carries only Opus or Vorbis.
                let rate = if info.a_bitrate() > 0 {
                    info.a_bitrate()
                } else {
                    128_000
                };
                out.extend(vec![
                    "-c:a".into(),
                    "libopus".into(),
                    "-b:a".into(),
                    rate.to_string(),
                ]);
            } else {
                // Audio never constrains where the video can be cut, so copying
                // it keeps it bit-identical and spends the generation of loss
                // on video.
                out.extend(args(&["-c:a", "copy"]));
            }
        }

        if options.fmt == Format::Source && faststart_container(&info.ext) {
            out.extend(args(&["-movflags", "+faststart"]));
        }
    }

    // Default stats come once a second, which makes the bar lurch.
    out.extend(args(&[
        "-stats_period",
        "0.25",
        "-progress",
        "pipe:1",
        "-nostats",
        output,
    ]));

    Plan {
        args: out,
        output: output.to_string(),
        duration,
        label,
        lossless: false,
    }
}

// --------------------------------------------------------------------------
// Running it
// --------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum ExportEvent {
    /// fraction 0..1, speed as a multiple of real time, seconds remaining.
    Progress {
        fraction: f64,
        speed: f64,
        eta: f64,
    },
    Finished {
        ok: bool,
        message: String,
    },
}

/// Runs a [`Plan`], reporting progress parsed from ffmpeg's `-progress` stream.
#[derive(Default)]
pub struct Exporter {
    rx: Option<Receiver<ExportEvent>>,
    cancelled: Arc<AtomicBool>,
    running: bool,
}

impl Exporter {
    pub fn running(&self) -> bool {
        self.running
    }

    pub fn start(&mut self, plan: Plan, ctx: egui::Context) {
        if self.running {
            return;
        }
        let (tx, rx) = channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        self.rx = Some(rx);
        self.cancelled = cancelled;
        self.running = true;

        thread::spawn(move || {
            // Whether a broken output may be cleared away later. ffmpeg opens
            // the destination with -y, so a file that was already there is lost
            // the moment the encode starts -- but if ffmpeg failed before
            // reaching it, that file is still whole, and removing it would
            // destroy something this export never touched.
            let created_output = !paths::exists(&plan.output);

            let spawned = command(&ffmpeg_path())
                .args(&plan.args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            let mut child = match spawned {
                Ok(child) => child,
                Err(_) => {
                    let _ = tx.send(ExportEvent::Finished {
                        ok: false,
                        message: "could not run ffmpeg".into(),
                    });
                    ctx.request_repaint();
                    return;
                }
            };

            // stderr is read on its own thread: ffmpeg writes to both pipes,
            // and a full one blocks the process rather than the reader.
            let mut stderr_pipe = child.stderr.take();
            let stderr_thread = thread::spawn(move || {
                let mut text = String::new();
                if let Some(pipe) = stderr_pipe.as_mut() {
                    let mut buf = Vec::new();
                    let _ = pipe.read_to_end(&mut buf);
                    text = String::from_utf8_lossy(&buf).into_owned();
                }
                text
            });

            if let Some(mut stdout) = child.stdout.take() {
                let mut pending = String::new();
                let mut buf = [0u8; 8192];
                loop {
                    if flag.load(Ordering::SeqCst) {
                        let _ = child.kill();
                        break;
                    }
                    match stdout.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                            if let Some(event) = parse_progress(&pending, plan.duration) {
                                let _ = tx.send(event);
                                ctx.request_repaint();
                            }
                            // Keep only the unfinished tail; a key can arrive
                            // split across two reads.
                            if let Some(index) = pending.rfind('\n') {
                                pending = pending[index + 1..].to_string();
                            }
                        }
                    }
                }
            }

            let status = child.wait();
            let stderr = stderr_thread.join().unwrap_or_default();
            let cancelled = flag.load(Ordering::SeqCst);
            let code = status.as_ref().ok().and_then(|s| s.code()).unwrap_or(-1);

            let discard_broken = || {
                // A killed or failed encode leaves a file that plays for a
                // second and then stops, which the next export then trips over
                // with "replace this?". Only ever remove one we put there.
                if created_output {
                    let _ = std::fs::remove_file(&plan.output);
                }
            };

            let event = if cancelled {
                discard_broken();
                ExportEvent::Finished {
                    ok: false,
                    message: "Export cancelled".into(),
                }
            } else if code == 0 && paths::exists(&plan.output) {
                let size = paths::size_of(&plan.output) as f64 / 1_000_000.0;
                ExportEvent::Finished {
                    ok: true,
                    message: format!("Saved {}  ·  {size:.0} MB", paths::basename(&plan.output)),
                }
            } else {
                discard_broken();
                let tail = stderr
                    .lines()
                    .rfind(|line| !line.trim().is_empty())
                    .map(|line| line.to_string());
                ExportEvent::Finished {
                    ok: false,
                    message: tail.unwrap_or_else(|| format!("ffmpeg failed (exit {code})")),
                }
            };
            let _ = tx.send(event);
            ctx.request_repaint();
        });
    }

    pub fn cancel(&mut self) {
        if self.running {
            self.cancelled.store(true, Ordering::SeqCst);
        }
    }

    /// Everything ffmpeg has said since the last call.
    pub fn poll(&mut self) -> Vec<ExportEvent> {
        let mut events = Vec::new();
        let Some(rx) = self.rx.as_ref() else {
            return events;
        };
        loop {
            match rx.try_recv() {
                Ok(event) => {
                    if matches!(event, ExportEvent::Finished { .. }) {
                        self.running = false;
                    }
                    events.push(event);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.running = false;
                    break;
                }
            }
        }
        if !self.running {
            self.rx = None;
        }
        events
    }
}

/// One progress block from ffmpeg, or None if it did not carry a position.
pub fn parse_progress(text: &str, total: f64) -> Option<ExportEvent> {
    let mut done: Option<f64> = None;
    let mut speed: Option<f64> = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key {
            "out_time_us"
                if value
                    .trim_start_matches('-')
                    .chars()
                    .all(|c| c.is_ascii_digit()) =>
            {
                done = value.parse::<i64>().ok().map(|us| us as f64 / 1_000_000.0);
            }
            "speed" if value.ends_with('x') => {
                speed = value[..value.len() - 1].trim().parse::<f64>().ok();
            }
            _ => {}
        }
    }
    let done = done?;
    let fraction = (done / total.max(MIN_DURATION)).clamp(0.0, 1.0);
    let speed = speed.unwrap_or(0.0);
    let remaining = total - done;
    let eta = if speed > 0.0 { remaining / speed } else { 0.0 };
    Some(ExportEvent::Progress {
        fraction,
        speed,
        eta: eta.max(0.0),
    })
}
