//! Convert: change the container, copying streams when possible.

use eframe::egui;

use super::*;
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
    target: Target,
    opts: Options,
    audio_kbps: u32,
    all: bool,
}

impl Default for ConvertState {
    fn default() -> Self {
        Self {
            target: Target::Video(Container::Mp4),
            opts: Options::default(),
            audio_kbps: 192,
            all: false,
        }
    }
}

impl ConvertState {
    fn ext(target: Target, info: &MediaInfo) -> String {
        match target {
            Target::Video(c) => c.ext().to_string(),
            Target::Audio(f) => f
                .ext(info.audio.first().map(|a| a.codec.as_str()).unwrap_or(""))
                .to_string(),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Ctx) -> Option<Task> {
        let info = require(ui, cx, false)?.clone();
        let all_infos: Vec<MediaInfo> = cx.clips.iter().filter_map(|c| c.info().cloned()).collect();
        let all = apply_to(ui, &mut self.all, all_infos.len());
        let targets = if all { all_infos } else { vec![info.clone()] };

        group(ui, "Format");
        field(ui, "Video", "", |ui| {
            segmented(
                ui,
                &mut self.target,
                &[
                    (Target::Video(Container::Mp4), "MP4"),
                    (Target::Video(Container::Mkv), "MKV"),
                    (Target::Video(Container::WebM), "WebM"),
                    (Target::Video(Container::Mov), "MOV"),
                    (Target::Video(Container::Avi), "AVI"),
                ],
            );
        });
        field(ui, "Audio only", "", |ui| {
            segmented(
                ui,
                &mut self.target,
                &[
                    (Target::Audio(Format::Mp3), "MP3"),
                    (Target::Audio(Format::Aac), "M4A"),
                    (Target::Audio(Format::Opus), "Opus"),
                    (Target::Audio(Format::Flac), "FLAC"),
                    (Target::Audio(Format::Wav), "WAV"),
                ],
            );
        });

        match self.target {
            Target::Video(c) => {
                self.opts.container = c;
                if let Some(v) = self.opts.video
                    && !c.video_choices().contains(&v)
                {
                    self.opts.video = None;
                }
                match convert::copies_video(&info, &self.opts) {
                    Some(true) => hint(
                        ui,
                        I_FLASH,
                        "The video is copied into the new format: fast, no quality lost.",
                    ),
                    Some(false) => hint(
                        ui,
                        I_INFO,
                        &format!(
                            "The video is re-encoded: {} can't hold {}.",
                            c.ext().to_uppercase(),
                            info.video
                                .as_ref()
                                .map(|v| v.codec.to_uppercase())
                                .unwrap_or_default()
                        ),
                    ),
                    None => {}
                }
                group(ui, "Video");
                let o = &mut self.opts;
                field(
                    ui,
                    "Codec",
                    "Automatic copies the video when the format allows it.",
                    |ui| {
                        let mut choice = o.video;
                        egui::ComboBox::from_id_salt("v_codec")
                            .selected_text(choice.map(|c| c.name()).unwrap_or("Automatic"))
                            .width(ui.available_width())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut choice, None, "Automatic");
                                for &vc in c.video_choices() {
                                    let ok = cx.caps.encoders.is_empty()
                                        || cx.caps.has(vc.software_encoder());
                                    ui.add_enabled_ui(ok, |ui| {
                                        ui.selectable_value(&mut choice, Some(vc), vc.name());
                                    });
                                }
                            });
                        o.video = choice;
                    },
                );
                if convert::copies_video(&info, o) != Some(true) {
                    field(
                        ui,
                        "Quality",
                        "Lower is better and bigger (18 ≈ visually lossless).",
                        |ui| {
                            ui.add(egui::Slider::new(&mut o.crf, 14..=34));
                        },
                    );
                    let codec = o.video.unwrap_or_else(|| c.default_video());
                    field(ui, "Encoder", "", |ui| {
                        backend_combo(ui, "v_backend", &mut o.backend, codec, cx.caps)
                    });
                    field(ui, "Encoding speed", "", |ui| {
                        let items: Vec<(Speed, &str)> =
                            Speed::ALL.iter().rev().map(|s| (*s, s.name())).collect();
                        combo(ui, "v_speed", &mut o.speed, &items);
                    });
                }
                group(ui, "Audio");
                field(ui, "Audio", "", |ui| {
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
                });
            }
            Target::Audio(f) => {
                if !info.has_audio() {
                    error_note(ui, "This clip has no audio.");
                }
                if f.lossy() {
                    field(ui, "Bitrate", "", |ui| {
                        let mut items = vec![
                            (320, "320 kbps"),
                            (256, "256 kbps"),
                            (192, "192 kbps"),
                            (128, "128 kbps"),
                            (96, "96 kbps"),
                        ];
                        if f == Format::Mp3 {
                            items.insert(0, (0, "Variable (high quality)"));
                        } else if self.audio_kbps == 0 {
                            self.audio_kbps = 192;
                        }
                        combo(ui, "a_kbps", &mut self.audio_kbps, &items);
                    });
                }
            }
        }

        ui.add_space(8.0);
        let n = targets.len();
        let label = if n > 1 {
            format!("Convert {n} files…")
        } else {
            "Convert & save…".into()
        };
        if action_button(ui, n > 0, &labeled(I_SAVE, &label)) {
            let (target, opts, caps, kbps) = (
                self.target,
                self.opts.clone(),
                cx.caps.clone(),
                self.audio_kbps,
            );
            let build = move |i: &MediaInfo, out: &std::path::Path| match target {
                Target::Video(_) => convert::build(i, out, &opts, &caps),
                Target::Audio(f) => audio::build_extract(i, out, f, kbps, 0),
            };
            if n == 1 {
                let out = save_as(Some(&info.path), "converted", &Self::ext(target, &info))?;
                return job_task("Converting", build(&info, &out));
            }
            let dir = pick_folder(Some(&info.path))?;
            let items = batch_outputs(targets, &dir, "converted", |i| Self::ext(target, i));
            return batch_task("Converting", "Converted", items, build);
        }
        None
    }
}
