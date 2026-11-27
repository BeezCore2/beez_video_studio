//! Audio Tools: extract, remove, normalize, volume, music.

use eframe::egui;

use super::*;
use crate::ui::media::{AUDIO_EXT, Frames, Source, media_ext};
use crate::video::audio::{self, Format, LOUDNESS, MusicMode};
use crate::video::codec::Container;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Extract,
    Remove,
    Normalize,
    Volume,
    Music,
}

pub struct AudioState {
    pub src: Source,
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
            src: Source::default(),
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
    let src = info.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if info.has_video() {
        Container::from_ext(&src).map(|c| c.ext().to_string()).unwrap_or_else(|| "mp4".into())
    } else if audio::codec_for_ext(&info.path).is_some() {
        src
    } else {
        "m4a".into()
    }
}

impl AudioState {
    pub fn ui(&mut self, ui: &mut egui::Ui, frames: &mut Frames, _caps: &Caps) -> Option<Task> {
        section(ui, "File");
        self.src.ui(ui, frames, "video or audio file", &media_ext());
        let info = self.src.info.clone()?;
        if !info.has_audio() {
            let p = Palette::current(ui);
            ui.label(egui::RichText::new(format!("{I_INFO}  This file has no audio.")).color(p.error));
            if info.has_video() {
                self.op = Op::Music;
            } else {
                return None;
            }
        }

        section(ui, "What to do");
        card(ui, |ui| {
            let mut items = vec![
                (Op::Extract, "Extract audio", "Save the sound as MP3, M4A…"),
                (Op::Normalize, "Normalize", "Even, standard loudness"),
                (Op::Volume, "Volume", "Make it louder or quieter"),
            ];
            if info.has_video() {
                items.push((Op::Remove, "Remove audio", "Silent video, no quality loss"));
                items.push((Op::Music, "Add music", "Replace or mix in a track"));
            }
            if !items.iter().any(|(o, _, _)| *o == self.op) {
                self.op = Op::Extract;
            }
            big_choices(ui, &mut self.op, &items);
        });

        section(ui, "Options");
        match self.op {
            Op::Extract => {
                setting(ui, I_AUDIO, "Format", "Original copies the sound exactly as it is stored", |ui| {
                    let items: Vec<(Format, &str)> = Format::ALL.iter().map(|f| (*f, f.name())).collect();
                    combo(ui, "a_fmt", &mut self.format, &items);
                });
                if self.format.lossy() {
                    setting(ui, "", "Bitrate", "", |ui| {
                        let mut items = vec![(320, "320 kbps"), (256, "256 kbps"), (192, "192 kbps"), (128, "128 kbps"), (96, "96 kbps"), (64, "64 kbps")];
                        if self.format == Format::Mp3 {
                            items.insert(0, (0, "Variable (high quality)"));
                        } else if self.kbps == 0 {
                            self.kbps = 192;
                        }
                        combo(ui, "a_kbps", &mut self.kbps, &items);
                    });
                }
                if info.audio.len() > 1 {
                    setting(ui, "", "Audio track", "", |ui| {
                        let labels: Vec<String> = info
                            .audio
                            .iter()
                            .enumerate()
                            .map(|(i, a)| {
                                let lang = if a.language.is_empty() || a.language == "und" { String::new() } else { format!(" · {}", a.language) };
                                format!("Track {} · {} · {} ch{lang}", i + 1, a.codec.to_uppercase(), a.channels)
                            })
                            .collect();
                        let items: Vec<(usize, &str)> = labels.iter().enumerate().map(|(i, l)| (i, l.as_str())).collect();
                        combo(ui, "a_track", &mut self.track, &items);
                    });
                }
            }
            Op::Remove => hint(ui, I_FLASH, "The video is copied without re-encoding, so nothing else changes."),
            Op::Normalize => {
                setting(ui, I_VOLUME, "Target loudness", "Measured over the whole file (EBU R128), then adjusted in one even step", |ui| {
                    let items: Vec<(usize, &str)> = LOUDNESS.iter().enumerate().map(|(i, l)| (i, l.0)).collect();
                    combo(ui, "a_loud", &mut self.loudness, &items);
                });
            }
            Op::Volume => {
                setting(ui, I_VOLUME, "Change", "+6 dB is about twice as loud. Loud boosts are limited so they don't distort.", |ui| {
                    ui.add(egui::Slider::new(&mut self.db, -20.0..=20.0).suffix(" dB").step_by(0.5));
                });
            }
            Op::Music => {
                ui.label(strong("Music or audio track"));
                self.music.ui(ui, frames, "audio file", &[AUDIO_EXT, crate::ui::media::VIDEO_EXT].concat());
                if info.has_audio() {
                    setting(ui, I_AUDIO, "Mode", "", |ui| {
                        segmented(ui, &mut self.mix, &[(true, "Mix with original"), (false, "Replace original")], 140.0);
                    });
                }
                if self.mix && info.has_audio() {
                    setting(ui, I_VOLUME, "Music volume", "Background level under the original sound. The music repeats if it is shorter.", |ui| {
                        ui.add(egui::Slider::new(&mut self.music_volume, 0.05..=1.5).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
                    });
                } else {
                    hint(ui, I_INFO, "The video keeps playing until the shorter of video and music ends.");
                }
            }
        }

        let enabled = match self.op {
            Op::Music => self.music.info.as_ref().is_some_and(|m| m.has_audio()),
            Op::Volume => self.db.abs() >= 0.05,
            _ => true,
        };
        if footer(ui, enabled, &labeled(I_SAVE, "Save")) {
            match self.op {
                Op::Extract => {
                    let track = self.track.min(info.audio.len().saturating_sub(1));
                    let ext = self.format.ext(&info.audio[track].codec);
                    let out = save_as(Some(&info.path), "audio", ext)?;
                    return job_task("Extracting audio", audio::build_extract(&info, &out, self.format, self.kbps, track));
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
                    return task("Normalizing loudness", move |pr| audio::normalize(&info, &out, lufs, tp, pr));
                }
                Op::Music => {
                    let music = self.music.info.clone()?;
                    let out = save_as(Some(&info.path), "music", &same_kind_ext(&info))?;
                    let mode = if self.mix { MusicMode::Mix { music_volume: self.music_volume } } else { MusicMode::Replace };
                    return job_task("Adding music", audio::build_music(&info, &music, &out, mode));
                }
            }
        }
        None
    }
}
