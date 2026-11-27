//! Resize & Rotate: resolution, rotation, flips, crop to an aspect ratio, frame rate.

use eframe::egui::{self, Sense, Vec2};

use super::*;
use crate::ui::media::{Frames, Source, VIDEO_EXT, View};
use crate::video::codec::{Container, HEIGHTS, VideoCodec};
use crate::video::edit::{self, Resize, TransformOptions, transform_filters};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResizeKind {
    Keep,
    Short(u32),
    Fit,
    Pad,
}

pub struct TransformState {
    pub src: Source,
    o: TransformOptions,
    kind: ResizeKind,
    box_w: u32,
    box_h: u32,
}

impl Default for TransformState {
    fn default() -> Self {
        Self {
            src: Source::default(),
            o: TransformOptions::default(),
            kind: ResizeKind::Keep,
            box_w: 1080,
            box_h: 1920,
        }
    }
}

const CROPS: [(Option<(u32, u32)>, &str); 7] = [
    (None, "No crop"),
    (Some((16, 9)), "16:9 (widescreen)"),
    (Some((9, 16)), "9:16 (Shorts, Reels, TikTok)"),
    (Some((1, 1)), "1:1 (square)"),
    (Some((4, 5)), "4:5 (Instagram portrait)"),
    (Some((4, 3)), "4:3"),
    (Some((21, 9)), "21:9 (cinema)"),
];

impl TransformState {
    pub fn ui(&mut self, ui: &mut egui::Ui, frames: &mut Frames, caps: &Caps) -> Option<Task> {
        section(ui, "Video");
        self.src.ui(ui, frames, "video", VIDEO_EXT);
        if needs_video(ui, self.src.info.as_ref()) {
            return None;
        }
        let info = self.src.video().cloned()?;
        let v = info.video.clone()?;
        let (sw, sh) = v.display_size();

        self.o.resize = match self.kind {
            ResizeKind::Keep => Resize::Keep,
            ResizeKind::Short(h) => Resize::Short(h),
            ResizeKind::Fit => Resize::Fit(self.box_w.max(2), self.box_h.max(2)),
            ResizeKind::Pad => Resize::Pad(self.box_w.max(2), self.box_h.max(2)),
        };
        let (_, (ow, oh)) = transform_filters(sw, sh, &self.o);

        section(ui, "Preview");
        card(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 300.0), Sense::hover());
            let view = View { rotate: self.o.rotate, flip_h: self.o.flip_h, flip_v: self.o.flip_v, crop: self.o.crop };
            frames.paint_transformed(ui, &info.path, self.src.poster_time(), 640, rect, &view);
            ui.add_space(6.0);
            ui.vertical_centered(|ui| {
                ui.label(strong(format!("{sw}×{sh}  {I_RIGHT}  {ow}×{oh}")));
            });
        });

        section(ui, "Rotate & flip");
        let o = &mut self.o;
        setting(ui, I_ROTATE, "Rotate", &format!("Turned {}° clockwise", o.rotate), |ui| {
            if ui.button(labeled(I_ROTATE, "Right")).clicked() {
                o.rotate = (o.rotate + 90) % 360;
            }
            if ui.button(labeled(I_ROTATE_LEFT, "Left")).clicked() {
                o.rotate = (o.rotate + 270) % 360;
            }
        });
        setting(ui, "", "Flip", "Mirror the picture", |ui| {
            ui.toggle_value(&mut o.flip_v, "Upside down");
            ui.toggle_value(&mut o.flip_h, "Mirror");
        });

        section(ui, "Size");
        setting(ui, I_CUT, "Crop to shape", "Cuts the edges to fit, keeping the centre", |ui| {
            combo(ui, "t_crop", &mut o.crop, &CROPS);
        });
        setting(ui, I_RESIZE, "Resolution", "Never upscales when limiting the size", |ui| {
            let mut items: Vec<(ResizeKind, String)> = vec![(ResizeKind::Keep, "Keep".into())];
            items.extend(HEIGHTS.iter().map(|&h| (ResizeKind::Short(h), format!("{h}p"))));
            items.push((ResizeKind::Fit, "Fit inside a size…".into()));
            items.push((ResizeKind::Pad, "Exact size with black bars…".into()));
            let refs: Vec<(ResizeKind, &str)> = items.iter().map(|(k, s)| (*k, s.as_str())).collect();
            combo(ui, "t_res", &mut self.kind, &refs);
        });
        if matches!(self.kind, ResizeKind::Fit | ResizeKind::Pad) {
            setting(ui, "", "Size", "Width × height in pixels", |ui| {
                for (w, h, l) in [(1920, 1080, "1920×1080"), (1080, 1920, "1080×1920"), (1080, 1080, "1080×1080")] {
                    if ui.small_button(l).clicked() {
                        self.box_w = w;
                        self.box_h = h;
                    }
                }
                ui.add(egui::DragValue::new(&mut self.box_h).range(16..=8192).suffix(" px"));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut self.box_w).range(16..=8192).suffix(" px"));
            });
        }
        let o = &mut self.o;
        setting(ui, I_CLOCK, "Frame rate", "", |ui| {
            combo(ui, "t_fps", &mut o.fps, &[(None, "Keep"), (Some(60.0), "60 fps"), (Some(50.0), "50 fps"), (Some(30.0), "30 fps"), (Some(25.0), "25 fps"), (Some(24.0), "24 fps")]);
        });

        let src_ext = info.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let container = Container::from_ext(&src_ext).unwrap_or(Container::Mp4);
        let lossless = o.rotation_only() && matches!(container, Container::Mp4 | Container::Mov | Container::Mkv);
        if lossless {
            hint(ui, I_FLASH, "Only rotating: saved instantly without re-encoding, by changing the video's rotation setting.");
        } else {
            setting(ui, I_FLASH, "Encoder", "", |ui| {
                backend_combo(ui, "t_backend", &mut o.backend, VideoCodec::H264, caps);
            });
        }

        let nothing = o.rotate == 0 && !o.flip_h && !o.flip_v && o.crop.is_none() && o.fps.is_none() && (ow, oh) == (sw, sh);
        if footer(ui, !nothing, &labeled(I_SAVE, "Save video")) {
            let out = save_as(Some(&info.path), "edited", container.ext())?;
            return job_task("Saving", edit::build_transform(&info, &out, o, caps));
        }
        None
    }
}
