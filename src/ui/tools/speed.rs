//! Change Speed.

use eframe::egui;

use super::*;
use crate::ui::media::{Frames, Source, media_ext};
use crate::video::codec::{Container, VideoCodec};
use crate::video::edit::{self, SpeedOptions};

#[derive(Default)]
pub struct SpeedState {
    pub src: Source,
    o: SpeedOptions,
}

impl SpeedState {
    pub fn ui(&mut self, ui: &mut egui::Ui, frames: &mut Frames, caps: &Caps) -> Option<Task> {
        section(ui, "Video");
        self.src.ui(ui, frames, "video", &media_ext());
        let info = self.src.info.clone()?;

        section(ui, "Speed");
        let o = &mut self.o;
        card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for f in [0.25, 0.5, 0.75, 1.25, 1.5, 2.0, 3.0, 4.0, 8.0] {
                    let selected = (o.factor - f).abs() < 1e-6;
                    if ui.add(egui::Button::selectable(selected, format!("{f}×")).min_size(egui::Vec2::new(56.0, 32.0))).clicked() {
                        o.factor = f;
                    }
                }
            });
            ui.add_space(6.0);
            ui.add(egui::Slider::new(&mut o.factor, 0.1..=16.0).logarithmic(true).suffix("×").max_decimals(2));
            ui.add_space(4.0);
            caption(
                ui,
                &format!(
                    "{} {I_RIGHT} {}",
                    fmt_time(info.duration, false),
                    fmt_time(info.duration / o.factor.max(0.01), false)
                ),
            );
        });

        if info.has_audio() {
            section(ui, "Audio");
            setting(ui, I_AUDIO, "Keep natural pitch", "Voices sound normal instead of high (fast) or deep (slow)", |ui| {
                ui.checkbox(&mut o.keep_pitch, "");
            });
            setting(ui, I_VOLUME, "Mute", "Leave the audio out", |ui| {
                ui.checkbox(&mut o.mute, "");
            });
        }
        if info.has_video() {
            setting(ui, I_FLASH, "Encoder", "", |ui| {
                backend_combo(ui, "s_backend", &mut o.backend, VideoCodec::H264, caps);
            });
        }

        let ok = (o.factor - 1.0).abs() > 1e-6;
        if footer(ui, ok, &labeled(I_SAVE, "Save")) {
            let src_ext = info.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let ext = match Container::from_ext(&src_ext) {
                Some(c) => c.ext().to_string(),
                None if info.has_video() => "mp4".into(),
                None => "m4a".into(),
            };
            let out = save_as(Some(&info.path), &format!("{}x", o.factor), &ext)?;
            return job_task("Changing speed", edit::build_speed(&info, &out, o, caps));
        }
        None
    }
}
