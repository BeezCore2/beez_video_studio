//! Join Clips: every video in the media bin, top to bottom.

use eframe::egui;

use super::*;
use crate::video::edit;

#[derive(Default)]
pub struct JoinState {
    force_reencode: bool,
}

impl JoinState {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Ctx) -> Option<Task> {
        let clips = cx.videos();
        group(ui, "Clips in order");
        if clips.len() < 2 {
            hint(
                ui,
                I_IMPORT,
                "Import two or more videos into the media bin. They are joined in the bin's order (right-click a clip to move it).",
            );
            return None;
        }
        for (i, c) in clips.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(secondary(ui, format!("{}.", i + 1)));
                ui.add(egui::Label::new(file_name(&c.path)).truncate());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    caption(ui, &fmt_time(c.duration, false));
                });
            });
        }
        let total: f64 = clips.iter().map(|c| c.duration).sum();
        ui.add_space(4.0);
        ui.label(strong(format!("Total {}", fmt_time(total, false))));
        ui.add_space(6.0);

        match edit::join_mismatch(&clips) {
            None if !self.force_reencode => hint(
                ui,
                I_FLASH,
                "All clips match: joined without re-encoding (fast, lossless).",
            ),
            None => {}
            Some(why) => {
                let (w, h) = clips[0]
                    .video
                    .as_ref()
                    .map(|v| v.display_size())
                    .unwrap_or((0, 0));
                hint(
                    ui,
                    I_INFO,
                    &format!(
                        "The clips differ ({why}), so they are re-encoded to {w}×{h} like the first clip, with black bars where needed."
                    ),
                );
            }
        }
        ui.checkbox(&mut self.force_reencode, "Always re-encode");
        ui.add_space(8.0);

        if action_button(
            ui,
            true,
            &labeled(I_SAVE, &format!("Join {} clips…", clips.len())),
        ) {
            let lossless = !self.force_reencode && edit::join_mismatch(&clips).is_none();
            let ext = if lossless {
                same_ext_or(&clips[0], "mp4")
            } else {
                "mp4".into()
            };
            let out = save_as(Some(&clips[0].path), "joined", &ext)?;
            return job_task(
                "Joining",
                edit::build_join(&clips, &out, self.force_reencode, cx.caps),
            );
        }
        None
    }
}
