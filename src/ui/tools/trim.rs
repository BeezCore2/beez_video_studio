//! Trim & Cut: exports the timeline's In–Out range, or removes it.

use eframe::egui;

use super::*;
use crate::video::codec::Container;
use crate::video::edit::{self, TrimMode, TrimOptions};

pub struct TrimState {
    mode: TrimMode,
    remove: bool,
}

impl Default for TrimState {
    fn default() -> Self {
        Self {
            mode: TrimMode::Precise,
            remove: false,
        }
    }
}

impl TrimState {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Ctx) -> Option<Task> {
        let info = require(ui, cx, false)?.clone();
        let clip = cx.clip?;

        group(
            ui,
            if self.remove {
                "Range to remove"
            } else {
                "Range to keep"
            },
        );
        range_summary(ui, clip);
        if !clip.has_range() {
            hint(
                ui,
                I_INFO,
                "The range is the whole clip. Mark In and Out on the timeline first.",
            );
        }

        group(ui, "Options");
        field(ui, "Action", "", |ui| {
            segmented(
                ui,
                &mut self.remove,
                &[(false, "Keep range"), (true, "Cut range out")],
            );
        });
        if self.remove {
            hint(
                ui,
                I_INFO,
                "The parts before and after are joined and re-encoded at near-lossless quality.",
            );
        } else {
            field(ui, "Cutting", "", |ui| {
                segmented(
                    ui,
                    &mut self.mode,
                    &[
                        (TrimMode::Fast, "Fast (lossless)"),
                        (TrimMode::Precise, "Frame-accurate"),
                    ],
                );
            });
            if self.mode == TrimMode::Fast {
                hint(
                    ui,
                    I_INFO,
                    "Copies the video without re-encoding. The start snaps to the keyframe before it, so the clip may begin a little early.",
                );
            } else {
                hint(
                    ui,
                    I_INFO,
                    "Re-encodes the range at near-lossless quality, cut exactly.",
                );
            }
        }

        ui.add_space(8.0);
        let (start, end) = (clip.range_in, clip.range_out);
        let length = if self.remove {
            info.duration - (end - start)
        } else {
            end - start
        };
        let ok = end > start && length > 0.05;
        if action_button(
            ui,
            ok,
            &labeled(
                I_SAVE,
                &format!("Export {}…", fmt_time(length.max(0.0), false)),
            ),
        ) {
            let src_ext = info
                .path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let fast = self.mode == TrimMode::Fast && !self.remove;
            let ext = if (fast || Container::from_ext(&src_ext).is_some()) && !src_ext.is_empty() {
                src_ext
            } else if !info.has_video() {
                "m4a".into()
            } else {
                "mp4".into()
            };
            let out = save_as(Some(&info.path), "trimmed", &ext)?;
            let o = TrimOptions {
                start,
                end,
                mode: self.mode,
                remove: self.remove,
            };
            return job_task("Trimming", edit::build_trim(&info, &out, &o, cx.caps));
        }
        None
    }
}
