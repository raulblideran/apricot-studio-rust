// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! What ffmpeg command each set of options produces.
//!
//! These never run ffmpeg -- they assert on the argument list, which is where
//! the inheritance rules actually live. `encode.rs` then proves the commands do
//! what these say they do.

mod common;

use apricot_studio::deinterlace::{BwdifProfile, Deinterlace, Method, Parity};
use apricot_studio::export::{self, Audio, Format, Options, Plan, Quality};
use apricot_studio::media::{AudioTrack, MediaInfo};
use common::{contains_sequence, has, index_of, sample_info, value_after, values_after};

const KF: [f64; 5] = [0.0, 2.0, 4.0, 44.000004, 46.0];

fn plan_for(info: &MediaInfo, start: f64, end: f64, options: Options) -> Plan {
    export::build(info, start, end, "/tmp/o.mp4", &KF, &options)
}

fn plain(start: f64, end: f64) -> Plan {
    plan_for(&sample_info(), start, end, Options::default())
}

fn number(args: &[String], flag: &str) -> f64 {
    value_after(args, flag)
        .unwrap_or_else(|| panic!("{flag} is missing"))
        .parse()
        .expect("a number")
}

// ----- the lossless path --------------------------------------------------

#[test]
fn lossless_taken_when_the_cut_starts_on_a_keyframe() {
    let plan = plain(44.000004, 58.0);
    assert!(plan.lossless);
    assert_eq!(value_after(&plan.args, "-c").as_deref(), Some("copy"));
    assert_eq!(plan.label, "stream copy");
}

#[test]
fn seeks_to_the_stored_keyframe_not_the_requested_time() {
    // The whole point: 44.0 asked for, 44.000004 on the command line. ffmpeg
    // seeks to the keyframe at or *before* the time it is given, so asking for
    // the rounded value rewinds a whole GOP and the clip comes out too long.
    let plan = plain(44.0, 58.0);
    assert!(plan.lossless);
    assert_eq!(value_after(&plan.args, "-ss").as_deref(), Some("44.000004"));
}

#[test]
fn duration_measured_from_the_keyframe() {
    let plan = plain(44.0, 58.0);
    // 58.0 - 44.000004, not 58.0 - 44.0, or the clip runs long.
    assert!((number(&plan.args, "-t") - 13.999996).abs() < 1e-5);
}

#[test]
fn lossless_declined_between_keyframes() {
    let plan = plain(45.0, 58.0);
    assert!(!plan.lossless);
    assert!(has(&plan.args, "libx264"));
}

#[test]
fn lossless_declined_when_a_size_target_is_set() {
    let options = Options {
        target_bytes: 10_000_000,
        ..Default::default()
    };
    let plan = plan_for(&sample_info(), 44.000004, 58.0, options);
    assert!(!plan.lossless, "a copy cannot honour a size target");
}

#[test]
fn lossless_declined_for_other_formats() {
    for fmt in [Format::WebM, Format::Gif] {
        let options = Options {
            fmt,
            ..Default::default()
        };
        let plan = plan_for(&sample_info(), 44.000004, 58.0, options);
        assert!(!plan.lossless, "{fmt:?} cannot be a stream copy");
    }
}

#[test]
fn a_copy_still_asks_for_faststart_on_mp4() {
    let plan = plain(44.000004, 58.0);
    assert!(contains_sequence(&plan.args, &["-movflags", "+faststart"]));
}

// ----- the seek strategy --------------------------------------------------
// Regression cover for the 516 ms audio/video desync.

fn seek_positions(args: &[String]) -> (Vec<usize>, Vec<usize>) {
    let input = index_of(args, "-i").expect("an input");
    let all: Vec<usize> = args
        .iter()
        .enumerate()
        .filter(|(_, arg)| *arg == "-ss")
        .map(|(index, _)| index)
        .collect();
    (
        all.iter().copied().filter(|n| *n < input).collect(),
        all.iter().copied().filter(|n| *n > input).collect(),
    )
}

#[test]
fn splits_the_seek_around_the_input() {
    let plan = plain(44.5, 58.0);
    let (before, after) = seek_positions(&plan.args);
    assert!(
        !before.is_empty(),
        "needs a coarse seek before -i to stay fast"
    );
    assert!(
        !after.is_empty(),
        "needs an exact seek after -i or copied audio desyncs"
    );
}

#[test]
fn the_two_seeks_add_up_to_the_cut_point() {
    let plan = plain(44.5, 58.0);
    let (before, after) = seek_positions(&plan.args);
    let coarse: f64 = plan.args[before[0] + 1].parse().unwrap();
    let fine: f64 = plan.args[after[0] + 1].parse().unwrap();
    assert!((coarse + fine - 44.5).abs() < 1e-5);
}

#[test]
fn no_coarse_seek_needed_near_the_start() {
    let plan = plain(1.0, 5.0);
    let (before, _) = seek_positions(&plan.args);
    assert!(before.is_empty());
}

#[test]
fn duration_is_a_length_not_an_endpoint() {
    let plan = plain(44.5, 58.0);
    assert!((number(&plan.args, "-t") - 13.5).abs() < 1e-5);
    assert!(
        !has(&plan.args, "-to"),
        "-to after a seek is ambiguous; -t is not"
    );
}

// ----- inheritance --------------------------------------------------------
// Regression cover for the B-frames that made exports play back laggy.

fn info_with(mutate: impl FnOnce(&mut MediaInfo)) -> MediaInfo {
    let mut info = sample_info();
    mutate(&mut info);
    info
}

#[test]
fn matches_a_source_with_no_b_frames() {
    let plan = plan_for(
        &info_with(|i| i.has_b_frames = 0),
        45.0,
        58.0,
        Options::default(),
    );
    assert_eq!(value_after(&plan.args, "-bf").as_deref(), Some("0"));
}

#[test]
fn matches_a_source_that_has_them() {
    let plan = plan_for(
        &info_with(|i| i.has_b_frames = 2),
        45.0,
        58.0,
        Options::default(),
    );
    assert_eq!(value_after(&plan.args, "-bf").as_deref(), Some("2"));
}

#[test]
fn b_frames_inherited_under_a_size_target_too() {
    let options = Options {
        target_bytes: 10_000_000,
        ..Default::default()
    };
    let plan = plan_for(&info_with(|i| i.has_b_frames = 0), 45.0, 58.0, options);
    assert_eq!(value_after(&plan.args, "-bf").as_deref(), Some("0"));
}

#[test]
fn hevc_takes_b_frames_through_x265_params() {
    let info = info_with(|i| {
        i.v_codec = "hevc".into();
        i.profile = "Main".into();
    });
    let plan = plan_for(&info, 45.0, 58.0, Options::default());
    assert!(value_after(&plan.args, "-x265-params")
        .unwrap()
        .contains("bframes=0"));
}

#[test]
fn carries_colour_metadata() {
    let plan = plain(45.0, 58.0);
    assert_eq!(
        value_after(&plan.args, "-color_primaries").as_deref(),
        Some("bt709")
    );
    assert_eq!(
        value_after(&plan.args, "-color_trc").as_deref(),
        Some("bt709")
    );
    assert_eq!(
        value_after(&plan.args, "-colorspace").as_deref(),
        Some("bt709")
    );
    assert_eq!(
        value_after(&plan.args, "-color_range").as_deref(),
        Some("tv")
    );
}

#[test]
fn omits_colour_tags_the_source_does_not_have() {
    let info = info_with(|i| {
        i.color_space = String::new();
        i.color_primaries = "unknown".into();
    });
    let plan = plan_for(&info, 45.0, 58.0, Options::default());
    assert!(!has(&plan.args, "-colorspace"));
    assert!(!has(&plan.args, "-color_primaries"));
}

#[test]
fn carries_pixel_format_and_profile() {
    let plan = plain(45.0, 58.0);
    assert_eq!(
        value_after(&plan.args, "-pix_fmt").as_deref(),
        Some("yuv420p")
    );
    assert_eq!(
        value_after(&plan.args, "-profile:v").as_deref(),
        Some("high")
    );
}

#[test]
fn gop_derived_from_measured_keyframes() {
    let two_second: Vec<f64> = (0..20).map(|i| i as f64 * 2.0).collect();
    let plan = export::build(
        &sample_info(),
        45.0,
        58.0,
        "/tmp/o.mp4",
        &two_second,
        &Options::default(),
    );
    assert_eq!(value_after(&plan.args, "-g").as_deref(), Some("120"));
}

#[test]
fn gop_falls_back_when_keyframes_are_unknown() {
    let plan = export::build(
        &sample_info(),
        45.0,
        58.0,
        "/tmp/o.mp4",
        &[],
        &Options::default(),
    );
    assert_eq!(value_after(&plan.args, "-g").as_deref(), Some("120"));
}

#[test]
fn preserves_variable_frame_timing() {
    let plan = plain(45.0, 58.0);
    assert_eq!(
        value_after(&plan.args, "-fps_mode").as_deref(),
        Some("passthrough")
    );
}

#[test]
fn audio_is_copied_never_re_encoded() {
    let plan = plain(45.0, 58.0);
    assert_eq!(value_after(&plan.args, "-c:a").as_deref(), Some("copy"));
}

// ----- audio mapping ------------------------------------------------------

fn two_tracks() -> MediaInfo {
    info_with(|i| {
        i.audio = vec![
            AudioTrack {
                index: 0,
                codec: "opus".into(),
                bitrate: 106_489,
                sample_rate: 48000,
                channels: 2,
                language: "und".into(),
                title: "Desktop".into(),
            },
            AudioTrack {
                index: 1,
                codec: "opus".into(),
                bitrate: 64_000,
                sample_rate: 48000,
                channels: 1,
                language: "und".into(),
                title: "Mic".into(),
            },
        ];
    })
}

#[test]
fn all_tracks_by_default() {
    let plan = plan_for(&two_tracks(), 45.0, 58.0, Options::default());
    assert!(values_after(&plan.args, "-map").contains(&"0:a".to_string()));
}

#[test]
fn a_single_track_can_be_chosen() {
    let options = Options {
        audio: Audio::Track(1),
        ..Default::default()
    };
    let plan = plan_for(&two_tracks(), 45.0, 58.0, options);
    let maps = values_after(&plan.args, "-map");
    assert!(maps.contains(&"0:a:1".to_string()), "{maps:?}");
    assert!(!maps.contains(&"0:a".to_string()), "{maps:?}");
}

#[test]
fn mute_maps_nothing() {
    let options = Options {
        audio: Audio::None,
        ..Default::default()
    };
    let plan = plan_for(&two_tracks(), 45.0, 58.0, options);
    let maps = values_after(&plan.args, "-map");
    assert_eq!(maps, vec!["0:v:0".to_string()]);
    assert!(!has(&plan.args, "-c:a"));
}

#[test]
fn silent_source_maps_nothing() {
    let plan = plan_for(
        &info_with(|i| i.audio.clear()),
        45.0,
        58.0,
        Options::default(),
    );
    assert_eq!(values_after(&plan.args, "-map"), vec!["0:v:0".to_string()]);
}

// ----- formats ------------------------------------------------------------

#[test]
fn gif_builds_its_own_palette() {
    let options = Options {
        fmt: Format::Gif,
        ..Default::default()
    };
    let plan = plan_for(&sample_info(), 45.0, 58.0, options);
    let chain = value_after(&plan.args, "-filter_complex").expect("a filter chain");
    assert!(chain.contains("palettegen"), "{chain}");
    assert!(chain.contains("paletteuse"), "{chain}");
    assert!(chain.contains("fps=15"), "{chain}");
    assert!(chain.contains("scale=480"), "{chain}");
}

#[test]
fn gif_seeks_the_input_rather_than_the_output() {
    // palettegen holds its palette back until end of stream and paletteuse
    // buffers every frame waiting for it, so an output-side seek would feed the
    // graph everything from the cut to the end of the file -- minutes of work,
    // hundreds of megabytes, and a palette built from footage the clip does not
    // contain.
    let options = Options {
        fmt: Format::Gif,
        ..Default::default()
    };
    let plan = plan_for(&sample_info(), 45.0, 58.0, options);
    let input = index_of(&plan.args, "-i").unwrap();
    let seek = index_of(&plan.args, "-ss").unwrap();
    let length = index_of(&plan.args, "-t").unwrap();
    assert!(seek < input, "the seek must be an input option");
    assert!(length < input, "the length must be an input option");
    assert_eq!(value_after(&plan.args, "-ss").as_deref(), Some("45.000000"));
}

#[test]
fn gif_does_not_fight_the_filter_chain() {
    let options = Options {
        fmt: Format::Gif,
        ..Default::default()
    };
    let plan = plan_for(&sample_info(), 45.0, 58.0, options);
    // paletteuse emits pal8 and the fps filter sets the rate, so pinning any of
    // these would fight the chain.
    for flag in ["-pix_fmt", "-g", "-fps_mode", "-c:a", "-map"] {
        assert!(!has(&plan.args, flag), "{flag} should not be set for a GIF");
    }
    assert!(has(&plan.args, "-an"));
}

#[test]
fn webm_uses_vp9() {
    let options = Options {
        fmt: Format::WebM,
        ..Default::default()
    };
    let plan = plan_for(&sample_info(), 45.0, 58.0, options);
    assert_eq!(
        value_after(&plan.args, "-c:v").as_deref(),
        Some("libvpx-vp9")
    );
}

#[test]
fn webm_copies_opus_rather_than_re_encoding_it() {
    let options = Options {
        fmt: Format::WebM,
        ..Default::default()
    };
    let plan = plan_for(&sample_info(), 45.0, 58.0, options);
    assert_eq!(value_after(&plan.args, "-c:a").as_deref(), Some("copy"));
}

#[test]
fn webm_converts_audio_it_cannot_carry() {
    // WebM carries only Opus or Vorbis, so AAC has to be converted.
    let info = info_with(|i| i.audio[0].codec = "aac".into());
    let options = Options {
        fmt: Format::WebM,
        ..Default::default()
    };
    let plan = plan_for(&info, 45.0, 58.0, options);
    assert_eq!(value_after(&plan.args, "-c:a").as_deref(), Some("libopus"));
}

#[test]
fn faststart_only_for_the_mp4_family() {
    for (ext, wanted) in [
        ("mp4", true),
        ("mov", true),
        ("m4v", true),
        ("mkv", false),
        ("webm", false),
    ] {
        let plan = plan_for(
            &info_with(|i| i.ext = ext.into()),
            45.0,
            58.0,
            Options::default(),
        );
        assert_eq!(
            contains_sequence(&plan.args, &["-movflags", "+faststart"]),
            wanted,
            "faststart for .{ext}"
        );
    }
}

// ----- WebM rate control --------------------------------------------------

fn webm_plan(options: Options) -> Plan {
    plan_for(&sample_info(), 45.0, 59.0, options)
}

#[test]
fn a_webm_size_target_sets_a_bitrate() {
    let plan = webm_plan(Options {
        fmt: Format::WebM,
        target_bytes: 10_000_000,
        ..Default::default()
    });
    let rate = number(&plan.args, "-b:v");
    assert!(rate > 0.0, "a target must produce a real bitrate");
}

#[test]
fn a_webm_size_target_drops_constant_quality() {
    // libvpx reads a CRF next to `-b:v 0` as "constant quality, ignore the
    // bitrate", which is how a size target asked for here used to be discarded
    // without a word.
    let plan = webm_plan(Options {
        fmt: Format::WebM,
        target_bytes: 10_000_000,
        ..Default::default()
    });
    assert!(
        !has(&plan.args, "-crf"),
        "a CRF beside a bitrate is ignored"
    );
}

#[test]
fn each_webm_preset_lowers_the_quality() {
    let crf_for = |quality| {
        number(
            &webm_plan(Options {
                fmt: Format::WebM,
                quality,
                ..Default::default()
            })
            .args,
            "-crf",
        )
    };
    assert!(crf_for(Quality::Source) < crf_for(Quality::Smaller));
    assert!(crf_for(Quality::Smaller) < crf_for(Quality::Smallest));
}

#[test]
fn a_webm_preset_stays_in_constant_quality_mode() {
    let plan = webm_plan(Options {
        fmt: Format::WebM,
        quality: Quality::Smaller,
        ..Default::default()
    });
    assert_eq!(value_after(&plan.args, "-b:v").as_deref(), Some("0"));
}

// ----- a size target keeps the codec --------------------------------------

#[test]
fn the_encoder_is_the_same_with_and_without_a_target() {
    for codec in ["h264", "hevc", "vp9", "av1"] {
        let info = info_with(|i| {
            i.v_codec = codec.into();
            i.profile = String::new();
        });
        let plain = export::encoder_label(&info, &Options::default());
        let targeted = export::encoder_label(
            &info,
            &Options {
                target_bytes: 10_000_000,
                ..Default::default()
            },
        );
        assert_eq!(
            plain, targeted,
            "a size target changed the codec for {codec}"
        );
    }
}

#[test]
fn a_vp9_source_stays_vp9_under_a_target() {
    // For a .webm source this is not a silent codec swap but a muxer refusing
    // the file, so the export failed outright.
    let info = info_with(|i| {
        i.v_codec = "vp9".into();
        i.ext = "webm".into();
        i.profile = String::new();
    });
    let plan = plan_for(
        &info,
        45.0,
        59.0,
        Options {
            target_bytes: 10_000_000,
            ..Default::default()
        },
    );
    assert_eq!(
        value_after(&plan.args, "-c:v").as_deref(),
        Some("libvpx-vp9")
    );
}

// ----- the size-target ceiling -------------------------------------------
// Regression cover for a 10 MB target that produced 10.3 MB. Solving the
// average bitrate from the budget is not enough: single-pass ABR overshoots on
// hard content and sits at -maxrate, so the *cap* is what decides whether the
// file fits. The invariant is that an encoder pinned at maxrate for the whole
// clip would still come in under the limit.

const CEILING_DURATION: f64 = 14.0;

fn worst_case_bytes(info: &MediaInfo, target_mb: i64) -> f64 {
    let options = Options {
        target_bytes: target_mb * 1_000_000,
        ..Default::default()
    };
    let plan = plan_for(info, 45.0, 45.0 + CEILING_DURATION, options);
    let maxrate = number(&plan.args, "-maxrate");
    let audio: i64 = info
        .audio
        .iter()
        .map(|t| if t.bitrate > 0 { t.bitrate } else { 128_000 })
        .sum();
    (maxrate + audio as f64) * CEILING_DURATION / 8.0
}

#[test]
fn the_ceiling_fits_the_budget() {
    let info = sample_info();
    for mb in [10, 25, 50, 100] {
        assert!(
            worst_case_bytes(&info, mb) <= (mb * 1_000_000) as f64,
            "an encoder pinned at -maxrate would exceed {mb} MB"
        );
    }
}

#[test]
fn the_ceiling_fits_with_no_audio_to_subtract() {
    let info = info_with(|i| i.audio.clear());
    assert!(worst_case_bytes(&info, 10) <= 10_000_000.0);
}

#[test]
fn the_ceiling_fits_with_several_loud_tracks() {
    let info = info_with(|i| {
        i.audio = vec![
            AudioTrack {
                index: 0,
                codec: "opus".into(),
                bitrate: 320_000,
                sample_rate: 48000,
                channels: 2,
                language: String::new(),
                title: String::new(),
            },
            AudioTrack {
                index: 1,
                codec: "opus".into(),
                bitrate: 256_000,
                sample_rate: 48000,
                channels: 2,
                language: String::new(),
                title: String::new(),
            },
        ];
    });
    assert!(worst_case_bytes(&info, 10) <= 10_000_000.0);
}

#[test]
fn the_buffer_cannot_bank_an_overshoot() {
    // A VBV buffer bigger than the rate lets the encoder average above the cap
    // on short clips, which is how the original overshoot happened.
    let plan = plan_for(
        &sample_info(),
        45.0,
        59.0,
        Options {
            target_bytes: 10_000_000,
            ..Default::default()
        },
    );
    assert!(number(&plan.args, "-bufsize") <= number(&plan.args, "-b:v"));
}

#[test]
fn no_cap_is_left_unset() {
    let plan = plan_for(
        &sample_info(),
        45.0,
        59.0,
        Options {
            target_bytes: 10_000_000,
            ..Default::default()
        },
    );
    assert!(
        value_after(&plan.args, "-maxrate").is_some(),
        "a size target without a cap is only a suggestion"
    );
}

// ----- quality presets ----------------------------------------------------

fn preset_args(quality: Quality) -> Vec<String> {
    plan_for(
        &sample_info(),
        45.0,
        59.0,
        Options {
            quality,
            ..Default::default()
        },
    )
    .args
}

#[test]
fn source_quality_caps_at_the_source_bitrate() {
    let args = preset_args(Quality::Source);
    assert_eq!(number(&args, "-maxrate"), sample_info().v_bitrate as f64);
}

#[test]
fn each_step_down_lowers_the_ceiling() {
    let ceiling = |q| number(&preset_args(q), "-maxrate");
    assert!(ceiling(Quality::Source) > ceiling(Quality::Smaller));
    assert!(ceiling(Quality::Smaller) > ceiling(Quality::Smallest));
}

#[test]
fn each_step_down_raises_crf() {
    let crf = |q| number(&preset_args(q), "-crf");
    assert!(crf(Quality::Source) < crf(Quality::Smaller));
    assert!(crf(Quality::Smaller) < crf(Quality::Smallest));
}

#[test]
fn the_presets_deliver_what_their_labels_promise() {
    // "About half" and "roughly a third", measured against the source itself
    // rather than against a fixed number.
    let source = sample_info().v_bitrate as f64;
    let share = |q| number(&preset_args(q), "-maxrate") / source;
    assert!((share(Quality::Source) - 1.0).abs() < 0.01);
    assert!((share(Quality::Smaller) - 0.5).abs() < 0.01);
    assert!((share(Quality::Smallest) - 0.3).abs() < 0.01);
}

#[test]
fn the_ceiling_is_a_share_of_this_file_not_a_fixed_number() {
    let small = info_with(|i| i.v_bitrate = 2_000_000);
    let large = info_with(|i| i.v_bitrate = 40_000_000);
    let ceiling = |info: &MediaInfo| {
        number(
            &plan_for(
                info,
                45.0,
                59.0,
                Options {
                    quality: Quality::Smaller,
                    ..Default::default()
                },
            )
            .args,
            "-maxrate",
        )
    };
    assert!((ceiling(&small) - 1_000_000.0).abs() < 1.0);
    assert!((ceiling(&large) - 20_000_000.0).abs() < 1.0);
}

#[test]
fn b_frames_still_inherited_at_every_preset() {
    for quality in [Quality::Source, Quality::Smaller, Quality::Smallest] {
        let info = info_with(|i| i.has_b_frames = 2);
        let plan = plan_for(
            &info,
            45.0,
            59.0,
            Options {
                quality,
                ..Default::default()
            },
        );
        assert_eq!(value_after(&plan.args, "-bf").as_deref(), Some("2"));
    }
}

#[test]
fn hevc_offsets_from_its_own_baseline() {
    let info = info_with(|i| {
        i.v_codec = "hevc".into();
        i.profile = "Main".into();
    });
    let crf = |quality| {
        number(
            &plan_for(
                &info,
                45.0,
                59.0,
                Options {
                    quality,
                    ..Default::default()
                },
            )
            .args,
            "-crf",
        )
    };
    assert_eq!(crf(Quality::Source), export::crf_for("hevc") as f64);
    assert_eq!(
        crf(Quality::Smaller),
        (export::crf_for("hevc") + Quality::Smaller.crf_offset()) as f64
    );
}

#[test]
fn a_reduced_preset_rules_out_a_stream_copy() {
    for quality in [Quality::Smaller, Quality::Smallest] {
        let plan = plan_for(
            &sample_info(),
            44.000004,
            58.0,
            Options {
                quality,
                ..Default::default()
            },
        );
        assert!(!plan.lossless, "a copy cannot be smaller than the source");
    }
}

#[test]
fn source_quality_still_allows_a_copy() {
    assert!(plain(44.000004, 58.0).lossless);
}

#[test]
fn inherits_everything_is_exact_about_what_it_claims() {
    assert!(Options::default().inherits_everything());
    assert!(!Options {
        quality: Quality::Smaller,
        ..Default::default()
    }
    .inherits_everything());
    assert!(!Options {
        target_bytes: 1,
        ..Default::default()
    }
    .inherits_everything());
    assert!(!Options {
        fmt: Format::WebM,
        ..Default::default()
    }
    .inherits_everything());
    // Audio selection is not part of the picture, so it does not rule a copy
    // out: dropping a track is still a copy of the ones that are kept.
    assert!(Options {
        audio: Audio::None,
        ..Default::default()
    }
    .inherits_everything());
}

// ----- size estimates -----------------------------------------------------

fn estimate(options: Options, duration: f64) -> i64 {
    export::estimate_bytes(&sample_info(), &options, duration)
}

#[test]
fn source_quality_estimates_near_the_source_bitrate() {
    let info = sample_info();
    let expected = (info.v_bitrate + info.total_audio_bitrate()) as f64 * 10.0 / 8.0;
    let got = estimate(Options::default(), 10.0) as f64;
    assert!((got - expected).abs() / expected < 0.01);
}

#[test]
fn smaller_presets_estimate_smaller() {
    let for_quality = |quality| {
        estimate(
            Options {
                quality,
                ..Default::default()
            },
            10.0,
        )
    };
    assert!(for_quality(Quality::Source) > for_quality(Quality::Smaller));
    assert!(for_quality(Quality::Smaller) > for_quality(Quality::Smallest));
}

#[test]
fn a_size_target_estimates_just_under_the_limit() {
    let got = estimate(
        Options {
            target_bytes: 10_000_000,
            ..Default::default()
        },
        10.0,
    );
    assert!(got < 10_000_000 && got > 8_000_000, "{got}");
}

#[test]
fn muting_audio_lowers_the_estimate() {
    let with_audio = estimate(Options::default(), 10.0);
    let muted = estimate(
        Options {
            audio: Audio::None,
            ..Default::default()
        },
        10.0,
    );
    assert!(muted < with_audio);
}

#[test]
fn the_webm_estimate_tracks_the_preset_too() {
    let for_quality = |quality| {
        estimate(
            Options {
                fmt: Format::WebM,
                quality,
                ..Default::default()
            },
            10.0,
        )
    };
    assert!(for_quality(Quality::Source) > for_quality(Quality::Smaller));
    // VP9 lands well under an equivalent H.264 bitrate.
    assert!(for_quality(Quality::Source) < estimate(Options::default(), 10.0));
}

#[test]
fn gif_declines_to_guess() {
    assert_eq!(
        estimate(
            Options {
                fmt: Format::Gif,
                ..Default::default()
            },
            10.0
        ),
        0
    );
}

#[test]
fn a_zero_length_selection_estimates_nothing() {
    assert_eq!(estimate(Options::default(), 0.0), 0);
}

// ----- which encoder ------------------------------------------------------

#[test]
fn maps_each_codec_to_its_encoder() {
    for (codec, encoder) in [
        ("h264", "libx264"),
        ("hevc", "libx265"),
        ("vp9", "libvpx-vp9"),
    ] {
        let info = info_with(|i| i.v_codec = codec.into());
        assert_eq!(export::encoder_label(&info, &Options::default()), encoder);
    }
}

#[test]
fn an_unknown_codec_lands_on_something_universal() {
    let info = info_with(|i| i.v_codec = "prores".into());
    assert_eq!(export::encoder_label(&info, &Options::default()), "libx264");
}

#[test]
fn av1_stays_av1() {
    // Which AV1 encoder depends on whether this machine has a render node, but
    // it must never become H.264 -- a codec swap behind the user's back.
    let info = info_with(|i| i.v_codec = "av1".into());
    let label = export::encoder_label(&info, &Options::default());
    assert!(label.contains("av1"), "{label}");
}

// ----- progress -----------------------------------------------------------

#[test]
fn asks_ffmpeg_for_machine_readable_progress() {
    let plan = plain(45.0, 58.0);
    assert!(contains_sequence(&plan.args, &["-progress", "pipe:1"]));
    assert!(has(&plan.args, "-nostats"));
    // Default stats come once a second, which makes the bar lurch.
    assert_eq!(
        value_after(&plan.args, "-stats_period").as_deref(),
        Some("0.25")
    );
}

#[test]
fn progress_is_read_as_a_fraction_of_the_clip() {
    let event =
        export::parse_progress("out_time_us=7000000\nspeed=1.50x\n", 14.0).expect("a position");
    match event {
        export::ExportEvent::Progress {
            fraction,
            speed,
            eta,
        } => {
            assert!((fraction - 0.5).abs() < 1e-6);
            assert!((speed - 1.5).abs() < 1e-6);
            // 7 seconds left at 1.5x.
            assert!((eta - 7.0 / 1.5).abs() < 1e-6);
        }
        other => panic!("expected progress, got {other:?}"),
    }
}

#[test]
fn a_block_with_no_position_reports_nothing() {
    assert!(export::parse_progress("speed=1.50x\nbitrate=N/A\n", 14.0).is_none());
}

#[test]
fn progress_never_leaves_the_range() {
    // ffmpeg can report a position past the end on the final block.
    let event = export::parse_progress("out_time_us=99000000\n", 14.0).unwrap();
    match event {
        export::ExportEvent::Progress { fraction, eta, .. } => {
            assert!((fraction - 1.0).abs() < 1e-9);
            assert!(eta >= 0.0);
        }
        other => panic!("expected progress, got {other:?}"),
    }
}

// ----- the output never becomes an option --------------------------------

#[test]
fn the_destination_is_the_last_argument() {
    for options in [
        Options::default(),
        Options {
            fmt: Format::WebM,
            ..Default::default()
        },
        Options {
            fmt: Format::Gif,
            ..Default::default()
        },
    ] {
        let plan = plan_for(&sample_info(), 45.0, 58.0, options);
        assert_eq!(plan.args.last().map(String::as_str), Some("/tmp/o.mp4"));
    }
}

#[test]
fn every_run_overwrites_deliberately_and_reads_no_stdin() {
    // -nostdin matters: without it ffmpeg competes for the terminal when the
    // app is started from one, and a stray keypress cancels an export.
    let plan = plain(45.0, 58.0);
    assert!(has(&plan.args, "-y"));
    assert!(has(&plan.args, "-nostdin"));
    assert!(has(&plan.args, "-hide_banner"));
}

// ----- deinterlacing ------------------------------------------------------

fn deinterlaced(method: Method) -> Options {
    Options {
        deinterlace: Some(Deinterlace {
            method,
            parity: Parity::Tff,
        }),
        ..Options::default()
    }
}

#[test]
fn bwdif_goes_first_in_the_filter_chain() {
    let plan = plan_for(&sample_info(), 45.0, 58.0, deinterlaced(Method::BWDIF));
    assert_eq!(
        value_after(&plan.args, "-vf").as_deref(),
        Some("bwdif=mode=send_field:parity=tff:deint=all")
    );
    assert!(plan.label.contains("BWDIF"), "{}", plan.label);
    assert!(plan.feed.is_none());
}

#[test]
fn a_deinterlaced_cut_on_a_keyframe_is_still_a_re_encode() {
    // The cut starts on a keyframe, which alone would be a stream copy.
    let plan = plan_for(&sample_info(), 2.0, 4.0, deinterlaced(Method::BWDIF));
    assert!(!plan.lossless);
    assert!(value_after(&plan.args, "-c").as_deref() != Some("copy"));
}

#[test]
fn double_rate_doubles_the_keyframe_interval_in_frames() {
    let single = plan_for(&sample_info(), 45.0, 58.0, Options::default());
    let double = plan_for(&sample_info(), 45.0, 58.0, deinterlaced(Method::BWDIF));
    let fast = plan_for(
        &sample_info(),
        45.0,
        58.0,
        deinterlaced(Method::Bwdif(BwdifProfile::Fast)),
    );
    assert_eq!(number(&double.args, "-g"), number(&single.args, "-g") * 2.0);
    assert_eq!(number(&fast.args, "-g"), number(&single.args, "-g"));
}

#[test]
fn bwdif_joins_the_front_of_an_existing_chain() {
    let gif = Options {
        fmt: Format::Gif,
        ..deinterlaced(Method::BWDIF)
    };
    let plan = plan_for(&sample_info(), 45.0, 48.0, gif);
    let chain = value_after(&plan.args, "-filter_complex").expect("the palette chain");
    assert!(chain.starts_with("bwdif="), "{chain}");
    assert!(chain.contains("palettegen"), "{chain}");
    assert!(!has(&plan.args, "-vf"), "one chain, not two");
}

#[test]
fn qtgmc_reads_its_picture_from_vspipe_and_its_sound_from_the_source() {
    let plan = plan_for(&sample_info(), 45.0, 58.0, deinterlaced(Method::QTGMC));
    let feed = plan.feed.as_ref().expect("QTGMC runs in vspipe");
    assert!(contains_sequence(
        &plan.args,
        &["-f", "yuv4mpegpipe", "-i", "pipe:0"]
    ));
    assert_eq!(
        index_of(&plan.args, "pipe:0")
            .map(|i| i < index_of(&plan.args, "/tmp/sample.mp4").unwrap()),
        Some(true),
        "the picture is input 0, the source input 1"
    );
    assert!(contains_sequence(&plan.args, &["-map", "0:v:0"]));
    assert!(contains_sequence(&plan.args, &["-map", "1:a"]));
    assert!(
        !has(&plan.args, "-vf"),
        "no ffmpeg deinterlace on top of QTGMC"
    );
    // 45s to 58s of a 60 fps source: frames 2700 to 3479.
    assert!(
        feed.script.contains("first, last = 2700, min(3479,"),
        "{}",
        feed.script
    );
    assert!(feed.script.contains(r#"Preset="Placebo""#));
    assert_eq!(feed.args, vec!["-c", "y4m", feed.script_path.as_str(), "-"]);
    assert!(plan.label.contains("QTGMC"));
}

#[test]
fn qtgmc_without_sound_opens_no_second_input() {
    let options = Options {
        audio: Audio::None,
        ..deinterlaced(Method::QTGMC)
    };
    let plan = plan_for(&sample_info(), 45.0, 58.0, options);
    assert!(!plan.args.iter().any(|arg| arg == "/tmp/sample.mp4"));
    assert!(!plan.args.iter().any(|arg| arg.starts_with("1:a")));
}
