//! Compress Video.

use std::path::PathBuf;

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

pub struct CompressState {
    pub files: Vec<Source>,
    level: Level,
    target_mb: f64,
    opts: Options,
}

impl Default for CompressState {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            level: Level::Preset(1),
            target_mb: 25.0,
            opts: Options::default(),
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
        file_list(ui, frames, &mut self.files, "videos", VIDEO_EXT, "Add one video, or several to compress them all with the same settings", false);

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
            setting(ui, I_PACKAGE, "Target size", "For each video. Encoded in two passes to land close to it.", |ui| {
                ui.add(egui::DragValue::new(&mut self.target_mb).range(0.5..=100_000.0).speed(0.5).suffix(" MB").max_decimals(1));
                for mb in [50.0, 25.0, 10.0, 8.0] {
                    if ui.small_button(format!("{mb} MB")).clicked() {
                        self.target_mb = mb;
                    }
                }
                if let Some(i) = &first
                    && ui.small_button("Half").on_hover_text("Half of the first video's size").clicked()
                {
                    self.target_mb = (i.size as f64 / 1024.0 / 1024.0 / 2.0 * 10.0).round() / 10.0;
                }
            });
        }

        // What the settings mean for the first video.
        if let Some(info) = &first {
            let opts = self.options();
            let (_, audio_bps) = compress::audio_args(info, opts.audio, compress::container(opts.codec));
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
        setting(ui, I_VIDEO, "Video codec", "H.265 and AV1 give smaller files at the same quality, but encode slower", |ui| {
            codec_combo(ui, "c_codec", &mut o.codec, &VideoCodec::ALL, caps);
        });
        setting(ui, I_FLASH, "Encoder", "GPU encoding is much faster; the CPU makes smaller files", |ui| {
            backend_combo(ui, "c_backend", &mut o.backend, o.codec, caps);
        });
        setting(ui, I_SPEED, "Encoding speed", "Slower settings squeeze out a smaller file", |ui| {
            let items: Vec<(Speed, &str)> = Speed::ALL.iter().rev().map(|s| (*s, s.name())).collect();
            combo(ui, "c_speed", &mut o.speed, &items);
        });
        setting(ui, I_RESIZE, "Resolution", "Lower resolutions make much smaller files. Never upscales.", |ui| {
            let mut items: Vec<(Resolution, String)> = vec![(Resolution::Original, "Keep original".into())];
            if self.level == Level::Target {
                items.push((Resolution::Auto, "Automatic (best for the size)".into()));
            }
            items.extend(HEIGHTS.iter().map(|&h| (Resolution::Limit(h), format!("{}p", h))));
            let refs: Vec<(Resolution, &str)> = items.iter().map(|(r, s)| (*r, s.as_str())).collect();
            combo(ui, "c_res", &mut o.resolution, &refs);
        });
        setting(ui, I_CLOCK, "Frame rate", "30 fps is plenty for most videos", |ui| {
            combo(ui, "c_fps", &mut o.max_fps, &[(None, "Keep original"), (Some(60), "60 fps at most"), (Some(30), "30 fps at most"), (Some(24), "24 fps at most")]);
        });

        section(ui, "Audio");
        setting(ui, I_AUDIO, "Audio", "Copying keeps the sound untouched when possible", |ui| {
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

        let ready: Vec<_> = self.files.iter().filter_map(|f| f.video().cloned()).collect();
        let loading = self.files.iter().any(|f| f.loading());
        let n = ready.len();
        let label = if n > 1 { format!("Compress {n} videos") } else { "Compress & save".into() };
        if footer(ui, n > 0 && !loading, &labeled(I_SAVE, &label)) {
            let opts = self.options();
            let ext = compress::container(opts.codec).ext();
            let caps = caps.clone();
            if n == 1 {
                let info = ready.into_iter().next()?;
                let out = save_as(Some(&info.path), "compressed", ext)?;
                return job_task("Compressing", compress::build(&info, &out, &opts, &caps));
            }
            let dir = pick_folder(Some(&ready[0].path))?;
            let items = batch_outputs(ready, &dir, "compressed", |_| ext.to_string());
            return batch_task("Compressing", "Compressed", items, move |i, out| compress::build(i, out, &opts, &caps));
        }
        None
    }
}
