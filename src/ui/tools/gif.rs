//! GIF / WebP from the timeline's In–Out range.

use eframe::egui;

use super::*;
use crate::video::convert::{self, AnimFormat, GifOptions};

#[derive(Default)]
pub struct GifState {
    o: GifOptions,
}

impl GifState {
    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Ctx) -> Option<Task> {
        let info = require(ui, cx, true)?.clone();
        let clip = cx.clip?;
        group(ui, "Range");
        range_summary(ui, clip);
        let length = clip.range_out - clip.range_in;
        self.o.start = clip.range_in;
        self.o.length = length;

        group(ui, "Output");
        let webp_ok =
            cx.caps.encoders.is_empty() || cx.caps.has("libwebp") || cx.caps.has("libwebp_anim");
        let o = &mut self.o;
        field(
            ui,
            "Format",
            "WebP is usually 3–5× smaller than GIF, with more colours.",
            |ui| {
                ui.add_enabled_ui(webp_ok, |ui| {
                    segmented(
                        ui,
                        &mut o.format,
                        &[(AnimFormat::Gif, "GIF"), (AnimFormat::WebP, "WebP")],
                    );
                });
            },
        );
        field(ui, "Width", "", |ui| {
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
        });
        field(
            ui,
            "Frame rate",
            "10–15 fps looks smooth for most clips.",
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
                field(ui, "Colours", "Fewer colours, smaller file.", |ui| {
                    ui.spacing_mut().slider_width = ui.available_width() - 60.0;
                    ui.add(egui::Slider::new(&mut o.colors, 16..=256));
                });
                ui.checkbox(&mut o.dither, "Smooth gradients (dithering)");
                ui.add_space(6.0);
            }
            AnimFormat::WebP => {
                field(ui, "Quality", "", |ui| {
                    ui.spacing_mut().slider_width = ui.available_width() - 60.0;
                    ui.add(egui::Slider::new(&mut o.quality, 30..=100));
                });
            }
        }
        let count = (length * o.fps as f64).round() as u64;
        hint(ui, I_INFO, &format!("{count} frames"));
        if count > 600 {
            error_note(
                ui,
                "That's a lot of frames: the file may be very large. Narrow the range or lower the frame rate.",
            );
        }

        ui.add_space(8.0);
        if action_button(
            ui,
            length > 0.05,
            &labeled(
                I_SAVE,
                &format!("Create {}…", o.format.ext().to_uppercase()),
            ),
        ) {
            let out = save_as(Some(&info.path), "clip", o.format.ext())?;
            return job_task(
                "Creating animation",
                convert::build_gif(&info, &out, o, cx.caps),
            );
        }
        None
    }
}
