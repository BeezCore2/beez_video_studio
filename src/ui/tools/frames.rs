//! Save Frames: still images from a video.

use eframe::egui::{self, Sense, Vec2};

use super::*;
use crate::ui::media::{Frames, Source, VIDEO_EXT};
use crate::video::frames::{self, ImageFormat, Mode};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Single,
    Every,
    Count,
    Sheet,
}

pub struct FramesState {
    pub src: Source,
    kind: Kind,
    time: f64,
    time_text: String,
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
            src: Source::default(),
            kind: Kind::Single,
            time: 0.0,
            time_text: fmt_time(0.0, true),
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
    pub fn ui(&mut self, ui: &mut egui::Ui, frames: &mut Frames, _caps: &Caps) -> Option<Task> {
        section(ui, "Video");
        self.src.ui(ui, frames, "video", VIDEO_EXT);
        if needs_video(ui, self.src.info.as_ref()) {
            return None;
        }
        let info = self.src.video().cloned()?;
        let dur = info.duration.max(0.1);

        section(ui, "What to save");
        card(ui, |ui| {
            big_choices(
                ui,
                &mut self.kind,
                &[
                    (Kind::Single, "One frame", "A still at a chosen time"),
                    (Kind::Every, "Every few seconds", "A series of images"),
                    (Kind::Count, "Evenly spaced", "A set number of images"),
                    (Kind::Sheet, "Contact sheet", "A grid of thumbnails"),
                ],
            );
        });

        match self.kind {
            Kind::Single => {
                card(ui, |ui| {
                    let (rect, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 300.0),
                        Sense::hover(),
                    );
                    frames.paint(ui, &info.path, self.time, 960, rect);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let max = (dur - 0.05).max(0.0);
                        let w = ui.available_width() - 140.0;
                        ui.spacing_mut().slider_width = w.max(100.0);
                        if ui
                            .add(egui::Slider::new(&mut self.time, 0.0..=max).show_value(false))
                            .changed()
                        {
                            self.time_text = fmt_time(self.time, true);
                        }
                        time_field(ui, &mut self.time_text, &mut self.time, max);
                    });
                });
            }
            Kind::Every => {
                setting(ui, I_CLOCK, "Interval", "", |ui| {
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
                setting(ui, I_PHOTO, "Number of images", "", |ui| {
                    ui.add(egui::DragValue::new(&mut self.count).range(1..=1000));
                });
            }
            Kind::Sheet => {
                setting(ui, I_GRID, "Grid", "Columns × rows", |ui| {
                    ui.add(egui::DragValue::new(&mut self.rows).range(1..=20));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut self.cols).range(1..=20));
                });
                setting(ui, I_RESIZE, "Thumbnail width", "", |ui| {
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
        setting(
            ui,
            I_PHOTO,
            "Image format",
            "PNG is lossless but larger",
            |ui| {
                segmented(
                    ui,
                    &mut self.format,
                    &[(ImageFormat::Jpg, "JPG"), (ImageFormat::Png, "PNG")],
                    60.0,
                );
            },
        );

        if footer(ui, true, &labeled(I_SAVE, "Save images")) {
            let ext = self.format.ext();
            let mode = match self.kind {
                Kind::Single => Mode::Single(self.time),
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
                    &format!("frame_{}", (self.time * 1000.0).round() as u64),
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
