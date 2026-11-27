//! Trim & Cut: a filmstrip timeline with start/end handles.

use std::path::PathBuf;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Vec2, pos2};

use super::*;
use crate::ui::media::{Frames, Source, media_ext};
use crate::video::codec::Container;
use crate::video::edit::{self, TrimMode, TrimOptions};

pub struct TrimState {
    pub src: Source,
    start: f64,
    end: f64,
    start_text: String,
    end_text: String,
    mode: TrimMode,
    remove: bool,
    loaded_for: Option<PathBuf>,
    /// Which handle is being dragged (true = end).
    dragging: Option<bool>,
}

impl Default for TrimState {
    fn default() -> Self {
        Self {
            src: Source::default(),
            start: 0.0,
            end: 0.0,
            start_text: String::new(),
            end_text: String::new(),
            mode: TrimMode::Precise,
            remove: false,
            loaded_for: None,
            dragging: None,
        }
    }
}

impl TrimState {
    fn sync_text(&mut self) {
        self.start_text = fmt_time(self.start, true);
        self.end_text = fmt_time(self.end, true);
    }

    /// Filmstrip with the selection; returns true when the selection changed.
    fn timeline(&mut self, ui: &mut egui::Ui, frames: &mut Frames, path: &std::path::Path, dur: f64) -> bool {
        let p = Palette::current(ui);
        let h = 72.0;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click_and_drag());
        let painter = ui.painter_at(rect.expand(8.0));
        painter.rect_filled(rect, 4.0, Color32::from_gray(20));

        // Filmstrip: evenly spaced frames across the whole video.
        let tile_w = h * 16.0 / 9.0;
        let n = (rect.width() / tile_w).ceil().max(1.0) as usize;
        let step = rect.width() / n as f32;
        for i in 0..n {
            let r = Rect::from_min_size(pos2(rect.left() + i as f32 * step, rect.top()), Vec2::new(step, h));
            let t = dur * (i as f64 + 0.5) / n as f64;
            if let Some(tex) = frames.get(ui.ctx(), path, t, 160) {
                // Fill the tile (crop the frame's centre).
                let [tw, th] = tex.size();
                let (fw, fh) = (tw as f32 / th as f32, r.width() / r.height());
                let uv = if fw > fh {
                    let k = fh / fw;
                    Rect::from_min_max(pos2((1.0 - k) / 2.0, 0.0), pos2((1.0 + k) / 2.0, 1.0))
                } else {
                    let k = fw / fh;
                    Rect::from_min_max(pos2(0.0, (1.0 - k) / 2.0), pos2(1.0, (1.0 + k) / 2.0))
                };
                painter.image(tex.id(), r, uv, Color32::WHITE);
            }
        }

        let x_of = |t: f64| rect.left() + (t / dur).clamp(0.0, 1.0) as f32 * rect.width();
        let t_of = |x: f32| ((x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * dur;
        let (xs, xe) = (x_of(self.start), x_of(self.end));
        let shade = Color32::from_black_alpha(170);
        if self.remove {
            painter.rect_filled(Rect::from_x_y_ranges(xs..=xe, rect.y_range()), 0.0, Color32::from_rgba_unmultiplied(200, 30, 30, 150));
        } else {
            painter.rect_filled(Rect::from_x_y_ranges(rect.left()..=xs, rect.y_range()), CornerRadius::ZERO, shade);
            painter.rect_filled(Rect::from_x_y_ranges(xe..=rect.right(), rect.y_range()), CornerRadius::ZERO, shade);
        }
        let sel = Rect::from_x_y_ranges(xs..=xe, rect.y_range());
        painter.rect_stroke(sel, 2.0, Stroke::new(3.0, p.accent), StrokeKind::Inside);
        for (x, label) in [(xs, "["), (xe, "]")] {
            let handle = Rect::from_center_size(pos2(x, rect.center().y), Vec2::new(12.0, h + 8.0));
            painter.rect_filled(handle, 3.0, p.accent);
            painter.text(handle.center(), Align2::CENTER_CENTER, label, FontId::monospace(12.0), p.on_accent);
        }

        // Interaction: drag the nearest handle; click moves the nearest handle there.
        let mut changed = false;
        if let Some(pos) = resp.interact_pointer_pos() {
            if resp.drag_started() || resp.clicked() || self.dragging.is_none() {
                self.dragging = Some((pos.x - xe).abs() < (pos.x - xs).abs());
            }
            let t = t_of(pos.x);
            match self.dragging {
                Some(true) => self.end = t.max(self.start + 0.1).min(dur),
                _ => self.start = t.min(self.end - 0.1).max(0.0),
            }
            changed = true;
        }
        if resp.drag_stopped() || !resp.is_pointer_button_down_on() {
            self.dragging = None;
        }
        resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        changed
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, frames: &mut Frames, caps: &Caps) -> Option<Task> {
        section(ui, "Video");
        self.src.ui(ui, frames, "video", &media_ext());
        let info = self.src.info.clone()?;
        if self.loaded_for.as_ref() != Some(&info.path) {
            self.loaded_for = Some(info.path.clone());
            self.start = 0.0;
            self.end = info.duration;
            self.sync_text();
        }
        let dur = info.duration.max(0.1);

        section(ui, if self.remove { "Part to remove" } else { "Part to keep" });
        let mut changed = false;
        card(ui, |ui| {
            if info.has_video() {
                // Start and end frames side by side.
                let w = (ui.available_width() - 12.0) / 2.0;
                let h = (w * 9.0 / 16.0).min(240.0);
                ui.horizontal(|ui| {
                    for t in [self.start, (self.end - 0.05).max(0.0)] {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
                        frames.paint(ui, &info.path, t, 480, r);
                    }
                });
                ui.add_space(8.0);
                changed |= self.timeline(ui, frames, &info.path, dur);
                ui.add_space(8.0);
            }
            ui.horizontal(|ui| {
                ui.label("Start");
                if time_field(ui, &mut self.start_text, &mut self.start, dur) {
                    self.start = self.start.min(self.end - 0.1).max(0.0);
                }
                ui.add_space(16.0);
                ui.label("End");
                if time_field(ui, &mut self.end_text, &mut self.end, dur) {
                    self.end = self.end.max(self.start + 0.1).min(dur);
                }
                ui.add_space(16.0);
                if ui.button("Reset").clicked() {
                    self.start = 0.0;
                    self.end = dur;
                    changed = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let kept = if self.remove { dur - (self.end - self.start) } else { self.end - self.start };
                    ui.label(strong(format!("Result: {}", fmt_time(kept.max(0.0), true))));
                });
            });
        });
        if changed {
            self.sync_text();
        }

        section(ui, "Options");
        setting(ui, I_CUT, "Action", "", |ui| {
            segmented(ui, &mut self.remove, &[(false, "Keep selection"), (true, "Cut selection out")], 120.0);
        });
        if !self.remove {
            setting(ui, I_FLASH, "Cutting", "", |ui| {
                segmented(ui, &mut self.mode, &[(TrimMode::Fast, "Fast (lossless)"), (TrimMode::Precise, "Frame-accurate")], 120.0);
            });
            if self.mode == TrimMode::Fast {
                hint(ui, I_INFO, "Fast copies the video without re-encoding. Cuts snap to the nearest keyframe before the start, so the clip may begin a little early.");
            } else {
                hint(ui, I_INFO, "Frame-accurate re-encodes the clip at near-lossless quality.");
            }
        } else {
            hint(ui, I_INFO, "The parts before and after the selection are joined and re-encoded at near-lossless quality.");
        }

        if footer(ui, self.end > self.start, &labeled(I_SAVE, "Save clip")) {
            let src_ext = info.path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            let fast = self.mode == TrimMode::Fast && !self.remove;
            let ext = if fast || Container::from_ext(&src_ext).is_some() {
                if src_ext.is_empty() { "mp4".to_string() } else { src_ext }
            } else if !info.has_video() {
                "m4a".to_string()
            } else {
                "mp4".to_string()
            };
            let out = save_as(Some(&info.path), "trimmed", &ext)?;
            let o = TrimOptions { start: self.start, end: self.end, mode: self.mode, remove: self.remove };
            return job_task("Trimming", edit::build_trim(&info, &out, &o, caps));
        }
        None
    }
}
