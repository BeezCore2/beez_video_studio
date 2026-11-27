//! Compress Video.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;

use super::*;
use crate::ui::media::{Frames, Source, VIDEO_EXT};
use crate::video::codec::{HEIGHTS, Speed};
use crate::video::compress::{self, AudioMode, Mode, Options, PRESETS, Resolution};
use crate::video::human_size;
use crate::video::probe::fmt_bitrate;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Preset(usize),
    Target,
}

/// Settings + files an estimate was made for.
type EstimateKey = (Options, Vec<PathBuf>);

/// Per-file results flowing back from the estimating thread.
type EstimateRx = mpsc::Receiver<(PathBuf, Result<u64, String>)>;

/// Background size estimates, redone shortly after the settings stop changing.
#[derive(Default)]
struct Estimator {
    key: Option<EstimateKey>,
    changed_at: Option<Instant>,
    results: HashMap<PathBuf, Result<u64, String>>,
    running: Option<(EstimateRx, Progress)>,
    /// The file being estimated right now.
    current: Option<PathBuf>,
}

/// Wait this long after the last change before estimating (sampling costs CPU).
const SETTLE: Duration = Duration::from_millis(800);

impl Estimator {
    fn cancel(&mut self) {
        if let Some((_, pr)) = self.running.take() {
            pr.cancel();
        }
        self.current = None;
    }

    /// Restart when the settings or files change; start once they have settled.
    fn update(
        &mut self,
        ctx: &egui::Context,
        key: EstimateKey,
        infos: Vec<MediaInfo>,
        caps: &Caps,
        can_start: bool,
    ) {
        if self.key.as_ref() != Some(&key) {
            self.cancel();
            self.results.clear();
            self.key = Some(key.clone());
            self.changed_at = Some(Instant::now());
        }
        if let Some((rx, _)) = &self.running {
            loop {
                match rx.try_recv() {
                    Ok((path, r)) => {
                        self.results.insert(path, r);
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        self.running = None;
                        break;
                    }
                }
            }
            self.current = self.running.as_ref().and_then(|_| {
                infos
                    .iter()
                    .map(|i| i.path.clone())
                    .find(|p| !self.results.contains_key(p))
            });
        }
        let Some(changed) = self.changed_at else {
            return;
        };
        if infos.is_empty() {
            return;
        }
        let waited = changed.elapsed();
        if waited < SETTLE || !can_start {
            ctx.request_repaint_after(SETTLE.saturating_sub(waited).max(Duration::from_millis(50)));
            return;
        }
        self.changed_at = None;
        let (tx, rx) = mpsc::channel();
        let progress = Progress::default();
        let (pr, opts, caps, ctx) = (progress.clone(), key.0.clone(), caps.clone(), ctx.clone());
        std::thread::spawn(move || {
            for info in infos {
                let r = compress::estimate(&info, &opts, &caps, &pr);
                if pr.is_cancelled() {
                    return;
                }
                if tx
                    .send((info.path.clone(), r.map_err(|e| format!("{e:#}"))))
                    .is_err()
                {
                    return;
                }
                ctx.request_repaint();
            }
        });
        self.running = Some((rx, progress));
    }

    /// The "Estimated result" card.
    fn ui(&self, ui: &mut egui::Ui, infos: &[MediaInfo], target: bool) {
        let p = Palette::current(ui);
        let mut total_before = 0u64;
        let mut total_after = 0u64;
        let mut complete = true;
        card(ui, |ui| {
            egui::Grid::new("estimates")
                .num_columns(2)
                .spacing([24.0, 6.0])
                .show(ui, |ui| {
                    for info in infos {
                        ui.label(crate::ui::media::file_name(&info.path));
                        match self.results.get(&info.path) {
                            Some(Ok(bytes)) => {
                                total_before += info.size;
                                total_after += bytes;
                                let smaller = *bytes < info.size;
                                let change = if smaller {
                                    format!(
                                        "{:.0}% smaller",
                                        (1.0 - *bytes as f64 / info.size as f64) * 100.0
                                    )
                                } else {
                                    "not smaller: try a lower quality or resolution".to_string()
                                };
                                let color = if smaller { p.success } else { p.error };
                                ui.horizontal(|ui| {
                                    ui.label(format!("{}  {I_RIGHT}  ", human_size(info.size)));
                                    ui.label(strong(format!("≈ {}", human_size(*bytes))));
                                    ui.label(
                                        egui::RichText::new(format!("  {change}")).color(color),
                                    );
                                });
                            }
                            Some(Err(e)) => {
                                complete = false;
                                ui.label(egui::RichText::new(e).color(p.error));
                            }
                            None => {
                                complete = false;
                                ui.horizontal(|ui| {
                                    let now = self.current.as_ref() == Some(&info.path);
                                    let waiting =
                                        self.changed_at.is_some() || self.running.is_some();
                                    if waiting {
                                        ui.spinner();
                                    }
                                    let text = match (&self.running, now) {
                                        (Some((_, pr)), true) => match pr.fraction() {
                                            Some(f) => format!("Estimating… {:.0}%", f * 100.0),
                                            None => "Estimating…".into(),
                                        },
                                        _ if waiting => "Waiting…".into(),
                                        _ => "—".into(),
                                    };
                                    caption(ui, &text);
                                });
                            }
                        }
                        ui.end_row();
                    }
                    if infos.len() > 1 && complete && total_before > 0 {
                        ui.label(strong("Total"));
                        ui.label(strong(format!(
                            "{}  {I_RIGHT}  ≈ {}",
                            human_size(total_before),
                            human_size(total_after)
                        )));
                        ui.end_row();
                    }
                });
        });
        if target {
            caption(
                ui,
                "With a target size, the result lands close to the size you set.",
            );
        } else {
            caption(
                ui,
                "Measured by encoding a few short samples with these settings. Videos whose scenes vary a lot can end up somewhat different.",
            );
        }
    }
}

pub struct CompressState {
    pub files: Vec<Source>,
    level: Level,
    target_mb: f64,
    opts: Options,
    estimate: Estimator,
}

impl Default for CompressState {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            level: Level::Preset(1),
            target_mb: 25.0,
            opts: Options::default(),
            estimate: Estimator::default(),
        }
    }
}

impl CompressState {
    pub fn add(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        for p in paths {
            if !self.files.iter().any(|f| f.path.as_ref() == Some(&p)) {
                self.files.push(Source::new(p, ctx));
            }
        }
    }

    fn options(&self) -> Options {
        let mut o = self.opts.clone();
        o.mode = match self.level {
            Level::Preset(i) => Mode::Quality(PRESETS[i].2),
            Level::Target => Mode::TargetSize(self.target_mb),
        };
        if self.level != Level::Target && o.resolution == Resolution::Auto {
            o.resolution = Resolution::Original;
        }
        o
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, frames: &mut Frames, caps: &Caps) -> Option<Task> {
        section(ui, "Videos");
        file_list(
            ui,
            frames,
            &mut self.files,
            "videos",
            VIDEO_EXT,
            "Add one video, or several to compress them all with the same settings",
            false,
        );

        section(ui, "Size");
        card(ui, |ui| {
            let mut items: Vec<(Level, &str, &str)> = PRESETS
                .iter()
                .enumerate()
                .map(|(i, (n, d, _))| (Level::Preset(i), *n, *d))
                .collect();
            items.push((Level::Target, "Target size", "An exact file size in MB"));
            if big_choices(ui, &mut self.level, &items) {
                self.opts.resolution = if self.level == Level::Target {
                    Resolution::Auto
                } else {
                    Resolution::Original
                };
            }
        });
        let first = self.files.iter().find_map(|f| f.video()).cloned();
        if self.level == Level::Target {
            setting(
                ui,
                I_PACKAGE,
                "Target size",
                "For each video. Encoded in two passes to land close to it.",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.target_mb)
                            .range(0.5..=100_000.0)
                            .speed(0.5)
                            .suffix(" MB")
                            .max_decimals(1),
                    );
                    for mb in [50.0, 25.0, 10.0, 8.0] {
                        if ui.small_button(format!("{mb} MB")).clicked() {
                            self.target_mb = mb;
                        }
                    }
                    if let Some(i) = &first
                        && ui
                            .small_button("Half")
                            .on_hover_text("Half of the first video's size")
                            .clicked()
                    {
                        self.target_mb =
                            (i.size as f64 / 1024.0 / 1024.0 / 2.0 * 10.0).round() / 10.0;
                    }
                },
            );
        }

        // What the settings mean for the first video.
        if let Some(info) = &first {
            let opts = self.options();
            let (_, audio_bps) =
                compress::audio_args(info, opts.audio, compress::container(opts.codec));
            match compress::plan(info, &opts, audio_bps) {
                Ok(plan) => {
                    let mut parts = vec![format!("Original: {}", human_size(info.size))];
                    if let Some(bps) = plan.video_bps {
                        parts.push(format!("video bitrate ≈ {}", fmt_bitrate(bps)));
                    }
                    if let Some((w, h)) = plan.size {
                        parts.push(format!("resized to {w}×{h}"));
                    }
                    hint(ui, I_INFO, &parts.join(" · "));
                }
                Err(e) => {
                    let p = Palette::current(ui);
                    ui.label(egui::RichText::new(format!("{I_ERROR}  {e}")).color(p.error));
                }
            }
        }

        section(ui, "Video");
        let o = &mut self.opts;
        setting(
            ui,
            I_VIDEO,
            "Video codec",
            "H.265 and AV1 give smaller files at the same quality, but encode slower",
            |ui| {
                codec_combo(ui, "c_codec", &mut o.codec, &VideoCodec::ALL, caps);
            },
        );
        setting(
            ui,
            I_FLASH,
            "Encoder",
            "GPU encoding is much faster; the CPU makes smaller files",
            |ui| {
                backend_combo(ui, "c_backend", &mut o.backend, o.codec, caps);
            },
        );
        setting(
            ui,
            I_SPEED,
            "Encoding speed",
            "Slower settings squeeze out a smaller file",
            |ui| {
                let items: Vec<(Speed, &str)> =
                    Speed::ALL.iter().rev().map(|s| (*s, s.name())).collect();
                combo(ui, "c_speed", &mut o.speed, &items);
            },
        );
        setting(
            ui,
            I_RESIZE,
            "Resolution",
            "Lower resolutions make much smaller files. Never upscales.",
            |ui| {
                let mut items: Vec<(Resolution, String)> =
                    vec![(Resolution::Original, "Keep original".into())];
                if self.level == Level::Target {
                    items.push((Resolution::Auto, "Automatic (best for the size)".into()));
                }
                items.extend(
                    HEIGHTS
                        .iter()
                        .map(|&h| (Resolution::Limit(h), format!("{}p", h))),
                );
                let refs: Vec<(Resolution, &str)> =
                    items.iter().map(|(r, s)| (*r, s.as_str())).collect();
                combo(ui, "c_res", &mut o.resolution, &refs);
            },
        );
        setting(
            ui,
            I_CLOCK,
            "Frame rate",
            "30 fps is plenty for most videos",
            |ui| {
                combo(
                    ui,
                    "c_fps",
                    &mut o.max_fps,
                    &[
                        (None, "Keep original"),
                        (Some(60), "60 fps at most"),
                        (Some(30), "30 fps at most"),
                        (Some(24), "24 fps at most"),
                    ],
                );
            },
        );

        section(ui, "Audio");
        setting(
            ui,
            I_AUDIO,
            "Audio",
            "Copying keeps the sound untouched when possible",
            |ui| {
                combo(
                    ui,
                    "c_audio",
                    &mut o.audio,
                    &[
                        (AudioMode::Keep, "Keep original"),
                        (AudioMode::Encode(192), "192 kbps (high)"),
                        (AudioMode::Encode(128), "128 kbps (good)"),
                        (AudioMode::Encode(96), "96 kbps"),
                        (AudioMode::Encode(64), "64 kbps (voice)"),
                        (AudioMode::Remove, "Remove audio"),
                    ],
                );
            },
        );

        let ready: Vec<_> = self
            .files
            .iter()
            .filter_map(|f| f.video().cloned())
            .collect();
        let loading = self.files.iter().any(|f| f.loading());

        // Expected size, shown before starting.
        if !ready.is_empty() {
            section(ui, "Estimated result");
            let key = (
                self.options(),
                ready.iter().map(|i| i.path.clone()).collect(),
            );
            // Don't sample while a real job runs (the page is disabled then) or before
            // the encoders are known.
            let can_start = ui.is_enabled() && !caps.encoders.is_empty() && !loading;
            let ctx = ui.ctx().clone();
            self.estimate
                .update(&ctx, key, ready.clone(), caps, can_start);
            self.estimate.ui(ui, &ready, self.level == Level::Target);
        }

        let n = ready.len();
        let label = if n > 1 {
            format!("Compress {n} videos")
        } else {
            "Compress & save".into()
        };
        if footer(ui, n > 0 && !loading, &labeled(I_SAVE, &label)) {
            let opts = self.options();
            let ext = compress::container(opts.codec).ext();
            let caps = caps.clone();
            if n == 1 {
                let info = ready.into_iter().next()?;
                let out = save_as(Some(&info.path), "compressed", ext)?;
                // Free the CPU for the real encode.
                self.estimate.cancel();
                return job_task("Compressing", compress::build(&info, &out, &opts, &caps));
            }
            let dir = pick_folder(Some(&ready[0].path))?;
            self.estimate.cancel();
            let items = batch_outputs(ready, &dir, "compressed", |_| ext.to_string());
            return batch_task("Compressing", "Compressed", items, move |i, out| {
                compress::build(i, out, &opts, &caps)
            });
        }
        None
    }
}
