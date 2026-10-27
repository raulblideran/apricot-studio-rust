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
use apricot_studio::codex::{self, Take};
use apricot_studio::deinterlace::{
    self, BwdifProfile, Deinterlace, Method, Parity, QtgmcPreset, Scan,
};
use apricot_studio::export::{self, Audio, Format, Options, Quality};
use apricot_studio::media::{self, AudioTrack};
use apricot_studio::paths;
use apricot_studio::platform;
use apricot_studio::ui::{chrome, timeline};
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

#[test]
fn keyframes_come_from_packet_flags_without_decoding() {
    // ffprobe's packet lines: pts_time, then flags. K is a keyframe, D a
    // discardable packet; a line with no time (N/A) is skipped.
    let text = "0.000000,K__\n0.033333,___\n2.000000,K_D\nN/A,K__\n1.000000,K__\n2.000000,K__\n";
    assert_eq!(media::keyframes_from_packets(text), vec![0.0, 1.0, 2.0]);
}

// ----- the filmstrip ------------------------------------------------------

#[test]
fn the_filmstrip_asks_for_a_fixed_number_of_frames_however_long_the_file() {
    // Two hours, all-intra or not: the budget, never one per keyframe.
    assert_eq!(media::thumb_schedule(7200.0).len(), media::MAX_THUMBS);
    // A short clip is not asked for more frames than it has half-seconds.
    assert_eq!(media::thumb_schedule(3.0).len(), 6);
    assert_eq!(media::thumb_schedule(0.1).len(), 1);
    assert!(media::thumb_schedule(0.0).is_empty());
    assert!(media::thumb_schedule(f64::NAN).is_empty());
}

#[test]
fn the_filmstrip_covers_the_file_from_zero_and_never_repeats() {
    for duration in [3.0, 37.5, 90.0, 7200.0] {
        let mut times = media::thumb_schedule(duration);
        let count = times.len();
        assert_eq!(times[0], 0.0, "the first frame fetched starts the strip");
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        times.dedup();
        assert_eq!(times.len(), count, "every slot exactly once");
        assert!(times.iter().all(|t| (0.0..duration).contains(t)));
        let step = duration / count as f64;
        for pair in times.windows(2) {
            assert!((pair[1] - pair[0] - step).abs() < 1e-9, "even slots");
        }
    }
}

#[test]
fn the_filmstrip_fills_coarse_to_fine() {
    // The first frames fetched are spread across the file, not bunched at the
    // start: after four, every quarter of a 90s file has one.
    let first: Vec<f64> = media::thumb_schedule(90.0).into_iter().take(4).collect();
    for quarter in 0..4 {
        let (lo, hi) = (quarter as f64 * 22.5, (quarter + 1) as f64 * 22.5);
        assert!(
            first.iter().any(|t| (lo..hi).contains(t)),
            "no early frame in {lo}..{hi}: {first:?}"
        );
    }
}

#[test]
fn a_filmstrip_frame_is_one_keyframe_on_one_thread() {
    let args = media::thumbnail_args("/v/a.mp4", 12.5);
    assert_eq!(
        common::value_after(&args, "-frames:v").as_deref(),
        Some("1")
    );
    assert_eq!(
        common::value_after(&args, "-skip_frame").as_deref(),
        Some("nokey")
    );
    assert_eq!(common::value_after(&args, "-ss").as_deref(), Some("12.500"));
    assert!(common::has(&args, "-noaccurate_seek"));
    assert!(args.iter().filter(|a| *a == "-threads").count() >= 1);
    assert!(
        common::index_of(&args, "-ss") < common::index_of(&args, "-i"),
        "an input seek, so only the keyframe there is decoded"
    );
}

#[test]
fn the_strip_keeps_frames_in_time_order_as_they_arrive() {
    let ctx = egui::Context::default();
    let mut strip = timeline::Timeline::default();
    strip.reset(90.0);
    let image = || egui::ColorImage::new([2, 2], vec![egui::Color32::BLACK; 4]);
    for at in media::thumb_schedule(90.0) {
        let texture = ctx.load_texture("t", image(), egui::TextureOptions::LINEAR);
        strip.add_thumbnail(at, texture);
    }
    let times = strip.thumb_times();
    assert!(times.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(times.len(), media::thumb_schedule(90.0).len());
    strip.reset(10.0);
    assert!(strip.thumb_times().is_empty());
}

// ----- interlaced sources --------------------------------------------------

#[test]
fn the_field_order_is_read_by_its_first_letter() {
    let tff = Some(Scan::Interlaced(Parity::Tff));
    let bff = Some(Scan::Interlaced(Parity::Bff));
    assert_eq!(deinterlace::scan_from_field_order("tt"), tff);
    // What ffmpeg itself writes for top-field-first material.
    assert_eq!(deinterlace::scan_from_field_order("tb"), tff);
    assert_eq!(deinterlace::scan_from_field_order("bb"), bff);
    assert_eq!(deinterlace::scan_from_field_order("bt"), bff);
    assert_eq!(
        deinterlace::scan_from_field_order("progressive"),
        Some(Scan::Progressive)
    );
    // Not saying is not the same as saying progressive.
    assert_eq!(deinterlace::scan_from_field_order(""), None);
    assert_eq!(deinterlace::scan_from_field_order("unknown"), None);
}

#[test]
fn idet_is_read_from_its_last_multi_frame_line() {
    let stderr = "\
[Parsed_idet_0 @ 0x1] Repeated Fields: Neither:   0 Top:     0 Bottom:     0
[Parsed_idet_0 @ 0x1] Single frame detection: TFF:   3 BFF:   0 Progressive:   9 Undetermined:   1
[Parsed_idet_0 @ 0x1] Multi frame detection: TFF:     0 BFF:     0 Progressive:     0 Undetermined:     0
[Parsed_idet_0 @ 0x2] Multi frame detection: TFF:   162 BFF:     0 Progressive:     4 Undetermined:   12
";
    assert_eq!(
        deinterlace::scan_from_idet(stderr),
        Some(Scan::Interlaced(Parity::Tff))
    );
    let bff = "Multi frame detection: TFF:   10 BFF:   200 Progressive:   5 Undetermined:   0";
    assert_eq!(
        deinterlace::scan_from_idet(bff),
        Some(Scan::Interlaced(Parity::Bff))
    );
}

#[test]
fn a_scattering_of_interlaced_frames_does_not_make_a_film_interlaced() {
    let film = "Multi frame detection: TFF:   40 BFF:   12 Progressive:   240 Undetermined:   8";
    assert_eq!(deinterlace::scan_from_idet(film), Some(Scan::Progressive));
    // Nothing decided, or no line at all: no verdict, rather than a guess.
    let nothing = "Multi frame detection: TFF: 0 BFF: 0 Progressive: 0 Undetermined: 300";
    assert_eq!(deinterlace::scan_from_idet(nothing), None);
    assert_eq!(deinterlace::scan_from_idet("no idet here"), None);
}

#[test]
fn idet_looks_at_a_bounded_stretch_on_one_core() {
    let args = deinterlace::idet_args("/v/a.mkv", 100.0);
    assert_eq!(common::value_after(&args, "-ss").as_deref(), Some("10.000"));
    assert_eq!(
        common::value_after(&args, "-frames:v").as_deref(),
        Some(deinterlace::IDET_FRAMES.to_string().as_str())
    );
    assert_eq!(common::value_after(&args, "-threads").as_deref(), Some("1"));
    assert_eq!(common::value_after(&args, "-vf").as_deref(), Some("idet"));
}

#[test]
fn both_deinterlacers_start_at_their_slowest() {
    assert_eq!(Method::BWDIF, Method::Bwdif(BwdifProfile::Slowest));
    assert_eq!(Method::QTGMC, Method::Qtgmc(QtgmcPreset::Placebo));
    assert_eq!(Method::BWDIF.profile_label(), "Slowest");
    assert!(Method::QTGMC.profile_label().starts_with("Slowest"));
    assert_eq!(deinterlace::QTGMC_PRESETS[0], QtgmcPreset::Placebo);
    assert_eq!(deinterlace::BWDIF_PROFILES[0], BwdifProfile::Slowest);
}

#[test]
fn the_slowest_profiles_keep_every_field() {
    assert_eq!(Method::BWDIF.rate_multiplier(), 2.0);
    assert_eq!(Method::QTGMC.rate_multiplier(), 2.0);
    assert_eq!(Method::Bwdif(BwdifProfile::Fast).rate_multiplier(), 1.0);
    assert_eq!(
        deinterlace::bwdif_filter(BwdifProfile::Slowest, Parity::Tff),
        "bwdif=mode=send_field:parity=tff:deint=all"
    );
    assert_eq!(
        deinterlace::bwdif_filter(BwdifProfile::Fast, Parity::Bff),
        "bwdif=mode=send_frame:parity=bff:deint=all"
    );
}

#[test]
fn the_qtgmc_script_cuts_by_frame_and_quotes_the_path() {
    let script = deinterlace::qtgmc_script(
        r#"C:\Videos\a "b".avi"#,
        r"C:\Temp\x.index",
        250,
        499,
        Parity::Bff,
        QtgmcPreset::Placebo,
    );
    // A backslash or a quote in the path must reach Python intact.
    assert!(script.contains(r#""C:\\Videos\\a \"b\".avi""#), "{script}");
    assert!(
        script.contains(r#"cachefile="C:\\Temp\\x.index""#),
        "{script}"
    );
    assert!(script.contains("first, last = 250, min(499,"), "{script}");
    assert!(
        script.contains(r#"Preset="Placebo", TFF=False"#),
        "{script}"
    );
    assert!(script.contains("set_output()"));
    // Every line Python reads at the top level starts at the left edge.
    for line in script.lines().filter(|l| !l.is_empty()) {
        let indented = line.starts_with("    ");
        let continues = line.trim_start().starts_with("clip = core.");
        assert!(!indented || continues, "stray indent: {line:?}");
    }
}

#[test]
fn deinterlacing_is_never_a_stream_copy() {
    let options = Options {
        deinterlace: Some(Deinterlace {
            method: Method::BWDIF,
            parity: Parity::Tff,
        }),
        ..Options::default()
    };
    assert!(!options.inherits_everything());
    assert!(Options::default().inherits_everything());
}

// ----- the Codex folder ---------------------------------------------------

#[test]
fn the_newest_static_gpl_ffmpeg_is_chosen() {
    let names = [
        "checksums.sha256",
        "ffmpeg-master-latest-win64-gpl.zip",
        "ffmpeg-n8.1-latest-win64-gpl-8.1.zip",
        "ffmpeg-n9.0-latest-win64-gpl-shared-9.0.zip",
        "ffmpeg-n9.0-latest-win64-lgpl-9.0.zip",
        "ffmpeg-n9.0-latest-win64-gpl-9.0.zip",
        "ffmpeg-n10.0-latest-linux64-gpl-10.0.tar.xz",
        "ffmpeg-n8.10-latest-win64-gpl-8.10.zip",
    ];
    assert_eq!(
        codex::pick_ffmpeg_asset(names),
        Some("ffmpeg-n9.0-latest-win64-gpl-9.0.zip")
    );
    assert_eq!(codex::pick_ffmpeg_asset(["checksums.sha256"]), None);
}

#[test]
fn a_checksum_is_found_by_exact_name() {
    let list = "\
0000000000000000000000000000000000000000000000000000000000000000  ffmpeg-n9.0-latest-win64-gpl-shared-9.0.zip
ABCDEF0123456789abcdef0123456789ABCDEF0123456789abcdef0123456789 *ffmpeg-n9.0-latest-win64-gpl-9.0.zip
nothex  other.zip
";
    assert_eq!(
        codex::checksum_for(list, "ffmpeg-n9.0-latest-win64-gpl-9.0.zip").as_deref(),
        Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
    );
    assert_eq!(codex::checksum_for(list, "other.zip"), None);
    assert_eq!(codex::checksum_for(list, "missing.zip"), None);
}

#[test]
fn only_the_two_binaries_come_out_of_the_ffmpeg_zip() {
    assert_eq!(
        codex::ffmpeg_destination("ffmpeg-n9.0-latest-win64-gpl-9.0/bin/ffmpeg.exe"),
        Some("ffmpeg.exe")
    );
    assert_eq!(
        codex::ffmpeg_destination("ffmpeg-n9.0-latest-win64-gpl-9.0/bin/ffprobe.exe"),
        Some("ffprobe.exe")
    );
    assert_eq!(
        codex::ffmpeg_destination("ffmpeg-n9.0-latest-win64-gpl-9.0/bin/ffplay.exe"),
        None
    );
    assert_eq!(codex::ffmpeg_destination("doc/ffmpeg.exe.html"), None);
}

#[test]
fn archive_entries_land_where_the_table_says_and_nowhere_else() {
    let tree = [Take::Tree {
        from: "vapoursynth/",
        into: "Lib/site-packages/vapoursynth/",
        skip: ".pdb",
    }];
    assert_eq!(
        codex::destination(&tree, "vapoursynth/vspipe.exe").as_deref(),
        Some("Lib/site-packages/vapoursynth/vspipe.exe")
    );
    assert_eq!(codex::destination(&tree, "vapoursynth/vspipe.pdb"), None);
    assert_eq!(
        codex::destination(&tree, "vapoursynth-81.dist-info/RECORD"),
        None
    );
    assert_eq!(codex::destination(&tree, "vapoursynth/"), None);
    // Nothing climbs out of the folder, however the archive spells it.
    assert_eq!(
        codex::destination(&tree, "vapoursynth/../../evil.dll"),
        None
    );
    assert_eq!(codex::destination(&tree, "vapoursynth\\..\\evil.dll"), None);

    let file = [Take::File {
        from: "x64/LSMASHSource.dll",
        to: "Lib/site-packages/vapoursynth/plugins/LSMASHSource.dll",
    }];
    assert!(codex::destination(&file, "x64/LSMASHSource.dll").is_some());
    assert_eq!(codex::destination(&file, "x86/LSMASHSource.dll"), None);
}

#[test]
fn the_embedded_python_is_pointed_at_site_packages_once() {
    let pth = "python313.zip\r\n.\r\n\r\n# Uncomment to run site.main() automatically\r\n#import site\r\n";
    let once = codex::with_site_packages(pth);
    assert!(once.lines().any(|line| line == "Lib\\site-packages"));
    assert!(once.starts_with("python313.zip"));
    assert_eq!(codex::with_site_packages(&once), once);
}

#[test]
fn every_pinned_download_is_https_with_a_full_checksum() {
    for pinned in codex::QTGMC_SET {
        assert!(pinned.url.starts_with("https://"), "{}", pinned.name);
        assert_eq!(pinned.sha256.len(), 64, "{}", pinned.name);
        assert!(pinned.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!pinned.take.is_empty(), "{} keeps nothing", pinned.name);
    }
    // The three plugins QTGMC calls, its script, and a source filter.
    let destinations: Vec<String> = codex::QTGMC_SET
        .iter()
        .flat_map(|pinned| pinned.take.iter())
        .filter_map(|take| match take {
            Take::File { to, .. } => Some(to.to_string()),
            Take::Tree { .. } => None,
        })
        .collect();
    for needed in [
        "libmvtools.dll",
        "vsznedi3.dll",
        "nnedi3_weights.bin",
        "RemoveGrainVS.dll",
        "LSMASHSource.dll",
        "havsfunc.py",
    ] {
        assert!(
            destinations.iter().any(|to| to.ends_with(needed)),
            "{needed} is not in the set"
        );
    }
}

#[test]
fn vspipe_sits_three_folders_below_the_python_that_runs_it() {
    let codex = std::path::Path::new("Codex");
    let vspipe = codex::vspipe_in(codex);
    let python_home = vspipe.ancestors().nth(4).expect("deep enough");
    assert_eq!(python_home, codex.join(codex::VAPOURSYNTH));
}

/// The real download, into a scratch Codex folder: everything fetched, every
/// checksum held, everything where VapourSynth and the app look for it. On
/// Windows it then runs the QTGMC check script through the downloaded vspipe.
///
/// Ignored by default -- it fetches a quarter of a gigabyte. CI runs it on
/// demand (`codex` in ci.yml); from Linux:
///   cargo test --features codex --test units codex_download -- --ignored
#[cfg(any(windows, feature = "codex"))]
#[test]
#[ignore]
fn codex_download() {
    let root = tempfile::tempdir().expect("a scratch folder");
    let dir = root.path().join(codex::FOLDER);
    let ctx = egui::Context::default();
    let mut download = codex::Download::start(
        vec![codex::Part::Ffmpeg, codex::Part::Qtgmc],
        dir.clone(),
        ctx,
    );
    let result = loop {
        let finished = download.poll().into_iter().find_map(|event| match event {
            codex::Progress::Finished(result) => Some(result),
            codex::Progress::Step { .. } => None,
        });
        if let Some(result) = finished {
            break result;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    result.expect("the download should succeed");
    assert!(dir.join("ffmpeg.exe").is_file());
    assert!(dir.join("ffprobe.exe").is_file());
    assert!(codex::qtgmc_ready(&dir));
    assert!(
        !root.path().join("Codex.partial").exists(),
        "the staging folder is cleared away"
    );
    let pth = std::fs::read_dir(dir.join(codex::VAPOURSYNTH))
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.path().extension().is_some_and(|x| x == "_pth"))
        .expect("Python's ._pth");
    let pth = std::fs::read_to_string(pth.path()).unwrap();
    assert!(pth.contains("Lib\\site-packages"));

    #[cfg(windows)]
    {
        let script = root.path().join("check.vpy");
        std::fs::write(&script, deinterlace::QTGMC_CHECK).unwrap();
        let out = std::process::Command::new(codex::vspipe_in(&dir))
            .args(["-i", &script.to_string_lossy(), "-"])
            .output()
            .expect("vspipe should start");
        assert!(
            out.status.success(),
            "QTGMC's check failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
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

// ----- holding a step button ----------------------------------------------

#[test]
fn a_short_hold_does_not_repeat() {
    // A press held for less than the delay is a click, not a gesture -- the
    // step it earns is the one `clicked()` gives on release.
    assert!(!chrome::repeat_due(0.0, 0.0));
    assert!(!chrome::repeat_due(chrome::REPEAT_DELAY - 0.01, 10.0));
}

#[test]
fn past_the_delay_it_repeats_at_the_interval() {
    assert!(chrome::repeat_due(
        chrome::REPEAT_DELAY,
        chrome::REPEAT_INTERVAL
    ));
    // Held long enough, but it acted a moment ago: not yet.
    assert!(!chrome::repeat_due(2.0, chrome::REPEAT_INTERVAL - 0.001));
    assert!(chrome::repeat_due(2.0, chrome::REPEAT_INTERVAL));
}

#[test]
fn the_delay_outlasts_a_simulated_click() {
    // egui_kittest advances a quarter second per frame, and a click there is a
    // press frame and a release frame. If the delay were shorter than that,
    // every click in the window suite would count as two steps.
    const {
        assert!(
            chrome::REPEAT_DELAY > 0.25,
            "a simulated click would repeat"
        )
    };
}

// ----- the timeline handles -----------------------------------------------

#[test]
fn the_pointer_is_on_a_handle_within_reach_of_one() {
    // 100 and 400 are where the in and out handles are drawn.
    assert!(timeline::over_handle(100.0, 100.0, 400.0));
    assert!(timeline::over_handle(400.0, 100.0, 400.0));
    assert!(timeline::over_handle(
        100.0 + timeline::GRAB_PX,
        100.0,
        400.0
    ));
    assert!(timeline::over_handle(
        400.0 - timeline::GRAB_PX,
        100.0,
        400.0
    ));
}

#[test]
fn the_pointer_is_not_on_a_handle_anywhere_else() {
    // A pixel past the grab distance is filmstrip, and the hover preview
    // belongs there: it is only over the handles that it hides what you are
    // reaching for.
    assert!(!timeline::over_handle(
        100.0 + timeline::GRAB_PX + 1.0,
        100.0,
        400.0
    ));
    assert!(!timeline::over_handle(250.0, 100.0, 400.0));
    assert!(!timeline::over_handle(0.0, 100.0, 400.0));
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

// ----- what a desktop hands over when a file is dropped -------------------

#[test]
fn a_dropped_file_arrives_as_a_uri() {
    let dropped = platform::paths_from_uri_list("file:///home/me/Videos/replay.mp4\r\n");
    assert_eq!(dropped, ["/home/me/Videos/replay.mp4"]);
}

#[test]
fn the_escapes_come_back_out() {
    // The other end of platform::file_uri: a space is %20, and a # would have
    // ended the URI early had it not been escaped in the first place.
    let dropped = platform::paths_from_uri_list("file:///tmp/my%20clip%20%232.mp4");
    assert_eq!(dropped, ["/tmp/my clip #2.mp4"]);

    // One escape per *byte*, so a name outside ASCII arrives as several of
    // them per character and only reads back once they are reassembled.
    assert_eq!(
        platform::paths_from_uri_list("file:///tmp/%C3%85%20%C3%AB%C3%B1%20%E2%80%93%20cut.mkv"),
        ["/tmp/Å ëñ – cut.mkv"]
    );
}

#[test]
fn a_uri_survives_the_round_trip_through_the_encoder() {
    // Built from the working directory rather than written out, because
    // "/tmp/x" is an absolute path on one of the three platforms this runs on
    // and a relative one on Windows, where paths::absolute would helpfully
    // resolve it against the current drive and the two ends would disagree
    // about a file neither of them is wrong about.
    let original = std::env::current_dir()
        .expect("a working directory")
        .join("Å ëñ – cut.mkv")
        .to_string_lossy()
        .replace('\\', "/");

    let round_tripped = platform::paths_from_uri_list(&platform::file_uri(&original));
    assert_eq!(round_tripped.len(), 1, "{round_tripped:?}");
    // Separators normalised on the way in and out: Windows spells them the
    // other way round, and file_uri does not promise to spell them back.
    assert_eq!(round_tripped[0].replace('\\', "/"), original);
}

#[test]
fn several_files_at_once() {
    let list = "file:///a.mp4\r\nfile:///b.mkv\r\n";
    assert_eq!(platform::paths_from_uri_list(list), ["/a.mp4", "/b.mkv"]);
}

#[test]
fn comments_and_blank_lines_are_not_files() {
    let list = "# a comment\r\n\r\nfile:///c.mp4\r\n";
    assert_eq!(platform::paths_from_uri_list(list), ["/c.mp4"]);
}

#[test]
fn only_local_files_come_through() {
    // A drag out of a browser, and the bookkeeping some file managers add.
    let list = "https://example.com/clip.mp4\r\nx-special/nautilus-clipboard\r\n";
    assert!(platform::paths_from_uri_list(list).is_empty());
}

#[test]
fn a_windows_drive_loses_the_slash_in_front_of_it() {
    // file:///C:/clip.mp4 is the spec-correct spelling, and the leading slash
    // is part of the URI rather than part of the path.
    assert_eq!(
        platform::paths_from_uri_list("file:///C:/Videos/replay.mp4"),
        ["C:/Videos/replay.mp4"]
    );
    // A directory that merely starts with a letter keeps its slash.
    assert_eq!(
        platform::paths_from_uri_list("file:///Camera/replay.mp4"),
        ["/Camera/replay.mp4"]
    );
}

#[test]
fn the_shapes_a_sender_may_use() {
    // localhost is spelled out by some senders, and a bare path by others,
    // neither of which is worth losing a drop over.
    assert_eq!(
        platform::paths_from_uri_list("file://localhost/tmp/d.mp4"),
        ["/tmp/d.mp4"]
    );
    assert_eq!(platform::paths_from_uri_list("/tmp/e.mp4"), ["/tmp/e.mp4"]);
}

#[test]
fn a_truncated_escape_is_left_alone_rather_than_dropped() {
    // Nothing sends this, but a parser that panics on it takes the window with
    // it, and a percent is a legal character in a filename.
    assert_eq!(
        platform::paths_from_uri_list("/tmp/100%.mp4"),
        ["/tmp/100%.mp4"]
    );
    assert_eq!(
        platform::paths_from_uri_list("/tmp/a%zz.mp4"),
        ["/tmp/a%zz.mp4"]
    );
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
            deinterlace: None,
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
