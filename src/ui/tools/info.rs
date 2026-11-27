//! Media Info: everything ffprobe knows about a file.

use eframe::egui::{self, Sense, Vec2};

use super::*;
use crate::ui::media::{Frames, Source, media_ext};
use crate::video::human_size;
use crate::video::probe::{fmt_bitrate, fmt_fps};

#[derive(Default)]
pub struct InfoState {
    pub src: Source,
}

/// Rows of (label, value) grouped in sections.
fn report(i: &MediaInfo) -> Vec<(String, Vec<(String, String)>)> {
    let mut out = Vec::new();
    let mut general = vec![
        ("File".to_string(), i.path.display().to_string()),
        ("Size".into(), human_size(i.size)),
        ("Duration".into(), fmt_time(i.duration, true)),
        (
            "Container".into(),
            format!("{} ({})", i.format_long, i.format),
        ),
    ];
    if let Some(b) = i.bit_rate {
        general.push(("Overall bitrate".into(), fmt_bitrate(b)));
    }
    for (k, v) in &i.tags {
        if matches!(
            k.to_lowercase().as_str(),
            "title" | "artist" | "album" | "creation_time" | "encoder" | "comment" | "date"
        ) {
            general.push((k.replace('_', " "), v.clone()));
        }
    }
    out.push(("General".to_string(), general));
    if let Some(v) = &i.video {
        let (w, h) = v.display_size();
        let mut rows = vec![
            (
                "Codec".to_string(),
                format!("{} ({})", v.codec.to_uppercase(), v.codec_long),
            ),
            ("Resolution".into(), format!("{w}×{h}")),
        ];
        if !v.profile.is_empty() {
            rows.push(("Profile".into(), v.profile.clone()));
        }
        if v.fps > 0.0 {
            rows.push(("Frame rate".into(), format!("{} fps", fmt_fps(v.fps))));
        }
        if v.rotation != 0 {
            rows.push((
                "Rotation".into(),
                format!(
                    "{}° (stored as {}×{})",
                    (360 - v.rotation) % 360,
                    v.width,
                    v.height
                ),
            ));
        }
        rows.push((
            "Pixel format".into(),
            format!(
                "{}{}",
                v.pix_fmt,
                if v.high_bit_depth() {
                    " (10-bit or more)"
                } else {
                    ""
                }
            ),
        ));
        if let Some(b) = v.bit_rate {
            rows.push(("Bitrate".into(), fmt_bitrate(b)));
        }
        if let Some(f) = v.frames {
            rows.push(("Frames".into(), f.to_string()));
        }
        out.push(("Video".into(), rows));
    }
    for (n, a) in i.audio.iter().enumerate() {
        let mut rows = vec![
            ("Codec".to_string(), a.codec.to_uppercase()),
            (
                "Channels".into(),
                if a.channel_layout.is_empty() {
                    a.channels.to_string()
                } else {
                    format!("{} ({})", a.channels, a.channel_layout)
                },
            ),
            ("Sample rate".into(), format!("{} Hz", a.sample_rate)),
        ];
        if let Some(b) = a.bit_rate {
            rows.push(("Bitrate".into(), fmt_bitrate(b)));
        }
        if !a.language.is_empty() && a.language != "und" {
            rows.push(("Language".into(), a.language.clone()));
        }
        let title = if i.audio.len() > 1 {
            format!("Audio track {}", n + 1)
        } else {
            "Audio".into()
        };
        out.push((title, rows));
    }
    if !i.subtitles.is_empty() {
        out.push((
            "Subtitles".into(),
            i.subtitles
                .iter()
                .enumerate()
                .map(|(n, s)| (format!("Track {}", n + 1), s.clone()))
                .collect(),
        ));
    }
    out
}

impl InfoState {
    pub fn ui(&mut self, ui: &mut egui::Ui, frames: &mut Frames, _caps: &Caps) -> Option<Task> {
        section(ui, "File");
        self.src.ui(ui, frames, "video or audio file", &media_ext());
        let info = self.src.info.clone()?;
        if info.has_video() {
            card(ui, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 260.0), Sense::hover());
                frames.paint(ui, &info.path, self.src.poster_time(), 640, rect);
            });
        }
        let rep = report(&info);
        for (title, rows) in &rep {
            section(ui, title);
            card(ui, |ui| {
                egui::Grid::new(title)
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for (k, v) in rows {
                            ui.label(secondary(ui, k.as_str()));
                            ui.add(egui::Label::new(v.as_str()).wrap());
                            ui.end_row();
                        }
                    });
            });
        }
        ui.add_space(8.0);
        if ui.button(labeled(I_COPY, "Copy as text")).clicked() {
            let text: String = rep
                .iter()
                .map(|(t, rows)| {
                    let body: String = rows.iter().map(|(k, v)| format!("  {k}: {v}\n")).collect();
                    format!("{t}\n{body}")
                })
                .collect();
            ui.ctx().copy_text(text);
        }
        None
    }
}
