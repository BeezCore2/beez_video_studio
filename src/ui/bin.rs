//! The media bin: imported clips, each with its own playhead and In/Out range.

use std::path::PathBuf;

use eframe::egui::{self, Align2, FontId, Rect, Sense, Stroke, StrokeKind, Vec2, pos2};

use super::media::{Frames, Source, file_name, media_ext, pick_files};
use super::theme::*;
use crate::video::probe::{MediaInfo, fmt_time};

/// A clip in the bin.
pub struct Clip {
    pub src: Source,
    /// Current position in seconds.
    pub playhead: f64,
    /// In/Out range (seconds) used by Trim, GIF and Save Frames.
    pub range_in: f64,
    pub range_out: f64,
    ready: bool,
}

impl Clip {
    pub fn new(path: PathBuf, ctx: &egui::Context) -> Self {
        Self {
            src: Source::new(path, ctx),
            playhead: 0.0,
            range_in: 0.0,
            range_out: 0.0,
            ready: false,
        }
    }

    /// Pick up the media details once ffprobe has read the file.
    pub fn poll(&mut self) {
        self.src.poll();
        if !self.ready
            && let Some(i) = &self.src.info
        {
            self.ready = true;
            self.range_out = i.duration;
            self.playhead = (i.duration * 0.1).min(10.0);
        }
    }

    pub fn info(&self) -> Option<&MediaInfo> {
        self.src.info.as_ref()
    }

    pub fn path(&self) -> Option<&PathBuf> {
        self.src.path.as_ref()
    }

    pub fn duration(&self) -> f64 {
        self.info().map(|i| i.duration).unwrap_or(0.0)
    }

    /// Length of one frame (for frame stepping).
    pub fn frame_len(&self) -> f64 {
        let fps = self
            .info()
            .and_then(|i| i.video.as_ref())
            .map(|v| v.fps)
            .filter(|f| *f > 0.0)
            .unwrap_or(30.0);
        1.0 / fps
    }

    pub fn set_playhead(&mut self, t: f64) {
        self.playhead = t.clamp(0.0, self.duration().max(0.0));
    }

    pub fn set_in(&mut self, t: f64) {
        self.range_in = t.clamp(0.0, (self.range_out - 0.1).max(0.0));
    }

    pub fn set_out(&mut self, t: f64) {
        self.range_out = t.clamp(self.range_in + 0.1, self.duration().max(0.1));
    }

    pub fn clear_range(&mut self) {
        self.range_in = 0.0;
        self.range_out = self.duration();
    }

    /// Whether the In/Out range is narrower than the whole clip.
    pub fn has_range(&self) -> bool {
        self.range_in > 0.01 || self.range_out < self.duration() - 0.01
    }
}

#[derive(Default)]
pub struct Bin {
    pub clips: Vec<Clip>,
    pub selected: usize,
}

impl Bin {
    /// Import files (skipping ones already in the bin) and select the first new one.
    pub fn import(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        let mut first_new = None;
        for p in paths {
            if let Some(i) = self.clips.iter().position(|c| c.path() == Some(&p)) {
                first_new.get_or_insert(i);
                continue;
            }
            self.clips.push(Clip::new(p, ctx));
            first_new.get_or_insert(self.clips.len() - 1);
        }
        if let Some(i) = first_new {
            self.selected = i;
        }
    }

    pub fn selected(&self) -> Option<&Clip> {
        self.clips.get(self.selected)
    }

    pub fn selected_mut(&mut self) -> Option<&mut Clip> {
        self.clips.get_mut(self.selected)
    }

    pub fn poll(&mut self) {
        for c in &mut self.clips {
            c.poll();
        }
        if self.selected >= self.clips.len() {
            self.selected = self.clips.len().saturating_sub(1);
        }
    }

    /// The bin panel. Returns true when the selection changed.
    pub fn ui(&mut self, ui: &mut egui::Ui, frames: &mut Frames) -> bool {
        let p = Palette::current(ui);
        let before = self.selected;
        ui.horizontal(|ui| {
            ui.label(strong("Media").size(15.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, true, I_ADD, "Import media…") {
                    let files = pick_files("Videos and audio", &media_ext());
                    self.import(files, ui.ctx());
                }
            });
        });
        ui.add_space(4.0);
        if self.clips.is_empty() {
            if drop_zone(ui, 120.0, "Import media", "or drop files here") {
                let files = pick_files("Videos and audio", &media_ext());
                self.import(files, ui.ctx());
            }
            return false;
        }

        enum Act {
            Remove(usize),
            Move(usize, isize),
            Reveal(PathBuf),
        }
        let mut act = None;
        let n = self.clips.len();
        egui::ScrollArea::vertical()
            .id_salt("bin")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for i in 0..n {
                    let clip = &self.clips[i];
                    let w = ui.available_width();
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 64.0), Sense::click());
                    let selected = i == self.selected;
                    let painter = ui.painter();
                    if selected {
                        painter.rect_filled(
                            rect,
                            6.0,
                            p.accent.gamma_multiply(if p.dark { 0.22 } else { 0.15 }),
                        );
                        painter.rect_stroke(
                            rect,
                            6.0,
                            Stroke::new(1.0, p.accent),
                            StrokeKind::Inside,
                        );
                    } else if resp.hovered() {
                        painter.rect_filled(rect, 6.0, p.card_hover);
                    }
                    let thumb =
                        Rect::from_min_size(rect.min + Vec2::new(6.0, 6.0), Vec2::new(92.0, 52.0));
                    painter.rect_filled(thumb, 3.0, egui::Color32::from_gray(10));
                    let path = clip.path().cloned().unwrap_or_default();
                    match clip.info() {
                        Some(info) if info.has_video() => {
                            frames.paint(ui, &path, clip.src.poster_time(), 192, thumb);
                        }
                        _ => {
                            let icon = if clip.src.error.is_some() {
                                I_ERROR
                            } else {
                                I_AUDIO
                            };
                            ui.painter().text(
                                thumb.center(),
                                Align2::CENTER_CENTER,
                                icon,
                                FontId::proportional(20.0),
                                p.accent,
                            );
                        }
                    }
                    let painter = ui.painter();
                    let x = thumb.right() + 10.0;
                    let text_w = rect.right() - x - 6.0;
                    let name = painter.layout(
                        file_name(&path),
                        FontId::new(13.0, semibold()),
                        p.text,
                        text_w,
                    );
                    let name_h = name.size().y.min(34.0);
                    painter
                        .with_clip_rect(Rect::from_min_size(
                            pos2(x, rect.top() + 8.0),
                            Vec2::new(text_w, 34.0),
                        ))
                        .galley(pos2(x, rect.top() + 8.0), name, p.text);
                    let detail = match (clip.info(), &clip.src.error) {
                        (_, Some(_)) => "Can't read this file".to_string(),
                        (Some(i), _) => match &i.video {
                            Some(v) => {
                                let (w, h) = v.display_size();
                                format!("{} · {w}×{h}", fmt_time(i.duration, false))
                            }
                            None => format!("{} · audio", fmt_time(i.duration, false)),
                        },
                        (None, None) => "Reading…".into(),
                    };
                    painter.text(
                        pos2(x, rect.top() + 12.0 + name_h),
                        Align2::LEFT_TOP,
                        detail,
                        FontId::proportional(11.5),
                        if clip.src.error.is_some() {
                            p.error
                        } else {
                            p.text2
                        },
                    );
                    if resp.clicked() {
                        self.selected = i;
                    }
                    resp.on_hover_text(path.display().to_string())
                        .context_menu(|ui| {
                            if ui.button(labeled(I_UP, "Move up")).clicked() {
                                act = Some(Act::Move(i, -1));
                                ui.close();
                            }
                            if ui.button(labeled(I_DOWN, "Move down")).clicked() {
                                act = Some(Act::Move(i, 1));
                                ui.close();
                            }
                            if ui.button(labeled(I_FOLDER, "Show in folder")).clicked() {
                                act = Some(Act::Reveal(path.clone()));
                                ui.close();
                            }
                            ui.separator();
                            if ui
                                .button(labeled(I_DELETE, "Remove from project"))
                                .clicked()
                            {
                                act = Some(Act::Remove(i));
                                ui.close();
                            }
                        });
                    ui.add_space(2.0);
                }
                ui.add_space(6.0);
                caption(
                    ui,
                    "Right-click a clip to reorder or remove it. Join Clips uses this order.",
                );
            });

        match act {
            Some(Act::Remove(i)) => {
                self.clips.remove(i);
                if self.selected > i || self.selected >= self.clips.len() {
                    self.selected = self.selected.saturating_sub(1);
                }
            }
            Some(Act::Move(i, d)) => {
                let j = i as isize + d;
                if j >= 0 && (j as usize) < self.clips.len() {
                    self.clips.swap(i, j as usize);
                    if self.selected == i {
                        self.selected = j as usize;
                    } else if self.selected == j as usize {
                        self.selected = i;
                    }
                }
            }
            Some(Act::Reveal(path)) => {
                let _ = super::show_in_folder(&path, path.parent().unwrap_or(&path));
            }
            None => {}
        }
        self.selected != before
    }
}
