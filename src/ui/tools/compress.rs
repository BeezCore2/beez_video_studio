//! Compress: quality presets or a target size, with a live size estimate.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;

use super::*;
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

    fn ui(&self, ui: &mut egui::Ui, infos: &[MediaInfo], target: bool) {
        let p = Palette::current(ui);
        let (mut before, mut after, mut complete) = (0u64, 0u64, true);
        egui::Frame::new()
            .fill(p.card)
            .stroke(egui::Stroke::new(1.0, p.border))
            .corner_radius(6.0)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for info in infos {
                    if infos.len() > 1 {
                        ui.label(egui::RichText::new(file_name(&info.path)).size(12.0));
                    }
                    match self.results.get(&info.path) {
                        Some(Ok(bytes)) => {
                            before += info.size;
                            after += bytes;
                            let smaller = *bytes < info.size;
                            ui.horizontal_wrapped(|ui| {
                                ui.label(secondary(
                                    ui,
                                    format!("{}  {I_RIGHT}", human_size(info.size)),
                                ));
                                ui.label(strong(format!("≈ {}", human_size(*bytes))).size(16.0));
                                let (text, color) = if smaller {
                                    (
                                        format!(
                                            "{:.0}% smaller",
                                            (1.0 - *bytes as f64 / info.size as f64) * 100.0
                                        ),
                                        p.success,
                                    )
                                } else {
                                    ("not smaller".to_string(), p.error)
                                };
                                ui.label(egui::RichText::new(text).color(color));
                            });
                        }
                        Some(Err(e)) => {
                            complete = false;
                            ui.add(
                                egui::Label::new(egui::RichText::new(e).color(p.error).size(12.0))
                                    .wrap(),
                            );
                        }
                        None => {
                            complete = false;
                            ui.horizontal(|ui| {
                                let now = self.current.as_ref() == Some(&info.path);
                                let waiting = self.changed_at.is_some() || self.running.is_some();
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
                }
                if infos.len() > 1 && complete && before > 0 {
                    ui.separator();
                    ui.label(strong(format!(
                        "Total {}  {I_RIGHT}  ≈ {}",
                        human_size(before),
                        human_size(after)
                    )));
                }
            });
        let note = if target {
            "The result lands close to the size you set."
        } else {
            "Measured by encoding short samples with these settings."
        };
        ui.add(egui::Label::new(secondary(ui, note).size(11.5)).wrap());
        ui.add_space(8.0);
    }
}

pub struct CompressState {
    level: Level,
    target_mb: f64,
    opts: Options,
    all: bool,
    estimate: Estimator,
}

impl Default for CompressState {
    fn default() -> Self {
        Self {
            level: Level::Preset(1),
            target_mb: 25.0,
            opts: Options::default(),
            all: false,
            estimate: Estimator::default(),
        }
    }
}

impl CompressState {
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

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Ctx) -> Option<Task> {
        let info = require(ui, cx, true)?.clone();
        let videos = cx.videos();
        let all = apply_to(ui, &mut self.all, videos.len());
        let targets: Vec<MediaInfo> = if all { videos } else { vec![info.clone()] };

        group(ui, "Size");
        let mut items: Vec<(Level, &str, &str)> = PRESETS
            .iter()
            .enumerate()
            .map(|(i, (n, d, _))| (Level::Preset(i), *n, *d))
            .collect();
        items.push((Level::Target, "Target size", "An exact file size in MB"));
        if choice_list(ui, &mut self.level, &items) {
            self.opts.resolution = if self.level == Level::Target {
                Resolution::Auto
            } else {
                Resolution::Original
            };
        }
        if self.level == Level::Target {
            field(
                ui,
                "Target size (each video)",
                "Encoded in two passes to land close to it.",
                |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.target_mb)
                                .range(0.5..=100_000.0)
                                .speed(0.5)
                                .suffix(" MB")
                                .max_decimals(1),
                        );
                        for mb in [8.0, 10.0, 25.0, 50.0] {
                            if ui.small_button(format!("{mb}")).clicked() {
                                self.target_mb = mb;
                            }
                        }
                        if ui
                            .small_button("½")
                            .on_hover_text("Half of this clip's size")
                            .clicked()
                        {
                            self.target_mb =
                                (info.size as f64 / 1024.0 / 1024.0 / 2.0 * 10.0).round() / 10.0;
                        }
                    });
                },
            );
        }

        // What the settings mean, and the expected size.
        let opts = self.options();
        let (_, audio_bps) =
            compress::audio_args(&info, opts.audio, compress::container(opts.codec));
        match compress::plan(&info, &opts, audio_bps) {
            Ok(plan) => {
                let mut parts = Vec::new();
                if let Some(bps) = plan.video_bps {
                    parts.push(format!("Video ≈ {}", fmt_bitrate(bps)));
                }
                if let Some((w, h)) = plan.size {
                    parts.push(format!("resized to {w}×{h}"));
                }
                if !parts.is_empty() {
                    hint(ui, I_INFO, &parts.join(" · "));
                }
            }
            Err(e) => error_note(ui, &e.to_string()),
        }
        group(ui, "Estimated result");
        let key = (
            opts.clone(),
            targets.iter().map(|i| i.path.clone()).collect(),
        );
        let can_start = ui.is_enabled() && !cx.caps.encoders.is_empty();
        let ctx = ui.ctx().clone();
        self.estimate
            .update(&ctx, key, targets.clone(), cx.caps, can_start);
        self.estimate.ui(ui, &targets, self.level == Level::Target);

        group(ui, "Video");
        let o = &mut self.opts;
        field(
            ui,
            "Codec",
            "H.265 and AV1 are smaller at the same quality, but slower.",
            |ui| {
                codec_combo(ui, "c_codec", &mut o.codec, &VideoCodec::ALL, cx.caps);
            },
        );
        field(ui, "Encoder", "", |ui| {
            backend_combo(ui, "c_backend", &mut o.backend, o.codec, cx.caps)
        });
        field(ui, "Encoding speed", "", |ui| {
            let items: Vec<(Speed, &str)> =
                Speed::ALL.iter().rev().map(|s| (*s, s.name())).collect();
            combo(ui, "c_speed", &mut o.speed, &items);
        });
        field(ui, "Resolution", "Never upscales.", |ui| {
            let mut items: Vec<(Resolution, String)> =
                vec![(Resolution::Original, "Keep original".into())];
            if self.level == Level::Target {
                items.push((Resolution::Auto, "Automatic (best for the size)".into()));
            }
            items.extend(
                HEIGHTS
                    .iter()
                    .map(|&h| (Resolution::Limit(h), format!("{h}p"))),
            );
            let refs: Vec<(Resolution, &str)> =
                items.iter().map(|(r, s)| (*r, s.as_str())).collect();
            combo(ui, "c_res", &mut o.resolution, &refs);
        });
        field(ui, "Frame rate", "", |ui| {
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
        });
        group(ui, "Audio");
        field(ui, "Audio", "", |ui| {
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
        });

        ui.add_space(8.0);
        let n = targets.len();
        let label = if n > 1 {
            format!("Compress {n} videos…")
        } else {
            "Compress & save…".into()
        };
        if action_button(ui, n > 0, &labeled(I_SAVE, &label)) {
            let ext = compress::container(opts.codec).ext();
            let caps = cx.caps.clone();
            if n == 1 {
                let out = save_as(Some(&info.path), "compressed", ext)?;
                self.estimate.cancel();
                return job_task("Compressing", compress::build(&info, &out, &opts, &caps));
            }
            let dir = pick_folder(Some(&info.path))?;
            self.estimate.cancel();
            let items = batch_outputs(targets, &dir, "compressed", |_| ext.to_string());
            return batch_task("Compressing", "Compressed", items, move |i, out| {
                compress::build(i, out, &opts, &caps)
            });
        }
        None
    }
}
