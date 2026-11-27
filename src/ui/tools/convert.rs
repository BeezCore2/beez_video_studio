//! Convert Format.

use std::path::PathBuf;

use eframe::egui;

use super::*;
use crate::ui::media::{Frames, Source, media_ext};
use crate::video::audio::{self, Format};
use crate::video::codec::{Container, Speed};
use crate::video::compress::AudioMode;
use crate::video::convert::{self, Options};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Video(Container),
    Audio(Format),
}

pub struct ConvertState {
    pub files: Vec<Source>,
    target: Target,
    opts: Options,
    audio_kbps: u32,
}

impl Default for ConvertState {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            target: Target::Video(Container::Mp4),
            opts: Options::default(),
            audio_kbps: 192,
        }
    }
}

impl ConvertState {
    pub fn add(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        for p in paths {
            self.files.push(Source::new(p, ctx));
        }
    }

    fn ext(&self, info: &MediaInfo) -> String {
        match self.target {
            Target::Video(c) => c.ext().to_string(),
            Target::Audio(f) => f
                .ext(info.audio.first().map(|a| a.codec.as_str()).unwrap_or(""))
                .to_string(),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, frames: &mut Frames, caps: &Caps) -> Option<Task> {
        section(ui, "Files");
        file_list(
            ui,
            frames,
            &mut self.files,
            "videos",
            &media_ext(),
            "Add one file, or several to convert them all",
            false,
        );

        section(ui, "Convert to");
        card(ui, |ui| {
            ui.label(strong("Video"));
            let video: Vec<(Target, &str)> = vec![
                (Target::Video(Container::Mp4), "MP4"),
                (Target::Video(Container::Mkv), "MKV"),
                (Target::Video(Container::WebM), "WebM"),
                (Target::Video(Container::Mov), "MOV"),
                (Target::Video(Container::Avi), "AVI"),
            ];
            segmented(ui, &mut self.target, &video, 80.0);
            ui.add_space(6.0);
            ui.label(strong("Audio only"));
            let audio: Vec<(Target, &str)> = vec![
                (Target::Audio(Format::Mp3), "MP3"),
                (Target::Audio(Format::Aac), "M4A"),
                (Target::Audio(Format::Opus), "Opus"),
                (Target::Audio(Format::Flac), "FLAC"),
                (Target::Audio(Format::Wav), "WAV"),
            ];
            segmented(ui, &mut self.target, &audio, 80.0);
        });

        let first = self.files.iter().find_map(|f| f.info.clone());
        match self.target {
            Target::Video(c) => {
                self.opts.container = c;
                if let Some(v) = self.opts.video
                    && !c.video_choices().contains(&v)
                {
                    self.opts.video = None;
                }
                if let Some(info) = &first
                    && let Some(copy) = convert::copies_video(info, &self.opts)
                {
                    if copy {
                        hint(
                            ui,
                            I_FLASH,
                            "The video can be copied into the new format: fast, and no quality is lost.",
                        );
                    } else {
                        hint(
                            ui,
                            I_INFO,
                            &format!(
                                "The video will be re-encoded, because {} files can't hold {}.",
                                c.ext().to_uppercase(),
                                info.video
                                    .as_ref()
                                    .map(|v| v.codec.to_uppercase())
                                    .unwrap_or_default()
                            ),
                        );
                    }
                }
                section(ui, "Video");
                let o = &mut self.opts;
                setting(
                    ui,
                    I_VIDEO,
                    "Video codec",
                    "Automatic copies the video when the format allows it",
                    |ui| {
                        let mut choice = o.video;
                        egui::ComboBox::from_id_salt("v_codec")
                            .selected_text(choice.map(|c| c.name()).unwrap_or("Automatic"))
                            .width(260.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut choice, None, "Automatic");
                                for &vc in c.video_choices() {
                                    let ok =
                                        caps.encoders.is_empty() || caps.has(vc.software_encoder());
                                    ui.add_enabled_ui(ok, |ui| {
                                        ui.selectable_value(&mut choice, Some(vc), vc.name());
                                    });
                                }
                            });
                        o.video = choice;
                    },
                );
                let reencode = first
                    .as_ref()
                    .map(|i| convert::copies_video(i, o) == Some(false))
                    .unwrap_or(true);
                if reencode {
                    setting(
                        ui,
                        I_SETTINGS,
                        "Quality",
                        "Lower numbers mean better quality and bigger files (18 ≈ visually lossless)",
                        |ui| {
                            ui.add(egui::Slider::new(&mut o.crf, 14..=34));
                        },
                    );
                    let codec = o.video.unwrap_or_else(|| c.default_video());
                    setting(ui, I_FLASH, "Encoder", "", |ui| {
                        backend_combo(ui, "v_backend", &mut o.backend, codec, caps);
                    });
                    setting(ui, I_SPEED, "Encoding speed", "", |ui| {
                        let items: Vec<(Speed, &str)> =
                            Speed::ALL.iter().rev().map(|s| (*s, s.name())).collect();
                        combo(ui, "v_speed", &mut o.speed, &items);
                    });
                }
                section(ui, "Audio");
                setting(
                    ui,
                    I_AUDIO,
                    "Audio",
                    "Keep copies the sound untouched when the format allows it",
                    |ui| {
                        combo(
                            ui,
                            "v_audio",
                            &mut o.audio,
                            &[
                                (AudioMode::Keep, "Keep original"),
                                (AudioMode::Encode(256), "256 kbps"),
                                (AudioMode::Encode(192), "192 kbps"),
                                (AudioMode::Encode(128), "128 kbps"),
                                (AudioMode::Remove, "Remove audio"),
                            ],
                        );
                    },
                );
            }
            Target::Audio(f) => {
                if f.lossy() {
                    section(ui, "Audio");
                    setting(
                        ui,
                        I_AUDIO,
                        "Bitrate",
                        "Higher sounds better and makes bigger files",
                        |ui| {
                            let mut items = vec![
                                (320, "320 kbps"),
                                (256, "256 kbps"),
                                (192, "192 kbps"),
                                (128, "128 kbps"),
                                (96, "96 kbps"),
                            ];
                            if f == Format::Mp3 {
                                items.insert(0, (0, "Variable (high quality)"));
                            }
                            combo(ui, "a_kbps", &mut self.audio_kbps, &items);
                        },
                    );
                }
            }
        }

        let ready: Vec<MediaInfo> = self.files.iter().filter_map(|f| f.info.clone()).collect();
        let loading = self.files.iter().any(|f| f.loading());
        let n = ready.len();
        let label = if n > 1 {
            format!("Convert {n} files")
        } else {
            "Convert & save".into()
        };
        if footer(ui, n > 0 && !loading, &labeled(I_SAVE, &label)) {
            let (target, opts, caps) = (self.target, self.opts.clone(), caps.clone());
            let kbps = if self.target == Target::Audio(Format::Mp3) || self.audio_kbps != 0 {
                self.audio_kbps
            } else {
                192
            };
            let build = move |i: &MediaInfo, out: &std::path::Path| match target {
                Target::Video(_) => convert::build(i, out, &opts, &caps),
                Target::Audio(f) => audio::build_extract(i, out, f, kbps, 0),
            };
            if n == 1 {
                let info = ready.into_iter().next()?;
                let out = save_as(Some(&info.path), "converted", &self.ext(&info))?;
                return job_task("Converting", build(&info, &out));
            }
            let dir = pick_folder(Some(&ready[0].path))?;
            let items = batch_outputs(ready, &dir, "converted", |i| self.ext(i));
            return batch_task("Converting", "Converted", items, build);
        }
        None
    }
}
