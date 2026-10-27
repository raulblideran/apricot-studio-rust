// Apricot Studio -- a small video trimmer.
// Copyright (C) 2026 Raul Blideran
//
// This program is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License as published by the Free Software
// Foundation, either version 3 of the License, or (at your option) any later
// version. See the LICENSE file for the full text.
//! The Codex folder: codecs a Windows build downloads on first run.
//!
//! The Windows zip carries the app alone. Started without ffmpeg, it asks
//! whether to fetch it, and puts what it fetches in a `Codex` folder beside the
//! .exe, where [`crate::ffmpeg`] looks first. Two parts:
//!
//! * **ffmpeg** -- BtbN's "gpl" build from GitHub, the most complete one
//!   published: x264, x265, SVT-AV1, libvpx and the rest, which is everything
//!   the export rules can ask for. It is rebuilt daily under a fixed name, so
//!   it cannot be pinned; the zip is checked against the `checksums.sha256`
//!   published with it instead.
//! * **QTGMC** -- VapourSynth, the Python it runs in, the QTGMC script and the
//!   plugins it calls. These are fixed versions with fixed checksums, below:
//!   a file that changes upstream fails the download rather than running.
//!   Nothing downloaded is executed while installing; Python's embeddable
//!   build and the VapourSynth wheel are both laid out by unpacking alone.
//!
//! Everything is unpacked into `Codex.partial` first and moved into place only
//! once every file has arrived and checked out, so a cancelled or failed
//! download leaves the Codex folder as it was.

use std::path::{Path, PathBuf};

/// The folder's name, beside the executable.
pub const FOLDER: &str = "Codex";

/// Where the QTGMC set lives inside the Codex folder.
pub const VAPOURSYNTH: &str = "vapoursynth";
const SITE_PACKAGES: &str = "Lib/site-packages";

/// The Codex folder for this executable.
pub fn codex_dir() -> Option<PathBuf> {
    crate::ffmpeg::executable_dir().map(|dir| dir.join(FOLDER))
}

/// Where `vspipe` sits in a Codex folder. VapourSynth finds its Python by
/// walking up from here to `python.exe` and `python3.dll` three folders above,
/// which is exactly where the embeddable Python is unpacked. Always `.exe`:
/// what the Codex holds is the Windows build, wherever it was unpacked.
pub fn vspipe_in(codex: &Path) -> PathBuf {
    codex
        .join(VAPOURSYNTH)
        .join(SITE_PACKAGES)
        .join("vapoursynth")
        .join("vspipe.exe")
}

/// Whether the QTGMC set is all there.
pub fn qtgmc_ready(codex: &Path) -> bool {
    let root = codex.join(VAPOURSYNTH);
    vspipe_in(codex).is_file()
        && QTGMC_SET.iter().all(|pinned| {
            pinned.take.iter().all(|take| match take {
                Take::File { to, .. } => root.join(to).is_file(),
                Take::Tree { .. } => true,
            })
        })
}

// --------------------------------------------------------------------------
// What is fetched
// --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Ffmpeg,
    Qtgmc,
}

impl Part {
    pub fn label(self) -> &'static str {
        match self {
            Part::Ffmpeg => "ffmpeg",
            Part::Qtgmc => "QTGMC (VapourSynth)",
        }
    }

    /// Roughly what it costs to fetch, for the question that asks.
    pub fn megabytes(self) -> u32 {
        match self {
            Part::Ffmpeg => 195,
            Part::Qtgmc => 62,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zip,
    SevenZip,
    /// A single file, kept as it is.
    Plain,
}

/// What to keep from an archive. Paths use `/` and are relative to the
/// archive on one side and to `Codex/vapoursynth` on the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Take {
    /// One file, from `from` in the archive to `to`.
    File {
        from: &'static str,
        to: &'static str,
    },
    /// Everything under `from` (all of it, when empty) into `into`, less any
    /// file ending in `skip` -- the debug symbols, which are most of the
    /// VapourSynth wheel by weight.
    Tree {
        from: &'static str,
        into: &'static str,
        skip: &'static str,
    },
}

/// One pinned download.
#[derive(Clone, Copy, Debug)]
pub struct Pinned {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub kind: Kind,
    pub take: &'static [Take],
}

/// The QTGMC set. QTGMC at its default settings calls three plugins -- mv,
/// rgvs and znedi3 -- besides VapourSynth's own filters; LSMASH reads the
/// source. The plugin versions and archive checksums are the ones VapourSynth's
/// plugin index (vsrepo) lists for 64-bit Windows.
pub const QTGMC_SET: &[Pinned] = &[
    Pinned {
        name: "Python 3.13.14 (embeddable)",
        url: "https://www.python.org/ftp/python/3.13.14/python-3.13.14-embed-amd64.zip",
        sha256: "90b4e5b9898b72d744650524bff92377c367f44bd5fbd09e3148656c080ad907",
        kind: Kind::Zip,
        take: &[Take::Tree {
            from: "",
            into: "",
            skip: "",
        }],
    },
    Pinned {
        name: "VapourSynth R81",
        url: "https://files.pythonhosted.org/packages/6c/c1/d1448dd3a34ac2d3f439496167b3ffe14d6c0b7f9647d468c2292a06e4c2/vapoursynth-81-cp312-abi3-win_amd64.whl",
        sha256: "8de011b9d372ca1593ccbe6ea164c5d093ef44905a69597fa3d8099d38ce381c",
        kind: Kind::Zip,
        take: &[Take::Tree {
            from: "vapoursynth/",
            into: "Lib/site-packages/vapoursynth/",
            skip: ".pdb",
        }],
    },
    Pinned {
        name: "vsutil 0.8.0",
        url: "https://files.pythonhosted.org/packages/35/89/daadc3c87b3c0f1f5ca36fd900222ea4c023af6a4acefd033ef7f55df3b5/vsutil-0.8.0-py3-none-any.whl",
        sha256: "b14751680f1aa47a8671a943932e160fc3b6029b3e60227875fd73d7a47dac86",
        kind: Kind::Zip,
        take: &[Take::Tree {
            from: "vsutil/",
            into: "Lib/site-packages/vsutil/",
            skip: "",
        }],
    },
    Pinned {
        name: "havsfunc (QTGMC)",
        url: "https://raw.githubusercontent.com/HomeOfVapourSynthEvolution/havsfunc/5baf9c2a1dac9da4de182cf3a4e12041c3e0d48b/havsfunc/havsfunc.py",
        sha256: "95f7342271e688e20845aaaa47d548ed6aede274a514bac666145d5514d0fe58",
        kind: Kind::Plain,
        take: &[Take::File {
            from: "",
            to: "Lib/site-packages/havsfunc.py",
        }],
    },
    Pinned {
        name: "MVTools v24",
        url: "https://github.com/dubhatervapoursynth/vapoursynth-mvtools/releases/download/v24/vapoursynth-mvtools-v24-win64.7z",
        sha256: "b9883003eed100d4ffd44344b658554076b89da6d45041b378e675720426bc95",
        kind: Kind::SevenZip,
        take: &[Take::File {
            from: "libmvtools.dll",
            to: "Lib/site-packages/vapoursynth/plugins/libmvtools.dll",
        }],
    },
    Pinned {
        name: "znedi3 r3.3",
        url: "https://github.com/sekrit-twc/znedi3/releases/download/r3.3/znedi3_r3.3.7z",
        sha256: "2f6512a971cbc94660b716f9689ee9a55ab6d9fd29484ea62d92a20151d56fd6",
        kind: Kind::SevenZip,
        take: &[
            Take::File {
                from: "vsznedi3.dll",
                to: "Lib/site-packages/vapoursynth/plugins/vsznedi3.dll",
            },
            // znedi3 looks for its weights beside its own DLL.
            Take::File {
                from: "nnedi3_weights.bin",
                to: "Lib/site-packages/vapoursynth/plugins/nnedi3_weights.bin",
            },
        ],
    },
    Pinned {
        name: "RemoveGrain R1",
        url: "https://github.com/vapoursynth/vs-removegrain/releases/download/R1/removegrain-r1.7z",
        sha256: "dc408c560de537d273e78607a776f357ba80d9a7a0074d3c10bc9f3e3376d35f",
        kind: Kind::SevenZip,
        take: &[Take::File {
            from: "win64/RemoveGrainVS.dll",
            to: "Lib/site-packages/vapoursynth/plugins/RemoveGrainVS.dll",
        }],
    },
    Pinned {
        name: "L-SMASH-Works r1296",
        url: "https://github.com/HomeOfAviSynthPlusEvolution/L-SMASH-Works/releases/download/1296.0.0.0/L-SMASH-Works-r1296.0.0.0.7z",
        sha256: "1b9a5ad3ed9ce4b57f6a2463dba4ead54f38aa20d42bd9980032cd14c0f270c3",
        kind: Kind::SevenZip,
        take: &[Take::File {
            from: "x64/LSMASHSource.dll",
            to: "Lib/site-packages/vapoursynth/plugins/LSMASHSource.dll",
        }],
    },
];

/// Where an archive entry lands under `Codex/vapoursynth`, or `None` if it is
/// not wanted. Entries that would climb out of the folder are never wanted.
pub fn destination(take: &[Take], entry: &str) -> Option<String> {
    let entry = entry.replace('\\', "/");
    if entry.ends_with('/') || entry.split('/').any(|part| part == "..") || entry.starts_with('/') {
        return None;
    }
    take.iter().find_map(|take| match *take {
        Take::File { from, to } => (entry == from).then(|| to.to_string()),
        Take::Tree { from, into, skip } => {
            let rest = entry.strip_prefix(from)?;
            if rest.is_empty() || (!skip.is_empty() && rest.ends_with(skip)) {
                return None;
            }
            Some(format!("{into}{rest}"))
        }
    })
}

/// The embeddable Python ignores `PYTHONPATH` and reads its search path from
/// its `._pth` file alone, so that is where `Lib/site-packages` has to be
/// named for `import havsfunc` to work.
pub fn with_site_packages(pth: &str) -> String {
    if pth.lines().any(|line| line.trim() == "Lib\\site-packages") {
        return pth.to_string();
    }
    let mut out = pth.trim_end().to_string();
    out.push_str("\r\nLib\\site-packages\r\n");
    out
}

// --------------------------------------------------------------------------
// ffmpeg: the newest release build, checked against its own checksum list
// --------------------------------------------------------------------------

pub const FFMPEG_RELEASE: &str = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest";

/// The newest numbered release among BtbN's assets, in its static "gpl"
/// flavour: `ffmpeg-n9.0-latest-win64-gpl-9.0.zip`, not the `-shared` one
/// (which needs its DLLs kept beside it), not `master` (a development
/// snapshot), not `lgpl` (no x264 or x265).
pub fn pick_ffmpeg_asset<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    names
        .into_iter()
        .filter_map(|name| {
            let version = name
                .strip_prefix("ffmpeg-n")?
                .strip_suffix(".zip")?
                .split_once("-latest-win64-gpl-")?;
            let (major, minor) = version.0.split_once('.')?;
            let key: (u32, u32) = (major.parse().ok()?, minor.parse().ok()?);
            (version.1 == version.0).then_some((key, name))
        })
        .max_by_key(|(key, _)| *key)
        .map(|(_, name)| name)
}

/// The checksum `sha256sum` printed for `name`.
pub fn checksum_for(list: &str, name: &str) -> Option<String> {
    list.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// The two binaries, wherever the zip keeps them (`<name>/bin/ffmpeg.exe`).
pub fn ffmpeg_destination(entry: &str) -> Option<&'static str> {
    let entry = entry.replace('\\', "/");
    ["ffmpeg.exe", "ffprobe.exe"]
        .into_iter()
        .find(|name| entry.ends_with(&format!("/bin/{name}")))
}

#[cfg(any(windows, feature = "codex"))]
pub use fetch::{Download, Progress};

#[cfg(any(windows, feature = "codex"))]
mod fetch {
    //! The download itself, on a worker thread.

    use std::fs::{self, File};
    use std::io::{self, Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use sha2::{Digest, Sha256};

    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    pub enum Progress {
        /// What is being fetched, and how far along that one file is.
        Step {
            label: String,
            done: u64,
            total: u64,
        },
        Finished(Result<(), String>),
    }

    /// A download in progress. Dropping it does not stop it; [`cancel`] does.
    ///
    /// [`cancel`]: Download::cancel
    pub struct Download {
        rx: Receiver<Progress>,
        cancelled: Arc<AtomicBool>,
        running: bool,
    }

    impl Download {
        pub fn start(parts: Vec<Part>, codex: PathBuf, ctx: egui::Context) -> Self {
            let (tx, rx) = channel();
            let cancelled = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&cancelled);
            thread::spawn(move || {
                let result = run(&parts, &codex, &tx, &flag, &ctx);
                let result = if flag.load(Ordering::SeqCst) {
                    Err("Download cancelled".to_string())
                } else {
                    result
                };
                let _ = tx.send(Progress::Finished(result));
                ctx.request_repaint();
            });
            Self {
                rx,
                cancelled,
                running: true,
            }
        }

        pub fn cancel(&self) {
            self.cancelled.store(true, Ordering::SeqCst);
        }

        pub fn running(&self) -> bool {
            self.running
        }

        /// Everything reported since the last call.
        pub fn poll(&mut self) -> Vec<Progress> {
            let mut out = Vec::new();
            loop {
                match self.rx.try_recv() {
                    Ok(event) => {
                        if matches!(event, Progress::Finished(_)) {
                            self.running = false;
                        }
                        out.push(event);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.running = false;
                        break;
                    }
                }
            }
            out
        }
    }

    struct Job<'a> {
        tx: &'a Sender<Progress>,
        cancelled: &'a AtomicBool,
        ctx: &'a egui::Context,
        agent: ureq::Agent,
        scratch: PathBuf,
    }

    impl Job<'_> {
        fn report(&self, label: &str, done: u64, total: u64) {
            let _ = self.tx.send(Progress::Step {
                label: label.to_string(),
                done,
                total,
            });
            self.ctx.request_repaint();
        }

        fn check(&self) -> Result<(), String> {
            if self.cancelled.load(Ordering::SeqCst) {
                Err("cancelled".into())
            } else {
                Ok(())
            }
        }

        fn get(&self, url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
            self.agent
                .get(url)
                .header(
                    "User-Agent",
                    concat!("apricot-studio/", env!("CARGO_PKG_VERSION")),
                )
                .call()
                .map_err(|e| format!("could not reach {}: {e}", host_of(url)))
        }

        fn text(&self, url: &str) -> Result<String, String> {
            self.get(url)?
                .into_body()
                .with_config()
                .limit(4 * 1024 * 1024)
                .read_to_string()
                .map_err(|e| format!("could not read {}: {e}", host_of(url)))
        }

        /// Fetch to a file, hashing as it arrives, and refuse it unless the
        /// hash is the one expected.
        fn fetch(&self, label: &str, url: &str, sha256: &str, to: &Path) -> Result<(), String> {
            let response = self.get(url)?;
            let total = response
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let mut body = response.into_body().into_reader();
            let mut file = File::create(to).map_err(|e| write_error(to, e))?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; 256 * 1024];
            let mut done = 0u64;
            let mut last_report = 0u64;
            loop {
                self.check()?;
                let n = body
                    .read(&mut buf)
                    .map_err(|e| format!("the download of {label} broke off: {e}"))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                file.write_all(&buf[..n]).map_err(|e| write_error(to, e))?;
                done += n as u64;
                if done - last_report >= 1024 * 1024 {
                    last_report = done;
                    self.report(label, done, total);
                }
            }
            self.report(label, done, total.max(done));
            let got = hex(&hasher.finalize());
            if !got.eq_ignore_ascii_case(sha256) {
                return Err(format!(
                    "{label} did not match its checksum, so it was not installed \
                     (expected {sha256}, got {got})"
                ));
            }
            Ok(())
        }
    }

    fn run(
        parts: &[Part],
        codex: &Path,
        tx: &Sender<Progress>,
        cancelled: &AtomicBool,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        let stage = codex.with_file_name(format!("{FOLDER}.partial"));
        let _ = fs::remove_dir_all(&stage);
        fs::create_dir_all(&stage).map_err(|e| write_error(&stage, e))?;
        let scratch = stage.join("downloads");
        fs::create_dir_all(&scratch).map_err(|e| write_error(&scratch, e))?;

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .build()
            .into();
        let job = Job {
            tx,
            cancelled,
            ctx,
            agent,
            scratch,
        };

        let result = (|| {
            for part in parts {
                match part {
                    Part::Ffmpeg => fetch_ffmpeg(&job, &stage)?,
                    Part::Qtgmc => fetch_qtgmc(&job, &stage.join(VAPOURSYNTH))?,
                }
            }
            job.check()?;
            install(parts, &stage, codex)
        })();
        let _ = fs::remove_dir_all(&stage);
        result
    }

    fn fetch_ffmpeg(job: &Job, stage: &Path) -> Result<(), String> {
        job.report("Finding the newest ffmpeg", 0, 0);
        let release: serde_json::Value = serde_json::from_str(&job.text(FFMPEG_RELEASE)?)
            .map_err(|e| format!("GitHub answered with something unreadable: {e}"))?;
        let assets = release
            .get("assets")
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();
        let url_of = |name: &str| {
            assets.iter().find_map(|asset| {
                (asset.get("name")?.as_str()? == name)
                    .then(|| {
                        asset
                            .get("browser_download_url")?
                            .as_str()
                            .map(String::from)
                    })
                    .flatten()
            })
        };
        let names: Vec<&str> = assets
            .iter()
            .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
            .collect();
        let name = pick_ffmpeg_asset(names.iter().copied())
            .ok_or("the ffmpeg release has no Windows build in it")?
            .to_string();
        let sums_url = url_of("checksums.sha256").ok_or("the ffmpeg release has no checksums")?;
        let sha256 = checksum_for(&job.text(&sums_url)?, &name)
            .ok_or("the ffmpeg checksum list does not cover the build")?;
        let zip_url = url_of(&name).ok_or("the ffmpeg build has no download link")?;

        let archive = job.scratch.join(&name);
        job.fetch("ffmpeg", &zip_url, &sha256, &archive)?;
        job.report("Unpacking ffmpeg", 0, 0);
        let mut found = 0;
        unzip(&archive, |entry| {
            let name = ffmpeg_destination(entry)?;
            found += 1;
            Some(stage.join(name))
        })?;
        if found < 2 {
            return Err("the ffmpeg download did not contain ffmpeg.exe and ffprobe.exe".into());
        }
        let _ = fs::remove_file(&archive);
        Ok(())
    }

    fn fetch_qtgmc(job: &Job, root: &Path) -> Result<(), String> {
        for pinned in QTGMC_SET {
            job.check()?;
            let file = job
                .scratch
                .join(pinned.url.rsplit('/').next().unwrap_or("download"));
            job.fetch(pinned.name, pinned.url, pinned.sha256, &file)?;
            job.report(&format!("Unpacking {}", pinned.name), 0, 0);
            let place = |entry: &str| destination(pinned.take, entry).map(|to| root.join(to));
            match pinned.kind {
                Kind::Zip => unzip(&file, place)?,
                Kind::SevenZip => un7z(&file, place)?,
                Kind::Plain => {
                    for take in pinned.take {
                        if let Take::File { to, .. } = take {
                            let target = root.join(to);
                            make_parent(&target)?;
                            fs::copy(&file, &target).map_err(|e| write_error(&target, e))?;
                        }
                    }
                }
            }
            let _ = fs::remove_file(&file);
        }

        let pth = fs::read_dir(root)
            .map_err(|e| write_error(root, e))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "_pth"))
            .ok_or("the Python download had no ._pth file")?;
        let text = fs::read_to_string(&pth).map_err(|e| write_error(&pth, e))?;
        fs::write(&pth, with_site_packages(&text)).map_err(|e| write_error(&pth, e))?;

        // Every pinned file is in place -- the same test the app uses.
        let probe = root.parent().unwrap_or(root);
        if !qtgmc_ready(probe) {
            return Err("the QTGMC download is incomplete".into());
        }
        Ok(())
    }

    /// Move what was staged into the Codex folder, replacing what was there.
    fn install(parts: &[Part], stage: &Path, codex: &Path) -> Result<(), String> {
        fs::create_dir_all(codex).map_err(|e| write_error(codex, e))?;
        for part in parts {
            match part {
                Part::Ffmpeg => {
                    for name in ["ffmpeg.exe", "ffprobe.exe"] {
                        let target = codex.join(name);
                        let _ = fs::remove_file(&target);
                        fs::rename(stage.join(name), &target)
                            .map_err(|e| write_error(&target, e))?;
                    }
                }
                Part::Qtgmc => {
                    let target = codex.join(VAPOURSYNTH);
                    let _ = fs::remove_dir_all(&target);
                    fs::rename(stage.join(VAPOURSYNTH), &target)
                        .map_err(|e| write_error(&target, e))?;
                }
            }
        }
        Ok(())
    }

    fn unzip(archive: &Path, mut place: impl FnMut(&str) -> Option<PathBuf>) -> Result<(), String> {
        let file = File::open(archive).map_err(|e| write_error(archive, e))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("a damaged archive: {e}"))?;
        for i in 0..zip.len() {
            let mut entry = zip
                .by_index(i)
                .map_err(|e| format!("a damaged archive: {e}"))?;
            if entry.is_dir() {
                continue;
            }
            let name = entry
                .name()
                .map_err(|e| format!("a damaged archive: {e}"))?
                .into_owned();
            let Some(target) = place(&name) else {
                continue;
            };
            make_parent(&target)?;
            let mut out = File::create(&target).map_err(|e| write_error(&target, e))?;
            io::copy(&mut entry, &mut out).map_err(|e| write_error(&target, e))?;
        }
        Ok(())
    }

    fn un7z(archive: &Path, mut place: impl FnMut(&str) -> Option<PathBuf>) -> Result<(), String> {
        let mut failure: Option<String> = None;
        sevenz_rust2::decompress_file_with_extract_fn(
            archive,
            Path::new(""),
            |entry, reader, _| {
                if entry.is_directory() {
                    return Ok(true);
                }
                // A solid archive is one compressed stream, so an entry that is
                // not wanted still has to be read through for the next one to
                // start in the right place -- and to pass its own CRC check.
                let Some(target) = place(entry.name()) else {
                    io::copy(reader, &mut io::sink())?;
                    return Ok(true);
                };
                let written = make_parent(&target).and_then(|()| {
                    let mut out = File::create(&target).map_err(|e| write_error(&target, e))?;
                    io::copy(reader, &mut out).map_err(|e| write_error(&target, e))
                });
                if let Err(e) = written {
                    failure = Some(e);
                    return Ok(false);
                }
                Ok(true)
            },
        )
        .map_err(|e| format!("a damaged archive: {e}"))?;
        failure.map_or(Ok(()), Err)
    }

    fn make_parent(path: &Path) -> Result<(), String> {
        match path.parent() {
            Some(parent) => fs::create_dir_all(parent).map_err(|e| write_error(parent, e)),
            None => Ok(()),
        }
    }

    fn write_error(path: &Path, error: io::Error) -> String {
        format!("could not write {}: {error}", path.display())
    }

    fn host_of(url: &str) -> &str {
        url.split("://")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or(url)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
