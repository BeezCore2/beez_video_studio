//! The viewer: the frame at the playhead (or live playback) and the transport controls.

use std::path::PathBuf;

use eframe::egui::{self, Align2, Color32, FontId, RichText, Sense, TextureHandle, Vec2};

use super::bin::Clip;
use super::media::{Frames, View, file_name, paint_placeholder, paint_texture};
use super::player::Player;
use super::theme::*;
use crate::video::probe::fmt_time;

/// What the transport buttons asked for.
pub enum Transport {
    TogglePlay,
    Seek(f64),
}

#[derive(Default)]
pub struct Viewer {
    /// Last frame shown, kept on screen while the next one decodes (no flicker).
    last: Option<(PathBuf, TextureHandle)>,
}

/// Width of the frames decoded for the viewer.
const VIEW_WIDTH: u32 = 1280;

impl Viewer {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        frames: &mut Frames,
        player: &Player,
        clip: &Clip,
        view: &View,
    ) -> Option<Transport> {
        let p = Palette::current(ui);
        let mut action = None;
        let transport_h = 46.0;
        let size = Vec2::new(
            ui.available_width(),
            (ui.available_height() - transport_h).max(80.0),
        );
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        ui.painter().rect_filled(rect, 4.0, Color32::BLACK);
        let screen = rect.shrink(8.0);

        let path = clip.path().cloned()?;
        let info = clip.info();
        match info {
            Some(i) if i.has_video() => {
                if player.is_playing(&path)
                    && let Some(tex) = &player.texture
                {
                    paint_texture(ui.painter(), tex, screen, view);
                } else {
                    let ctx = ui.ctx().clone();
                    let tex = frames.get(&ctx, &path, clip.playhead, VIEW_WIDTH).cloned();
                    match (tex, &self.last) {
                        (Some(t), _) => {
                            paint_texture(ui.painter(), &t, screen, view);
                            self.last = Some((path.clone(), t));
                        }
                        (None, Some((lp, lt))) if *lp == path => {
                            paint_texture(ui.painter(), lt, screen, view);
                        }
                        _ => {
                            paint_placeholder(ui, screen, frames.failed(&path));
                        }
                    }
                }
            }
            Some(_) => {
                ui.painter().text(
                    screen.center() - Vec2::new(0.0, 14.0),
                    Align2::CENTER_CENTER,
                    I_AUDIO,
                    FontId::proportional(40.0),
                    p.accent,
                );
                ui.painter().text(
                    screen.center() + Vec2::new(0.0, 26.0),
                    Align2::CENTER_CENTER,
                    file_name(&path),
                    FontId::proportional(14.0),
                    Color32::from_gray(200),
                );
            }
            None => {
                let msg = clip.src.error.clone().unwrap_or_else(|| "Reading…".into());
                ui.painter().text(
                    screen.center(),
                    Align2::CENTER_CENTER,
                    msg,
                    FontId::proportional(14.0),
                    Color32::from_gray(200),
                );
            }
        }

        // Transport.
        let dur = clip.duration();
        let has_video = info.is_some_and(|i| i.has_video());
        let playing = player.is_playing(&path);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.set_height(34.0);
            ui.label(
                RichText::new(fmt_time(clip.playhead, true))
                    .monospace()
                    .size(15.0)
                    .strong(),
            );
            ui.label(
                RichText::new(format!(" / {}", fmt_time(dur, true)))
                    .monospace()
                    .color(p.text2),
            );
            let w = 5.0 * 36.0 + 4.0 * 6.0;
            ui.add_space(((ui.available_width() - w) / 2.0 - 150.0).max(8.0));
            let step = clip.frame_len();
            if icon_button(ui, has_video, I_PREV, "Go to In point (Home)") {
                action = Some(Transport::Seek(clip.range_in));
            }
            if icon_button(ui, has_video, I_LEFT, "Previous frame (←)") {
                action = Some(Transport::Seek(clip.playhead - step));
            }
            let (icon, tip) = if playing {
                (I_PAUSE, "Pause (Space)")
            } else {
                (I_PLAY, "Play preview (Space)")
            };
            let play = egui::Button::new(RichText::new(icon).size(16.0).color(p.on_accent))
                .fill(p.accent)
                .corner_radius(17.0)
                .min_size(Vec2::splat(34.0));
            if ui.add_enabled(has_video, play).on_hover_text(tip).clicked() {
                action = Some(Transport::TogglePlay);
            }
            if icon_button(ui, has_video, I_RIGHT, "Next frame (→)") {
                action = Some(Transport::Seek(clip.playhead + step));
            }
            if icon_button(ui, has_video, I_NEXT, "Go to Out point (End)") {
                action = Some(Transport::Seek(clip.range_out));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                caption(ui, "Preview plays without sound");
            });
        });
        action
    }
}
