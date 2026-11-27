//! Join Videos.

use std::path::PathBuf;

use eframe::egui;

use super::*;
use crate::ui::media::{Frames, Source, VIDEO_EXT};
use crate::video::codec::Container;
use crate::video::edit;

#[derive(Default)]
pub struct JoinState {
    pub clips: Vec<Source>,
    force_reencode: bool,
}

impl JoinState {
    pub fn add(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        for p in paths {
            self.clips.push(Source::new(p, ctx));
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, frames: &mut Frames, caps: &Caps) -> Option<Task> {
        section(ui, "Clips, joined from top to bottom");
        file_list(
            ui,
            frames,
            &mut self.clips,
            "videos",
            VIDEO_EXT,
            "Add two or more clips to join them into one video",
            true,
        );

        let ready: Vec<MediaInfo> = self
            .clips
            .iter()
            .filter_map(|c| c.video().cloned())
            .collect();
        let loading = self.clips.iter().any(|c| c.loading());
        if ready.len() >= 2 {
            let total: f64 = ready.iter().map(|c| c.duration).sum();
            match edit::join_mismatch(&ready) {
                None if !self.force_reencode => hint(
                    ui,
                    I_FLASH,
                    &format!(
                        "All clips match, so they are joined without re-encoding: fast and lossless. Total length {}.",
                        fmt_time(total, false)
                    ),
                ),
                None => hint(
                    ui,
                    I_INFO,
                    &format!("Total length {}.", fmt_time(total, false)),
                ),
                Some(why) => {
                    let (w, h) = ready[0]
                        .video
                        .as_ref()
                        .map(|v| v.display_size())
                        .unwrap_or((0, 0));
                    hint(
                        ui,
                        I_INFO,
                        &format!(
                            "The clips differ ({why}), so they are re-encoded to {w}×{h} like the first clip, with black bars where needed. Total length {}.",
                            fmt_time(total, false)
                        ),
                    );
                }
            }
        }
        section(ui, "Options");
        setting(
            ui,
            I_SETTINGS,
            "Always re-encode",
            "Use this if the lossless join plays back incorrectly",
            |ui| {
                ui.checkbox(&mut self.force_reencode, "");
            },
        );

        let n = ready.len();
        if footer(
            ui,
            n >= 2 && !loading,
            &labeled(I_SAVE, &format!("Join {n} clips")),
        ) {
            let lossless = !self.force_reencode && edit::join_mismatch(&ready).is_none();
            let first_ext = ready[0]
                .path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .filter(|e| Container::from_ext(e).is_some())
                .unwrap_or_else(|| "mp4".into());
            let ext = if lossless { first_ext } else { "mp4".into() };
            let out = save_as(Some(&ready[0].path), "joined", &ext)?;
            return job_task(
                "Joining",
                edit::build_join(&ready, &out, self.force_reencode, caps),
            );
        }
        None
    }
}
