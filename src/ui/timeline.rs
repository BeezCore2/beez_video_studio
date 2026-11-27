//! The timeline: ruler, playhead, video filmstrip track, audio waveform track, and the
//! In/Out range with draggable handles.

use eframe::egui::{
    self, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2,
};

use super::bin::Clip;
use super::media::{Frames, Waves};
use super::theme::*;
use crate::video::probe::{fmt_time, parse_time};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    Playhead,
    In,
    Out,
}

#[derive(Default)]
pub struct Timeline {
    drag: Option<Drag>,
    in_text: String,
    out_text: String,
}

/// A typed time (`1:23.5`): shows `value` while not being edited; returns the new time
/// when the user finishes typing a valid one.
fn time_edit(ui: &mut egui::Ui, salt: &str, value: f64, buf: &mut String) -> Option<f64> {
    let id = ui.make_persistent_id(salt);
    if !ui.memory(|m| m.has_focus(id)) {
        *buf = fmt_time(value, true);
    }
    let r = ui.add(
        egui::TextEdit::singleline(buf)
            .id(id)
            .desired_width(96.0)
            .font(egui::TextStyle::Monospace),
    );
    if r.lost_focus() {
        let t = parse_time(buf);
        *buf = fmt_time(t.unwrap_or(value), true);
        return t;
    }
    None
}

/// What the user did on the timeline this frame.
#[derive(Default)]
pub struct Event {
    /// The playhead was moved by hand (stop playback, show the new frame).
    pub seeked: bool,
}

const GUTTER: f32 = 56.0;
const RULER: f32 = 22.0;
const VIDEO: f32 = 58.0;
const AUDIO: f32 = 38.0;

/// A ruler label: `0:10` for whole-second steps, `0:00.5` / `0:02.25` for fine steps
/// (only as many decimals as the step needs).
pub fn ruler_label(t: f64, step: f64) -> String {
    let prec = if step >= 1.0 {
        0
    } else if ((step * 10.0) - (step * 10.0).round()).abs() < 1e-9 {
        1
    } else {
        2
    };
    let scale = 10f64.powi(prec as i32);
    let units = (t.max(0.0) * scale).round() as u64;
    let whole = units / scale as u64;
    let frac = units % scale as u64;
    let (h, m, s) = (whole / 3600, whole / 60 % 60, whole % 60);
    let mut out = if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    };
    if prec > 0 {
        out.push_str(&format!(".{frac:0prec$}"));
    }
    out
}

/// Tick spacing (seconds) so labels are at least `min_px` apart.
pub fn tick_step(duration: f64, width: f32, min_px: f32) -> f64 {
    const STEPS: [f64; 15] = [
        0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0,
    ];
    let px_per_s = width as f64 / duration.max(0.001);
    STEPS
        .into_iter()
        .find(|s| s * px_per_s >= min_px as f64)
        .unwrap_or(7200.0)
}

impl Timeline {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        frames: &mut Frames,
        waves: &mut Waves,
        clip: &mut Clip,
    ) -> Event {
        let mut ev = Event::default();
        let p = Palette::current(ui);
        let dur = clip.duration().max(0.001);
        let Some(info) = clip.info().cloned() else {
            ui.centered_and_justified(|ui| caption(ui, "Reading the clip…"));
            return ev;
        };

        // Toolbar: In/Out controls and the selection length.
        ui.horizontal(|ui| {
            let t = clip.playhead;
            if ui
                .button(labeled("[", "Set In"))
                .on_hover_text("Mark the start of the range here (I)")
                .clicked()
            {
                clip.set_in(t);
            }
            if ui
                .button(labeled("]", "Set Out"))
                .on_hover_text("Mark the end of the range here (O)")
                .clicked()
            {
                clip.set_out(t);
            }
            if command(ui, clip.has_range(), I_CLEAR, "Clear range") {
                clip.clear_range();
            }
            ui.add_space(12.0);
            ui.label(secondary(ui, "In"));
            if let Some(t) = time_edit(ui, "range_in", clip.range_in, &mut self.in_text) {
                clip.set_in(t);
            }
            ui.label(secondary(ui, "Out"));
            if let Some(t) = time_edit(ui, "range_out", clip.range_out, &mut self.out_text) {
                clip.set_out(t);
            }
            ui.label(secondary(ui, "  Range"));
            ui.label(
                RichText::new(fmt_time(clip.range_out - clip.range_in, true))
                    .monospace()
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "{} / {}",
                        fmt_time(clip.playhead, true),
                        fmt_time(dur, true)
                    ))
                    .monospace()
                    .color(p.text2),
                );
            });
        });
        ui.add_space(4.0);

        let height = RULER + VIDEO + 4.0 + AUDIO;
        let (full, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
        let area = Rect::from_min_max(pos2(full.left() + GUTTER, full.top()), full.max);
        let resp = ui.interact(
            area,
            ui.scope_id().with("timeline"),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(full);
        let ruler = Rect::from_min_size(area.min, Vec2::new(area.width(), RULER));
        let vtrack = Rect::from_min_size(
            pos2(area.left(), ruler.bottom()),
            Vec2::new(area.width(), VIDEO),
        );
        let atrack = Rect::from_min_size(
            pos2(area.left(), vtrack.bottom() + 4.0),
            Vec2::new(area.width(), AUDIO),
        );
        let x_of = |t: f64| area.left() + (t / dur).clamp(0.0, 1.0) as f32 * area.width();
        let t_of = |x: f32| ((x - area.left()) / area.width()).clamp(0.0, 1.0) as f64 * dur;

        // Track labels.
        let track_bg = p.card;
        for (r, icon, name) in [(vtrack, I_VIDEO, "V1"), (atrack, I_AUDIO, "A1")] {
            let label = Rect::from_min_max(
                pos2(full.left(), r.top()),
                pos2(area.left() - 4.0, r.bottom()),
            );
            painter.rect_filled(label, 4.0, track_bg);
            painter.text(
                label.center() - Vec2::new(0.0, 7.0),
                Align2::CENTER_CENTER,
                icon,
                FontId::proportional(13.0),
                p.text2,
            );
            painter.text(
                label.center() + Vec2::new(0.0, 9.0),
                Align2::CENTER_CENTER,
                name,
                FontId::proportional(10.0),
                p.text2,
            );
        }

        // Ruler.
        painter.rect_filled(ruler, 0.0, track_bg);
        let step = tick_step(dur, area.width(), 80.0);
        let minor = step / 5.0;
        let mut i = 0u64;
        loop {
            let t = i as f64 * minor;
            if t > dur + 1e-9 {
                break;
            }
            let x = x_of(t);
            let major = i.is_multiple_of(5);
            let h = if major { 10.0 } else { 4.0 };
            painter.line_segment(
                [pos2(x, ruler.bottom() - h), pos2(x, ruler.bottom())],
                Stroke::new(1.0, p.text2.gamma_multiply(0.7)),
            );
            if major {
                painter.text(
                    pos2(x + 3.0, ruler.top() + 2.0),
                    Align2::LEFT_TOP,
                    ruler_label(t, step),
                    FontId::proportional(10.0),
                    p.text2,
                );
            }
            i += 1;
        }

        // Video track: frames across the whole clip.
        painter.rect_filled(vtrack, 3.0, Color32::from_gray(12));
        if info.has_video() {
            let path = info.path.clone();
            let aspect = info.video.as_ref().map(|v| {
                let (w, h) = v.display_size();
                w as f32 / h.max(1) as f32
            });
            let tile_w = VIDEO * aspect.unwrap_or(16.0 / 9.0).clamp(0.4, 3.0);
            let n = (vtrack.width() / tile_w).ceil().clamp(1.0, 60.0) as usize;
            let step_w = vtrack.width() / n as f32;
            for k in 0..n {
                let r = Rect::from_min_size(
                    pos2(vtrack.left() + k as f32 * step_w, vtrack.top()),
                    Vec2::new(step_w, VIDEO),
                );
                let t = dur * (k as f64 + 0.5) / n as f64;
                if let Some(tex) = frames.get(ui.ctx(), &path, t, 160) {
                    let [tw, th] = tex.size();
                    let (fw, fh) = (tw as f32 / th as f32, r.width() / r.height());
                    let uv = if fw > fh {
                        let q = fh / fw;
                        Rect::from_min_max(pos2((1.0 - q) / 2.0, 0.0), pos2((1.0 + q) / 2.0, 1.0))
                    } else {
                        let q = fw / fh;
                        Rect::from_min_max(pos2(0.0, (1.0 - q) / 2.0), pos2(1.0, (1.0 + q) / 2.0))
                    };
                    painter.image(tex.id(), r.shrink2(Vec2::new(0.5, 0.0)), uv, Color32::WHITE);
                }
            }
        } else {
            painter.text(
                vtrack.center(),
                Align2::CENTER_CENTER,
                "No video",
                FontId::proportional(12.0),
                p.text2,
            );
        }

        // Audio track: waveform.
        painter.rect_filled(atrack, 3.0, p.well);
        if info.has_audio() {
            if let Some(tex) = waves.get(ui.ctx(), &info.path) {
                painter.image(
                    tex.id(),
                    atrack,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                painter.text(
                    atrack.center(),
                    Align2::CENTER_CENTER,
                    "…",
                    FontId::proportional(12.0),
                    p.text2,
                );
            }
        } else {
            painter.text(
                atrack.center(),
                Align2::CENTER_CENTER,
                "No audio",
                FontId::proportional(12.0),
                p.text2,
            );
        }

        // In/Out range: shade what is outside, bracket handles at both ends.
        let tracks = Rect::from_min_max(vtrack.min, atrack.max);
        let (xi, xo) = (x_of(clip.range_in), x_of(clip.range_out));
        let shade = Color32::from_black_alpha(150);
        if xi > tracks.left() {
            painter.rect_filled(
                Rect::from_x_y_ranges(tracks.left()..=xi, tracks.y_range()),
                0.0,
                shade,
            );
        }
        if xo < tracks.right() {
            painter.rect_filled(
                Rect::from_x_y_ranges(xo..=tracks.right(), tracks.y_range()),
                0.0,
                shade,
            );
        }
        if clip.has_range() {
            painter.rect_stroke(
                Rect::from_x_y_ranges(xi..=xo, tracks.y_range()),
                2.0,
                Stroke::new(2.0, p.accent),
                StrokeKind::Inside,
            );
            painter.rect_filled(
                Rect::from_x_y_ranges(xi..=xo, ruler.bottom() - 4.0..=ruler.bottom()),
                0.0,
                p.accent,
            );
        }
        for (x, label) in [(xi, "["), (xo, "]")] {
            let handle = Rect::from_center_size(
                pos2(x, tracks.center().y),
                Vec2::new(10.0, tracks.height()),
            );
            painter.rect_filled(handle, 3.0, p.accent);
            painter.text(
                handle.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::monospace(11.0),
                p.on_accent,
            );
        }

        // Playhead.
        let xp = x_of(clip.playhead);
        let ph = p.playhead;
        painter.line_segment(
            [pos2(xp, ruler.top()), pos2(xp, atrack.bottom())],
            Stroke::new(2.0, ph),
        );
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(xp - 6.0, ruler.top()),
                pos2(xp + 6.0, ruler.top()),
                pos2(xp, ruler.top() + 8.0),
            ],
            ph,
            Stroke::NONE,
        ));

        // Interaction: drag a range handle, otherwise scrub the playhead.
        if let Some(pos) = resp.interact_pointer_pos() {
            if resp.drag_started() || resp.clicked() || self.drag.is_none() {
                let on_tracks = pos.y >= tracks.top();
                self.drag = Some(if on_tracks && (pos.x - xi).abs() <= 7.0 {
                    Drag::In
                } else if on_tracks && (pos.x - xo).abs() <= 7.0 {
                    Drag::Out
                } else {
                    Drag::Playhead
                });
            }
            let t = t_of(pos.x);
            match self.drag {
                Some(Drag::In) => clip.set_in(t),
                Some(Drag::Out) => clip.set_out(t),
                _ => {
                    clip.set_playhead(t);
                    ev.seeked = true;
                }
            }
        }
        if !resp.is_pointer_button_down_on() {
            self.drag = None;
        }
        if let Some(h) = resp.hover_pos()
            && h.y >= tracks.top()
            && ((h.x - xi).abs() <= 7.0 || (h.x - xo).abs() <= 7.0)
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        ev
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks() {
        // Ruler labels: whole seconds without milliseconds; tenths only for fine steps.
        assert_eq!(ruler_label(10.0, 5.0), "0:10");
        assert_eq!(ruler_label(65.0, 15.0), "1:05");
        assert_eq!(ruler_label(0.5, 0.5), "0:00.5");
        assert_eq!(ruler_label(2.25, 0.25), "0:02.25");
        assert_eq!(ruler_label(3725.0, 300.0), "1:02:05");
        // 60 s over 800 px: 13.3 px per second → 10 s ticks (133 px).
        assert_eq!(tick_step(60.0, 800.0, 80.0), 10.0);
        assert_eq!(tick_step(5.0, 1000.0, 80.0), 0.5);
        assert_eq!(tick_step(7200.0, 800.0, 80.0), 1800.0);
    }
}
