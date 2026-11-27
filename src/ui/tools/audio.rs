//! Audio: extract, remove, normalize, volume, music.

use eframe::egui;

use super::*;
use crate::ui::media::{AUDIO_EXT, Source, VIDEO_EXT, pick_file};
use crate::video::audio::{self, Format, LOUDNESS, MusicMode};
use crate::video::codec::Container;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Extract,
    Normalize,
    Volume,
    Remove,
    Music,
}

pub struct AudioState {
    op: Op,
    format: Format,
    kbps: u32,
    track: usize,
    db: f64,
    loudness: usize,
    music: Source,
    mix: bool,
    music_volume: f32,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            op: Op::Extract,
            format: Format::Mp3,
            kbps: 0,
            track: 0,
            db: 6.0,
            loudness: 0,
            music: Source::default(),
            mix: true,
            music_volume: 0.25,
        }
    }
}

/// Extension for outputs that keep the file's kind (video stays video, audio stays audio).
fn same_kind_ext(info: &MediaInfo) -> String {
    let src = info
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if info.has_video() {
        Container::from_ext(&src)
            .map(|c| c.ext().to_string())
            .unwrap_or_else(|| "mp4".into())
    } else if audio::codec_for_ext(&info.path).is_some() {
        src
    } else {
        "m4a".into()
    }
}

impl AudioState {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Ctx) -> Option<Task> {
        let info = require(ui, cx, false)?.clone();
        self.music.poll();
        // The chosen track may not exist in a newly selected clip.
        self.track = self.track.min(info.audio.len().saturating_sub(1));

        let mut items = vec![
            (Op::Extract, "Extract audio", "Save the sound as MP3, M4A…"),
            (
                Op::Normalize,
                "Normalize loudness",
                "Even, standard loudness",
            ),
            (Op::Volume, "Volume", "Louder or quieter"),
        ];
        if info.has_video() {
            items.push((Op::Remove, "Remove audio", "Silent video, no quality loss"));
            items.push((Op::Music, "Add music", "Replace or mix in a track"));
        }
        if !info.has_audio() {
            error_note(ui, "This clip has no audio.");
            if info.has_video() {
                items.retain(|(o, _, _)| *o == Op::Music);
            } else {
                return None;
            }
        }
        if !items.iter().any(|(o, _, _)| *o == self.op) {
            self.op = items[0].0;
        }
        group(ui, "Action");
        choice_list(ui, &mut self.op, &items);

        group(ui, "Options");
        match self.op {
            Op::Extract => {
                field(
                    ui,
                    "Format",
                    "Original copies the sound exactly as it is.",
                    |ui| {
                        let items: Vec<(Format, &str)> =
                            Format::ALL.iter().map(|f| (*f, f.name())).collect();
                        combo(ui, "a_fmt", &mut self.format, &items);
                    },
                );
                if self.format.lossy() {
                    field(ui, "Bitrate", "", |ui| {
                        let mut items = vec![
                            (320, "320 kbps"),
                            (256, "256 kbps"),
                            (192, "192 kbps"),
                            (128, "128 kbps"),
                            (96, "96 kbps"),
                            (64, "64 kbps"),
                        ];
                        if self.format == Format::Mp3 {
                            items.insert(0, (0, "Variable (high quality)"));
                        } else if self.kbps == 0 {
                            self.kbps = 192;
                        }
                        combo(ui, "a_kbps", &mut self.kbps, &items);
                    });
                }
                if info.audio.len() > 1 {
                    field(ui, "Track", "", |ui| {
                        let labels: Vec<String> = info
                            .audio
                            .iter()
                            .enumerate()
                            .map(|(i, a)| {
                                let lang = if a.language.is_empty() || a.language == "und" {
                                    String::new()
                                } else {
                                    format!(" · {}", a.language)
                                };
                                format!(
                                    "Track {} · {} · {} ch{lang}",
                                    i + 1,
                                    a.codec.to_uppercase(),
                                    a.channels
                                )
                            })
                            .collect();
                        let items: Vec<(usize, &str)> = labels
                            .iter()
                            .enumerate()
                            .map(|(i, l)| (i, l.as_str()))
                            .collect();
                        combo(ui, "a_track", &mut self.track, &items);
                    });
                }
            }
            Op::Remove => hint(ui, I_FLASH, "The video is copied without re-encoding."),
            Op::Normalize => {
                field(
                    ui,
                    "Target loudness",
                    "Measured over the whole file (EBU R128), then adjusted in one even step.",
                    |ui| {
                        let items: Vec<(usize, &str)> =
                            LOUDNESS.iter().enumerate().map(|(i, l)| (i, l.0)).collect();
                        combo(ui, "a_loud", &mut self.loudness, &items);
                    },
                );
            }
            Op::Volume => {
                field(
                    ui,
                    "Change",
                    "+6 dB ≈ twice as loud. Boosts are limited so they don't distort.",
                    |ui| {
                        ui.spacing_mut().slider_width = ui.available_width() - 80.0;
                        ui.add(
                            egui::Slider::new(&mut self.db, -20.0..=20.0)
                                .suffix(" dB")
                                .step_by(0.5),
                        );
                    },
                );
            }
            Op::Music => {
                field(ui, "Music file", "", |ui| {
                    match (&self.music.path, &self.music.info) {
                        (Some(p), Some(i)) => {
                            ui.label(strong(file_name(p)));
                            caption(ui, &i.summary());
                        }
                        (Some(p), None) => {
                            ui.label(file_name(p));
                            match &self.music.error {
                                Some(e) => error_note(ui, e),
                                None => caption(ui, "Reading…"),
                            }
                        }
                        _ => {}
                    }
                    let label = if self.music.path.is_some() {
                        "Change…"
                    } else {
                        "Choose audio file…"
                    };
                    if ui
                        .add(
                            egui::Button::new(labeled(I_OPEN, label))
                                .min_size(egui::Vec2::new(ui.available_width(), 30.0)),
                        )
                        .clicked()
                        && let Some(p) = pick_file("Audio", &[AUDIO_EXT, VIDEO_EXT].concat())
                    {
                        self.music.set(p, ui.ctx());
                    }
                });
                if info.has_audio() {
                    field(ui, "Mode", "", |ui| {
                        segmented(ui, &mut self.mix, &[(true, "Mix in"), (false, "Replace")]);
                    });
                }
                if self.mix && info.has_audio() {
                    field(
                        ui,
                        "Music volume",
                        "The music repeats if it is shorter than the video.",
                        |ui| {
                            ui.spacing_mut().slider_width = ui.available_width() - 70.0;
                            ui.add(
                                egui::Slider::new(&mut self.music_volume, 0.05..=1.5)
                                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                            );
                        },
                    );
                } else {
                    hint(
                        ui,
                        I_INFO,
                        "Plays until the shorter of video and music ends.",
                    );
                }
            }
        }

        ui.add_space(8.0);
        let enabled = match self.op {
            Op::Music => self.music.info.as_ref().is_some_and(|m| m.has_audio()),
            Op::Volume => self.db.abs() >= 0.05,
            _ => true,
        };
        if action_button(ui, enabled, &labeled(I_SAVE, "Save…")) {
            match self.op {
                Op::Extract => {
                    let track = self.track.min(info.audio.len().saturating_sub(1));
                    let ext = self.format.ext(&info.audio[track].codec);
                    let out = save_as(Some(&info.path), "audio", ext)?;
                    return job_task(
                        "Extracting audio",
                        audio::build_extract(&info, &out, self.format, self.kbps, track),
                    );
                }
                Op::Remove => {
                    let out = save_as(Some(&info.path), "silent", &same_kind_ext(&info))?;
                    return job_task("Removing audio", audio::build_remove(&info, &out));
                }
                Op::Volume => {
                    let out = save_as(Some(&info.path), "volume", &same_kind_ext(&info))?;
                    return job_task("Changing volume", audio::build_volume(&info, &out, self.db));
                }
                Op::Normalize => {
                    let out = save_as(Some(&info.path), "normalized", &same_kind_ext(&info))?;
                    let (_, lufs, tp) = LOUDNESS[self.loudness.min(LOUDNESS.len() - 1)];
                    return task("Normalizing loudness", move |pr| {
                        audio::normalize(&info, &out, lufs, tp, pr)
                    });
                }
                Op::Music => {
                    let music = self.music.info.clone()?;
                    let out = save_as(Some(&info.path), "music", &same_kind_ext(&info))?;
                    let mode = if self.mix {
                        MusicMode::Mix {
                            music_volume: self.music_volume,
                        }
                    } else {
                        MusicMode::Replace
                    };
                    return job_task(
                        "Adding music",
                        audio::build_music(&info, &music, &out, mode),
                    );
                }
            }
        }
        None
    }
}
