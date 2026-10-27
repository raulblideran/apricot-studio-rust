// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! The preview: pictures and sound, both decoded by ffmpeg.
//!
//! There is no media framework here, and that is deliberate. The app is already
//! a front end for ffmpeg -- it is what reads the file, what cuts it, and what
//! the Flatpak carries its own build of. Adding GStreamer for Linux, Media
//! Foundation for Windows and AVFoundation for macOS would mean three more
//! decoders that can disagree with the one doing the export about what a frame
//! is.
//!
//! So playback is two pipes out of one program. Video arrives as raw frames
//! scaled to the panel; audio arrives as float samples and is handed to the
//! sound card. Whichever is playing owns the clock: with sound, position is
//! however many samples the card has taken, because that is the one measure the
//! user can hear; without it, the wall clock. Frames are shown when the clock
//! reaches them, which is what keeps the picture on the sound rather than on
//! however fast frames happen to arrive.
//!
//! Seeking while paused is a separate one-shot decode of a single frame, so
//! dragging the playhead does not spin up a decoder per pixel.

use std::collections::VecDeque;
use std::io::Read;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::deinterlace::{self, Parity};
use crate::ffmpeg::{command, ffmpeg_path};
use crate::media::MediaInfo;

/// The preview is scaled down before it leaves ffmpeg. A 4K source decoded at
/// full size costs several hundred megabytes a second over a pipe to draw into
/// a panel a third that size.
const PREVIEW_MAX_W: i64 = 1280;

/// How many decoded frames may sit in front of the clock. The decoder blocks
/// when the queue is full, which is what stops a fast machine reading the whole
/// file into memory.
const FRAME_QUEUE: usize = 8;

/// Matches the volume the PyQt version opened at.
const VOLUME: f32 = 0.8;

struct Frame {
    image: egui::ColorImage,
    pts: f64,
}

/// The samples the sound card has not taken yet, and how many it has.
struct AudioSink {
    buffer: Mutex<VecDeque<f32>>,
    played: AtomicU64,
    channels: u32,
    rate: u32,
    finished: AtomicBool,
}

impl AudioSink {
    /// Seconds of sound the card has actually played.
    fn elapsed(&self) -> f64 {
        self.played.load(Ordering::Relaxed) as f64 / self.rate.max(1) as f64
    }
}

/// One running decode: the processes behind it and the queue in front.
struct Stream {
    video: Receiver<Frame>,
    cancelled: Arc<AtomicBool>,
    sink: Option<Arc<AudioSink>>,
    _audio_stream: Option<cpal::Stream>,
    /// Where this decode was started from, which the clock counts up from.
    origin: f64,
    started: Instant,
    /// A frame that has arrived but is not due yet. Held rather than shown, so
    /// the picture runs at the file's rate instead of at the decoder's.
    pending: Option<Frame>,
}

#[derive(Default)]
pub struct Player {
    info: Option<MediaInfo>,
    size: [usize; 2],
    fps: f64,

    stream: Option<Stream>,
    /// Held here rather than in [`Stream`] only because it is built inside
    /// `start_audio`, which cannot reach the stream it is being built for.
    audio_stream: Option<cpal::Stream>,
    playing: bool,
    position: f64,

    texture: Option<egui::TextureHandle>,
    /// The still decode for a seek while paused, and the generation it belongs
    /// to, so a slow one cannot overwrite a newer answer.
    still: Option<Receiver<Frame>>,
    still_generation: u64,
    pending_still: Option<f64>,
    error: Option<String>,
    /// Set when the picture is to be shown deinterlaced, with the field that
    /// comes first.
    deinterlace: Option<Parity>,
}

impl Player {
    pub fn open(&mut self, info: &MediaInfo) {
        self.close();
        // Even widths only: an odd one makes the scaler pick a chroma layout
        // the rawvideo muxer then pads, and every frame comes out sheared.
        let width = info.width.clamp(2, PREVIEW_MAX_W) & !1;
        let height = if info.width > 0 {
            ((info.height * width / info.width).max(2)) & !1
        } else {
            2
        };
        self.size = [width as usize, height as usize];
        // A preview does not need more than 60 fps, and a file claiming 1000
        // (some captures do) would otherwise be decoded at that rate.
        self.fps = if info.fps > 0.0 {
            info.fps.min(60.0)
        } else {
            30.0
        };
        self.info = Some(info.clone());
        self.position = 0.0;
        self.deinterlace = None;
        self.request_still(0.0);
    }

    /// Show the picture deinterlaced, or stop doing so, from the current frame.
    pub fn set_deinterlace(&mut self, parity: Option<Parity>) {
        if self.deinterlace == parity {
            return;
        }
        self.deinterlace = parity;
        if self.info.is_some() {
            self.seek(self.position);
        }
    }

    /// The preview's filter chain: scaled to the panel, and made whole first
    /// when the source is interlaced and that was asked for. Scaling first
    /// would blend the two fields into each other and leave the deinterlacer
    /// nothing to separate.
    fn video_filter(&self) -> String {
        let [width, height] = self.size;
        let scale = format!("scale={width}:{height}");
        match self.deinterlace {
            Some(parity) => format!("{},{scale}", deinterlace::preview_filter(parity)),
            None => scale,
        }
    }

    pub fn close(&mut self) {
        self.stop_stream();
        self.info = None;
        self.texture = None;
        self.still = None;
        self.pending_still = None;
        self.position = 0.0;
        self.playing = false;
        self.error = None;
    }

    pub fn is_open(&self) -> bool {
        self.info.is_some()
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn duration(&self) -> f64 {
        self.info.as_ref().map(|i| i.duration).unwrap_or(0.0)
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        self.texture.as_ref()
    }

    pub fn aspect(&self) -> f32 {
        if self.size[1] == 0 {
            16.0 / 9.0
        } else {
            self.size[0] as f32 / self.size[1] as f32
        }
    }

    // ----- transport ----------------------------------------------------

    pub fn play(&mut self) {
        if self.info.is_none() || self.playing {
            return;
        }
        // Starting from the very end would decode nothing at all, which reads
        // as a dead button. Every player treats it as "play it again".
        if self.position >= self.duration() - 0.02 {
            self.position = 0.0;
        }
        self.start_stream(self.position);
        self.playing = true;
    }

    pub fn pause(&mut self) {
        if !self.playing {
            return;
        }
        self.playing = false;
        self.stop_stream();
        // The picture stops on the frame that was showing, which is the one the
        // clock last passed -- so nothing has to be decoded to pause.
    }

    pub fn toggle(&mut self) {
        if self.playing {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Move the playhead, decoding whatever is needed to show it.
    pub fn seek(&mut self, seconds: f64) {
        let Some(info) = self.info.as_ref() else {
            return;
        };
        let target = seconds.clamp(0.0, info.duration);
        self.position = target;
        if self.playing {
            self.start_stream(target);
        } else {
            self.request_still(target);
        }
    }

    // ----- the running decode -------------------------------------------

    fn stop_stream(&mut self) {
        if let Some(stream) = self.stream.take() {
            stream.cancelled.store(true, Ordering::SeqCst);
            if let Some(sink) = stream.sink.as_ref() {
                sink.finished.store(true, Ordering::SeqCst);
            }
            // Dropping the cpal stream stops the callback; dropping the
            // receiver unblocks a decoder waiting on a full queue, which is
            // what lets it notice the flag and exit.
        }
    }

    fn start_stream(&mut self, from: f64) {
        self.stop_stream();
        let Some(info) = self.info.clone() else {
            return;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let [width, height] = self.size;
        let fps = self.fps;
        let filter = self.video_filter();

        let (tx, rx) = sync_channel::<Frame>(FRAME_QUEUE);
        let flag = Arc::clone(&cancelled);
        let path = info.path.clone();
        thread::spawn(move || {
            let args: Vec<String> = [
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-ss",
                &format!("{from:.6}"),
                "-i",
                &path,
                "-an",
                "-vf",
                &filter,
                // Constant rate for the preview: the presentation time of a
                // frame is then its index, and a variable-frame-rate capture
                // does not need its timestamps carried through a pipe that has
                // nowhere to put them.
                "-fps_mode",
                "cfr",
                "-r",
                &format!("{fps}"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();

            let Ok(mut child) = command(&ffmpeg_path())
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            else {
                return;
            };
            if let Some(mut pipe) = child.stdout.take() {
                let frame_bytes = width * height * 3;
                let mut buf = vec![0u8; frame_bytes];
                let mut index = 0u64;
                loop {
                    if flag.load(Ordering::SeqCst) {
                        break;
                    }
                    if pipe.read_exact(&mut buf).is_err() {
                        break;
                    }
                    let frame = Frame {
                        image: egui::ColorImage::from_rgb([width, height], &buf),
                        pts: from + index as f64 / fps,
                    };
                    index += 1;
                    // Blocks while the queue is full, which is the whole of the
                    // pacing: the decoder runs exactly as far ahead as the
                    // queue is deep.
                    if tx.send(frame).is_err() {
                        break;
                    }
                }
            }
            kill(&mut child);
        });

        let sink = self.start_audio(&info, from, &cancelled);
        self.stream = Some(Stream {
            video: rx,
            cancelled,
            sink,
            _audio_stream: None,
            origin: from,
            started: Instant::now(),
            pending: None,
        });
        // The cpal stream has to outlive the callback, so it is stored beside
        // the rest rather than dropped at the end of this function.
        if let Some(stream) = self.stream.as_mut() {
            stream._audio_stream = self.audio_stream.take();
        }
    }

    fn start_audio(
        &mut self,
        info: &MediaInfo,
        from: f64,
        cancelled: &Arc<AtomicBool>,
    ) -> Option<Arc<AudioSink>> {
        if !info.has_audio() {
            return None;
        }
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let supported = device.default_output_config().ok()?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as u32;
        let config: cpal::StreamConfig = supported.into();

        let sink = Arc::new(AudioSink {
            buffer: Mutex::new(VecDeque::new()),
            played: AtomicU64::new(0),
            channels,
            rate,
            finished: AtomicBool::new(false),
        });

        // The decoder: float samples at exactly the rate and layout the card
        // asked for, so nothing has to resample on the way in.
        let feeder = Arc::clone(&sink);
        let flag = Arc::clone(cancelled);
        let path = info.path.clone();
        thread::spawn(move || {
            let args: Vec<String> = [
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-ss",
                &format!("{from:.6}"),
                "-i",
                &path,
                "-vn",
                "-map",
                "0:a:0",
                "-f",
                "f32le",
                "-ac",
                &channels.to_string(),
                "-ar",
                &rate.to_string(),
                "-",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();

            let Ok(mut child) = command(&ffmpeg_path())
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            else {
                return;
            };
            if let Some(mut pipe) = child.stdout.take() {
                let mut raw = vec![0u8; 16 * 1024];
                loop {
                    if flag.load(Ordering::SeqCst) {
                        break;
                    }
                    // Keep at most a second or so buffered; the rest stays in
                    // the pipe, where it costs nothing and is thrown away with
                    // the process on a seek.
                    let queued = feeder.buffer.lock().map(|b| b.len()).unwrap_or(0);
                    if queued > (rate * channels) as usize {
                        thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    }
                    match pipe.read(&mut raw) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let samples = raw[..n - (n % 4)]
                                .chunks_exact(4)
                                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]) * VOLUME);
                            if let Ok(mut buffer) = feeder.buffer.lock() {
                                buffer.extend(samples);
                            }
                        }
                    }
                }
            }
            feeder.finished.store(true, Ordering::SeqCst);
            kill(&mut child);
        });

        // The callback: hand over whatever is buffered, count what was taken,
        // and pad with silence rather than glitching if the decoder is behind.
        let reader = Arc::clone(&sink);
        let stream = device
            .build_output_stream(
                config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let mut taken = 0;
                    if let Ok(mut buffer) = reader.buffer.lock() {
                        for slot in out.iter_mut() {
                            match buffer.pop_front() {
                                Some(sample) => {
                                    *slot = sample;
                                    taken += 1;
                                }
                                None => *slot = 0.0,
                            }
                        }
                    } else {
                        out.fill(0.0);
                    }
                    reader.played.fetch_add(
                        (taken / reader.channels.max(1) as usize) as u64,
                        Ordering::Relaxed,
                    );
                },
                |_error| {},
                None,
            )
            .ok()?;
        stream.play().ok()?;
        self.audio_stream = Some(stream);
        Some(sink)
    }

    fn request_still(&mut self, seconds: f64) {
        let Some(info) = self.info.as_ref() else {
            return;
        };
        // One in flight at a time. Dragging the playhead produces a seek per
        // mouse move, and starting a decoder for each would spawn dozens.
        if self.still.is_some() {
            self.pending_still = Some(seconds);
            return;
        }
        self.pending_still = None;
        self.still_generation += 1;

        let (tx, rx) = sync_channel::<Frame>(1);
        let [width, height] = self.size;
        let path = info.path.clone();
        let filter = self.video_filter();
        thread::spawn(move || {
            let args: Vec<String> = [
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-ss",
                &format!("{seconds:.6}"),
                "-i",
                &path,
                "-frames:v",
                "1",
                "-an",
                "-vf",
                &filter,
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();

            let Ok(mut child) = command(&ffmpeg_path())
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
            else {
                return;
            };
            if let Some(mut pipe) = child.stdout.take() {
                let mut buf = vec![0u8; width * height * 3];
                if pipe.read_exact(&mut buf).is_ok() {
                    let _ = tx.send(Frame {
                        image: egui::ColorImage::from_rgb([width, height], &buf),
                        pts: seconds,
                    });
                }
            }
            kill(&mut child);
        });
        self.still = Some(rx);
    }

    // ----- per-frame ----------------------------------------------------

    /// Advance the clock, show whatever frame it has reached, and say whether
    /// the window should paint again soon.
    pub fn update(&mut self, ctx: &egui::Context) {
        if let Some(rx) = self.still.as_ref() {
            match rx.try_recv() {
                Ok(frame) => {
                    self.show(ctx, frame);
                    self.still = None;
                }
                Err(TryRecvError::Disconnected) => self.still = None,
                Err(TryRecvError::Empty) => {}
            }
            if self.still.is_none() {
                if let Some(pending) = self.pending_still.take() {
                    self.request_still(pending);
                }
            }
        }

        if !self.playing {
            return;
        }

        let clock = {
            let Some(stream) = self.stream.as_ref() else {
                return;
            };
            // The sound card's own count where there is sound, because that is
            // the clock the user can hear. Wall time otherwise, and wall time
            // again once the audio track has run out before the picture has.
            let audio_running = stream.sink.as_ref().is_some_and(|sink| {
                !sink.finished.load(Ordering::SeqCst)
                    || sink.buffer.lock().map(|b| !b.is_empty()).unwrap_or(false)
            });
            match stream.sink.as_ref() {
                Some(sink) if audio_running => stream.origin + sink.elapsed(),
                _ => stream.origin + stream.started.elapsed().as_secs_f64(),
            }
        };

        // Take every frame the clock has already passed, keeping the last: a
        // machine that fell behind catches up by skipping rather than by
        // playing in slow motion. A frame that is not due yet is held for the
        // pass that reaches it.
        let mut exhausted = false;
        let mut latest: Option<Frame> = None;
        let due = |frame: &Frame| frame.pts <= clock + 1.0 / 240.0;
        if let Some(stream) = self.stream.as_mut() {
            if let Some(held) = stream.pending.take() {
                if due(&held) {
                    latest = Some(held);
                } else {
                    stream.pending = Some(held);
                }
            }
            if stream.pending.is_none() {
                loop {
                    match stream.video.try_recv() {
                        Ok(frame) if due(&frame) => latest = Some(frame),
                        Ok(frame) => {
                            stream.pending = Some(frame);
                            break;
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            exhausted = true;
                            break;
                        }
                    }
                }
            }
        }

        self.position = clock.min(self.duration());
        if let Some(frame) = latest {
            self.show(ctx, frame);
        }

        if exhausted && self.position >= self.duration() - 0.05 {
            self.playing = false;
            self.position = self.duration();
            self.stop_stream();
        }
        // Playback is the one thing here that is not event-driven.
        ctx.request_repaint();
    }

    fn show(&mut self, ctx: &egui::Context, frame: Frame) {
        let options = egui::TextureOptions::LINEAR;
        match self.texture.as_mut() {
            Some(texture) => texture.set(frame.image, options),
            None => {
                self.texture = Some(ctx.load_texture("preview", frame.image, options));
            }
        }
        let _ = frame.pts;
    }
}

fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
