//! Resize & Rotate: resolution, rotation, flips, crop to an aspect ratio, frame rate.
//! The viewer previews the rotation, flips and crop live.

use eframe::egui;

use super::*;
use crate::ui::media::View;
use crate::video::codec::{Container, HEIGHTS};
use crate::video::edit::{self, Resize, TransformOptions, transform_filters};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResizeKind {
    Keep,
    Short(u32),
    Fit,
    Pad,
}

pub struct TransformState {
    o: TransformOptions,
    kind: ResizeKind,
    box_w: u32,
    box_h: u32,
}

impl Default for TransformState {
    fn default() -> Self {
        Self {
            o: TransformOptions::default(),
            kind: ResizeKind::Keep,
            box_w: 1080,
            box_h: 1920,
        }
    }
}

const CROPS: [(Option<(u32, u32)>, &str); 7] = [
    (None, "No crop"),
    (Some((16, 9)), "16:9 widescreen"),
    (Some((9, 16)), "9:16 Shorts, Reels, TikTok"),
    (Some((1, 1)), "1:1 square"),
    (Some((4, 5)), "4:5 Instagram portrait"),
    (Some((4, 3)), "4:3"),
    (Some((21, 9)), "21:9 cinema"),
];

impl TransformState {
    /// The preview transform for the viewer.
    pub fn view(&self) -> View {
        View {
            rotate: self.o.rotate,
            flip_h: self.o.flip_h,
            flip_v: self.o.flip_v,
            crop: self.o.crop,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Ctx) -> Option<Task> {
        let info = require(ui, cx, true)?.clone();
        let v = info.video.clone()?;
        let (sw, sh) = v.display_size();
        self.o.resize = match self.kind {
            ResizeKind::Keep => Resize::Keep,
            ResizeKind::Short(h) => Resize::Short(h),
            ResizeKind::Fit => Resize::Fit(self.box_w.max(2), self.box_h.max(2)),
            ResizeKind::Pad => Resize::Pad(self.box_w.max(2), self.box_h.max(2)),
        };
        let (_, (ow, oh)) = transform_filters(sw, sh, &self.o);
        hint(
            ui,
            I_RESIZE,
            &format!("{sw}×{sh}  {I_RIGHT}  {ow}×{oh}   (previewed in the viewer)"),
        );

        group(ui, "Rotate & flip");
        let o = &mut self.o;
        field(
            ui,
            "Rotate",
            &format!("Turned {}° clockwise", o.rotate),
            |ui| {
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 8.0) / 2.0;
                    if ui
                        .add(
                            egui::Button::new(labeled(I_ROTATE_LEFT, "Left"))
                                .min_size(egui::Vec2::new(w, 30.0)),
                        )
                        .clicked()
                    {
                        o.rotate = (o.rotate + 270) % 360;
                    }
                    if ui
                        .add(
                            egui::Button::new(labeled(I_ROTATE, "Right"))
                                .min_size(egui::Vec2::new(w, 30.0)),
                        )
                        .clicked()
                    {
                        o.rotate = (o.rotate + 90) % 360;
                    }
                });
            },
        );
        field(ui, "Flip", "", |ui| {
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 8.0) / 2.0;
                ui.add(
                    egui::Button::selectable(o.flip_h, "Mirror").min_size(egui::Vec2::new(w, 30.0)),
                )
                .clicked()
                .then(|| o.flip_h = !o.flip_h);
                ui.add(
                    egui::Button::selectable(o.flip_v, "Upside down")
                        .min_size(egui::Vec2::new(w, 30.0)),
                )
                .clicked()
                .then(|| o.flip_v = !o.flip_v);
            });
        });

        group(ui, "Size");
        field(
            ui,
            "Crop to shape",
            "Cuts the edges, keeping the centre.",
            |ui| combo(ui, "t_crop", &mut o.crop, &CROPS),
        );
        field(ui, "Resolution", "", |ui| {
            let mut items: Vec<(ResizeKind, String)> = vec![(ResizeKind::Keep, "Keep".into())];
            items.extend(
                HEIGHTS
                    .iter()
                    .map(|&h| (ResizeKind::Short(h), format!("{h}p"))),
            );
            items.push((ResizeKind::Fit, "Fit inside a size…".into()));
            items.push((ResizeKind::Pad, "Exact size with black bars…".into()));
            let refs: Vec<(ResizeKind, &str)> =
                items.iter().map(|(k, s)| (*k, s.as_str())).collect();
            combo(ui, "t_res", &mut self.kind, &refs);
        });
        if matches!(self.kind, ResizeKind::Fit | ResizeKind::Pad) {
            field(ui, "Width × height", "", |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.box_w)
                            .range(16..=8192)
                            .suffix(" px"),
                    );
                    ui.label("×");
                    ui.add(
                        egui::DragValue::new(&mut self.box_h)
                            .range(16..=8192)
                            .suffix(" px"),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    for (w, h, l) in [
                        (1920, 1080, "1920×1080"),
                        (1080, 1920, "1080×1920"),
                        (1080, 1080, "1080×1080"),
                    ] {
                        if ui.small_button(l).clicked() {
                            self.box_w = w;
                            self.box_h = h;
                        }
                    }
                });
            });
        }
        let o = &mut self.o;
        field(ui, "Frame rate", "", |ui| {
            combo(
                ui,
                "t_fps",
                &mut o.fps,
                &[
                    (None, "Keep"),
                    (Some(60.0), "60 fps"),
                    (Some(50.0), "50 fps"),
                    (Some(30.0), "30 fps"),
                    (Some(25.0), "25 fps"),
                    (Some(24.0), "24 fps"),
                ],
            );
        });

        let ext = same_ext_or(&info, "mp4");
        let container = Container::from_ext(&ext).unwrap_or(Container::Mp4);
        if o.rotation_only()
            && matches!(container, Container::Mp4 | Container::Mov | Container::Mkv)
        {
            hint(
                ui,
                I_FLASH,
                "Only rotating: saved instantly, without re-encoding.",
            );
        } else {
            field(ui, "Encoder", "", |ui| {
                backend_combo(ui, "t_backend", &mut o.backend, VideoCodec::H264, cx.caps)
            });
        }

        ui.add_space(8.0);
        let nothing = o.rotate == 0
            && !o.flip_h
            && !o.flip_v
            && o.crop.is_none()
            && o.fps.is_none()
            && (ow, oh) == (sw, sh);
        if action_button(ui, !nothing, &labeled(I_SAVE, "Save video…")) {
            let out = save_as(Some(&info.path), "edited", &ext)?;
            return job_task("Saving", edit::build_transform(&info, &out, o, cx.caps));
        }
        None
    }
}
