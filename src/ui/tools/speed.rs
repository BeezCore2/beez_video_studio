//! Speed.

use eframe::egui;

use super::*;
use crate::video::edit::{self, SpeedOptions};

#[derive(Default)]
pub struct SpeedState {
    o: SpeedOptions,
}

impl SpeedState {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Ctx) -> Option<Task> {
        let info = require(ui, cx, false)?.clone();
        let o = &mut self.o;

        group(ui, "Speed");
        let presets = [0.25, 0.5, 0.75, 1.25, 1.5, 2.0, 3.0, 4.0, 8.0];
        let gap = ui.spacing().item_spacing.x;
        let w = ((ui.available_width() - gap * 2.0) / 3.0).floor();
        for row in presets.chunks(3) {
            ui.horizontal(|ui| {
                for &f in row {
                    let selected = (o.factor - f).abs() < 1e-6;
                    if toggle(ui, selected, &format!("{f}×"), w) {
                        o.factor = f;
                    }
                }
            });
        }
        ui.add_space(4.0);
        ui.spacing_mut().slider_width = ui.available_width() - 70.0;
        ui.add(
            egui::Slider::new(&mut o.factor, 0.1..=16.0)
                .logarithmic(true)
                .suffix("×")
                .max_decimals(2),
        );
        hint(
            ui,
            I_CLOCK,
            &format!(
                "{}  {I_RIGHT}  {}",
                fmt_time(info.duration, false),
                fmt_time(info.duration / o.factor.max(0.01), false)
            ),
        );

        if info.has_audio() {
            group(ui, "Audio");
            ui.checkbox(&mut o.keep_pitch, "Keep natural pitch");
            ui.checkbox(&mut o.mute, "Mute");
            ui.add_space(6.0);
        }
        if info.has_video() {
            field(ui, "Encoder", "", |ui| {
                backend_combo(ui, "s_backend", &mut o.backend, VideoCodec::H264, cx.caps)
            });
        }

        ui.add_space(8.0);
        let ok = (o.factor - 1.0).abs() > 1e-6;
        if action_button(ui, ok, &labeled(I_SAVE, "Save…")) {
            let ext = same_ext_or(&info, if info.has_video() { "mp4" } else { "m4a" });
            let out = save_as(Some(&info.path), &format!("{}x", o.factor), &ext)?;
            return job_task("Changing speed", edit::build_speed(&info, &out, o, cx.caps));
        }
        None
    }
}
