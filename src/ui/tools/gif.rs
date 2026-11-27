//! Video to GIF / animated WebP.

use eframe::egui::{self, Sense, Vec2};

use super::*;
use crate::ui::media::{Frames, Source, VIDEO_EXT};
use crate::video::convert::{self, AnimFormat, GifOptions};

#[derive(Default)]
pub struct GifState {
    pub src: Source,
    o: GifOptions,
    loaded_for: Option<std::path::PathBuf>,
}

impl GifState {
    pub fn ui(&mut self, ui: &mut egui::Ui, frames: &mut Frames, caps: &Caps) -> Option<Task> {
        section(ui, "Video");
        self.src.ui(ui, frames, "video", VIDEO_EXT);
        let info = self.src.video().cloned();
        if needs_video(ui, self.src.info.as_ref()) {
            return None;
        }
        let info = info?;
        if self.loaded_for.as_ref() != Some(&info.path) {
            self.loaded_for = Some(info.path.clone());
            self.o.start = 0.0;
            self.o.length = info.duration.min(5.0);
        }
        let dur = info.duration.max(0.1);

        section(ui, "Clip");
        card(ui, |ui| {
            let h = 260.0;
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
            frames.paint(ui, &info.path, self.o.start, 640, rect);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Start");
                ui.add(
                    egui::Slider::new(&mut self.o.start, 0.0..=(dur - 0.1).max(0.0))
                        .custom_formatter(|v, _| fmt_time(v, true))
                        .step_by(0.1),
                );
            });
            let max_len = (dur - self.o.start).clamp(0.1, 60.0);
            ui.horizontal(|ui| {
                ui.label("Length");
                ui.add(
                    egui::Slider::new(&mut self.o.length, 0.1..=max_len)
                        .suffix(" s")
                        .step_by(0.1)
                        .max_decimals(1),
                );
            });
            self.o.length = self.o.length.min(max_len);
        });

        section(ui, "Output");
        let webp_ok = caps.encoders.is_empty() || caps.has("libwebp") || caps.has("libwebp_anim");
        let o = &mut self.o;
        setting(
            ui,
            I_PHOTO,
            "Format",
            "WebP is usually 3–5× smaller than GIF and shows more colours",
            |ui| {
                ui.add_enabled_ui(webp_ok, |ui| {
                    segmented(
                        ui,
                        &mut o.format,
                        &[(AnimFormat::Gif, "GIF"), (AnimFormat::WebP, "WebP")],
                        70.0,
                    );
                });
            },
        );
        setting(
            ui,
            I_RESIZE,
            "Width",
            "Smaller pictures make much smaller files",
            |ui| {
                combo(
                    ui,
                    "g_w",
                    &mut o.width,
                    &[
                        (320, "320 px"),
                        (480, "480 px"),
                        (640, "640 px"),
                        (800, "800 px"),
                        (1080, "1080 px"),
                        (0, "Original"),
                    ],
                );
            },
        );
        setting(
            ui,
            I_CLOCK,
            "Frame rate",
            "10–15 fps looks smooth for most clips",
            |ui| {
                combo(
                    ui,
                    "g_fps",
                    &mut o.fps,
                    &[
                        (8, "8 fps"),
                        (10, "10 fps"),
                        (12, "12 fps"),
                        (15, "15 fps"),
                        (20, "20 fps"),
                        (25, "25 fps"),
                        (30, "30 fps"),
                    ],
                );
            },
        );
        match o.format {
            AnimFormat::Gif => {
                setting(
                    ui,
                    I_SETTINGS,
                    "Colours",
                    "Up to 256. Fewer colours, smaller file",
                    |ui| {
                        ui.add(egui::Slider::new(&mut o.colors, 16..=256));
                    },
                );
                setting(
                    ui,
                    "",
                    "Smooth gradients (dithering)",
                    "Hides colour banding; turn off for flat graphics and smaller files",
                    |ui| {
                        ui.checkbox(&mut o.dither, "");
                    },
                );
            }
            AnimFormat::WebP => {
                setting(ui, I_SETTINGS, "Quality", "", |ui| {
                    ui.add(egui::Slider::new(&mut o.quality, 30..=100));
                });
            }
        }
        let count = (o.length * o.fps as f64).round() as u64;
        hint(
            ui,
            I_INFO,
            &format!("{count} frames · {}", fmt_time(o.length, true)),
        );
        if count > 600 {
            hint(
                ui,
                I_ERROR,
                "That's a lot of frames: the file may be very large. Try a shorter clip or a lower frame rate.",
            );
        }

        if footer(
            ui,
            true,
            &labeled(I_SAVE, &format!("Create {}", o.format.ext().to_uppercase())),
        ) {
            let out = save_as(Some(&info.path), "clip", o.format.ext())?;
            return job_task(
                "Creating animation",
                convert::build_gif(&info, &out, o, caps),
            );
        }
        None
    }
}
