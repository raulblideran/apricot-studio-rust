// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! Pure-function tests. No ffmpeg, no window, no disk beyond temporary files.

mod common;

use apricot_studio::app::{delete_candidate, delete_prompt, Exported, MIN_CLIP_BYTES, VERSION};
use apricot_studio::export::{self, Audio, Format, Options, Quality};
use apricot_studio::media::{self, AudioTrack};
use apricot_studio::paths;
use apricot_studio::ui::chrome;
use common::sample_info;

// ----- timecode -----------------------------------------------------------

#[test]
fn formats_with_milliseconds() {
    assert_eq!(media::fmt_tc(0.0), "00:00:00.000");
    assert_eq!(media::fmt_tc(42.5), "00:00:42.500");
    assert_eq!(media::fmt_tc(3661.25), "01:01:01.250");
}

#[test]
fn negative_clamps_to_zero() {
    assert_eq!(media::fmt_tc(-5.0), "00:00:00.000");
}

#[test]
fn timecodes_round_trip() {
    for value in [0.0, 1.5, 42.5, 90.966, 3661.25] {
        let back = media::parse_tc(&media::fmt_tc(value)).expect("should parse");
        assert!(
            (back - value).abs() < 0.001,
            "round trip failed for {value}: got {back}"
        );
    }
}

#[test]
fn accepts_short_forms() {
    assert_eq!(media::parse_tc("7.5"), Some(7.5));
    assert_eq!(media::parse_tc("1:02.25"), Some(62.25));
    assert_eq!(media::parse_tc(" 00:00:42.500 "), Some(42.5));
}

#[test]
fn rejects_nonsense() {
    for bad in ["", "   ", "junk", "1:2:3:4", "aa:bb"] {
        assert_eq!(media::parse_tc(bad), None, "{bad:?} should not parse");
    }
}

// ----- track labels -------------------------------------------------------

#[test]
fn translates_pipewire_device_names() {
    // gpu-screen-recorder names tracks after the nodes it captured.
    assert_eq!(
        media::clean_title("device:default_output|device:default_input"),
        "Desktop + Mic"
    );
    assert_eq!(media::clean_title("device:default_output"), "Desktop");
}

#[test]
fn leaves_ordinary_titles_alone() {
    assert_eq!(media::clean_title("Commentary"), "Commentary");
    assert_eq!(media::clean_title(""), "");
}

#[test]
fn truncates_runaway_titles() {
    assert!(media::clean_title(&"x".repeat(200)).chars().count() <= 32);
}

#[test]
fn label_describes_the_track() {
    let track = AudioTrack {
        index: 0,
        codec: "opus".into(),
        bitrate: 106_489,
        sample_rate: 48000,
        channels: 2,
        language: "eng".into(),
        title: "Desktop".into(),
    };
    assert_eq!(track.label(), "Track 1 · Desktop — Opus stereo 106k");
}

#[test]
fn label_survives_missing_metadata() {
    // Matroska keeps no per-stream bitrate, so this is the common case there.
    let bare = AudioTrack {
        index: 1,
        codec: "opus".into(),
        bitrate: 0,
        sample_rate: 48000,
        channels: 0,
        language: String::new(),
        title: String::new(),
    };
    assert_eq!(bare.label(), "Track 2 — Opus");
}

// ----- what a file is -----------------------------------------------------

#[test]
fn audio_helpers() {
    let info = sample_info();
    assert!(info.has_audio());
    assert_eq!(info.a_codec(), "opus");
    assert_eq!(info.total_audio_bitrate(), 106_489);
}

#[test]
fn no_audio() {
    let info = MediaInfoBuilder::new().audio(vec![]).build();
    assert!(!info.has_audio());
    assert_eq!(info.a_codec(), "");
    assert_eq!(info.total_audio_bitrate(), 0);
}

#[test]
fn totals_every_track() {
    let info = MediaInfoBuilder::new()
        .audio(vec![track(0, 100_000), track(1, 50_000)])
        .build();
    assert_eq!(info.total_audio_bitrate(), 150_000);
}

#[test]
fn frame_duration() {
    assert!((sample_info().frame_duration() - 1.0 / 60.0).abs() < 1e-9);
    // A file that reports no frame rate must not divide by zero.
    assert!(MediaInfoBuilder::new().fps(0.0).build().frame_duration() > 0.0);
}

#[test]
fn profile_translated_for_the_encoder() {
    assert_eq!(
        MediaInfoBuilder::new()
            .profile("High")
            .build()
            .encoder_profile(),
        "high"
    );
    assert_eq!(
        MediaInfoBuilder::new()
            .profile("High 10")
            .build()
            .encoder_profile(),
        "high10"
    );
    assert_eq!(
        MediaInfoBuilder::new()
            .codec("hevc")
            .profile("Main 10")
            .build()
            .encoder_profile(),
        "main10"
    );
}

#[test]
fn unknown_profile_is_omitted_rather_than_guessed() {
    assert_eq!(
        MediaInfoBuilder::new()
            .codec("av1")
            .profile("Main")
            .build()
            .encoder_profile(),
        ""
    );
}

#[test]
fn summary_reads_as_a_sentence() {
    let summary = sample_info().summary();
    assert!(summary.contains("2560×1440"), "{summary}");
    assert!(summary.contains("60fps"), "{summary}");
    assert!(summary.contains("H.264"), "{summary}");
    assert!(summary.contains("Opus"), "{summary}");
}

// ----- output naming ------------------------------------------------------

#[test]
fn adds_clip_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("replay.mp4");
    std::fs::write(&src, b"").unwrap();
    assert_eq!(
        export::default_output(&src.to_string_lossy(), None),
        dir.path().join("replay_clip.mp4").to_string_lossy()
    );
}

#[test]
fn never_overwrites_an_existing_clip() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("replay.mp4");
    for name in ["replay.mp4", "replay_clip.mp4", "replay_clip2.mp4"] {
        std::fs::write(dir.path().join(name), b"").unwrap();
    }
    assert_eq!(
        export::default_output(&src.to_string_lossy(), None),
        dir.path().join("replay_clip3.mp4").to_string_lossy()
    );
}

#[test]
fn the_extension_follows_the_format() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("replay.mp4").to_string_lossy().into_owned();
    assert!(export::default_output(&src, Some("webm")).ends_with("replay_clip.webm"));
    assert!(export::default_output(&src, Some("gif")).ends_with("replay_clip.gif"));
}

// ----- keyframes ----------------------------------------------------------

#[test]
fn matches_only_a_keyframe_it_is_practically_on() {
    let keyframes = [0.0, 2.0, 4.000004, 6.0];
    assert_eq!(export::on_keyframe(4.0, &keyframes), Some(4.000004));
    assert_eq!(export::on_keyframe(4.5, &keyframes), None);
}

#[test]
fn matching_returns_the_stored_time_not_the_asked_one() {
    // ffmpeg seeks to the keyframe at or before the time it is given, so a
    // request for 44.0 when the keyframe sits at 44.000004 rewinds a whole GOP.
    let keyframes = [44.000004];
    assert_eq!(export::on_keyframe(44.0, &keyframes), Some(44.000004));
}

#[test]
fn snapping_moves_to_the_nearest_within_tolerance() {
    let keyframes = [0.0, 2.0, 4.0];
    assert_eq!(export::snap_to_keyframe(2.4, &keyframes, 1.0), 2.0);
    assert_eq!(export::snap_to_keyframe(2.4, &keyframes, 0.1), 2.4);
}

// ----- size arithmetic ----------------------------------------------------

#[test]
fn a_size_target_solves_a_bitrate_that_fits() {
    let info = sample_info();
    let options = Options {
        target_bytes: 10_000_000,
        ..Default::default()
    };
    let duration = 30.0;
    let video = export::target_video_bitrate(&info, &options, duration);
    let audio = info.total_audio_bitrate();
    let total_bytes = (video + audio) as f64 * duration / 8.0;
    assert!(
        total_bytes <= 10_000_000.0,
        "solved bitrate would overshoot: {total_bytes}"
    );
}

#[test]
fn the_estimate_is_close_to_the_target() {
    let info = sample_info();
    let options = Options {
        target_bytes: 25_000_000,
        ..Default::default()
    };
    let estimate = export::estimate_bytes(&info, &options, 30.0);
    assert_eq!(estimate, (25_000_000.0 * export::SIZE_MARGIN) as i64);
}

#[test]
fn the_estimate_scales_with_the_selection() {
    let info = sample_info();
    let options = Options::default();
    let short = export::estimate_bytes(&info, &options, 5.0);
    let long = export::estimate_bytes(&info, &options, 10.0);
    assert!((long as f64 / short as f64 - 2.0).abs() < 0.01);
}

// ----- the chamfer --------------------------------------------------------

#[test]
fn the_notch_has_six_corners() {
    let shape = chrome::chamfer_polygon(100.0, 30.0, 7.0);
    assert_eq!(shape.len(), 6);
}

#[test]
fn the_notch_cuts_the_two_opposite_corners() {
    let shape = chrome::chamfer_polygon(100.0, 30.0, 7.0);
    // Top-left and bottom-right are cut; the other two are square.
    assert!(shape.iter().any(|p| p.x == 100.0 && p.y == 0.0));
    assert!(shape.iter().any(|p| p.x == 0.0 && p.y == 30.0));
    assert!(!shape.iter().any(|p| p.x == 0.0 && p.y == 0.0));
    assert!(!shape.iter().any(|p| p.x == 100.0 && p.y == 30.0));
}

#[test]
fn a_notch_larger_than_the_button_is_clamped() {
    // A cut bigger than the shape stops being a notch and starts being a
    // triangle, so it is held to half the shorter side.
    let shape = chrome::chamfer_polygon(40.0, 20.0, 500.0);
    assert!(shape.iter().all(|p| p.x >= 0.0 && p.x <= 40.0));
    assert!(shape.iter().all(|p| p.y >= 0.0 && p.y <= 20.0));
}

#[test]
fn a_zero_cut_is_an_ordinary_rectangle() {
    let shape = chrome::chamfer_polygon(50.0, 20.0, 0.0);
    assert!(shape.iter().any(|p| p.x == 0.0 && p.y == 0.0));
    assert!(shape.iter().any(|p| p.x == 50.0 && p.y == 20.0));
}

// ----- deleting the original ---------------------------------------------

fn exported_pair(clip_bytes: usize) -> (tempfile::TempDir, Exported) {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("replay.mp4");
    let clip = dir.path().join("replay_clip.mp4");
    std::fs::write(&source, vec![0u8; 4096]).unwrap();
    std::fs::write(&clip, vec![0u8; clip_bytes]).unwrap();
    let record = Exported {
        source: source.to_string_lossy().into_owned(),
        output: clip.to_string_lossy().into_owned(),
        duration: 90.0,
        kept: 14.0,
    };
    (dir, record)
}

#[test]
fn offers_deletion_when_the_clip_is_real() {
    let (_dir, record) = exported_pair(MIN_CLIP_BYTES as usize + 1);
    assert!(delete_candidate(Some(&record)).is_some());
}

#[test]
fn refuses_when_the_clip_is_too_small_to_be_real() {
    let (_dir, record) = exported_pair(10);
    assert_eq!(delete_candidate(Some(&record)), None);
}

#[test]
fn refuses_when_there_was_no_export() {
    assert_eq!(delete_candidate(None), None);
}

#[test]
fn refuses_when_the_clip_is_the_source() {
    let (_dir, mut record) = exported_pair(MIN_CLIP_BYTES as usize + 1);
    record.output = record.source.clone();
    assert_eq!(delete_candidate(Some(&record)), None);
}

#[test]
fn refuses_when_the_source_is_already_gone() {
    let (_dir, record) = exported_pair(MIN_CLIP_BYTES as usize + 1);
    std::fs::remove_file(&record.source).unwrap();
    assert_eq!(delete_candidate(Some(&record)), None);
}

#[test]
fn the_prompt_says_what_is_being_thrown_away() {
    let record = Exported {
        source: "/tmp/replay.mp4".into(),
        output: "/tmp/replay_clip.mp4".into(),
        duration: 90.0,
        kept: 14.0,
    };
    let (text, detail) = delete_prompt(&record, 110_000_000);
    assert!(text.contains("replay.mp4"), "{text}");
    assert!(detail.contains("110 MB"), "{detail}");
    assert!(detail.contains("14.0s"), "{detail}");
    assert!(detail.contains("16%"), "{detail}");
    assert!(detail.contains("84%"), "{detail}");
    // The clip that earned the offer is named, so the two files cannot be
    // confused at the moment one of them is about to be deleted.
    assert!(detail.contains("replay_clip.mp4"), "{detail}");
}

#[test]
fn the_prompt_reads_the_captured_file_not_the_open_one() {
    // The numbers come from the record, so an export finishing after another
    // file was opened still describes the file that was cut.
    let record = Exported {
        source: "/tmp/other.mp4".into(),
        output: "/tmp/other_clip.mp4".into(),
        duration: 10.0,
        kept: 5.0,
    };
    let (_, detail) = delete_prompt(&record, 1_000_000);
    assert!(detail.contains("00:00:10.000"), "{detail}");
    assert!(detail.contains("50%"), "{detail}");
}

// ----- paths --------------------------------------------------------------

#[test]
fn splitting_an_extension_ignores_dots_in_directories() {
    let (stem, ext) = paths::split_ext("/home/me/my.videos/clip");
    assert_eq!(stem, "/home/me/my.videos/clip");
    assert_eq!(ext, "");
}

#[test]
fn splitting_takes_the_last_extension() {
    let (stem, ext) = paths::split_ext("/tmp/replay.tar.mp4");
    assert_eq!(stem, "/tmp/replay.tar");
    assert_eq!(ext, ".mp4");
}

#[test]
fn a_dotfile_is_not_an_extension() {
    let (stem, ext) = paths::split_ext("/tmp/.hidden");
    assert_eq!(stem, "/tmp/.hidden");
    assert_eq!(ext, "");
}

#[test]
fn absolute_paths_compare_equal_however_they_are_written() {
    assert!(paths::same_file("/tmp/./clip.mp4", "/tmp/a/../clip.mp4"));
    assert!(!paths::same_file("/tmp/clip.mp4", "/tmp/other.mp4"));
}

// ----- packaging ----------------------------------------------------------

#[test]
fn the_version_matches_the_metainfo() {
    // The flatpak build reads the release list, so a release cannot go out
    // labelled as the one before it.
    let metainfo = include_str!("../packaging/io.github.raulblideran.ApricotStudio.metainfo.xml");
    let newest = metainfo
        .lines()
        .find_map(|line| {
            let line = line.trim();
            line.strip_prefix("<release version=\"")
                .and_then(|rest| rest.split('"').next())
                .map(|version| version.to_string())
        })
        .expect("the metainfo should list a release");
    assert_eq!(newest, VERSION, "metainfo and Cargo.toml disagree");
}

#[test]
fn the_desktop_entry_names_the_binary_and_the_icon() {
    let entry = include_str!("../packaging/io.github.raulblideran.ApricotStudio.desktop");
    assert!(entry.contains("Exec=apricot-studio %f"), "{entry}");
    assert!(
        entry.contains("Icon=io.github.raulblideran.ApricotStudio"),
        "{entry}"
    );
    // Opening a video from the file manager is how most people will reach it.
    assert!(entry.contains("MimeType=video/"), "{entry}");
}

#[test]
fn the_flatpak_manifest_installs_what_the_app_needs() {
    let manifest = include_str!("../packaging/io.github.raulblideran.ApricotStudio.yaml");
    for needed in [
        "apricot-studio",
        "io.github.raulblideran.ApricotStudio.desktop",
        "io.github.raulblideran.ApricotStudio.metainfo.xml",
        "io.github.raulblideran.ApricotStudio.svg",
    ] {
        assert!(
            manifest.contains(needed),
            "manifest never mentions {needed}"
        );
    }
    // The wastebasket the delete-original option writes into, and the sockets
    // the preview needs.
    assert!(manifest.contains("--filesystem=~/.local/share/Trash:create"));
    assert!(manifest.contains("--socket=pulseaudio"));
}

#[test]
fn every_format_and_quality_choice_is_offered_exactly_once() {
    use apricot_studio::app::QUALITY_CHOICES;
    let labels: Vec<&str> = QUALITY_CHOICES.iter().map(|c| c.label).collect();
    let mut sorted = labels.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), labels.len(), "a quality label is repeated");
    // The four size targets people actually run into.
    for target in [10_000_000, 25_000_000, 50_000_000, 100_000_000] {
        assert!(
            QUALITY_CHOICES.iter().any(|c| c.target == target),
            "no choice fits {target} bytes"
        );
    }
    // Only "Same as source" may be a lossless copy.
    let inherits = |choice: &apricot_studio::app::QualityChoice| {
        Options {
            fmt: Format::Source,
            audio: Audio::All,
            target_bytes: choice.target,
            quality: choice.quality,
        }
        .inherits_everything()
    };
    assert_eq!(
        QUALITY_CHOICES.iter().filter(|c| inherits(c)).count(),
        1,
        "exactly one choice should reproduce the source"
    );
    assert_eq!(QUALITY_CHOICES[0].quality, Quality::Source);
}

// ----- a small builder, so a test can vary one field ---------------------

pub struct MediaInfoBuilder(apricot_studio::media::MediaInfo);

impl Default for MediaInfoBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn track(index: usize, bitrate: i64) -> AudioTrack {
    AudioTrack {
        index,
        codec: "opus".into(),
        bitrate,
        sample_rate: 48000,
        channels: 2,
        language: String::new(),
        title: String::new(),
    }
}

impl MediaInfoBuilder {
    pub fn new() -> Self {
        Self(sample_info())
    }

    pub fn codec(mut self, codec: &str) -> Self {
        self.0.v_codec = codec.into();
        self
    }

    pub fn profile(mut self, profile: &str) -> Self {
        self.0.profile = profile.into();
        self
    }

    pub fn fps(mut self, fps: f64) -> Self {
        self.0.fps = fps;
        self
    }

    pub fn audio(mut self, audio: Vec<AudioTrack>) -> Self {
        self.0.audio = audio;
        self
    }

    pub fn build(self) -> apricot_studio::media::MediaInfo {
        self.0
    }
}
