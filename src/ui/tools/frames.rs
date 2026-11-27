//! Save Frames: the frame at the playhead, a series, or a contact sheet.

use eframe::egui;

use super::*;
use crate::video::frames::{self, ImageFormat, Mode};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Single,
    Every,
    Count,
    Sheet,
}

pub struct FramesState {
    kind: Kind,
    every: f64,
    count: u32,
    cols: u32,
    rows: u32,
    width: u32,
    format: ImageFormat,
}

impl Default for FramesState {
    fn default() -> Self {
        Self {
            kind: Kind::Single,
            every: 5.0,
            count: 12,
            cols: 4,
            rows: 4,
            width: 320,
            format: ImageFormat::Jpg,
        }
    }
}

impl FramesState {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Ctx) -> Option<Task> {
        let info = require(ui, cx, true)?.clone();
        let clip = cx.clip?;
        let dur = info.duration.max(0.1);

        group(ui, "What to save");
        choice_list(
            ui,
            &mut self.kind,
            &[
                (Kind::Single, "Current frame", "The frame at the playhead"),
                (Kind::Every, "Every few seconds", "A series of images"),
                (Kind::Count, "Evenly spaced", "A set number of images"),
                (Kind::Sheet, "Contact sheet", "A grid of thumbnails"),
            ],
        );
        match self.kind {
            Kind::Single => hint(
                ui,
                I_CAMERA,
                &format!(
                    "Frame at {}. Move the playhead to choose another.",
                    fmt_time(clip.playhead, true)
                ),
            ),
            Kind::Every => {
                field(ui, "Interval", "", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.every)
                            .range(0.1..=3600.0)
                            .speed(0.5)
                            .suffix(" s"),
                    );
                });
                hint(
                    ui,
                    I_INFO,
                    &format!(
                        "About {} images.",
                        (dur / self.every.max(0.1)).floor() as u64 + 1
                    ),
                );
            }
            Kind::Count => {
                field(ui, "Number of images", "", |ui| {
                    ui.add(egui::DragValue::new(&mut self.count).range(1..=1000));
                });
            }
            Kind::Sheet => {
                field(ui, "Grid (columns × rows)", "", |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.cols).range(1..=20));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut self.rows).range(1..=20));
                    });
                });
                field(ui, "Thumbnail width", "", |ui| {
                    combo(
                        ui,
                        "f_w",
                        &mut self.width,
                        &[
                            (160, "160 px"),
                            (240, "240 px"),
                            (320, "320 px"),
                            (480, "480 px"),
                        ],
                    );
                });
            }
        }
        field(ui, "Image format", "PNG is lossless but larger.", |ui| {
            segmented(
                ui,
                &mut self.format,
                &[(ImageFormat::Jpg, "JPG"), (ImageFormat::Png, "PNG")],
            );
        });

        ui.add_space(8.0);
        if action_button(ui, true, &labeled(I_SAVE, "Save images…")) {
            let ext = self.format.ext();
            let mode = match self.kind {
                Kind::Single => Mode::Single(clip.playhead.min(dur - 0.05).max(0.0)),
                Kind::Every => Mode::Every(self.every),
                Kind::Count => Mode::Count(self.count),
                Kind::Sheet => Mode::Sheet {
                    cols: self.cols,
                    rows: self.rows,
                    width: self.width,
                },
            };
            let out = match self.kind {
                Kind::Single => save_as(
                    Some(&info.path),
                    &format!("frame_{}", (clip.playhead * 1000.0).round() as u64),
                    ext,
                )?,
                Kind::Sheet => save_as(Some(&info.path), "contact_sheet", ext)?,
                Kind::Every | Kind::Count => {
                    let dir = pick_folder(Some(&info.path))?;
                    let stem = info
                        .path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or("frame".into());
                    dir.join(format!("{stem}.{ext}"))
                }
            };
            return job_task(
                "Saving frames",
                frames::build(&info, &out, mode, self.format),
            );
        }
        None
    }
}
