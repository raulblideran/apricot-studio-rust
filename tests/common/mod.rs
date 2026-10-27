// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! Synthetic test media, and the helpers for reading an argument list.
//!
//! The suite builds its own clips with ffmpeg rather than reading anything from
//! the user's video library, so it is reproducible and safe to run anywhere.
//! The `allp` fixture deliberately mirrors what gpu-screen-recorder produces:
//! H.264 High, all-P (no B-frames), a 2-second GOP and Opus audio in MP4.

#![allow(dead_code)] // each test file uses a different half of this

use std::path::PathBuf;
use std::process::Command;

use apricot_studio::media::{AudioTrack, MediaInfo};

pub const DURATION: u32 = 6;
pub const FPS: u32 = 60;
/// A keyframe every 2s, as the real recordings have.
pub const GOP: u32 = FPS * 2;

/// Hard ceiling on any fixture. These clips are a couple of megabytes; anything
/// approaching this means an encode has run away, and stopping early beats
/// filling the disk.
const MAX_FIXTURE_BYTES: u64 = 32 * 1024 * 1024;

fn cache_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("apricot-rust-tests");
    std::fs::create_dir_all(&dir).expect("test cache directory");
    dir
}

/// Whether ffmpeg is present at all. Everything that encodes checks this and
/// skips rather than failing, so the suite is still green on a machine that
/// only has the source.
pub fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn run_ffmpeg(args: &[String]) {
    // -fs is an output option, so it belongs immediately before the
    // destination, which is always the last argument.
    let (output, rest) = args.split_last().expect("an output path");
    let mut full: Vec<String> = vec!["-v".into(), "error".into(), "-nostdin".into(), "-y".into()];
    full.extend(rest.iter().cloned());
    full.push("-fs".into());
    full.push(MAX_FIXTURE_BYTES.to_string());
    full.push(output.clone());

    let result = Command::new("ffmpeg")
        .args(&full)
        .output()
        .expect("ffmpeg should run");
    assert!(
        result.status.success(),
        "fixture build failed:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
    assert!(
        size < MAX_FIXTURE_BYTES,
        "fixture {output} hit the size cap; the encode ran away"
    );
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

fn video_args() -> Vec<String> {
    words(&[
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-crf",
        "26",
        "-pix_fmt",
        "yuv420p",
        "-profile:v",
        "high",
        "-g",
        "120",
        // Fixed GOP with no scene-cut keyframes, so tests can rely on keyframes
        // landing exactly every two seconds.
        "-x264-params",
        "scenecut=0:open_gop=0",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-color_range",
        "tv",
    ])
}

fn source_args() -> Vec<String> {
    words(&[
        "-f",
        "lavfi",
        "-i",
        &format!("testsrc2=size=640x360:rate={FPS}:duration={DURATION}"),
    ])
}

fn tone_args(freq: u32) -> Vec<String> {
    words(&[
        "-f",
        "lavfi",
        "-i",
        &format!("sine=frequency={freq}:duration={DURATION}"),
    ])
}

/// Path to a cached fixture, building it on first use.
///
/// * `allp` -- H.264 all-P + Opus, mirroring a screen-recorder capture
/// * `bframes` -- the same but encoded with B-frames, to prove inheritance both ways
/// * `twoaudio` -- two Opus tracks, as a mic + desktop capture would have
/// * `noaudio` -- video only
/// * `interlaced` -- 30i, top field first, flagged as such
pub fn fixture(kind: &str) -> String {
    let path = cache_dir().join(format!("{kind}.mp4"));
    let path_str = path.to_string_lossy().into_owned();
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > 1024 {
        return path_str;
    }

    let mut args = source_args();
    match kind {
        "allp" => {
            args.extend(tone_args(440));
            args.extend(video_args());
            args.extend(words(&["-bf", "0", "-c:a", "libopus", "-b:a", "96k"]));
        }
        "bframes" => {
            args.extend(tone_args(440));
            args.extend(video_args());
            args.extend(words(&["-bf", "2", "-c:a", "libopus", "-b:a", "96k"]));
        }
        "twoaudio" => {
            args.extend(tone_args(440));
            args.extend(tone_args(880));
            args.extend(video_args());
            args.extend(words(&[
                "-bf",
                "0",
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-map",
                "2:a:0",
                "-c:a",
                "libopus",
                "-b:a",
                "96k",
                "-metadata:s:a:0",
                "title=device:default_output",
                "-metadata:s:a:1",
                "title=Mic",
            ]));
        }
        "noaudio" => {
            args.extend(video_args());
            args.extend(words(&["-bf", "0", "-an"]));
        }
        "interlaced" => {
            // 60p woven into 30i, top field first, and encoded as interlaced
            // so the container says so: what a camcorder or a broadcast
            // capture hands over.
            args.extend(tone_args(440));
            args.extend(words(&[
                "-vf",
                "tinterlace=mode=interleave_top,setfield=tff",
            ]));
            args.extend(video_args());
            args.extend(words(&[
                "-flags",
                "+ildct+ilme",
                "-bf",
                "0",
                "-c:a",
                "libopus",
                "-b:a",
                "96k",
            ]));
        }
        other => panic!("unknown fixture {other:?}"),
    }
    args.push(path_str.clone());
    run_ffmpeg(&args);
    path_str
}

/// Real keyframe timestamps, as the app itself reads them.
pub fn keyframes(path: &str) -> Vec<f64> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time,flags",
            "-of",
            "csv=p=0",
            path,
        ])
        .output()
        .expect("ffprobe should run");
    apricot_studio::media::keyframes_from_packets(&String::from_utf8_lossy(&out.stdout))
}

/// A [`MediaInfo`] standing in for a screen-recorder capture.
pub fn sample_info() -> MediaInfo {
    MediaInfo {
        path: "/tmp/sample.mp4".into(),
        duration: 90.0,
        ext: "mp4".into(),
        v_codec: "h264".into(),
        width: 2560,
        height: 1440,
        fps: 60.0,
        pix_fmt: "yuv420p".into(),
        profile: "High".into(),
        level: 51,
        v_bitrate: 9_507_948,
        has_b_frames: 0,
        color_range: "tv".into(),
        color_primaries: "bt709".into(),
        color_transfer: "bt709".into(),
        color_space: "bt709".into(),
        field_order: "progressive".into(),
        audio: vec![AudioTrack {
            index: 0,
            codec: "opus".into(),
            bitrate: 106_489,
            sample_rate: 48000,
            channels: 2,
            language: "und".into(),
            title: "Desktop".into(),
        }],
    }
}

// ----- reading an argument list -------------------------------------------

/// The value ffmpeg would take for `flag`, or None if the flag is absent.
pub fn value_after(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

/// Every value given for a flag that may appear more than once.
pub fn values_after(args: &[String], flag: &str) -> Vec<String> {
    args.iter()
        .enumerate()
        .filter(|(_, arg)| *arg == flag)
        .filter_map(|(index, _)| args.get(index + 1).cloned())
        .collect()
}

pub fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

/// Whether `needle` appears as a run of consecutive arguments.
pub fn contains_sequence(args: &[String], needle: &[&str]) -> bool {
    args.windows(needle.len())
        .any(|window| window.iter().zip(needle).all(|(a, b)| a == b))
}

/// How many arguments before `flag`, so "is it an input or an output option"
/// can be asked.
pub fn index_of(args: &[String], flag: &str) -> Option<usize> {
    args.iter().position(|arg| arg == flag)
}
