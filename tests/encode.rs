// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! Integration tests: build a command, actually run ffmpeg, inspect the result.
//!
//! Every assertion here corresponds to a guarantee the app makes about its
//! output. Several of them exist because that exact guarantee was once quietly
//! broken -- the clip still played, it was just wrong -- which is the only kind
//! of bug this project has produced so far.
//!
//! Slower than the rest of the suite (real encodes), so `run-tests.sh --fast`
//! skips this file.

mod common;

use std::process::Command;

use apricot_studio::deinterlace::{
    self, BwdifProfile, Deinterlace, Method, Parity, QtgmcPreset, Scan,
};
use apricot_studio::export::{self, Audio, Format, Options, Quality};
use apricot_studio::media;
use common::{fixture, have_ffmpeg, keyframes, FPS};

const ONE_FRAME: f64 = 1.0 / FPS as f64;
/// Opus frames are 20 ms; copied audio lands on one.
const AUDIO_PACKET: f64 = 0.020;

/// Export a range and hand back where it landed.
///
/// Returns None when there is no ffmpeg, which is the whole of this file's
/// skipping logic: nothing here can say anything without one.
fn run_export(
    name: &str,
    kind: &str,
    start: f64,
    end: f64,
    options: Options,
) -> Option<(export::Plan, String)> {
    if !have_ffmpeg() {
        return None;
    }
    let source = fixture(kind);
    let info = media::probe(&source).expect("the fixture should probe");
    let dir = std::env::temp_dir().join("apricot-rust-tests/out");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let out = dir
        .join(format!("{name}.{}", options.ext_for(&info)))
        .to_string_lossy()
        .into_owned();
    let _ = std::fs::remove_file(&out);

    let plan = export::build(&info, start, end, &out, &keyframes(&source), &options);
    let result = Command::new("ffmpeg")
        .args(&plan.args)
        .output()
        .expect("ffmpeg should run");
    assert!(
        result.status.success(),
        "ffmpeg failed:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        std::path::Path::new(&out).exists(),
        "ffmpeg reported success but wrote nothing"
    );
    Some((plan, out))
}

// ----- reading the result -------------------------------------------------

fn probe_field(path: &str, kind: &str, field: &str) -> String {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            match kind {
                "audio" => "a:0",
                _ => "v:0",
            },
            "-show_entries",
            &format!("stream={field}"),
            "-of",
            "csv=p=0",
            path,
        ])
        .output()
        .expect("ffprobe should run");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn number_field(path: &str, kind: &str, field: &str) -> f64 {
    probe_field(path, kind, field).parse().unwrap_or(0.0)
}

/// Container duration -- the longest stream, so audio overhang counts.
fn container_duration(path: &str) -> f64 {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
            path,
        ])
        .output()
        .expect("ffprobe should run");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0.0)
}

/// One stream's own duration.
///
/// Worth having separately: a stream copy cannot split an audio packet, so the
/// last one overhangs the cut by up to its own length and drags the container
/// duration with it. Video is where frame-exactness is actually promised.
fn stream_duration(path: &str) -> f64 {
    number_field(path, "video", "duration")
}

fn frame_types(path: &str, count: usize) -> String {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-read_intervals",
            &format!("%+{}", count / FPS as usize + 1),
            "-show_entries",
            "frame=pict_type",
            "-of",
            "csv=p=0",
            path,
        ])
        .output()
        .expect("ffprobe should run");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .collect::<String>()
        .replace(',', "")
        .chars()
        .take(count)
        .collect()
}

/// MD5 of the raw video packets -- proves a stream copy was bit-exact.
fn video_md5(path: &str, seek: Option<f64>, length: Option<f64>) -> String {
    let mut args: Vec<String> = vec!["-v".into(), "error".into()];
    if let Some(seek) = seek {
        args.push("-ss".into());
        args.push(format!("{seek:.6}"));
    }
    args.push("-i".into());
    args.push(path.to_string());
    if let Some(length) = length {
        args.push("-t".into());
        args.push(format!("{length:.6}"));
    }
    args.extend(
        ["-map", "0:v", "-c", "copy", "-f", "md5", "-"]
            .iter()
            .map(|s| s.to_string()),
    );
    let out = Command::new("ffmpeg")
        .args(&args)
        .output()
        .expect("ffmpeg should run");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn audio_stream_count(path: &str) -> usize {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=index",
            "-of",
            "csv=p=0",
            path,
        ])
        .output()
        .expect("ffprobe should run");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
}

/// The 2.0s keyframe, which every lossless case cuts on.
fn second_keyframe(kind: &str) -> f64 {
    keyframes(&fixture(kind))[1]
}

// ----- the lossless cut ---------------------------------------------------
// The headline promise: cutting on a keyframe copies bytes, it does not encode.

#[test]
fn video_is_bit_identical_to_the_source() {
    if !have_ffmpeg() {
        return;
    }
    let source = fixture("allp");
    let keyframe = second_keyframe("allp");
    let Some((plan, out)) = run_export(
        "lossless_identical",
        "allp",
        keyframe,
        keyframe + 2.0,
        Options::default(),
    ) else {
        return;
    };
    assert!(plan.lossless);
    assert_eq!(
        video_md5(&out, None, None),
        video_md5(&source, Some(keyframe), Some(2.0)),
        "a stream copy must reproduce the source bytes exactly"
    );
}

#[test]
fn the_duration_is_what_was_asked_for() {
    // Regression: asking for 14s once produced 16.02s, because ffmpeg seeks to
    // the keyframe at-or-before the given time and the stored timestamp is not
    // exactly round. Uses a real keyframe, which is what exposed it.
    let keyframe = if have_ffmpeg() {
        second_keyframe("allp")
    } else {
        return;
    };
    let Some((_, out)) = run_export(
        "lossless_duration",
        "allp",
        keyframe,
        keyframe + 2.0,
        Options::default(),
    ) else {
        return;
    };
    assert!(
        (stream_duration(&out) - 2.0).abs() <= ONE_FRAME,
        "{}",
        stream_duration(&out)
    );
    // The container runs slightly longer, and legitimately so: a copy cannot
    // split an audio packet at either end, so the clip carries a partial packet
    // at the head and another at the tail. Two packets is the whole honest
    // allowance -- anything beyond that is a real overrun.
    assert!(
        container_duration(&out) <= 2.0 + 2.0 * AUDIO_PACKET + 1e-6,
        "container overran by more than the audio packet grid: {}",
        container_duration(&out)
    );
}

#[test]
fn requesting_a_rounded_time_still_lands_on_the_keyframe() {
    if !have_ffmpeg() {
        return;
    }
    let keyframe = second_keyframe("allp");
    assert!((keyframe - 2.0).abs() < 0.001, "fixture keyframe moved");
    // Ask for the round number, not the stored one.
    let Some((_, out)) = run_export("lossless_rounded", "allp", 2.0, 4.0, Options::default())
    else {
        return;
    };
    assert!((stream_duration(&out) - 2.0).abs() <= ONE_FRAME);
}

#[test]
fn the_frame_count_is_exact() {
    // 2.0s at 60fps is 120 frames, no more: the clearest statement that the
    // copy did not rewind to an earlier keyframe.
    if !have_ffmpeg() {
        return;
    }
    let keyframe = second_keyframe("allp");
    let Some((_, out)) = run_export(
        "lossless_frames",
        "allp",
        keyframe,
        keyframe + 2.0,
        Options::default(),
    ) else {
        return;
    };
    assert_eq!(number_field(&out, "video", "nb_frames"), 2.0 * FPS as f64);
}

#[test]
fn audio_leads_video_by_at_most_one_packet() {
    if !have_ffmpeg() {
        return;
    }
    let keyframe = second_keyframe("allp");
    let Some((_, out)) = run_export(
        "lossless_sync",
        "allp",
        keyframe,
        keyframe + 2.0,
        Options::default(),
    ) else {
        return;
    };
    let video = number_field(&out, "video", "start_time");
    let audio = number_field(&out, "audio", "start_time");
    // Audio can only begin on a packet boundary, and video on a frame, so the
    // worst honest case is one of each.
    assert!(
        (video - audio).abs() < AUDIO_PACKET + ONE_FRAME,
        "A/V drift of {:.0} ms",
        (video - audio).abs() * 1000.0
    );
}

#[test]
fn the_copy_keeps_the_source_codec_and_audio() {
    if !have_ffmpeg() {
        return;
    }
    let keyframe = second_keyframe("allp");
    let Some((_, out)) = run_export(
        "lossless_codec",
        "allp",
        keyframe,
        keyframe + 2.0,
        Options::default(),
    ) else {
        return;
    };
    assert_eq!(probe_field(&out, "video", "codec_name"), "h264");
    assert_eq!(probe_field(&out, "audio", "codec_name"), "opus");
}

// ----- the re-encoded cut -------------------------------------------------

#[test]
fn a_re_encode_is_frame_exact() {
    let Some((plan, out)) = run_export("reencode_duration", "allp", 1.0, 3.0, Options::default())
    else {
        return;
    };
    assert!(!plan.lossless, "1.0s sits mid-GOP");
    assert!(
        (stream_duration(&out) - 2.0).abs() <= 2.0 * ONE_FRAME,
        "{}",
        stream_duration(&out)
    );
}

#[test]
fn starts_between_keyframes_without_running_long() {
    // A naive input seek would rewind to 0.0 and add a second.
    let Some((_, out)) = run_export("reencode_length", "allp", 1.0, 3.0, Options::default()) else {
        return;
    };
    assert!(
        container_duration(&out) < 2.0 + 4.0 * ONE_FRAME,
        "{}",
        container_duration(&out)
    );
}

#[test]
fn audio_and_video_start_together() {
    // Regression: copied audio once began at the preceding keyframe while video
    // began on the exact frame, leaving them 516 ms apart.
    let Some((_, out)) = run_export("reencode_sync", "allp", 1.0, 3.0, Options::default()) else {
        return;
    };
    let video = number_field(&out, "video", "start_time");
    let audio = number_field(&out, "audio", "start_time");
    assert!(
        (video - audio).abs() < AUDIO_PACKET,
        "A/V drift of {:.0} ms",
        (video - audio).abs() * 1000.0
    );
}

#[test]
fn geometry_and_colour_are_inherited() {
    if !have_ffmpeg() {
        return;
    }
    let source = fixture("allp");
    let Some((_, out)) = run_export("reencode_inherit", "allp", 1.0, 3.0, Options::default())
    else {
        return;
    };
    for field in [
        "width",
        "height",
        "pix_fmt",
        "r_frame_rate",
        "color_primaries",
        "color_transfer",
        "color_space",
        "color_range",
        "profile",
    ] {
        assert_eq!(
            probe_field(&out, "video", field),
            probe_field(&source, "video", field),
            "{field} was not inherited"
        );
    }
}

// ----- the GOP structure --------------------------------------------------
// Regression cover for the exports that played back laggy. The source encodes
// all-P for low latency; an encoder left to itself adds B-frames, which forces
// the decoder to reorder and shows up as stutter.

#[test]
fn an_all_p_source_produces_an_all_p_clip() {
    let Some((_, out)) = run_export("gop_allp", "allp", 1.0, 3.0, Options::default()) else {
        return;
    };
    assert_eq!(number_field(&out, "video", "has_b_frames"), 0.0);
    assert!(!frame_types(&out, 40).contains('B'));
}

#[test]
fn a_b_frame_source_keeps_its_b_frames() {
    // Inheritance has to work in both directions, or it is just a hardcoded 0.
    if !have_ffmpeg() {
        return;
    }
    let source = fixture("bframes");
    assert_eq!(number_field(&source, "video", "has_b_frames"), 2.0);
    let Some((_, out)) = run_export("gop_bframes", "bframes", 1.0, 3.0, Options::default()) else {
        return;
    };
    assert!(number_field(&out, "video", "has_b_frames") > 0.0);
    assert!(frame_types(&out, 40).contains('B'));
}

#[test]
fn the_structure_holds_under_a_size_target() {
    let options = Options {
        target_bytes: 2_000_000,
        ..Default::default()
    };
    let Some((_, out)) = run_export("gop_target", "allp", 1.0, 3.0, options) else {
        return;
    };
    assert_eq!(number_field(&out, "video", "has_b_frames"), 0.0);
}

// ----- size targets -------------------------------------------------------
// Regression cover for a 10 MB target that produced 10.3 MB and was rejected.

#[test]
fn a_size_target_lands_under_the_requested_size() {
    for mb in [1i64, 2, 4] {
        let options = Options {
            target_bytes: mb * 1_000_000,
            ..Default::default()
        };
        let Some((_, out)) = run_export(&format!("size{mb}"), "allp", 0.5, 4.5, options) else {
            return;
        };
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(
            size <= (mb * 1_000_000) as u64,
            "{:.2} MB exceeds the {mb} MB limit",
            size as f64 / 1e6
        );
    }
}

#[test]
fn a_size_target_uses_a_worthwhile_share_of_the_budget() {
    // Aiming under is the point; aiming *far* under wastes quality for nothing.
    let options = Options {
        target_bytes: 2_000_000,
        ..Default::default()
    };
    let Some((_, out)) = run_export("size_share", "allp", 0.5, 4.5, options) else {
        return;
    };
    let size = std::fs::metadata(&out).unwrap().len();
    assert!(
        size > 1_000_000,
        "only {:.2} MB of a 2 MB budget was used",
        size as f64 / 1e6
    );
}

// ----- audio selection ----------------------------------------------------

#[test]
fn all_tracks_are_kept_by_default() {
    let Some((_, out)) = run_export("audio_all", "twoaudio", 1.0, 3.0, Options::default()) else {
        return;
    };
    assert_eq!(audio_stream_count(&out), 2);
}

#[test]
fn a_single_track_can_be_exported() {
    let options = Options {
        audio: Audio::Track(1),
        ..Default::default()
    };
    let Some((_, out)) = run_export("audio_one", "twoaudio", 1.0, 3.0, options) else {
        return;
    };
    assert_eq!(audio_stream_count(&out), 1);
}

#[test]
fn mute_produces_no_audio_stream() {
    let options = Options {
        audio: Audio::None,
        ..Default::default()
    };
    let Some((_, out)) = run_export("audio_mute", "allp", 1.0, 3.0, options) else {
        return;
    };
    assert_eq!(audio_stream_count(&out), 0);
}

#[test]
fn a_silent_source_exports_cleanly() {
    let Some((_, out)) = run_export("audio_none", "noaudio", 1.0, 3.0, Options::default()) else {
        return;
    };
    assert_eq!(audio_stream_count(&out), 0);
    assert!(stream_duration(&out) > 1.0);
}

#[test]
fn track_titles_are_read_back() {
    // Needs ffprobe 7+ to surface per-stream titles from an MP4. On an older
    // one every track is still found and selectable, just listed by number, so
    // this checks what it can and does not fail the suite for the ffprobe it
    // was run against.
    if !have_ffmpeg() {
        return;
    }
    let source = fixture("twoaudio");
    let info = media::probe(&source).expect("the fixture should probe");
    assert_eq!(info.audio.len(), 2);
    if info.audio[0].title.is_empty() {
        return; // this ffprobe does not report them
    }
    assert_eq!(info.audio[0].title, "Desktop");
    assert_eq!(info.audio[1].title, "Mic");
    assert!(info.audio[0].label().contains("Desktop"));
}

// ----- formats ------------------------------------------------------------

#[test]
fn webm_is_vp9_and_carries_its_audio() {
    let options = Options {
        fmt: Format::WebM,
        ..Default::default()
    };
    let Some((_, out)) = run_export("format_webm", "allp", 1.0, 2.0, options) else {
        return;
    };
    assert_eq!(probe_field(&out, "video", "codec_name"), "vp9");
    assert_eq!(probe_field(&out, "audio", "codec_name"), "opus");
}

#[test]
fn a_gif_is_a_gif_with_real_dimensions() {
    let options = Options {
        fmt: Format::Gif,
        ..Default::default()
    };
    let Some((_, out)) = run_export("format_gif", "allp", 1.0, 2.0, options) else {
        return;
    };
    assert_eq!(probe_field(&out, "video", "codec_name"), "gif");
    assert_eq!(number_field(&out, "video", "width"), 480.0);
    assert_eq!(audio_stream_count(&out), 0);
}

#[test]
fn a_gif_is_cut_from_the_clip_alone() {
    // It used to read the rest of the file to build its palette -- minutes and
    // gigabytes on a long recording, and colours taken from footage that was
    // never in the clip. A one-second GIF of a six-second source is the cheap
    // way to notice: the duration is the tell.
    let options = Options {
        fmt: Format::Gif,
        ..Default::default()
    };
    let Some((_, out)) = run_export("format_gif_short", "allp", 1.0, 2.0, options) else {
        return;
    };
    assert!(
        container_duration(&out) < 1.5,
        "the GIF ran to {}s from a 1s selection",
        container_duration(&out)
    );
}

// ----- the quality presets ------------------------------------------------

#[test]
fn each_preset_produces_a_smaller_file_than_the_last() {
    let mut sizes = Vec::new();
    for (name, quality) in [
        ("quality_source", Quality::Source),
        ("quality_smaller", Quality::Smaller),
        ("quality_smallest", Quality::Smallest),
    ] {
        let options = Options {
            quality,
            ..Default::default()
        };
        let Some((_, out)) = run_export(name, "allp", 0.5, 4.5, options) else {
            return;
        };
        sizes.push(std::fs::metadata(&out).unwrap().len());
    }
    assert!(sizes[0] > sizes[1], "{sizes:?}");
    assert!(sizes[1] > sizes[2], "{sizes:?}");
}

// ----- interlaced sources -------------------------------------------------

fn tff_bwdif(profile: BwdifProfile) -> Options {
    Options {
        deinterlace: Some(Deinterlace {
            method: Method::Bwdif(profile),
            parity: Parity::Tff,
        }),
        ..Options::default()
    }
}

#[test]
fn an_interlaced_source_says_so_and_a_progressive_one_does_not() {
    if !have_ffmpeg() {
        return;
    }
    let interlaced = media::probe(&fixture("interlaced")).expect("probes");
    assert_eq!(
        deinterlace::scan_from_field_order(&interlaced.field_order),
        Some(Scan::Interlaced(Parity::Tff)),
        "field_order was {:?}",
        interlaced.field_order
    );
    let progressive = media::probe(&fixture("allp")).expect("probes");
    assert_eq!(
        deinterlace::scan_from_field_order(&progressive.field_order),
        Some(Scan::Progressive)
    );
}

#[test]
fn idet_finds_the_interlacing_by_looking() {
    // The path taken when the container does not say: the pictures alone.
    if !have_ffmpeg() {
        return;
    }
    let source = fixture("interlaced");
    let out = Command::new("ffmpeg")
        .args(deinterlace::idet_args(&source, 6.0))
        .output()
        .expect("ffmpeg should run");
    assert_eq!(
        deinterlace::scan_from_idet(&String::from_utf8_lossy(&out.stderr)),
        Some(Scan::Interlaced(Parity::Tff))
    );
}

#[test]
fn bwdif_at_its_slowest_turns_each_field_into_a_frame() {
    let Some((_, out)) = run_export(
        "bwdif_slowest",
        "interlaced",
        0.0,
        2.0,
        tff_bwdif(BwdifProfile::Slowest),
    ) else {
        return;
    };
    assert_eq!(probe_field(&out, "video", "field_order"), "progressive");
    assert_eq!(probe_field(&out, "video", "r_frame_rate"), "60/1");
    let frames = number_field(&out, "video", "nb_frames");
    assert!(
        (frames - 120.0).abs() <= 2.0,
        "{frames} frames for 2s at 60p"
    );
}

#[test]
fn bwdif_fast_keeps_the_frame_rate() {
    let Some((_, out)) = run_export(
        "bwdif_fast",
        "interlaced",
        0.0,
        2.0,
        tff_bwdif(BwdifProfile::Fast),
    ) else {
        return;
    };
    assert_eq!(probe_field(&out, "video", "field_order"), "progressive");
    assert_eq!(probe_field(&out, "video", "r_frame_rate"), "30/1");
}

#[test]
fn qtgmc_exports_through_vspipe_where_it_is_installed() {
    // Skips unless VapourSynth with QTGMC is on this machine; CI's on-demand
    // Codex job is what runs it on Windows.
    if !have_ffmpeg() || !deinterlace::qtgmc_available() {
        return;
    }
    let source = fixture("interlaced");
    let info = media::probe(&source).expect("probes");
    let dir = std::env::temp_dir().join("apricot-rust-tests/out");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let out = dir.join("qtgmc.mp4").to_string_lossy().into_owned();
    let _ = std::fs::remove_file(&out);
    let options = Options {
        deinterlace: Some(Deinterlace {
            method: Method::Qtgmc(QtgmcPreset::Faster),
            parity: Parity::Tff,
        }),
        ..Options::default()
    };
    let plan = export::build(&info, 1.0, 2.0, &out, &keyframes(&source), &options);
    let mut exporter = export::Exporter::default();
    exporter.start(plan, egui::Context::default());
    let message = loop {
        let finished = exporter.poll().into_iter().find_map(|event| match event {
            export::ExportEvent::Finished { ok, message } => Some((ok, message)),
            export::ExportEvent::Progress { .. } => None,
        });
        if let Some(finished) = finished {
            break finished;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(message.0, "QTGMC export failed: {}", message.1);
    assert_eq!(probe_field(&out, "video", "r_frame_rate"), "60/1");
    let frames = number_field(&out, "video", "nb_frames");
    assert!(
        (frames - 60.0).abs() <= 2.0,
        "{frames} frames for 1s at 60p"
    );
}
