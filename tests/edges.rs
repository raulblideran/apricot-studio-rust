// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! The awkward half: malformed input, hostile filenames, degenerate ranges and
//! sources that will not open.
//!
//! None of this is what the app is for. All of it is what it will be handed.

mod common;

use apricot_studio::export::{self, Format, Options};
use apricot_studio::media::{self, MediaInfo};
use apricot_studio::paths;
use common::{have_ffmpeg, sample_info, value_after};

const KF: [f64; 5] = [0.0, 2.0, 4.0, 44.000004, 46.0];

fn info_with(mutate: impl FnOnce(&mut MediaInfo)) -> MediaInfo {
    let mut info = sample_info();
    mutate(&mut info);
    info
}

fn number(args: &[String], flag: &str) -> f64 {
    value_after(args, flag)
        .unwrap_or_else(|| panic!("{flag} is missing"))
        .parse()
        .expect("a number")
}

// ----- the container extension -------------------------------------------

fn ext_of(path: &str) -> String {
    // The same reading probe() does, exercised through the parser so the two
    // cannot drift apart.
    let json = r#"{"format":{"duration":"10.0"},
                   "streams":[{"codec_type":"video","codec_name":"h264",
                               "r_frame_rate":"60/1","width":640,"height":360}]}"#;
    media::parse_probe(path, json)
        .expect("a readable document")
        .ext
}

#[test]
fn a_dotted_directory_does_not_leak_into_the_extension() {
    // "~/my.videos/clip" would otherwise make the extension "videos/clip",
    // which then lands in the output filename and the container checks.
    assert_eq!(ext_of("/home/me/my.videos/clip"), "mp4");
}

#[test]
fn a_file_with_no_extension_falls_back() {
    assert_eq!(ext_of("/tmp/recording"), "mp4");
}

#[test]
fn the_extension_is_lowercased() {
    assert_eq!(ext_of("/tmp/CLIP.MKV"), "mkv");
}

#[test]
fn a_double_extension_takes_the_last() {
    assert_eq!(ext_of("/tmp/clip.tar.mkv"), "mkv");
}

#[test]
fn the_extension_never_contains_a_separator() {
    for path in ["/home/me/my.videos/clip", "/tmp/a.b/c", "/tmp/clip.mp4"] {
        assert!(!ext_of(path).contains('/'), "{path}");
    }
}

// ----- hostile timecodes --------------------------------------------------

#[test]
fn rejects_the_values_a_parser_accepts_but_a_file_cannot_have() {
    // A NaN in-point propagates through every clamp -- comparisons against NaN
    // are all false -- and then corrupts the timecode display.
    for bad in ["nan", "NaN", "inf", "-inf", "infinity", "1e400"] {
        assert_eq!(media::parse_tc(bad), None, "{bad:?} should not parse");
    }
}

#[test]
fn rejects_negative_positions() {
    for bad in ["-5", "-0.1", "-1:00"] {
        assert_eq!(media::parse_tc(bad), None, "{bad:?} should not parse");
    }
}

#[test]
fn rejects_junk() {
    for bad in ["", "   ", "abc", "12:", ":30", "1:2:3:4", "--", "1..2"] {
        assert_eq!(media::parse_tc(bad), None, "{bad:?} should not parse");
    }
}

#[test]
fn still_accepts_everything_legitimate() {
    for good in ["0", "0.0", "00:00:00.000", "1:00", "10:00:00.000", "59.999"] {
        assert!(media::parse_tc(good).is_some(), "{good:?} should parse");
    }
}

#[test]
fn the_formatter_survives_values_it_should_never_see() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.0] {
        assert_eq!(media::fmt_tc(bad), "00:00:00.000", "{bad}");
    }
}

#[test]
fn the_formatter_handles_very_long_files() {
    // Ten hours of replay buffer is absurd and someone will still do it.
    assert_eq!(media::fmt_tc(36_000.0), "10:00:00.000");
    assert_eq!(media::fmt_tc(359_999.999), "99:59:59.999");
}

// ----- degenerate ranges --------------------------------------------------

#[test]
fn a_negative_start_never_produces_a_negative_seek() {
    let plan = export::build(
        &sample_info(),
        -5.0,
        10.0,
        "/tmp/o.mp4",
        &KF,
        &Options::default(),
    );
    // Clamped to the file, and the clamp lands it on the keyframe at zero.
    assert!(plan.args.iter().all(|arg| !arg.starts_with("-0.")));
    assert!(plan.duration > 0.0);
}

#[test]
fn a_start_past_the_end_is_clamped_into_the_file() {
    let info = sample_info();
    let plan = export::build(&info, 500.0, 600.0, "/tmp/o.mp4", &KF, &Options::default());
    assert!(plan.duration > 0.0);
    let start = number(&plan.args, "-ss");
    assert!(start <= info.duration, "seek past the end: {start}");
}

#[test]
fn a_reversed_range_does_not_produce_a_negative_duration() {
    let plan = export::build(
        &sample_info(),
        50.0,
        10.0,
        "/tmp/o.mp4",
        &KF,
        &Options::default(),
    );
    assert!(plan.duration > 0.0);
    assert!(number(&plan.args, "-t") > 0.0);
}

#[test]
fn a_zero_length_range_still_yields_at_least_one_frame() {
    let info = sample_info();
    let plan = export::build(&info, 10.0, 10.0, "/tmp/o.mp4", &KF, &Options::default());
    assert!((plan.duration - info.frame_duration()).abs() < 1e-9);
}

#[test]
fn a_reversed_range_stays_positive_on_a_keyframe_too() {
    // The lossless path has its own arithmetic, and a negative -t there would
    // also become the divisor for the progress fraction.
    let plan = export::build(
        &sample_info(),
        44.000004,
        10.0,
        "/tmp/o.mp4",
        &KF,
        &Options::default(),
    );
    assert!(plan.duration > 0.0);
    assert!(number(&plan.args, "-t") > 0.0);
}

#[test]
fn a_zero_length_range_on_a_keyframe_yields_a_frame() {
    let info = sample_info();
    let plan = export::build(
        &info,
        44.000004,
        44.000004,
        "/tmp/o.mp4",
        &KF,
        &Options::default(),
    );
    assert!(plan.lossless);
    assert!((plan.duration - info.frame_duration()).abs() < 1e-9);
}

#[test]
fn the_duration_is_never_nan() {
    for (start, end) in [
        (f64::NAN, 10.0),
        (10.0, f64::NAN),
        (f64::INFINITY, 10.0),
        (0.0, f64::NEG_INFINITY),
    ] {
        let plan = export::build(
            &sample_info(),
            start,
            end,
            "/tmp/o.mp4",
            &KF,
            &Options::default(),
        );
        assert!(plan.duration.is_finite(), "{start} -> {end}");
        assert!(plan.duration > 0.0, "{start} -> {end}");
    }
}

// ----- size arithmetic at its limits -------------------------------------

#[test]
fn a_near_zero_duration_does_not_explode() {
    let rate = export::target_video_bitrate(
        &sample_info(),
        &Options {
            target_bytes: 10_000_000,
            ..Default::default()
        },
        0.000_001,
    );
    assert!(rate <= export::MAX_BITRATE, "{rate}");
    assert!(rate >= 100_000, "{rate}");
}

#[test]
fn an_impossible_target_still_yields_a_usable_bitrate() {
    // One byte for ninety seconds is not a request anyone can honour, but the
    // encoder must still be given something it will accept.
    let rate = export::target_video_bitrate(
        &sample_info(),
        &Options {
            target_bytes: 1,
            ..Default::default()
        },
        90.0,
    );
    assert_eq!(rate, 100_000);
}

#[test]
fn audio_alone_exceeding_the_budget_does_not_go_negative() {
    let info = info_with(|i| i.audio[0].bitrate = 5_000_000);
    let rate = export::target_video_bitrate(
        &info,
        &Options {
            target_bytes: 10_000,
            ..Default::default()
        },
        60.0,
    );
    assert!(rate > 0, "{rate}");
}

// ----- awkward filenames --------------------------------------------------

const AWKWARD: [&str; 7] = [
    "my clip.mp4",
    "clip #2.mp4",
    "100% done.mp4",
    "it's here.mp4",
    "clip (final).mp4",
    "réplay.mp4",
    "a;b&c.mp4",
];

#[test]
fn default_output_handles_them_all() {
    let dir = tempfile::tempdir().unwrap();
    for name in AWKWARD {
        let source = dir.path().join(name);
        std::fs::write(&source, b"x").unwrap();
        let out = export::default_output(&source.to_string_lossy(), None);
        assert!(out.ends_with(".mp4"), "{out}");
        assert!(out.contains("_clip"), "{out}");
        assert!(!paths::exists(&out), "{out} already exists");
    }
}

#[test]
fn paths_reach_ffmpeg_as_single_arguments() {
    // Nothing is quoted or escaped, because nothing goes through a shell: the
    // process is spawned with an argument list.
    for name in AWKWARD {
        let path = format!("/tmp/{name}");
        let info = info_with(|i| i.path = path.clone());
        let plan = export::build(
            &info,
            1.0,
            2.0,
            "/tmp/out dir/o.mp4",
            &KF,
            &Options::default(),
        );
        assert!(plan.args.contains(&path), "{path} was mangled");
        assert!(plan.args.contains(&"/tmp/out dir/o.mp4".to_string()));
    }
}

#[test]
fn a_file_uri_encodes_what_a_parser_would_trip_over() {
    // "my clip #2.mp4" pasted straight into a file:// string is one a parser
    // may read as ending at the #.
    let uri = apricot_studio::platform::file_uri("/tmp/my clip #2.mp4");
    assert!(uri.starts_with("file://"), "{uri}");
    assert!(!uri.contains(' '), "{uri}");
    assert!(!uri.contains('#'), "{uri}");
    assert!(uri.contains("%20") && uri.contains("%23"), "{uri}");
}

// ----- keyframe robustness ------------------------------------------------

#[test]
fn unsorted_keyframes_do_not_produce_a_silly_gop() {
    let scrambled = [10.0, 0.0, 6.0, 2.0, 8.0, 4.0];
    let plan = export::build(
        &sample_info(),
        45.0,
        50.0,
        "/tmp/o.mp4",
        &scrambled,
        &Options::default(),
    );
    let gop = number(&plan.args, "-g");
    assert!((1.0..=1800.0).contains(&gop), "{gop}");
}

#[test]
fn duplicate_keyframes_do_not_divide_by_zero() {
    let repeated = [2.0, 2.0, 2.0, 2.0];
    let plan = export::build(
        &sample_info(),
        45.0,
        50.0,
        "/tmp/o.mp4",
        &repeated,
        &Options::default(),
    );
    assert!(number(&plan.args, "-g") >= 1.0);
}

#[test]
fn a_single_keyframe_falls_back() {
    let plan = export::build(
        &sample_info(),
        45.0,
        50.0,
        "/tmp/o.mp4",
        &[0.0],
        &Options::default(),
    );
    assert_eq!(number(&plan.args, "-g"), 120.0);
}

#[test]
fn keyframes_beyond_the_duration_are_harmless() {
    let beyond = [0.0, 2.0, 4.0, 10_000.0];
    let plan = export::build(
        &sample_info(),
        45.0,
        50.0,
        "/tmp/o.mp4",
        &beyond,
        &Options::default(),
    );
    assert!(number(&plan.args, "-g") >= 1.0);
}

#[test]
fn snapping_with_an_empty_list_is_identity() {
    assert_eq!(export::snap_to_keyframe(12.5, &[], 5.0), 12.5);
}

#[test]
fn zero_tolerance_snaps_only_on_an_exact_hit() {
    assert_eq!(export::snap_to_keyframe(2.0, &[2.0], 0.0), 2.0);
    assert_eq!(export::snap_to_keyframe(2.1, &[2.0], 0.0), 2.1);
}

#[test]
fn keyframe_matching_ignores_non_finite_input() {
    assert_eq!(export::on_keyframe(f64::NAN, &KF), None);
    assert_eq!(export::on_keyframe(f64::INFINITY, &KF), None);
}

// ----- output destinations ------------------------------------------------

#[test]
fn a_format_change_replaces_the_extension_rather_than_appending() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("replay.mp4").to_string_lossy().into_owned();
    let out = export::default_output(&source, Some("webm"));
    assert!(out.ends_with("_clip.webm"), "{out}");
    assert!(!out.contains(".mp4"), "{out}");
}

#[test]
fn a_leading_dot_on_the_extension_is_not_doubled() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("replay.mp4").to_string_lossy().into_owned();
    assert_eq!(
        export::default_output(&source, Some(".gif")),
        export::default_output(&source, Some("gif"))
    );
}

#[test]
fn a_source_without_an_extension_still_gets_one() {
    // Otherwise ffmpeg has no way to pick a muxer.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("recording").to_string_lossy().into_owned();
    assert!(export::default_output(&source, None).ends_with(".mp4"));
}

#[test]
fn exactly_one_dot_before_the_extension() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("replay.mp4").to_string_lossy().into_owned();
    let out = export::default_output(&source, Some("webm"));
    assert_eq!(paths::basename(&out).matches('.').count(), 1, "{out}");
}

// ----- sources that will not open ----------------------------------------

#[test]
fn rejects_a_document_with_no_video_stream() {
    let json = r#"{"format":{"duration":"10.0"},
                   "streams":[{"codec_type":"audio","codec_name":"opus"}]}"#;
    let error = media::parse_probe("/tmp/x.mp4", json).unwrap_err();
    assert!(error.contains("no video stream"), "{error}");
}

#[test]
fn rejects_a_document_with_no_duration() {
    let json = r#"{"streams":[{"codec_type":"video","codec_name":"h264"}]}"#;
    let error = media::parse_probe("/tmp/x.mp4", json).unwrap_err();
    assert!(error.contains("duration"), "{error}");
}

#[test]
fn rejects_something_that_is_not_json_at_all() {
    let error = media::parse_probe("/tmp/x.mp4", "<html>not json</html>").unwrap_err();
    assert!(!error.is_empty());
}

#[test]
fn an_empty_document_is_a_failure_not_a_panic() {
    assert!(media::parse_probe("/tmp/x.mp4", "").is_err());
    assert!(media::parse_probe("/tmp/x.mp4", "{}").is_err());
}

#[test]
fn sparse_metadata_still_summarises() {
    // Matroska omits per-stream bitrates, and plenty of files carry no colour
    // tags at all. None of that may stop the header line being drawn.
    let json = r#"{"format":{"duration":"10.0","size":"1000000"},
                   "streams":[{"codec_type":"video","codec_name":"h264"}]}"#;
    let info = media::parse_probe("/tmp/x.mkv", json).expect("a usable file");
    let summary = info.summary();
    assert!(summary.contains("no audio"), "{summary}");
    assert!(
        info.v_bitrate > 0,
        "bitrate should fall back to size/duration"
    );
}

#[test]
fn a_frame_rate_of_zero_does_not_divide_by_zero() {
    let json = r#"{"format":{"duration":"10.0"},
                   "streams":[{"codec_type":"video","codec_name":"h264",
                               "r_frame_rate":"0/0","avg_frame_rate":"0/0"}]}"#;
    let info = media::parse_probe("/tmp/x.mp4", json).expect("a usable file");
    assert!(info.frame_duration() > 0.0);
    assert!(info.fps > 0.0);
}

#[test]
fn a_variable_frame_rate_prefers_the_nominal_one() {
    // gpu-screen-recorder writes VFR files where avg_frame_rate is a messy
    // fraction; r_frame_rate is the one worth believing.
    let json = r#"{"format":{"duration":"10.0"},
                   "streams":[{"codec_type":"video","codec_name":"h264",
                               "r_frame_rate":"60/1",
                               "avg_frame_rate":"5994000/100001"}]}"#;
    let info = media::parse_probe("/tmp/x.mp4", json).expect("a usable file");
    assert_eq!(info.fps, 60.0);
}

#[test]
fn rejects_a_missing_file() {
    if !have_ffmpeg() {
        return;
    }
    let error = media::probe("/tmp/definitely-not-here-9f8a7b6c.mp4").unwrap_err();
    assert!(!error.is_empty());
}

#[test]
fn rejects_a_text_file_wearing_a_video_extension() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fake.mp4");
    std::fs::write(&path, b"this is not a video, it is a sentence").unwrap();
    assert!(media::probe(&path.to_string_lossy()).is_err());
}

#[test]
fn rejects_an_empty_file() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.mp4");
    std::fs::write(&path, b"").unwrap();
    assert!(media::probe(&path.to_string_lossy()).is_err());
}

#[test]
fn rejects_a_directory() {
    if !have_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    assert!(media::probe(&dir.path().to_string_lossy()).is_err());
}

// ----- the waveform -------------------------------------------------------

#[test]
fn silent_audio_does_not_break_the_waveform_scaling() {
    // Scaling to the loudest moment would divide by zero on a silent track.
    let silence = vec![0u8; 4000];
    let peaks = media::peaks_from_pcm(&silence, 100);
    assert_eq!(peaks.len(), 100);
    assert!(peaks.iter().all(|p| *p == 0.0));
}

#[test]
fn a_quiet_recording_is_scaled_up_to_be_readable() {
    // Game capture often peaks well below full scale, which would draw as a
    // flat line.
    let mut pcm = Vec::new();
    for i in 0..2000 {
        let sample = if i == 1000 { 3000i16 } else { 300 };
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    let peaks = media::peaks_from_pcm(&pcm, 50);
    let loudest = peaks.iter().copied().fold(0.0f32, f32::max);
    assert!(
        (loudest - 1.0).abs() < 1e-6,
        "the peak should reach the top"
    );
}

#[test]
fn a_dangling_byte_from_a_killed_decode_is_ignored() {
    let pcm = vec![0u8; 101]; // an odd number: half a sample at the end
    assert_eq!(media::peaks_from_pcm(&pcm, 10).len(), 10);
}

#[test]
fn no_audio_at_all_produces_no_envelope() {
    assert!(media::peaks_from_pcm(&[], 100).is_empty());
}

// ----- options that contradict each other --------------------------------

#[test]
fn a_gif_ignores_a_size_target_rather_than_obeying_it_badly() {
    // A GIF's rate control is its palette; there is no bitrate to solve.
    let options = Options {
        fmt: Format::Gif,
        target_bytes: 10_000_000,
        ..Default::default()
    };
    assert_eq!(export::estimate_bytes(&sample_info(), &options, 10.0), 0);
}

#[test]
fn a_silent_source_asked_for_a_track_still_exports() {
    let info = info_with(|i| i.audio.clear());
    let options = Options {
        audio: apricot_studio::export::Audio::Track(3),
        ..Default::default()
    };
    let plan = export::build(&info, 45.0, 50.0, "/tmp/o.mp4", &KF, &options);
    assert!(!plan.args.iter().any(|arg| arg == "0:a:3"));
}
