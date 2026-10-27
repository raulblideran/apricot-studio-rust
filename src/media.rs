// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! Everything Apricot Studio learns about a source file.
//!
//! A single synchronous ffprobe call fills in [`MediaInfo`] (cheap: header read
//! only). The three expensive extras -- keyframe positions, the audio waveform
//! and the filmstrip -- run on worker threads so the interface never blocks
//! waiting for them.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::deinterlace::{self, Scan};
use crate::ffmpeg::{command, ffmpeg_path, ffprobe_path};

pub const THUMB_W: usize = 106;
pub const THUMB_H: usize = 60;

/// The most filmstrip frames a file ever gets, however long it is. Zoomed all
/// the way out the strip shows a dozen; this leaves room to zoom in.
pub const MAX_THUMBS: usize = 160;
/// And no more than one per this many seconds, so a short clip is not asked
/// for a frame it does not have.
const THUMB_MIN_INTERVAL: f64 = 0.5;

/// Bucket the waveform decode into roughly this many samples regardless of how
/// long the file is, so a two-hour recording costs the same as a 90s clip.
const WAVE_SAMPLE_BUDGET: f64 = 2_000_000.0;

/// A header read on a local file is milliseconds. This is not a performance
/// budget but a way out: [`probe`] runs synchronously on the interface thread,
/// so a stalled network mount or a failing disk would otherwise hang the window
/// with no way to close it.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

// --------------------------------------------------------------------------
// Timecode
// --------------------------------------------------------------------------

/// Seconds -> HH:MM:SS.mmm.
pub fn fmt_tc(seconds: f64) -> String {
    // Guard the non-finite cases: they run through every timeline repaint, and
    // one bad value would otherwise print nonsense forever.
    if !seconds.is_finite() {
        return "00:00:00.000".to_string();
    }
    let seconds = seconds.max(0.0);
    let total_ms = (seconds * 1000.0).round() as i64;
    let (h, rem) = (total_ms / 3_600_000, total_ms % 3_600_000);
    let (m, rem) = (rem / 60_000, rem % 60_000);
    let (s, ms) = (rem / 1000, rem % 1000);
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

/// HH:MM:SS.mmm / MM:SS.mmm / SS.mmm -> seconds. None if unparseable.
pub fn parse_tc(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0_f64;
    for part in parts {
        let value: f64 = part.trim().parse().ok()?;
        total = total * 60.0 + value;
    }
    // A parse happily accepts "nan" and "inf". A NaN in-point propagates
    // through every clamp (comparisons against NaN are all false) and then
    // corrupts the timecode display; a negative one is simply not a position in
    // a file.
    if !total.is_finite() || total < 0.0 {
        return None;
    }
    Some(total)
}

// --------------------------------------------------------------------------
// Parsing helpers. ffprobe writes most numbers as strings.
// --------------------------------------------------------------------------

fn num_f64(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn num_i64(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// "opus" -> "Opus", the way Python's `str.title()` spells it.
fn title_case(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut fresh = true;
    for ch in value.chars() {
        if ch.is_alphabetic() {
            if fresh {
                out.extend(ch.to_uppercase());
            } else {
                out.extend(ch.to_lowercase());
            }
            fresh = false;
        } else {
            out.push(ch);
            fresh = true;
        }
    }
    out
}

fn clip_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        value.to_string()
    } else {
        let kept: String = value.chars().take(limit - 1).collect();
        format!("{kept}…")
    }
}

/// ffprobe spells profiles for humans; encoders want their own tokens. Anything
/// not listed here is simply not passed through, which lets the encoder choose.
fn encoder_profile_for(codec: &str, profile: &str) -> &'static str {
    let profile = profile.to_lowercase();
    match codec {
        "h264" => match profile.as_str() {
            "baseline" | "constrained baseline" => "baseline",
            "main" => "main",
            "high" => "high",
            "high 10" => "high10",
            "high 4:2:2" => "high422",
            "high 4:4:4 predictive" => "high444",
            _ => "",
        },
        "hevc" => match profile.as_str() {
            "main" => "main",
            "main 10" => "main10",
            "rext" => "mainstillpicture",
            _ => "",
        },
        _ => "",
    }
}

/// gpu-screen-recorder names its tracks after the PipeWire nodes it captured,
/// which is accurate but unreadable in a dropdown.
fn device_name(part: &str) -> &str {
    match part {
        "default_output" => "Desktop",
        "default_input" => "Mic",
        other => other,
    }
}

pub fn clean_title(title: &str) -> String {
    if !title.contains("device:") && !title.contains('|') {
        return clip_chars(title, 32);
    }
    let mut parts: Vec<&str> = Vec::new();
    for raw in title.split('|') {
        let stripped = raw.strip_prefix("device:").unwrap_or(raw);
        if stripped.is_empty() {
            continue;
        }
        let named = device_name(stripped);
        if !parts.contains(&named) {
            parts.push(named); // de-duplicate, keep order
        }
    }
    clip_chars(&parts.join(" + "), 32)
}

// --------------------------------------------------------------------------
// What a file is
// --------------------------------------------------------------------------

/// One audio stream. `index` counts audio streams, so it suits `-map 0:a:N`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioTrack {
    pub index: usize,
    pub codec: String,
    pub bitrate: i64,
    pub sample_rate: i64,
    pub channels: i64,
    pub language: String,
    pub title: String,
}

impl AudioTrack {
    pub fn label(&self) -> String {
        let name = if !self.title.is_empty() {
            self.title.clone()
        } else {
            match self.language.as_str() {
                "eng" => "English".to_string(),
                "und" => String::new(),
                other => other.to_uppercase(),
            }
        };
        let head = if name.is_empty() {
            format!("Track {}", self.index + 1)
        } else {
            format!("Track {} · {name}", self.index + 1)
        };
        let layout = match self.channels {
            1 => "mono".to_string(),
            2 => "stereo".to_string(),
            0 => String::new(),
            n => format!("{n}ch"),
        };
        let rate = if self.bitrate > 0 {
            format!("{}k", self.bitrate / 1000)
        } else {
            String::new()
        };
        let detail = [title_case(&self.codec), layout, rate]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if detail.is_empty() {
            head
        } else {
            format!("{head} — {detail}")
        }
    }
}

/// The subset of a file's parameters that the export inherits.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaInfo {
    pub path: String,
    pub duration: f64,
    pub ext: String,

    pub v_codec: String,
    pub width: i64,
    pub height: i64,
    pub fps: f64,
    pub pix_fmt: String,
    pub profile: String,
    pub level: i64,
    pub v_bitrate: i64,
    /// Reorder depth. Screen recorders encode 0 (all-P) for low latency; an
    /// encoder left to its own devices will happily add B-frames the source
    /// never had, which costs decode work and reorder delay on playback.
    pub has_b_frames: i64,
    pub color_range: String,
    pub color_primaries: String,
    pub color_transfer: String,
    pub color_space: String,
    /// ffprobe's word for the scan: `progressive`, `tt`, `bb`, `tb`, `bt`, or
    /// empty/`unknown` when the stream does not say.
    pub field_order: String,

    pub audio: Vec<AudioTrack>,
}

impl MediaInfo {
    /// The first track stands in wherever a single answer is wanted.
    pub fn has_audio(&self) -> bool {
        !self.audio.is_empty()
    }

    pub fn a_codec(&self) -> &str {
        self.audio.first().map(|t| t.codec.as_str()).unwrap_or("")
    }

    pub fn a_bitrate(&self) -> i64 {
        self.audio.first().map(|t| t.bitrate).unwrap_or(0)
    }

    /// All tracks together -- what a size target has to make room for.
    pub fn total_audio_bitrate(&self) -> i64 {
        self.audio.iter().map(|t| t.bitrate).sum()
    }

    /// The source profile spelled the way the encoder expects, or "".
    pub fn encoder_profile(&self) -> &'static str {
        encoder_profile_for(&self.v_codec, &self.profile)
    }

    pub fn frame_duration(&self) -> f64 {
        if self.fps > 0.0 {
            1.0 / self.fps
        } else {
            1.0 / 30.0
        }
    }

    pub fn file_name(&self) -> String {
        Path::new(&self.path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.clone())
    }

    pub fn summary(&self) -> String {
        let mut bits = vec![
            format!("{}×{}", self.width, self.height),
            format!("{}fps", trim_float(self.fps)),
        ];
        let codec = self
            .v_codec
            .to_uppercase()
            .replace("HEVC", "H.265")
            .replace("H264", "H.264");
        bits.push(format!("{codec} {}", self.profile).trim().to_string());
        if self.v_bitrate > 0 {
            bits.push(format!("{:.1} Mb/s", self.v_bitrate as f64 / 1_000_000.0));
        }
        if !self.has_audio() {
            bits.push("no audio".to_string());
        } else {
            // Matroska keeps no per-stream bitrate, so it is often simply absent.
            let rate = if self.a_bitrate() > 0 {
                format!(" {}k", self.a_bitrate() / 1000)
            } else {
                String::new()
            };
            let extra = if self.audio.len() > 1 {
                format!(" +{}", self.audio.len() - 1)
            } else {
                String::new()
            };
            bits.push(format!("{}{rate}{extra}", title_case(self.a_codec())));
        }
        bits.join("  ·  ")
    }
}

/// `60` rather than `60.000000`, the way Python's `%g` writes a frame rate.
fn trim_float(value: f64) -> String {
    let mut s = format!("{value:.6}");
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    s
}

// --------------------------------------------------------------------------
// Probing
// --------------------------------------------------------------------------

/// Run a command, killing it if it outlives `timeout`.
///
/// `Child::wait` cannot be given a deadline portably, so the wait is polled.
/// The normal case returns in milliseconds and never sleeps twice.
fn run_with_timeout(
    mut child: Child,
    timeout: Duration,
) -> std::io::Result<(bool, String, String)> {
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let err_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break Some(status),
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            None => thread::sleep(Duration::from_millis(5)),
        }
    };

    let out = out_handle.join().unwrap_or_default();
    let err = err_handle.join().unwrap_or_default();
    match status {
        Some(status) => Ok((
            status.success(),
            String::from_utf8_lossy(&out).into_owned(),
            String::from_utf8_lossy(&err).into_owned(),
        )),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out",
        )),
    }
}

/// Read a file's parameters. `Err` if it isn't playable video.
pub fn probe(path: &str) -> Result<MediaInfo, String> {
    let child = command(&ffprobe_path())
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
            path,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run ffprobe: {e}"))?;

    let (ok, out, err) = match run_with_timeout(child, PROBE_TIMEOUT) {
        Ok(result) => result,
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
            return Err(format!(
                "ffprobe gave up after {}s -- is the drive still there?",
                PROBE_TIMEOUT.as_secs()
            ))
        }
        Err(e) => return Err(e.to_string()),
    };
    if !ok {
        let message = err.trim();
        return Err(if message.is_empty() {
            "ffprobe could not read this file".to_string()
        } else {
            message.to_string()
        });
    }
    parse_probe(path, &out)
}

/// The half of [`probe`] that does not need a subprocess, so a malformed
/// document can be handed to it directly by a test.
pub fn parse_probe(path: &str, json: &str) -> Result<MediaInfo, String> {
    let data: Value = serde_json::from_str(if json.trim().is_empty() { "{}" } else { json })
        .map_err(|e| format!("ffprobe returned something unreadable: {e}"))?;
    let empty = Vec::new();
    let streams = data
        .get("streams")
        .and_then(|s| s.as_array())
        .unwrap_or(&empty);
    let format = data.get("format").cloned().unwrap_or(Value::Null);

    let video = streams
        .iter()
        .find(|s| text(s.get("codec_type")) == "video")
        .ok_or_else(|| "no video stream in this file".to_string())?;
    let audio_streams: Vec<&Value> = streams
        .iter()
        .filter(|s| text(s.get("codec_type")) == "audio")
        .collect();

    let duration = num_f64(format.get("duration"))
        .filter(|d| *d > 0.0)
        .or_else(|| num_f64(video.get("duration")))
        .unwrap_or(0.0);
    // Written the long way round because a NaN duration must fail here rather
    // than sail through a comparison that is false either way.
    if !duration.is_finite() || duration <= 0.0 {
        return Err("could not determine duration".to_string());
    }

    // r_frame_rate is the nominal rate; gpu-screen-recorder writes VFR files
    // where avg_frame_rate is a messy fraction, so prefer the nominal one.
    let mut fps = 30.0;
    for key in ["r_frame_rate", "avg_frame_rate"] {
        let raw = text(video.get(key));
        if let Some((num, den)) = raw.split_once('/') {
            let (n, d) = (
                num.trim().parse::<f64>().unwrap_or(0.0),
                den.trim().parse::<f64>().unwrap_or(0.0),
            );
            if n > 0.0 && d > 0.0 {
                fps = n / d;
                break;
            }
        }
    }

    let mut audio = Vec::new();
    for (index, stream) in audio_streams.iter().enumerate() {
        let tags = stream.get("tags").cloned().unwrap_or(Value::Null);
        audio.push(AudioTrack {
            index,
            codec: text(stream.get("codec_name")),
            bitrate: num_i64(stream.get("bit_rate")).unwrap_or(0),
            sample_rate: num_i64(stream.get("sample_rate")).unwrap_or(0),
            channels: num_i64(stream.get("channels")).unwrap_or(0),
            language: text(tags.get("language")),
            title: clean_title(&text(tags.get("title"))),
        });
    }
    let a_bitrate = audio.first().map(|t| t.bitrate).unwrap_or(0);

    // Stream bitrate is often absent in Matroska; fall back to the container
    // total minus audio, and finally to size/duration.
    let mut v_bitrate = num_i64(video.get("bit_rate")).unwrap_or(0);
    if v_bitrate == 0 {
        let mut total = num_i64(format.get("bit_rate")).unwrap_or(0);
        if total == 0 {
            let size = num_i64(format.get("size")).unwrap_or(0);
            total = if size > 0 {
                (size as f64 * 8.0 / duration) as i64
            } else {
                0
            };
        }
        v_bitrate = (total - a_bitrate).max(0);
    }

    // Split the basename, not the whole path: a dot in a *directory* name
    // ("~/my.videos/clip") would otherwise make the extension "videos/clip",
    // which then lands in the output filename and the container checks.
    let ext = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, ext)| ext.to_lowercase())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "mp4".to_string());

    Ok(MediaInfo {
        path: path.to_string(),
        duration,
        ext,
        v_codec: text(video.get("codec_name")),
        width: num_i64(video.get("width")).unwrap_or(0),
        height: num_i64(video.get("height")).unwrap_or(0),
        fps,
        pix_fmt: text(video.get("pix_fmt")),
        profile: text(video.get("profile")),
        level: num_i64(video.get("level")).unwrap_or(0),
        v_bitrate,
        has_b_frames: num_i64(video.get("has_b_frames")).unwrap_or(0),
        color_range: text(video.get("color_range")),
        color_primaries: text(video.get("color_primaries")),
        color_transfer: text(video.get("color_transfer")),
        color_space: text(video.get("color_space")),
        field_order: text(video.get("field_order")),
        audio,
    })
}

// --------------------------------------------------------------------------
// The background loaders
// --------------------------------------------------------------------------

/// One background probe: a worker thread with a child process behind it.
///
/// A cancelled loader must go quiet rather than merely be ignored, because the
/// process behind it is decoding a video file. The worker owns its child and
/// checks the flag between reads, so cancelling never blocks the interface on
/// a process that is busy -- which is the one thread it cannot afford to wait
/// for.
pub struct Loader<T> {
    cancelled: Arc<AtomicBool>,
    rx: Option<Receiver<T>>,
    done: bool,
}

impl<T> Default for Loader<T> {
    fn default() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            rx: None,
            done: true,
        }
    }
}

impl<T: Send + 'static> Loader<T> {
    fn spawn<F>(program: PathBuf, args: Vec<String>, ctx: egui::Context, work: F) -> Self
    where
        F: FnOnce(std::process::ChildStdout, &Sender<T>, &Arc<AtomicBool>, &egui::Context)
            + Send
            + 'static,
    {
        let (tx, rx) = channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        thread::spawn(move || {
            let spawned = command(&program)
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .stdin(Stdio::null())
                .spawn();
            let Ok(mut child) = spawned else {
                ctx.request_repaint();
                return;
            };
            if let Some(stdout) = child.stdout.take() {
                work(stdout, &tx, &flag, &ctx);
            }
            // Harmless once it has already exited, and the only thing that
            // stops it when the answer is no longer wanted.
            let _ = child.kill();
            let _ = child.wait();
            drop(tx);
            ctx.request_repaint();
        });
        Self {
            cancelled,
            rx: Some(rx),
            done: false,
        }
    }

    /// A worker that starts its own processes, for a job that is many small
    /// ones rather than one long one.
    fn run<F>(ctx: egui::Context, work: F) -> Self
    where
        F: FnOnce(&Sender<T>, &Arc<AtomicBool>, &egui::Context) + Send + 'static,
    {
        let (tx, rx) = channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        thread::spawn(move || {
            work(&tx, &flag, &ctx);
            drop(tx);
            ctx.request_repaint();
        });
        Self {
            cancelled,
            rx: Some(rx),
            done: false,
        }
    }

    pub fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.rx = None;
        self.done = true;
    }

    /// Everything that has arrived since the last call.
    pub fn drain(&mut self) -> Vec<T> {
        let mut out = Vec::new();
        let Some(rx) = self.rx.as_ref() else {
            return out;
        };
        loop {
            match rx.try_recv() {
                Ok(item) => out.push(item),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.done = true;
                    break;
                }
            }
        }
        out
    }

    pub fn running(&self) -> bool {
        !self.done
    }
}

/// Read the whole of a pipe, giving up the moment the job is cancelled.
fn read_all(
    mut pipe: std::process::ChildStdout,
    cancelled: &Arc<AtomicBool>,
    mut on_chunk: impl FnMut(&[u8]),
) {
    let mut buf = [0u8; 64 * 1024];
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return;
        }
        match pipe.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => on_chunk(&buf[..n]),
        }
    }
}

/// Keyframe timestamps, read from packet flags.
///
/// Packets, not frames: asking for frames makes ffprobe decode each one it
/// reports, and on an all-intra source (ProRes, DV, MJPEG) every frame is a
/// keyframe, so that was the whole file decoded at full speed on every core.
/// The packet headers already say which ones are keyframes.
pub fn load_keyframes(path: &str, ctx: egui::Context) -> Loader<Vec<f64>> {
    let args = [
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "packet=pts_time,flags",
        "-of",
        "csv=p=0",
        path,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    Loader::spawn(ffprobe_path(), args, ctx, |pipe, tx, cancelled, ctx| {
        let mut text = String::new();
        read_all(pipe, cancelled, |chunk| {
            text.push_str(&String::from_utf8_lossy(chunk));
        });
        if cancelled.load(Ordering::SeqCst) {
            return;
        }
        let _ = tx.send(keyframes_from_packets(&text));
        ctx.request_repaint();
    })
}

/// Keyframe times out of ffprobe's `pts_time,flags` packet lines, sorted.
pub fn keyframes_from_packets(text: &str) -> Vec<f64> {
    let mut times: Vec<f64> = text
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim().split(',');
            let time = fields.next()?.parse::<f64>().ok()?;
            let flags = fields.next()?;
            (flags.contains('K') && time.is_finite()).then_some(time)
        })
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times.dedup();
    times
}

/// Peak envelope of the audio, as `buckets` values in 0..1.
pub fn load_waveform(info: &MediaInfo, ctx: egui::Context) -> Loader<Vec<f32>> {
    const BUCKETS: usize = 2000;
    if !info.has_audio() {
        return Loader::default();
    }
    let rate = (WAVE_SAMPLE_BUDGET / info.duration.max(1.0)) as i64;
    let rate = rate.clamp(400, 8000);
    let args = [
        "-v",
        "error",
        "-nostdin",
        "-i",
        &info.path,
        "-vn",
        "-ac",
        "1",
        "-ar",
        &rate.to_string(),
        "-f",
        "s16le",
        "-",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    Loader::spawn(ffmpeg_path(), args, ctx, |pipe, tx, cancelled, ctx| {
        let mut pcm: Vec<u8> = Vec::new();
        read_all(pipe, cancelled, |chunk| pcm.extend_from_slice(chunk));
        if cancelled.load(Ordering::SeqCst) {
            return;
        }
        let _ = tx.send(peaks_from_pcm(&pcm, BUCKETS));
        ctx.request_repaint();
    })
}

/// Whether the source is interlaced, and which field comes first.
///
/// The container's own flag answers at once when it gives one. When it does
/// not, ffmpeg's `idet` looks at a few hundred frames -- one core, a second or
/// two -- and the answer arrives with the other loaders'.
pub fn detect_scan(info: &MediaInfo, ctx: egui::Context) -> Loader<Scan> {
    if let Some(scan) = deinterlace::scan_from_field_order(&info.field_order) {
        let (tx, rx) = channel();
        let _ = tx.send(scan);
        return Loader {
            cancelled: Arc::new(AtomicBool::new(false)),
            rx: Some(rx),
            done: false,
        };
    }
    let args = deinterlace::idet_args(&info.path, info.duration);
    Loader::run(ctx, move |tx, cancelled, ctx| {
        let Ok(mut child) = command(&ffmpeg_path())
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        else {
            return;
        };
        let mut text = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let mut buf = [0u8; 8192];
            while !cancelled.load(Ordering::SeqCst) {
                match pipe.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => text.push_str(&String::from_utf8_lossy(&buf[..n])),
                }
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        if cancelled.load(Ordering::SeqCst) {
            return;
        }
        if let Some(scan) = deinterlace::scan_from_idet(&text) {
            let _ = tx.send(scan);
            ctx.request_repaint();
        }
    })
}

/// Bucket signed 16-bit mono PCM into a peak envelope.
///
/// Game capture often peaks well below full scale, which would draw as a flat
/// line, so the envelope is scaled to the loudest moment: the shape becomes
/// readable while the relative dynamics that make a gunshot stand out survive.
pub fn peaks_from_pcm(pcm: &[u8], buckets: usize) -> Vec<f32> {
    // A killed process can leave a dangling byte, which is half a sample.
    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let total = samples.len();
    if total == 0 || buckets == 0 {
        return Vec::new();
    }
    let mut peaks = Vec::with_capacity(buckets);
    for i in 0..buckets {
        let lo = total * i / buckets;
        let hi = (total * (i + 1) / buckets).max(lo + 1).min(total);
        let chunk = &samples[lo..hi];
        let peak = chunk
            .iter()
            .map(|s| (*s as i32).unsigned_abs().min(32768))
            .max()
            .unwrap_or(0);
        peaks.push((peak as f32 / 32768.0).min(1.0));
    }
    let ceiling = peaks.iter().copied().fold(0.0_f32, f32::max);
    if ceiling > 0.02 {
        for peak in &mut peaks {
            *peak = (*peak / ceiling).min(1.0);
        }
    }
    peaks
}

/// When the filmstrip's frames are taken, in the order they are fetched.
///
/// A fixed budget spread evenly over the file -- one slot per
/// [`THUMB_MIN_INTERVAL`], at most [`MAX_THUMBS`] -- with each frame taken at
/// the start of its slot, so frame `i` is drawn from its own time to the next
/// one's and the first starts at zero.
///
/// The order is coarse to fine (bit-reversed): the first few cover the whole
/// file, and each later one splits a gap in two. The strip is all there almost
/// at once and sharpens, rather than filling in left to right.
pub fn thumb_schedule(duration: f64) -> Vec<f64> {
    if !duration.is_finite() || duration <= 0.0 {
        return Vec::new();
    }
    let count = ((duration / THUMB_MIN_INTERVAL).ceil() as usize).clamp(1, MAX_THUMBS);
    // Walk the fractions 0, 1/2, 1/4, 3/4, 1/8 ... of the next power of two up
    // and map each onto a slot, keeping the first visit to each. Bit-reversing
    // the slot numbers directly would skip the ones past `count` and bunch the
    // early frames into the start of the file.
    let span = count.next_power_of_two();
    let bits = span.trailing_zeros();
    let mut seen = vec![false; count];
    let mut slots = Vec::with_capacity(count);
    for j in 0..span {
        let fraction = if bits == 0 {
            0
        } else {
            j.reverse_bits() >> (usize::BITS - bits)
        };
        let slot = fraction * count / span;
        if !seen[slot] {
            seen[slot] = true;
            slots.push(slot);
        }
    }
    slots
        .into_iter()
        .map(|i| duration * i as f64 / count as f64)
        .collect()
}

/// Filmstrip frames: a fixed number of them, one decoder at a time.
///
/// Each frame is its own short ffmpeg run: seek to the slot, decode the one
/// keyframe at or before it, stop. That is a bounded amount of work however
/// long the file is or however its keyframes are spaced -- decoding every
/// keyframe in one pass was the whole file on an all-intra source, at full
/// speed on every core. One thread, run one after another, keeps the strip to
/// a single core while the preview is the thing being looked at.
pub fn load_thumbnails(info: &MediaInfo, ctx: egui::Context) -> Loader<(f64, egui::ColorImage)> {
    let schedule = thumb_schedule(info.duration);
    if schedule.is_empty() {
        return Loader::default();
    }
    let path = info.path.clone();
    Loader::run(ctx, move |tx, cancelled, ctx| {
        for at in schedule {
            if cancelled.load(Ordering::SeqCst) {
                return;
            }
            let Some(image) = thumbnail_at(&path, at, cancelled) else {
                continue;
            };
            if tx.send((at, image)).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    })
}

/// The arguments for one filmstrip frame.
///
/// `-noaccurate_seek` with keyframes-only decoding hands back the keyframe the
/// seek landed on, rather than decoding forward to the exact instant; for a
/// picture 106 pixels wide the nearest keyframe is the right frame.
pub fn thumbnail_args(path: &str, at: f64) -> Vec<String> {
    [
        "-v",
        "error",
        "-nostdin",
        "-threads",
        "1",
        "-skip_frame",
        "nokey",
        "-noaccurate_seek",
        "-ss",
        &format!("{at:.3}"),
        "-i",
        path,
        "-an",
        "-sn",
        "-frames:v",
        "1",
        "-vf",
        &format!("scale={THUMB_W}:{THUMB_H}"),
        "-threads",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn thumbnail_at(path: &str, at: f64, cancelled: &Arc<AtomicBool>) -> Option<egui::ColorImage> {
    const FRAME_BYTES: usize = THUMB_W * THUMB_H * 3;
    let mut child = command(&ffmpeg_path())
        .args(thumbnail_args(path, at))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut frame: Vec<u8> = Vec::with_capacity(FRAME_BYTES);
    if let Some(pipe) = child.stdout.take() {
        read_all(pipe, cancelled, |chunk| frame.extend_from_slice(chunk));
    }
    let _ = child.kill();
    let _ = child.wait();
    (frame.len() >= FRAME_BYTES)
        .then(|| egui::ColorImage::from_rgb([THUMB_W, THUMB_H], &frame[..FRAME_BYTES]))
}
