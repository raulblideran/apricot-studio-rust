// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! The window: layout, transport, keyboard, and everything the buttons do.

use egui::{Align, Color32, Key, Layout, RichText, Vec2};

use crate::codex::{self, Part};
use crate::deinterlace::{
    self, BwdifProfile, Deinterlace, Method, QtgmcPreset, Scan, BWDIF_PROFILES, QTGMC_PRESETS,
};
use crate::dnd;
use crate::export::{self, Audio, Exporter, Format, Options, Quality};
use crate::media::{self, fmt_tc, parse_tc, Loader, MediaInfo};
use crate::paths;
use crate::platform;
use crate::player::Player;
use crate::sandbox;
use crate::settings::Settings;
use crate::theme::{self, Theme};
use crate::ui::chrome::{self, Kind, Look};
use crate::ui::dialogs::{self, Act, Button, Dialog};
use crate::ui::timeline::Timeline;

/// The metainfo carries this too, because that is what the flatpak build reads.
/// A test asserts the two agree, so a release cannot go out labelled as the one
/// before it -- which is the mistake this constant exists to make impossible.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Below this an "export" cannot be a real clip, and the original must be kept.
pub const MIN_CLIP_BYTES: u64 = 1024;

/// The button labels that carry a glyph, and the two names the picture answers
/// to.
///
/// A typed glyph is a bet that every face this binary ships has it, and
/// "❚❚  Pause" lost that bet: U+275A is in none of the four -- Ubuntu-Light,
/// NotoEmoji, emoji-icon-font, Rajdhani -- so the button read as two empty
/// boxes. U+23F8 below comes out of emoji-icon-font, the same face that already
/// draws the ⏮ on Start. They are named here rather than typed at the call
/// site so tests/window.rs can check every one of them against the real fonts.
pub const PLAY: &str = "▶  Play";
pub const PAUSE: &str = "⏸  Pause";
pub const START: &str = "⏮  Start";
pub const BACK_10S: &str = "◀◀  10s";
pub const BACK_1F: &str = "◀  1f";
pub const FWD_1F: &str = "1f  ▶";
pub const FWD_10S: &str = "10s  ▶▶";
/// The bare nudges beside the In and Out timecodes.
pub const STEP_BACK: &str = "◀";
pub const STEP_FWD: &str = "▶";

/// Every label above, for the test that checks each glyph resolves.
pub const GLYPH_LABELS: &[&str] = &[
    PLAY, PAUSE, START, BACK_10S, BACK_1F, FWD_1F, FWD_10S, STEP_BACK, STEP_FWD,
];

/// The header menu an interlaced file brings with it, while nothing is chosen.
pub const DEINTERLACE: &str = "Deinterlace";

/// What the empty picture says, and the name its click answers to.
pub const OPEN_PROMPT: &str = "Open a video, or drop one here";
/// What the picture is once there is something on it.
pub const PICTURE: &str = "Play/pause";

/// What the export may be, described by intent rather than by numbers. The
/// presets scale against the source's own bitrate, so "smaller" means smaller
/// than this file.
pub struct QualityChoice {
    pub label: &'static str,
    pub tooltip: &'static str,
    pub quality: Quality,
    pub target: i64,
}

pub const QUALITY_CHOICES: &[QualityChoice] = &[
    QualityChoice {
        label: "Same as source",
        tooltip: "Reproduces the original: same bitrate, same everything.\n\
                  Cuts on a keyframe are copied rather than re-encoded.",
        quality: Quality::Source,
        target: 0,
    },
    QualityChoice {
        label: "Smaller file",
        tooltip: "About half the original bitrate. Noticeably lighter,\n\
                  still good enough to watch full screen.",
        quality: Quality::Smaller,
        target: 0,
    },
    QualityChoice {
        label: "Smallest worth keeping",
        tooltip: "Roughly a third of the original bitrate.\n\
                  Visibly compressed on close inspection.",
        quality: Quality::Smallest,
        target: 0,
    },
    QualityChoice {
        label: "Fit 10 MB",
        tooltip: "Discord's free upload limit.",
        quality: Quality::Source,
        target: 10_000_000,
    },
    QualityChoice {
        label: "Fit 25 MB",
        tooltip: "Comfortable for most chat apps and email.",
        quality: Quality::Source,
        target: 25_000_000,
    },
    QualityChoice {
        label: "Fit 50 MB",
        tooltip: "Discord Nitro Basic.",
        quality: Quality::Source,
        target: 50_000_000,
    },
    QualityChoice {
        label: "Fit 100 MB",
        tooltip: "Discord Nitro.",
        quality: Quality::Source,
        target: 100_000_000,
    },
];

/// What an export was actually made from, captured when it started.
///
/// Nothing about a finished export may be read from the currently-open file.
/// Opening another video mid-encode is easy and reasonable, and if the delete
/// prompt consulted the live selection it would offer to destroy whatever
/// happens to be loaded when the encode lands, rather than the file that was
/// cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Exported {
    pub source: String,
    pub output: String,
    pub duration: f64,
    pub kept: f64,
}

/// Headline and detail for the offer to remove the file that was cut.
///
/// Every number comes from the record captured when the export started rather
/// than from whatever happens to be open now. The wording lives out here, as
/// [`sandbox::explain`] does, so it can be tested without a modal dialog on
/// screen -- which is how the duration below came to be read off the wrong file.
pub fn delete_prompt(record: &Exported, source_bytes: u64) -> (String, String) {
    let share = if record.duration > 0.0 {
        record.kept / record.duration * 100.0
    } else {
        0.0
    };
    let text = format!("Delete “{}”?", paths::basename(&record.source));
    let detail = format!(
        "{:.0} MB  ·  {} long\n\n\
         Your clip kept {:.1}s of it — about {:.0}%. The other {:.0}% is only in \
         this file, and cannot be recovered from the clip.\n\n\
         The clip you just exported is not affected:\n{}",
        source_bytes as f64 / 1_000_000.0,
        fmt_tc(record.duration),
        record.kept,
        share,
        100.0 - share,
        paths::basename(&record.output)
    );
    (text, detail)
}

/// Which file may be offered for deletion, and the clip that earns it.
///
/// Separate from the prompt so the decision can be tested without driving a
/// modal dialog -- it is the one piece of logic here that can destroy data.
/// Returns None whenever deletion must not be offered at all.
pub fn delete_candidate(record: Option<&Exported>) -> Option<(String, String)> {
    let record = record?;
    // Deliberately the captured source, not whatever is open now: another file
    // may have been opened while this export was encoding.
    let (source, clip) = (record.source.clone(), record.output.clone());
    if source.is_empty() || clip.is_empty() {
        return None;
    }
    // Refuse unless the clip is demonstrably real. Deleting an original on the
    // strength of a zero-byte output would be a disaster.
    if !paths::is_file(&clip) || paths::size_of(&clip) < MIN_CLIP_BYTES {
        return None;
    }
    if !paths::is_file(&source) {
        return None;
    }
    if paths::same_file(&clip, &source) {
        return None;
    }
    Some((source, clip))
}

/// The dialog a codec download shows while it runs.
#[cfg(any(windows, feature = "codex"))]
fn download_dialog(label: &str, done: u64, total: u64) -> Dialog {
    let megabytes = |bytes: u64| bytes as f64 / 1_000_000.0;
    let (text, progress) = if total > 0 {
        (
            format!(
                "{label}  ·  {:.0} of {:.0} MB",
                megabytes(done),
                megabytes(total)
            ),
            done as f32 / total as f32,
        )
    } else if done > 0 {
        (format!("{label}  ·  {:.0} MB", megabytes(done)), -1.0)
    } else {
        (format!("{label}…"), -1.0)
    };
    let folder = codex::codex_dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|| codex::FOLDER.to_string());
    Dialog {
        progress: Some(progress),
        ..Dialog::message("Downloading codecs", &text, &format!("Into {folder}"))
            .with_buttons(vec![Button::new("Cancel", Act::CancelDownload).escape()])
    }
}

/// Where to get the one thing the app cannot work without.
fn missing_ffmpeg_detail() -> String {
    let where_it_looked = "It is looked for beside this application first, then on PATH.";
    let advice = if cfg!(target_os = "windows") {
        "Download them into the Codex folder with the button below, install them \
         with `winget install Gyan.FFmpeg`, or put ffmpeg.exe and ffprobe.exe in \
         the Codex folder beside apricot-studio.exe."
    } else if cfg!(target_os = "macos") {
        "Install it with `brew install ffmpeg`, or put ffmpeg and ffprobe in \
         the application bundle beside the binary."
    } else {
        "Install it from your distribution's packages -- `ffmpeg` on most, or \
         `rpm-ostree install ffmpeg` on an image-based system. The Flatpak \
         build carries its own and never needs this."
    };
    format!("{advice}\n\n{where_it_looked}")
}

/// Whether this build can fetch codecs itself: a Windows build always, any
/// other only when compiled with the `codex` feature (the test suite's).
const CAN_DOWNLOAD: bool = cfg!(any(windows, feature = "codex"));

/// What the Codex folder is still missing.
fn parts_needed() -> Vec<Part> {
    let mut parts = Vec::new();
    if !crate::ffmpeg::missing().is_empty() {
        parts.push(Part::Ffmpeg);
    }
    if codex::codex_dir().is_some_and(|dir| !codex::qtgmc_ready(&dir)) {
        parts.push(Part::Qtgmc);
    }
    parts
}

/// The question a Windows build asks before its window opens, when it has no
/// ffmpeg. Asked with the system's own message box, before anything else is
/// drawn, so the answer is in before the window needs it.
pub fn codecs_prompt() -> (String, String) {
    let parts = parts_needed();
    let megabytes: u32 = parts.iter().map(|part| part.megabytes()).sum();
    let names: Vec<&str> = parts.iter().map(|part| part.label()).collect();
    (
        "Codecs missing".to_string(),
        format!(
            "Apricot Studio needs codecs to open and export video, and they are \
             not installed on this computer.\n\n\
             Download them now? That is {} (about {megabytes} MB), from their \
             official releases on GitHub and python.org, into the \"{}\" folder \
             beside Apricot Studio.",
            names.join(" and "),
            codex::FOLDER
        ),
    )
}

/// What the delete flow is waiting on, so the answer can be acted on when it
/// arrives rather than inside the dialog.
#[derive(Clone)]
struct PendingDelete {
    source: String,
    clip: String,
}

/// The two file choosers, behind a seam.
///
/// The real ones are the desktop's, which under a Flatpak means the portal --
/// the whole reason "Locate the file…" can reach a folder the sandbox hides.
/// They are held as functions rather than called directly so the test suite can
/// click every button in the window without a modal chooser stopping the run,
/// which is how the PyQt version shipped a crashing Open button for two
/// releases.
pub struct Pickers {
    /// Given a folder to start in, the file the user picked.
    pub open: OpenPicker,
    /// Given a folder and a suggested name, where to write the clip.
    pub save: SavePicker,
}

pub type OpenPicker = Box<dyn Fn(&str) -> Option<String>>;
pub type SavePicker = Box<dyn Fn(&str, &str) -> Option<String>>;

impl Default for Pickers {
    fn default() -> Self {
        Self {
            open: Box::new(|start| {
                rfd::FileDialog::new()
                    .set_title("Open video")
                    .set_directory(start)
                    .add_filter("Video files", platform::VIDEO_EXTS)
                    .add_filter("All files", &["*"])
                    .pick_file()
                    .map(|path| path.to_string_lossy().into_owned())
            }),
            save: Box::new(|folder, name| {
                rfd::FileDialog::new()
                    .set_title("Save clip as")
                    .set_directory(folder)
                    .set_file_name(name)
                    .save_file()
                    .map(|path| path.to_string_lossy().into_owned())
            }),
        }
    }
}

pub struct App {
    settings: Settings,
    theme: &'static Theme,
    /// The user's preference, kept under Default's rules rather than the
    /// current theme's. A theme with no palette of its own paints its own
    /// colour but must not *overwrite* the choice underneath: going Default ->
    /// Cyberpunk -> Default has to come back to the accent that was picked, not
    /// to Night City crimson.
    accent: String,
    look: Look,

    info: Option<MediaInfo>,
    keyframes: Vec<f64>,
    timeline: Timeline,
    player: Player,
    exporter: Exporter,

    keyframe_loader: Loader<Vec<f64>>,
    waveform_loader: Loader<Vec<f32>>,
    thumb_loader: Loader<(f64, egui::ColorImage)>,
    scan_loader: Loader<Scan>,

    /// Whether the source is interlaced, once that is known.
    scan: Option<Scan>,
    /// The deinterlacer chosen. Off until asked for, so a cut on a keyframe
    /// of an interlaced file is still a lossless copy.
    deinterlacer: Option<Method>,
    /// Where each method's profile was last left, so switching between the
    /// two does not lose it.
    bwdif_profile: BwdifProfile,
    qtgmc_preset: QtgmcPreset,
    /// Whether QTGMC can run here: `None` until the background check answers.
    qtgmc_ok: Option<bool>,
    qtgmc_check: Option<std::sync::mpsc::Receiver<bool>>,
    #[cfg(any(windows, feature = "codex"))]
    download: Option<codex::Download>,
    /// What the current or last download was for, so a failure can retry it.
    download_parts: Vec<Part>,

    in_text: String,
    out_text: String,
    out_dir: String,
    out_name: String,
    out_ext: String,

    fmt: Format,
    audio_index: usize,
    audio_options: Vec<(String, Audio)>,
    quality_index: usize,
    delete_source: bool,

    status: String,
    progress: Option<f32>,
    last_output: Option<String>,
    exported: Option<Exported>,
    pending_delete: Option<PendingDelete>,
    dialog: Option<Dialog>,

    pickers: Pickers,
    pending_open: Option<String>,
    /// When the window size was last written down. A resize is a drag, and a
    /// drag is sixty frames a second; the file does not need all of them.
    window_saved: std::time::Instant,
    stop_at_out: bool,
    resume_after_scrub: bool,
    first_frame: bool,
    /// Where the decoded frame was drawn this frame, so the scanline overlay
    /// can leave it alone. `None` when there is nothing to show.
    video_rect: Option<egui::Rect>,
    /// Files dropped on the window, on the one platform where the toolkit does
    /// not report them. `None` everywhere else, and in the test suite, which
    /// has no window to drop anything on.
    drops: Option<dnd::Drops>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, path: Option<String>) -> Self {
        let mut app = Self::with_context(&cc.egui_ctx, path);
        // Here rather than in `with_context`, which the test suite calls: this
        // registers a process-wide sink on the clipboard thread, and a window
        // that was never opened has nothing to drop a file on.
        app.drops = dnd::watch(&cc.egui_ctx);
        app
    }

    /// The window, built against a bare context.
    ///
    /// Separate from [`App::new`] so the test suite can drive real frames
    /// without opening a window -- which is how "every button was clicked" can
    /// be a test rather than a hope.
    pub fn with_context(ctx: &egui::Context, path: Option<String>) -> Self {
        let settings = Settings::load();
        let theme = theme::get(Some(&settings.theme));
        let accent = theme::normalise(Some(&settings.accent));

        ctx.set_fonts(theme::font_definitions());
        let look = Look {
            theme,
            accent: theme.normalise(Some(&accent)),
            quiet: false,
        };
        theme::apply(ctx, theme, &look.accent);

        let mut app = Self {
            settings,
            theme,
            accent,
            look,
            info: None,
            keyframes: Vec::new(),
            timeline: Timeline::default(),
            player: Player::default(),
            exporter: Exporter::default(),
            keyframe_loader: Loader::default(),
            waveform_loader: Loader::default(),
            thumb_loader: Loader::default(),
            scan_loader: Loader::default(),
            scan: None,
            deinterlacer: None,
            bwdif_profile: BwdifProfile::Slowest,
            qtgmc_preset: QtgmcPreset::Placebo,
            qtgmc_ok: None,
            qtgmc_check: None,
            #[cfg(any(windows, feature = "codex"))]
            download: None,
            download_parts: Vec::new(),
            in_text: fmt_tc(0.0),
            out_text: fmt_tc(0.0),
            out_dir: String::new(),
            out_name: String::new(),
            out_ext: ".mp4".to_string(),
            fmt: Format::Source,
            audio_index: 0,
            audio_options: vec![("Keep audio".to_string(), Audio::All)],
            quality_index: 0,
            delete_source: false,
            status: String::new(),
            progress: None,
            last_output: None,
            exported: None,
            pending_delete: None,
            dialog: None,
            pickers: Pickers::default(),
            pending_open: path,
            window_saved: std::time::Instant::now(),
            stop_at_out: false,
            resume_after_scrub: false,
            first_frame: true,
            video_rect: None,
            drops: None,
        };

        // Everything this app does is a call to ffmpeg, so say so once at
        // startup rather than failing per action afterwards. On Linux it is a
        // package; on Windows and macOS it is usually a download, and the
        // message names both places the app looks.
        let missing = crate::ffmpeg::missing();
        if !missing.is_empty() {
            app.status = format!("{} not found", missing.join(" and "));
            let mut dialog = Dialog::warning(
                "ffmpeg is missing",
                &format!(
                    "Apricot Studio needs {} to open or export anything.",
                    missing.join(" and ")
                ),
                &missing_ffmpeg_detail(),
            );
            if CAN_DOWNLOAD {
                dialog = dialog.with_buttons(vec![
                    Button::new("Download codecs", Act::Download(parts_needed())).default_button(),
                    Button::new("Close", Act::Close).escape(),
                ]);
            }
            app.dialog = Some(dialog);
        }
        app
    }

    /// The window, with the answer to [`codecs_prompt`] already in: a yes
    /// starts the download before the first frame is drawn.
    pub fn with_download(
        cc: &eframe::CreationContext<'_>,
        path: Option<String>,
        yes: bool,
    ) -> Self {
        let mut app = Self::new(cc, path);
        if yes {
            app.start_download(&cc.egui_ctx, parts_needed());
        }
        app
    }

    fn loaded(&self) -> bool {
        self.info.is_some()
    }

    // ----- what the outside can ask -------------------------------------
    //
    // The window is otherwise entirely private. These exist so the suite can
    // drive real frames and then ask what happened, rather than asserting on
    // pixels.

    /// Replace the file choosers. See [`Pickers`].
    pub fn set_pickers(&mut self, pickers: Pickers) {
        self.pickers = pickers;
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded()
    }

    /// What the source was found to be, once the check has answered.
    pub fn scan(&self) -> Option<Scan> {
        self.scan
    }

    /// The deinterlacer that will be applied, if any.
    pub fn deinterlacer(&self) -> Option<Method> {
        self.deinterlacer
    }

    /// Choose a deinterlacer, or none. The preview follows at once.
    pub fn set_deinterlacer(&mut self, method: Option<Method>) {
        match method {
            Some(Method::Bwdif(profile)) => self.bwdif_profile = profile,
            Some(Method::Qtgmc(preset)) => self.qtgmc_preset = preset,
            None => {}
        }
        self.deinterlacer = method;
        let parity = self.deinterlace().map(|choice| choice.parity);
        self.player.set_deinterlace(parity);
        self.on_options_changed();
    }

    fn deinterlace(&self) -> Option<Deinterlace> {
        Some(Deinterlace {
            method: self.deinterlacer?,
            parity: self.scan?.parity()?,
        })
    }

    pub fn source_path(&self) -> Option<&str> {
        self.info.as_ref().map(|info| info.path.as_str())
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    /// Where the picture was drawn on the last frame, which is the rectangle
    /// the scanline overlay is told to leave alone.
    pub fn video_rect(&self) -> Option<egui::Rect> {
        self.video_rect
    }

    pub fn theme_key(&self) -> &'static str {
        self.theme.key
    }

    /// The accent the user chose, which a theme owning its colour must not
    /// overwrite.
    pub fn chosen_accent(&self) -> &str {
        &self.accent
    }

    /// The accent actually on screen, which under such a theme is its own.
    pub fn painted_accent(&self) -> &str {
        &self.look.accent
    }

    pub fn selection(&self) -> (f64, f64) {
        (self.timeline.in_point(), self.timeline.out_point())
    }

    pub fn keyframes(&self) -> &[f64] {
        &self.keyframes
    }

    pub fn destination(&self) -> String {
        self.output_path()
    }

    pub fn badge_text(&self) -> Option<String> {
        self.badge().map(|(text, _)| text)
    }

    /// The headline of whatever dialog is up, if one is.
    pub fn dialog_text(&self) -> Option<&str> {
        self.dialog.as_ref().map(|dialog| dialog.text.as_str())
    }

    pub fn dismiss_dialog(&mut self) {
        self.dialog = None;
    }

    pub fn is_playing(&self) -> bool {
        self.player.playing()
    }

    pub fn exporting(&self) -> bool {
        self.exporter.running()
    }

    pub fn delete_armed(&self) -> bool {
        self.delete_source
    }

    pub fn position(&self) -> f64 {
        self.player.position()
    }

    pub fn seek_to(&mut self, seconds: f64) {
        self.seek(seconds);
    }

    pub fn set_in_point(&mut self, seconds: f64) {
        self.timeline.set_in(seconds);
        self.sync_marks();
    }

    pub fn set_out_point(&mut self, seconds: f64) {
        self.timeline.set_out(seconds);
        self.sync_marks();
    }

    /// Where the clip will be written, as the two fields the user sees.
    pub fn set_destination(&mut self, folder: &str, name: &str) {
        self.out_dir = folder.to_string();
        self.out_name = name.to_string();
    }

    // ----- theme --------------------------------------------------------

    /// Restyle everything around a theme and accent, and remember both.
    ///
    /// The single choke point: anything that a switch has to reach goes here,
    /// because a second path is how the timeline or the overlay ends up still
    /// wearing the previous theme.
    fn apply_look(&mut self, ctx: &egui::Context, theme: &'static Theme, accent: Option<&str>) {
        self.theme = theme;
        if let Some(colour) = accent {
            self.accent = theme::normalise(Some(colour));
        }
        self.look.theme = theme;
        self.look.accent = theme.normalise(Some(&self.accent));
        theme::apply(ctx, theme, &self.look.accent);
        self.settings.theme = theme.key.to_string();
        self.settings.accent = self.accent.clone();
        self.settings.save();
    }

    // ----- loading ------------------------------------------------------

    fn load(&mut self, ctx: &egui::Context, path: &str) {
        let path = paths::absolute(&paths::expand_user(path));
        let info = match media::probe(&path) {
            Ok(info) => info,
            Err(error) => {
                self.report_open_failure(&path, &error);
                return;
            }
        };

        self.cancel_loaders();
        self.keyframes.clear();
        self.timeline.reset(info.duration);
        self.keyframe_loader = media::load_keyframes(&path, ctx.clone());
        self.waveform_loader = media::load_waveform(&info, ctx.clone());
        self.thumb_loader = media::load_thumbnails(&info, ctx.clone());
        self.scan_loader = media::detect_scan(&info, ctx.clone());
        self.scan = None;

        self.player.open(&info);
        // A new file starts progressive until shown otherwise, and with no
        // deinterlacer: the last file's choice says nothing about this one.
        self.deinterlacer = None;
        self.rebuild_audio_options(&info);
        self.info = Some(info);
        // The two calls above hand out enabled controls on the strength of the
        // file alone. Only this knows that GIF has no use for either, so
        // without it a reload leaves the quality box live and inert.
        self.on_options_changed();
        self.suggest_output(&path);
        self.sync_marks();
        self.status.clear();
        self.last_output = None;
        self.stop_at_out = false;
        self.settings.push_recent(&path);
    }

    fn cancel_loaders(&mut self) {
        self.keyframe_loader.cancel();
        self.waveform_loader.cancel();
        self.thumb_loader.cancel();
        self.scan_loader.cancel();
    }

    /// Say why a file would not open, and offer whatever can be done about it.
    ///
    /// Only the sandbox case gets a dialog of its own. Everything else is
    /// ffprobe telling the truth, and repeating it is the most useful thing
    /// this can do.
    fn report_open_failure(&mut self, path: &str, error: &str) {
        let trouble = sandbox::diagnose(path);
        if trouble.kind != sandbox::Kind::Blocked {
            self.dialog = Some(Dialog::warning(
                "Cannot open",
                &paths::basename(path),
                error,
            ));
            return;
        }
        let (text, detail) = sandbox::explain(&trouble);
        let command = sandbox::override_command(&trouble.folder);
        self.dialog = Some(
            Dialog::warning("Cannot open", &text, &detail).with_buttons(vec![
                // The one fix that works without leaving the app, so it is the
                // default.
                Button::new("Locate the file…", Act::Locate(trouble.folder.clone()))
                    .default_button(),
                Button::new("Copy command", Act::CopyCommand(command)),
                Button::new("Close", Act::Close).escape(),
            ]),
        );
    }

    /// Unload the current video and go back to the empty window.
    fn close_file(&mut self) {
        if !self.loaded() {
            return;
        }
        if self.exporter.running() {
            self.dialog = Some(Dialog::message(
                "Export in progress",
                "Cancel the export before closing this file.",
                "",
            ));
            return;
        }
        self.close_source();
        self.status.clear();
    }

    /// Let go of a file that no longer exists.
    fn close_source(&mut self) {
        self.player.close();
        self.cancel_loaders();
        self.info = None;
        self.scan = None;
        self.deinterlacer = None;
        self.keyframes.clear();
        self.timeline.reset(0.0);
        self.out_dir.clear();
        self.out_name.clear();
        self.out_ext = ".mp4".to_string();
        self.in_text = fmt_tc(0.0);
        self.out_text = fmt_tc(0.0);
        self.last_output = None;
    }

    fn open_dialog(&mut self, ctx: &egui::Context, start: Option<String>) {
        let start = start.unwrap_or_else(|| match self.info.as_ref() {
            Some(info) => paths::dirname(&info.path),
            None => paths::videos_dir().to_string_lossy().into_owned(),
        });
        if let Some(path) = (self.pickers.open)(&start) {
            self.load(ctx, &path);
        }
    }

    // ----- export options -----------------------------------------------

    /// Only offer what the file actually has.
    fn rebuild_audio_options(&mut self, info: &MediaInfo) {
        self.audio_options.clear();
        self.audio_index = 0;
        if !info.has_audio() {
            self.audio_options
                .push(("No audio in source".to_string(), Audio::None));
            return;
        }
        if info.audio.len() > 1 {
            self.audio_options
                .push((format!("All {} tracks", info.audio.len()), Audio::All));
            for track in &info.audio {
                self.audio_options
                    .push((track.label(), Audio::Track(track.index)));
            }
        } else {
            self.audio_options
                .push(("Keep audio".to_string(), Audio::All));
        }
        self.audio_options
            .push(("Mute  ·  no audio".to_string(), Audio::None));
    }

    fn options(&self) -> Options {
        let audio = self
            .audio_options
            .get(self.audio_index)
            .map(|(_, audio)| *audio)
            .unwrap_or(Audio::All);
        let choice = &QUALITY_CHOICES[self.quality_index.min(QUALITY_CHOICES.len() - 1)];
        Options {
            fmt: self.fmt,
            audio,
            // A size target is meaningless for GIF, whose rate control is a
            // palette.
            target_bytes: if self.fmt == Format::Gif {
                0
            } else {
                choice.target
            },
            quality: choice.quality,
            deinterlace: self.deinterlace(),
        }
    }

    /// Folder and name recombined, with the extension the format implies.
    fn output_path(&self) -> String {
        let folder = paths::expand_user(self.out_dir.trim());
        let name = self.out_name.trim();
        if folder.is_empty() || name.is_empty() {
            return String::new();
        }
        let ext = match self.info.as_ref() {
            Some(info) => self.options().ext_for(info),
            None => "mp4".to_string(),
        };
        // Typing "clip.mp4" into the name box should not yield "clip.mp4.mp4".
        let (stem, typed) = paths::split_ext(name);
        let name = if typed.trim_start_matches('.').eq_ignore_ascii_case(&ext) {
            stem
        } else {
            name.to_string()
        };
        paths::join(&folder, &format!("{name}.{ext}"))
    }

    /// Fill in a destination that will not collide with anything.
    fn suggest_output(&mut self, source: &str) {
        let ext = match self.info.as_ref() {
            Some(info) => self.options().ext_for(info),
            None => "mp4".to_string(),
        };
        // Next to the source is the useful answer, but a file located through
        // the portal sits in a one-file directory nothing can be written into.
        // Pick the folder before naming the clip, so the collision check below
        // runs against the folder the clip will actually land in.
        let beside = paths::join(&sandbox::output_dir_for(source), &paths::basename(source));
        let suggestion = export::default_output(&beside, Some(&ext));
        self.out_dir = paths::dirname(&suggestion);
        self.out_name = paths::split_ext(&paths::basename(&suggestion)).0;
        self.out_ext = format!(".{ext}");
    }

    fn on_format_changed(&mut self) {
        if let Some(info) = self.info.as_ref() {
            // The name is the user's; only the extension follows the format.
            self.out_ext = format!(".{}", self.options().ext_for(info));
        }
        self.on_options_changed();
    }

    fn on_options_changed(&mut self) {
        // Nothing to recompute: the badge is built from the current state every
        // frame. Kept as a named step because the places that must not forget
        // to reconsider the options are the same ones the PyQt version listed.
    }

    /// Say plainly which of the two very different paths an export will take.
    fn badge(&self) -> Option<(String, Color32)> {
        let info = self.info.as_ref()?;
        let options = self.options();
        let start = self.timeline.in_point();
        let span = (self.timeline.out_point() - start).max(0.0);
        let size = export::estimate_bytes(info, &options, span);
        // "about" because a quality-driven encode comes in under its ceiling on
        // easy footage; only a size target is close to exact.
        let estimate = if size > 0 {
            format!("  ·  about {:.0} MB", size as f64 / 1_000_000.0)
        } else {
            String::new()
        };

        // An interlaced source left as it is gets said so, since the choice
        // that would change it only appears for such a file.
        let estimate = if self.scan.is_some_and(|scan| scan != Scan::Progressive)
            && self.deinterlacer.is_none()
        {
            format!("{estimate}  ·  interlaced")
        } else {
            estimate
        };

        if options.inherits_everything() && export::on_keyframe(start, &self.keyframes).is_some() {
            // Colour carries the distinction, and it is the theme's lossless
            // green rather than the accent -- an accent that could stand in for
            // it would make a re-encode look free.
            return Some((
                format!("Lossless copy  ·  instant, bit-perfect{estimate}"),
                theme::c32(self.theme.lossless),
            ));
        }
        let label = export::encoder_label(info, &options);
        let label = match options.deinterlace {
            Some(choice) => format!("{label} · {}", choice.method.name()),
            None => label,
        };
        let detail = if options.target_bytes > 0 {
            format!("fitting {} MB", options.target_bytes / 1_000_000)
        } else if options.quality != Quality::Source {
            QUALITY_CHOICES[self.quality_index].label.to_lowercase()
        } else {
            "matched to source".to_string()
        };
        Some((
            format!("Re-encode  ·  {detail}  ·  {label}{estimate}"),
            self.look.accent(),
        ))
    }

    // ----- playback -----------------------------------------------------

    fn toggle_play(&mut self) {
        if !self.loaded() {
            return;
        }
        if !self.player.playing() {
            self.stop_at_out = false;
        }
        self.player.toggle();
    }

    fn play_selection(&mut self) {
        if !self.loaded() {
            return;
        }
        self.player.seek(self.timeline.in_point());
        self.stop_at_out = true;
        self.player.play();
    }

    fn seek(&mut self, seconds: f64) {
        if !self.loaded() {
            return;
        }
        self.stop_at_out = false;
        let target = seconds.clamp(0.0, self.duration());
        self.player.seek(target);
        self.timeline.set_position(target);
    }

    fn nudge(&mut self, delta: f64) {
        if self.loaded() {
            self.seek(self.player.position() + delta);
        }
    }

    fn step(&mut self, frames: f64) {
        let Some(info) = self.info.as_ref() else {
            return;
        };
        let frame = info.frame_duration();
        self.player.pause();
        self.seek(self.player.position() + frames * frame);
    }

    fn duration(&self) -> f64 {
        self.info.as_ref().map(|i| i.duration).unwrap_or(0.0)
    }

    // ----- in / out -----------------------------------------------------

    fn set_in(&mut self) {
        if self.loaded() {
            let value = self.player.position().min(self.timeline.out_point());
            self.timeline.set_in(value);
            self.sync_marks();
        }
    }

    fn set_out(&mut self) {
        if self.loaded() {
            let value = self.player.position().max(self.timeline.in_point());
            self.timeline.set_out(value);
            self.sync_marks();
        }
    }

    /// Move the in-point onto the nearest keyframe, whatever the distance.
    ///
    /// Hitting one by dragging is fiddly, and it is the difference between a
    /// ten-second encode and an instant copy.
    fn snap_in_to_keyframe(&mut self) {
        let Some(info) = self.info.as_ref() else {
            return;
        };
        if self.keyframes.is_empty() {
            return;
        }
        let target =
            export::snap_to_keyframe(self.timeline.in_point(), &self.keyframes, info.duration);
        let value = target.min(self.timeline.out_point());
        self.timeline.set_in(value);
        self.sync_marks();
    }

    fn bump_in(&mut self, frames: f64) {
        if let Some(info) = self.info.as_ref() {
            let value = self.timeline.in_point() + frames * info.frame_duration();
            self.timeline.set_in(value);
            self.sync_marks();
        }
    }

    fn bump_out(&mut self, frames: f64) {
        if let Some(info) = self.info.as_ref() {
            let value = self.timeline.out_point() + frames * info.frame_duration();
            self.timeline.set_out(value);
            self.sync_marks();
        }
    }

    fn sync_marks(&mut self) {
        self.in_text = fmt_tc(self.timeline.in_point());
        self.out_text = fmt_tc(self.timeline.out_point());
    }

    fn commit_in_text(&mut self) {
        if let Some(value) = parse_tc(&self.in_text) {
            if self.loaded() {
                self.timeline.set_in(value.min(self.timeline.out_point()));
            }
        }
        self.sync_marks();
    }

    fn commit_out_text(&mut self) {
        if let Some(value) = parse_tc(&self.out_text) {
            if self.loaded() {
                self.timeline.set_out(value.max(self.timeline.in_point()));
            }
        }
        self.sync_marks();
    }

    // ----- export -------------------------------------------------------

    fn choose_output(&mut self) {
        if !self.loaded() {
            return;
        }
        let current = self.output_path();
        let picked = (self.pickers.save)(&paths::dirname(&current), &paths::basename(&current));
        if let Some(path) = picked {
            self.out_dir = paths::dirname(&path);
            self.out_name = paths::split_ext(&paths::basename(&path)).0;
        }
    }

    fn start_export(&mut self, ctx: &egui::Context, overwrite_confirmed: bool) {
        let Some(info) = self.info.clone() else {
            return;
        };
        if self.exporter.running() {
            return;
        }
        let (start, end) = (self.timeline.in_point(), self.timeline.out_point());
        if end - start < info.frame_duration() {
            self.dialog = Some(Dialog::message(
                "Nothing to export",
                "The in and out points are at the same place.",
                "",
            ));
            return;
        }
        let output = self.output_path();
        if output.is_empty() {
            self.dialog = Some(Dialog::message(
                "No destination",
                "Choose where to save the clip.",
                "",
            ));
            return;
        }
        if paths::same_file(&output, &info.path) {
            self.dialog = Some(Dialog::warning(
                "Same file",
                "The clip would overwrite the file you are cutting.",
                "",
            ));
            return;
        }
        let folder = {
            let dir = paths::dirname(&output);
            if dir.is_empty() {
                ".".to_string()
            } else {
                dir
            }
        };
        if !paths::is_dir(&folder) {
            self.dialog = Some(Dialog::warning(
                "No such folder",
                &format!("{folder} does not exist."),
                "",
            ));
            return;
        }
        // The name is kept across exports rather than regenerated, so the
        // second export of the same name would silently replace the first. Ask
        // instead.
        if paths::exists(&output) && !overwrite_confirmed {
            self.dialog = Some(
                Dialog::message(
                    "Replace the existing clip?",
                    &format!(
                        "“{}” already exists in that folder.",
                        paths::basename(&output)
                    ),
                    "Exporting will overwrite it.",
                )
                .with_buttons(vec![
                    Button::new("Save", Act::Overwrite),
                    Button::new("Cancel", Act::Close).escape().default_button(),
                ]),
            );
            return;
        }

        self.player.pause();
        let plan = export::build(&info, start, end, &output, &self.keyframes, &self.options());
        // Pin down what is being cut now, while it is unambiguous.
        self.exported = Some(Exported {
            source: info.path.clone(),
            output: output.clone(),
            duration: info.duration,
            kept: end - start,
        });
        self.progress = Some(0.0);
        self.status = if plan.lossless {
            "Copying…".to_string()
        } else {
            format!("Encoding with {}…", plan.label)
        };
        self.look.quiet = true;
        self.exporter.start(plan, ctx.clone());
    }

    fn poll_export(&mut self) {
        for event in self.exporter.poll() {
            match event {
                export::ExportEvent::Progress {
                    fraction,
                    speed,
                    eta,
                } => {
                    self.progress = Some(fraction as f32);
                    let mut parts = vec![format!("{:.0}%", fraction * 100.0)];
                    if speed > 0.0 {
                        parts.push(format!("{speed:.2}×"));
                    }
                    if eta >= 1.0 {
                        parts.push(format!("about {eta:.0}s left"));
                    }
                    self.status = parts.join("  ·  ");
                }
                export::ExportEvent::Finished { ok, message } => {
                    self.look.quiet = false;
                    self.progress = None;
                    self.status = message;
                    if ok {
                        // The name stays exactly as typed: it is the user's,
                        // and rewriting it to a generated one discards a
                        // deliberate choice. Overwriting is prevented by asking
                        // before the export instead.
                        self.last_output =
                            self.exported.as_ref().map(|record| record.output.clone());
                        if self.delete_source {
                            self.offer_delete();
                        }
                    }
                }
            }
        }
    }

    // ----- deleting the source ------------------------------------------

    /// Offer to remove the file we just cut from. Never acts unprompted.
    fn offer_delete(&mut self) {
        let record = self.exported.take();
        let Some((source, clip)) = delete_candidate(record.as_ref()) else {
            if record.is_some() {
                self.status =
                    "Kept the original — the exported clip could not be verified.".to_string();
            }
            return;
        };
        let record = record.expect("a candidate implies a record");
        let (text, detail) = delete_prompt(&record, paths::size_of(&source));
        self.pending_delete = Some(PendingDelete { source, clip });
        self.dialog = Some(
            Dialog::warning("Delete the original?", &text, &detail).with_buttons(vec![
                // The recoverable option is the default.
                Button::new("Move to Trash", Act::Trash).default_button(),
                Button::new("Delete permanently", Act::ConfirmForever).destructive(),
                Button::new("Keep the file", Act::Keep).escape(),
            ]),
        );
    }

    fn confirm_forever(&mut self) {
        let Some(pending) = self.pending_delete.clone() else {
            return;
        };
        // Irreversible, so it gets its own deliberate confirmation.
        self.dialog = Some(
            Dialog::warning(
                "Permanently delete?",
                &format!("Permanently delete “{}”?", paths::basename(&pending.source)),
                "This does not go to the Trash. It cannot be undone by you, by this \
                 app, or by the file manager.",
            )
            .with_buttons(vec![
                Button::new("Cancel", Act::Keep).default_button().escape(),
                Button::new("Delete permanently", Act::DeleteForever).destructive(),
            ]),
        );
    }

    fn perform_delete(&mut self, forever: bool) {
        let Some(pending) = self.pending_delete.take() else {
            return;
        };
        let result = if forever {
            platform::delete_forever(&pending.source)
        } else {
            platform::move_to_trash(&pending.source)
        };
        if let Err(error) = result {
            self.dialog = Some(Dialog::warning(
                "Could not delete",
                &paths::basename(&pending.source),
                &error,
            ));
            return;
        }
        let where_ = if forever {
            "Deleted"
        } else {
            "Moved to Trash:"
        };
        self.settings.forget(&pending.source);
        // Only let go of the file if it is the one that just went away; another
        // video may have been opened while this export was running.
        if self
            .info
            .as_ref()
            .is_some_and(|info| paths::same_file(&info.path, &pending.source))
        {
            self.close_source();
        }
        self.status = format!(
            "{where_} {}  ·  clip saved as {}",
            paths::basename(&pending.source),
            paths::basename(&pending.clip)
        );
    }

    fn keep_original(&mut self) {
        if let Some(pending) = self.pending_delete.take() {
            self.status = format!("Kept {}.", paths::basename(&pending.source));
        }
    }

    // ----- per-frame ----------------------------------------------------

    fn drain_loaders(&mut self, ctx: &egui::Context) {
        if let Some(times) = self.keyframe_loader.drain().pop() {
            self.keyframes = times.clone();
            self.timeline.set_keyframes(times);
        }
        if let Some(peaks) = self.waveform_loader.drain().pop() {
            self.timeline.set_peaks(peaks);
        }
        for (at, image) in self.thumb_loader.drain() {
            // Named by time: egui keys debug output by name, and a name shared
            // by every frame of the strip says nothing about which is which.
            let texture = ctx.load_texture(
                format!("thumb@{at:.3}"),
                image,
                egui::TextureOptions::LINEAR,
            );
            self.timeline.add_thumbnail(at, texture);
        }
        if let Some(scan) = self.scan_loader.drain().pop() {
            self.scan = Some(scan);
            if scan != Scan::Progressive {
                self.check_qtgmc(ctx);
            }
        }
        if let Some(rx) = self.qtgmc_check.as_ref() {
            if let Ok(ok) = rx.try_recv() {
                self.qtgmc_ok = Some(ok);
                self.qtgmc_check = None;
            }
        }
    }

    /// Find out, off the interface thread, whether QTGMC can run. Starting
    /// VapourSynth takes most of a second, and it is only worth asking once a
    /// file has turned out to need it.
    fn check_qtgmc(&mut self, ctx: &egui::Context) {
        if self.qtgmc_ok.is_some() || self.qtgmc_check.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(deinterlace::qtgmc_available());
            ctx.request_repaint();
        });
        self.qtgmc_check = Some(rx);
    }

    // ----- the Codex download -------------------------------------------

    fn downloading(&self) -> bool {
        #[cfg(any(windows, feature = "codex"))]
        return self.download.as_ref().is_some_and(|d| d.running());
        #[cfg(not(any(windows, feature = "codex")))]
        false
    }

    fn start_download(&mut self, ctx: &egui::Context, parts: Vec<Part>) {
        let _ = ctx;
        if parts.is_empty() || self.downloading() {
            return;
        }
        self.download_parts = parts.clone();
        #[cfg(any(windows, feature = "codex"))]
        {
            let Some(dir) = codex::codex_dir() else {
                return;
            };
            self.download = Some(codex::Download::start(parts, dir, ctx.clone()));
            self.dialog = Some(download_dialog("Starting", 0, 0));
        }
    }

    #[cfg(any(windows, feature = "codex"))]
    fn poll_download(&mut self, ctx: &egui::Context) {
        let Some(download) = self.download.as_mut() else {
            return;
        };
        for event in download.poll() {
            match event {
                codex::Progress::Step { label, done, total } => {
                    self.dialog = Some(download_dialog(&label, done, total));
                }
                codex::Progress::Finished(result) => {
                    self.download = None;
                    self.finish_download(ctx, result);
                    return;
                }
            }
        }
    }

    #[cfg(any(windows, feature = "codex"))]
    fn finish_download(&mut self, ctx: &egui::Context, result: Result<(), String>) {
        match result {
            Ok(()) => {
                crate::ffmpeg::refresh();
                self.dialog = None;
                self.status = format!("Codecs installed in the {} folder", codex::FOLDER);
                if self.download_parts.contains(&Part::Qtgmc) {
                    self.qtgmc_ok = None;
                    self.qtgmc_check = None;
                    if self.scan.is_some_and(|scan| scan != Scan::Progressive) {
                        self.check_qtgmc(ctx);
                    }
                }
                if let Some(path) = self.pending_open.take() {
                    self.load(ctx, &path);
                }
            }
            Err(error) => {
                let retry = Act::Download(self.download_parts.clone());
                let (title, text) = if error == "Download cancelled" {
                    ("Download cancelled", "Nothing was installed.")
                } else {
                    ("Download failed", "The codecs could not be downloaded.")
                };
                self.dialog = Some(Dialog::warning(title, text, &error).with_buttons(vec![
                    Button::new("Try again", retry).default_button(),
                    Button::new("Close", Act::Close).escape(),
                ]));
            }
        }
    }

    fn handle_dropped(&mut self, ctx: &egui::Context) {
        // Two routes, never both at once: egui's list is what winit reports on
        // X11, Windows and macOS, and [`dnd`] is what fills the silence on
        // Wayland, where winit reports nothing at all.
        let mut dropped: Vec<String> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|file| file.path())
                .map(|path| path.to_string_lossy().into_owned())
                .collect()
        });
        if let Some(drops) = &self.drops {
            dropped.extend(drops.drain());
        }
        // Read first and discarded second, deliberately. Swapping the file out
        // from under a "delete the original?" dialog would be answering it for
        // a file that is no longer open -- and holding the drop until the
        // dialog is answered would open it minutes later, out of nowhere.
        if dropped.is_empty() || self.dialog.is_some() {
            return;
        }
        match dropped.iter().find(|path| platform::is_video(path)) {
            Some(path) => {
                let path = path.clone();
                self.load(ctx, &path);
            }
            // Saying nothing here looks exactly like a drop that never
            // arrived, which is the failure [`dnd`] exists to end.
            None => {
                self.status = format!(
                    "{} is not a video Apricot Studio can open.",
                    paths::basename(&dropped[0])
                );
            }
        }
    }

    /// Whether a file is being held over the window right now.
    fn drag_hovering(&self, ctx: &egui::Context) -> bool {
        self.drops.as_ref().is_some_and(dnd::Drops::hovering)
            || ctx.input(|i| !i.raw.hovered_files.is_empty())
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Only when nothing is being typed into, so a timecode is never
        // intercepted.
        if ctx.memory(|m| m.focused().is_some()) {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                ctx.memory_mut(|m| m.surrender_focus(m.focused().unwrap()));
            }
            return;
        }
        let (keys, modifiers) = ctx.input(|i| {
            let pressed: Vec<Key> = [
                Key::Space,
                Key::ArrowLeft,
                Key::ArrowRight,
                Key::J,
                Key::L,
                Key::K,
                Key::I,
                Key::O,
                Key::S,
                Key::Z,
                Key::Plus,
                Key::Equals,
                Key::Minus,
                Key::Home,
                Key::End,
                Key::W,
                Key::Enter,
                Key::Escape,
            ]
            .into_iter()
            .filter(|key| i.key_pressed(*key))
            .collect();
            (pressed, i.modifiers)
        });

        for key in keys {
            let (shift, ctrl) = (modifiers.shift, modifiers.command);
            match key {
                Key::Space => self.toggle_play(),
                Key::ArrowLeft => {
                    if shift {
                        self.nudge(-1.0)
                    } else {
                        self.step(-1.0)
                    }
                }
                Key::ArrowRight => {
                    if shift {
                        self.nudge(1.0)
                    } else {
                        self.step(1.0)
                    }
                }
                Key::J => self.nudge(-10.0),
                Key::L => self.nudge(10.0),
                Key::K => self.player.pause(),
                Key::I => self.set_in(),
                Key::S => self.snap_in_to_keyframe(),
                Key::Z => {
                    if self.timeline.zoomed() {
                        self.timeline.zoom_all()
                    } else {
                        self.timeline.zoom_to_selection()
                    }
                }
                Key::Plus | Key::Equals => self.timeline.zoom_in(),
                Key::Minus => self.timeline.zoom_out(),
                Key::O if ctrl => self.open_dialog(ctx, None),
                Key::O => self.set_out(),
                Key::Home => self.seek(0.0),
                Key::End => {
                    let end = self.duration();
                    self.seek(end)
                }
                Key::W if ctrl => self.close_file(),
                Key::Enter if ctrl => self.start_export(ctx, false),
                Key::Escape if self.exporter.running() => self.exporter.cancel(),
                _ => {}
            }
        }
    }

    fn handle_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        let Some(act) = dialogs::show(ctx, &self.look, &dialog) else {
            return;
        };
        self.dialog = None;
        match act {
            Act::Close => {}
            Act::Locate(folder) => {
                // The chooser is the portal's, which runs outside the sandbox,
                // so it can reach the folder this app cannot -- and grants us
                // the file.
                self.open_dialog(ctx, Some(folder));
            }
            Act::CopyCommand(command) => {
                ctx.copy_text(command);
                self.status = "Command copied. Run it, then restart Apricot Studio.".to_string();
            }
            Act::Overwrite => self.start_export(ctx, true),
            Act::Trash => self.perform_delete(false),
            Act::ConfirmForever => self.confirm_forever(),
            Act::DeleteForever => self.perform_delete(true),
            Act::Keep => self.keep_original(),
            Act::Download(parts) => self.start_download(ctx, parts),
            Act::CancelDownload => {
                #[cfg(any(windows, feature = "codex"))]
                if let Some(download) = self.download.as_ref() {
                    download.cancel();
                    // Stays up until the worker has stopped and said so.
                    self.dialog = Some(download_dialog("Cancelling", 0, 0));
                }
            }
        }
    }
}

// --------------------------------------------------------------------------
// The layout
// --------------------------------------------------------------------------

impl App {
    fn panel_frame(&self) -> egui::Frame {
        egui::Frame::NONE
            .fill(theme::c32(self.theme.background))
            .inner_margin(egui::Margin {
                left: 12,
                right: 12,
                top: 10,
                bottom: 10,
            })
    }

    fn label(&self, ui: &mut egui::Ui, value: &str) {
        ui.label(RichText::new(value).color(theme::c32(self.theme.text)));
    }

    fn dim_label(&self, ui: &mut egui::Ui, value: &str) {
        ui.label(RichText::new(value).color(theme::c32(self.theme.text_dim)));
    }

    fn separator(&self, ui: &mut egui::Ui) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 0.0, theme::c32(self.theme.surface));
    }

    fn header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let look = self.look.clone();
        ui.horizontal(|ui| {
            if chrome::button(ui, &look, "Open…", Kind::Normal, true)
                .on_hover_text("Ctrl+O")
                .clicked()
            {
                self.open_dialog(ctx, None);
            }

            let recent_button = chrome::button(ui, &look, "Recent", Kind::Menu, true)
                .on_hover_text("Files opened previously");
            let recent = self.settings.recent();
            let mut chosen: Option<String> = None;
            let mut clear = false;
            egui::Popup::menu(&recent_button).show(|ui| {
                ui.set_min_width(220.0);
                if recent.is_empty() {
                    ui.add_enabled(false, egui::Button::new("No recent files"));
                }
                for path in &recent {
                    if ui
                        .button(paths::basename(path))
                        .on_hover_text(path)
                        .clicked()
                    {
                        chosen = Some(path.clone());
                    }
                }
                if !recent.is_empty() {
                    ui.separator();
                    if ui.button("Clear list").clicked() {
                        clear = true;
                    }
                }
            });
            if clear {
                self.settings.clear_recent();
            }
            if let Some(path) = chosen {
                self.load(ctx, &path);
            }

            if self.scan.is_some_and(|scan| scan != Scan::Progressive) {
                self.deinterlace_menu(ui, &look);
            }

            ui.add_space(6.0);
            let (title, meta) = match self.info.as_ref() {
                Some(info) => (info.file_name(), info.summary()),
                None => (
                    "No file loaded".to_string(),
                    "Open a video to begin".to_string(),
                ),
            };
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(
                    RichText::new(title)
                        .color(theme::c32(self.theme.text))
                        .size(theme::base_size(self.theme) - 1.0)
                        .strong(),
                );
                ui.label(
                    RichText::new(meta)
                        .color(theme::c32(self.theme.text_dim))
                        .size(theme::base_size(self.theme) - 2.0),
                );
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if chrome::button(ui, &look, "Close", Kind::Normal, self.loaded())
                    .on_hover_text("Unload this video (Ctrl+W)")
                    .clicked()
                {
                    self.close_file();
                }
                // A theme with its own colour has nothing for the picker to
                // offer, so the swatch is not there to be pressed.
                if !self.theme.accents.is_empty() {
                    let swatch = chrome::button(ui, &look, "", Kind::Swatch, true)
                        .on_hover_text("Accent colour");
                    let mut picked: Option<String> = None;
                    egui::Popup::menu(&swatch).show(|ui| {
                        ui.set_min_width(150.0);
                        for (name, colour) in self.theme.accents {
                            // Shown as a colour rather than described, and the
                            // swatch is part of the control: the colour *is*
                            // the choice, so clicking it has to pick it.
                            let row = ui.horizontal(|ui| {
                                let (rect, _) =
                                    ui.allocate_exact_size(Vec2::splat(14.0), egui::Sense::hover());
                                ui.painter().rect(
                                    rect,
                                    egui::CornerRadius::same(3),
                                    theme::c32(colour),
                                    egui::Stroke::new(
                                        1.0,
                                        theme::c32(&theme::lighten(colour, 0.25)),
                                    ),
                                    egui::StrokeKind::Inside,
                                );
                                ui.selectable_label(*colour == self.accent, *name)
                            });
                            // Either half of the row: the name, or the colour
                            // beside it.
                            let clicked = row.inner.clicked()
                                || row.response.interact(egui::Sense::click()).clicked();
                            if clicked {
                                picked = Some((*colour).to_string());
                            }
                        }
                    });
                    if let Some(colour) = picked {
                        self.apply_look(ctx, self.theme, Some(&colour));
                    }
                }

                let theme_button = chrome::button(ui, &look, "Theme", Kind::Menu, true)
                    .on_hover_text("Appearance");
                let mut picked: Option<&'static Theme> = None;
                egui::Popup::menu(&theme_button).show(|ui| {
                    for candidate in theme::THEMES {
                        if ui
                            .selectable_label(candidate.key == self.theme.key, candidate.name)
                            .clicked()
                        {
                            picked = Some(candidate);
                        }
                    }
                });
                if let Some(theme) = picked {
                    self.apply_look(ctx, theme, None);
                }
            });
        });
    }

    fn video(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, egui::Sense::click());
        ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        self.video_rect = None;
        let loaded = self.loaded();

        // The largest control in the window, and until now it had no
        // accessible name at all -- which is also why no test could reach it.
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                true,
                if loaded { PICTURE } else { OPEN_PROMPT },
            )
        });

        match self.player.texture() {
            Some(texture) => {
                // Fit inside the panel, keeping the source's shape.
                let aspect = self.player.aspect();
                let mut size = Vec2::new(rect.width(), rect.width() / aspect);
                if size.y > rect.height() {
                    size = Vec2::new(rect.height() * aspect, rect.height());
                }
                let frame = egui::Rect::from_center_size(rect.center(), size);
                ui.painter().image(
                    texture.id(),
                    frame,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                self.video_rect = Some(frame);
            }
            None if !loaded => {
                let look = self.look.clone();
                let hovering = self.drag_hovering(&ctx);
                let (label, ink) = if hovering {
                    ("Drop to open", theme::c32(&self.look.accent))
                } else {
                    (OPEN_PROMPT, theme::c32(self.theme.text_dim))
                };
                chrome::centred_text(
                    ui.painter(),
                    &look,
                    rect,
                    label,
                    ink,
                    theme::base_size(self.theme),
                );
                // The picture is the drop target, so it is the picture that has
                // to look like one while a file is over it.
                if hovering {
                    ui.painter().rect_stroke(
                        rect.shrink(6.0),
                        self.theme.radius,
                        egui::Stroke::new(1.0, theme::c32(&self.look.accent)),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            None => {}
        }

        // The picture is the one surface with nothing else to do with a click.
        // With a file open, every video player treats it as play/pause. With
        // none, it is the biggest "open a video" target in the window -- it
        // says so, and it takes a dropped file -- so a click on it opening the
        // chooser is the only reading that is not a dead control.
        if !loaded && response.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if response.clicked() {
            if loaded {
                self.toggle_play();
            } else {
                self.open_dialog(&ctx, None);
            }
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui) {
        let look = self.look.clone();
        let loaded = self.loaded();
        ui.horizontal(|ui| {
            let playing = self.player.playing();
            let label = if playing { PAUSE } else { PLAY };
            if chrome::button(ui, &look, label, Kind::Normal, loaded)
                .on_hover_text("Space")
                .clicked()
            {
                self.toggle_play();
            }
            ui.label(
                RichText::new(format!(
                    "{} / {}",
                    fmt_tc(self.player.position()),
                    fmt_tc(self.duration())
                ))
                .monospace()
                .color(theme::c32(self.theme.text)),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if chrome::button(ui, &look, FWD_10S, Kind::Normal, loaded)
                    .on_hover_text("L")
                    .clicked()
                {
                    self.nudge(10.0);
                }
                // The two frame steps repeat while held: moving ten frames was
                // ten clicks. Everything else here is a jump, and a jump that
                // repeats is a way to lose your place.
                let forward = chrome::button(ui, &look, FWD_1F, Kind::Step, loaded)
                    .on_hover_text("Right arrow  ·  hold to keep stepping");
                for _ in 0..chrome::presses(ui, &forward) {
                    self.step(1.0);
                }
                let back = chrome::button(ui, &look, BACK_1F, Kind::Step, loaded)
                    .on_hover_text("Left arrow  ·  hold to keep stepping");
                for _ in 0..chrome::presses(ui, &back) {
                    self.step(-1.0);
                }
                if chrome::button(ui, &look, BACK_10S, Kind::Normal, loaded)
                    .on_hover_text("J")
                    .clicked()
                {
                    self.nudge(-10.0);
                }
                if chrome::button(ui, &look, START, Kind::Normal, loaded)
                    .on_hover_text("Home")
                    .clicked()
                {
                    self.seek(0.0);
                }
            });
        });
    }

    fn marks(&mut self, ui: &mut egui::Ui) {
        let look = self.look.clone();
        let loaded = self.loaded();
        ui.horizontal_wrapped(|ui| {
            self.label(ui, "In");
            let response = ui.add_enabled(
                loaded,
                egui::TextEdit::singleline(&mut self.in_text)
                    .desired_width(120.0)
                    .horizontal_align(Align::Center)
                    .font(egui::TextStyle::Monospace),
            );
            if response.lost_focus() {
                self.commit_in_text();
            }
            let back = chrome::button(ui, &look, STEP_BACK, Kind::Step, loaded)
                .on_hover_text("Back one frame  ·  hold to keep going");
            for _ in 0..chrome::presses(ui, &back) {
                self.bump_in(-1.0);
            }
            let forward = chrome::button(ui, &look, STEP_FWD, Kind::Step, loaded)
                .on_hover_text("Forward one frame  ·  hold to keep going");
            for _ in 0..chrome::presses(ui, &forward) {
                self.bump_in(1.0);
            }
            if chrome::button(ui, &look, "Set to playhead  (I)", Kind::Normal, loaded).clicked() {
                self.set_in();
            }
            if chrome::button(ui, &look, "Snap", Kind::Normal, loaded)
                .on_hover_text(
                    "Snap the in-point to the nearest keyframe (S)\n\
                     The export then becomes an instant, bit-perfect copy",
                )
                .clicked()
            {
                self.snap_in_to_keyframe();
            }

            ui.add_space(18.0);
            self.label(ui, "Out");
            let response = ui.add_enabled(
                loaded,
                egui::TextEdit::singleline(&mut self.out_text)
                    .desired_width(120.0)
                    .horizontal_align(Align::Center)
                    .font(egui::TextStyle::Monospace),
            );
            if response.lost_focus() {
                self.commit_out_text();
            }
            let back = chrome::button(ui, &look, STEP_BACK, Kind::Step, loaded)
                .on_hover_text("Back one frame  ·  hold to keep going");
            for _ in 0..chrome::presses(ui, &back) {
                self.bump_out(-1.0);
            }
            let forward = chrome::button(ui, &look, STEP_FWD, Kind::Step, loaded)
                .on_hover_text("Forward one frame  ·  hold to keep going");
            for _ in 0..chrome::presses(ui, &forward) {
                self.bump_out(1.0);
            }
            if chrome::button(ui, &look, "Set to playhead  (O)", Kind::Normal, loaded).clicked() {
                self.set_out();
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if chrome::button(ui, &look, "Play selection", Kind::Normal, loaded).clicked() {
                    self.play_selection();
                }
                let span = self.timeline.out_point() - self.timeline.in_point();
                let text = if loaded {
                    format!("Selection  {span:.3}s")
                } else {
                    "—".to_string()
                };
                ui.label(RichText::new(text).color(look.accent()).strong());
            });
        });
    }

    fn output_row(&mut self, ui: &mut egui::Ui) {
        let look = self.look.clone();
        let loaded = self.loaded();
        ui.horizontal(|ui| {
            self.label(ui, "Folder");
            // The name box and the extension keep their own width; the folder
            // takes what is left, which is the field with the longest content.
            let reserved = 320.0;
            let width = (ui.available_width() - reserved).max(160.0);
            ui.add_enabled(
                loaded,
                egui::TextEdit::singleline(&mut self.out_dir).desired_width(width),
            )
            .on_hover_text("Folder the clip is written to");
            if chrome::button(ui, &look, "…", Kind::Normal, loaded)
                .on_hover_text("Choose where to save")
                .clicked()
            {
                self.choose_output();
            }
            ui.add_space(12.0);
            self.label(ui, "Name");
            ui.add_enabled(
                loaded,
                egui::TextEdit::singleline(&mut self.out_name).desired_width(180.0),
            )
            .on_hover_text("Clip name, without the extension");
            let ext = self.out_ext.clone();
            self.dim_label(ui, &ext);
        });
    }

    fn options_row(&mut self, ui: &mut egui::Ui) {
        let loaded = self.loaded();
        let gif = self.fmt == Format::Gif;
        let has_audio = self.info.as_ref().is_some_and(|info| info.has_audio());
        ui.horizontal_wrapped(|ui| {
            self.label(ui, "Format");
            let mut format = self.fmt;
            egui::ComboBox::from_id_salt("format")
                .selected_text(match format {
                    Format::Source => "Same as source",
                    Format::WebM => "WebM  ·  VP9",
                    Format::Gif => "GIF",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut format, Format::Source, "Same as source");
                    ui.selectable_value(&mut format, Format::WebM, "WebM  ·  VP9");
                    ui.selectable_value(&mut format, Format::Gif, "GIF");
                });
            if format != self.fmt {
                self.fmt = format;
                self.on_format_changed();
            }

            ui.add_space(12.0);
            // GIF carries no audio at all, so neither box has anything to say.
            ui.add_enabled_ui(loaded && has_audio && !gif, |ui| {
                self.label(ui, "Audio");
                let mut index = self.audio_index;
                let selected = self
                    .audio_options
                    .get(index)
                    .map(|(label, _)| label.clone())
                    .unwrap_or_default();
                egui::ComboBox::from_id_salt("audio")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for (i, (label, _)) in self.audio_options.iter().enumerate() {
                            ui.selectable_value(&mut index, i, label);
                        }
                    });
                self.audio_index = index;
            });

            ui.add_space(12.0);
            ui.add_enabled_ui(loaded && !gif, |ui| {
                self.label(ui, "Quality");
                let mut index = self.quality_index;
                egui::ComboBox::from_id_salt("quality")
                    .selected_text(QUALITY_CHOICES[index].label)
                    .show_ui(ui, |ui| {
                        for (i, choice) in QUALITY_CHOICES.iter().enumerate() {
                            ui.selectable_value(&mut index, i, choice.label)
                                .on_hover_text(choice.tooltip);
                        }
                    });
                self.quality_index = index;
            });

            ui.add_space(18.0);
            // Armed, this destroys a file. It should not look like the other
            // options, and it deliberately ignores the accent.
            let destructive = theme::c32(self.theme.destructive);
            ui.scope(|ui| {
                let visuals = ui.visuals_mut();
                visuals.widgets.inactive.bg_fill = theme::c32(self.theme.sunken);
                visuals.widgets.hovered.bg_fill = theme::c32(self.theme.sunken);
                visuals.selection.bg_fill = destructive;
                visuals.widgets.inactive.bg_stroke =
                    egui::Stroke::new(1.0, theme::c32(self.theme.indicator_border));
                let colour = if self.delete_source {
                    theme::c32(self.theme.destructive_text)
                } else if loaded {
                    theme::c32(self.theme.text_dim)
                } else {
                    theme::c32(self.theme.checkbox_disabled)
                };
                let label = RichText::new("Delete original").color(colour);
                let label = if self.delete_source {
                    label.strong()
                } else {
                    label
                };
                ui.add_enabled(loaded, egui::Checkbox::new(&mut self.delete_source, label))
                    .on_hover_text(
                        "After a successful export, offer to remove the file you cut from.\n\
                         You are always asked first, and Trash is the default.",
                    );
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some((text, colour)) = self.badge() {
                    ui.label(RichText::new(text).color(colour).strong());
                }
            });
        });
    }

    /// The Deinterlace menu, in the header beside Recent: the method, then
    /// that method's profiles. Only there for an interlaced source, so a file
    /// that has no use for it is not asked about it.
    fn deinterlace_menu(&mut self, ui: &mut egui::Ui, look: &Look) {
        let current = self.deinterlacer;
        let title = match current {
            Some(method) => format!("Deinterlace · {}", method.name()),
            None => DEINTERLACE.to_string(),
        };
        let button = chrome::button(ui, look, &title, Kind::Menu, true)
            .on_hover_text("This video is interlaced: two half-pictures per frame");
        let mut picked: Option<Option<Method>> = None;
        let mut offer_qtgmc = false;
        let qtgmc_ok = self.qtgmc_ok == Some(true);
        // On Windows an absent QTGMC is one download away, so it stays
        // clickable and the click offers that download.
        let qtgmc_clickable = qtgmc_ok || (CAN_DOWNLOAD && self.qtgmc_ok == Some(false));
        egui::Popup::menu(&button).show(|ui| {
            ui.set_min_width(220.0);
            if ui
                .selectable_label(current.is_none(), "Off")
                .on_hover_text(
                    "Keep the fields as they are. A cut on a keyframe stays a lossless copy.",
                )
                .clicked()
            {
                picked = Some(None);
            }
            if ui
                .selectable_label(matches!(current, Some(Method::Bwdif(_))), "BWDIF")
                .on_hover_text("ffmpeg's own deinterlacer: quick, and clean on most footage")
                .clicked()
            {
                picked = Some(Some(Method::Bwdif(self.bwdif_profile)));
            }
            let qtgmc = ui
                .add_enabled_ui(qtgmc_clickable, |ui| {
                    ui.selectable_label(matches!(current, Some(Method::Qtgmc(_))), "QTGMC")
                })
                .inner
                .on_hover_text(
                    "VapourSynth's QTGMC: many times slower, and the best there is on \
                     motion. The preview shows BWDIF; the export uses QTGMC.",
                )
                .on_disabled_hover_text(if self.qtgmc_ok.is_none() {
                    "Looking for VapourSynth…"
                } else {
                    "Needs VapourSynth with QTGMC (havsfunc, mvtools, znedi3, \
                     removegrain and a source filter), and vspipe on PATH"
                });
            if qtgmc.clicked() {
                if qtgmc_ok {
                    picked = Some(Some(Method::Qtgmc(self.qtgmc_preset)));
                } else {
                    offer_qtgmc = true;
                }
            }

            let Some(method) = current else {
                return;
            };
            ui.separator();
            ui.label(RichText::new("Profile").color(theme::c32(self.theme.text_dim)));
            match method {
                Method::Bwdif(chosen) => {
                    for profile in BWDIF_PROFILES {
                        if ui
                            .selectable_label(profile == chosen, profile.label())
                            .on_hover_text(profile.tooltip())
                            .clicked()
                        {
                            picked = Some(Some(Method::Bwdif(profile)));
                        }
                    }
                }
                Method::Qtgmc(chosen) => {
                    for preset in QTGMC_PRESETS {
                        if ui
                            .selectable_label(preset == chosen, preset.label())
                            .clicked()
                        {
                            picked = Some(Some(Method::Qtgmc(preset)));
                        }
                    }
                }
            }
        });

        if let Some(method) = picked {
            self.set_deinterlacer(method);
        }
        if offer_qtgmc {
            self.dialog = Some(
                Dialog::message(
                    "QTGMC is not installed",
                    &format!(
                        "Download QTGMC (about {} MB) into the {} folder?",
                        Part::Qtgmc.megabytes(),
                        codex::FOLDER
                    ),
                    "It runs in VapourSynth, which comes with it, along with the Python \
                     it needs and the plugins QTGMC calls. Nothing else on this computer \
                     is changed.",
                )
                .with_buttons(vec![
                    Button::new("Download", Act::Download(vec![Part::Qtgmc])).default_button(),
                    Button::new("Not now", Act::Close).escape(),
                ]),
            );
        }
    }

    fn export_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let look = self.look.clone();
        let loaded = self.loaded();
        let running = self.exporter.running();
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if chrome::button(ui, &look, "Export clip", Kind::Primary, loaded && !running)
                    .on_hover_text("Ctrl+Return")
                    .clicked()
                {
                    self.start_export(ctx, false);
                }
                if running && chrome::button(ui, &look, "Cancel", Kind::Normal, true).clicked() {
                    self.exporter.cancel();
                }
                let has_output = self
                    .last_output
                    .as_ref()
                    .is_some_and(|path| paths::exists(path));
                if has_output
                    && chrome::button(ui, &look, "Show in folder", Kind::Normal, true).clicked()
                {
                    if let Some(path) = self.last_output.clone() {
                        platform::reveal(&path);
                    }
                }
                if let Some(fraction) = self.progress {
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_width(220.0)
                            .desired_height(8.0)
                            .corner_radius(self.theme.radius)
                            .fill(look.accent()),
                    );
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let status = self.status.clone();
                    self.dim_label(ui, &status);
                });
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.exporter.running() {
            self.exporter.cancel();
        }
        self.cancel_loaders();
        self.player.close();
    }
}

impl App {
    /// One frame of the whole window.
    ///
    /// Takes a [`Ui`](egui::Ui) rather than a context so the test suite can run
    /// real frames through it.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        if self.first_frame {
            self.first_frame = false;
            // Applied here as well as at construction: the context a window
            // ends up drawing into is not always the one the app was built
            // against, and a theme that only half arrived is worse than none.
            ctx.set_fonts(theme::font_definitions());
            theme::apply(ctx, self.theme, &self.look.accent);
            // A file named on the command line waits for a codec download that
            // is under way: opening it without ffmpeg only fails.
            if !self.downloading() {
                if let Some(path) = self.pending_open.take() {
                    self.load(ctx, &path);
                }
            }
            // Font definitions only take effect at the start of a pass, and a
            // theme with a face of its own asks for that face by name -- which
            // is not bound yet. Throwing this frame away and running it again
            // is how egui is meant to be told "I changed something a layout
            // already depended on".
            ctx.request_discard("the theme's fonts were only just installed");
            return;
        }

        self.drain_loaders(ctx);
        #[cfg(any(windows, feature = "codex"))]
        self.poll_download(ctx);
        self.poll_export();
        self.player.update(ctx);
        self.look.quiet = self.exporter.running() || self.player.playing();

        // The player owns the clock; the timeline only shows where it is.
        let position = self.player.position();
        self.timeline.set_position(position);
        if self.stop_at_out && self.player.playing() && position >= self.timeline.out_point() {
            self.player.pause();
            self.stop_at_out = false;
        }

        self.handle_dropped(ctx);
        if self.dialog.is_none() {
            self.handle_keys(ctx);
        }

        let frame = self.panel_frame();
        egui::Panel::top("header")
            .frame(frame)
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| self.header(ui, ctx));

        egui::Panel::bottom("controls")
            .frame(frame)
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 9.0;
                self.transport(ui);

                let look = self.look.clone();
                let action = self.timeline.show(ui, &look);
                if action.scrub_started {
                    // Pause while dragging the playhead, then resume if we
                    // were playing.
                    self.resume_after_scrub = self.player.playing();
                    self.player.pause();
                }
                if let Some(seconds) = action.seeked {
                    self.seek(seconds);
                }
                if action.in_changed || action.out_changed {
                    self.sync_marks();
                }
                if action.scrub_ended && self.resume_after_scrub {
                    self.resume_after_scrub = false;
                    self.player.play();
                }

                self.separator(ui);
                self.marks(ui);
                self.separator(ui);
                self.output_row(ui);
                self.options_row(ui);
                self.export_row(ui, ctx);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(ui, |ui| self.video(ui));

        // Over everything, and after everything, so no control can be drawn on
        // top of the lines -- except the picture, which the overlay skips.
        chrome::scanlines(ctx, &self.look, self.video_rect);
        self.handle_dialog(ctx);

        // Come back the size the window was left at. Written while running
        // rather than on close, because a session that ends in a crash or a
        // logout should still remember -- but at most once a second, because a
        // resize is a drag and a drag is sixty frames of it.
        let size = ctx.input(|i| i.viewport().inner_rect.map(|rect| rect.size()));
        if let Some(size) = size {
            let moved = (size.x - self.settings.window[0]).abs() > 1.0
                || (size.y - self.settings.window[1]).abs() > 1.0;
            if moved && self.window_saved.elapsed().as_secs_f32() > 1.0 {
                self.settings.window = [size.x, size.y];
                self.settings.save();
                self.window_saved = std::time::Instant::now();
            }
        }
    }
}
