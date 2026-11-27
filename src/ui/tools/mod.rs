//! One state struct + `ui` method per tool. A `ui` method returns a [`Task`] when the
//! user presses the main action; the shell runs it on a background thread.

mod audio;
mod compress;
mod convert;
mod frames;
mod gif;
mod info;
mod join;
mod speed;
mod transform;
mod trim;

pub use audio::AudioState;
pub use compress::CompressState;
pub use convert::ConvertState;
pub use frames::FramesState;
pub use gif::GifState;
pub use info::InfoState;
pub use join::JoinState;
pub use speed::SpeedState;
pub use transform::TransformState;
pub use trim::TrimState;

use std::path::{Path, PathBuf};

use anyhow::Result;
use eframe::egui::{self, RichText, Vec2};

use super::media::{Frames, Source, file_name, media_row, pick_files};
use super::theme::*;
use crate::video::codec::{Backend, VideoCodec};
use crate::video::ffmpeg::Caps;
use crate::video::job::{self, Job};
use crate::video::probe::{MediaInfo, fmt_time, parse_time};
use crate::video::progress::Progress;

/// Background work: a label for the progress bar and the job itself.
pub type Task = (String, Box<dyn FnOnce(&Progress) -> Result<String> + Send>);

pub fn task(
    label: impl Into<String>,
    f: impl FnOnce(&Progress) -> Result<String> + Send + 'static,
) -> Option<Task> {
    Some((label.into(), Box::new(f)))
}

/// Run a prepared job as a task.
pub fn job_task(label: &str, job: Result<Job>) -> Option<Task> {
    task(label, move |pr| job::execute(job?, pr))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Compress,
    Convert,
    Gif,
    Trim,
    Join,
    Transform,
    Speed,
    Audio,
    Frames,
    Info,
}

impl Tool {
    pub const GROUPS: [(&'static str, &'static [Tool]); 3] = [
        ("Compress & convert", &[Tool::Compress, Tool::Convert, Tool::Gif]),
        ("Edit", &[Tool::Trim, Tool::Join, Tool::Transform, Tool::Speed]),
        ("Audio & more", &[Tool::Audio, Tool::Frames, Tool::Info]),
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Compress => "Compress Video",
            Tool::Convert => "Convert Format",
            Tool::Gif => "Video to GIF",
            Tool::Trim => "Trim & Cut",
            Tool::Join => "Join Videos",
            Tool::Transform => "Resize & Rotate",
            Tool::Speed => "Change Speed",
            Tool::Audio => "Audio Tools",
            Tool::Frames => "Save Frames",
            Tool::Info => "Media Info",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Tool::Compress => I_PACKAGE,
            Tool::Convert => I_CONVERT,
            Tool::Gif => I_PHOTO,
            Tool::Trim => I_CUT,
            Tool::Join => I_MERGE,
            Tool::Transform => I_ROTATE,
            Tool::Speed => I_SPEED,
            Tool::Audio => I_AUDIO,
            Tool::Frames => I_CAMERA,
            Tool::Info => I_INFO,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Tool::Compress => "Make videos smaller: pick a quality level or an exact target size.",
            Tool::Convert => "Change to MP4, MKV, WebM, MOV, AVI or audio. Copies streams when it can.",
            Tool::Gif => "Turn a clip into a sharp, small animated GIF or WebP.",
            Tool::Trim => "Keep a part of a video, or cut a section out of it.",
            Tool::Join => "Put several clips together into one video.",
            Tool::Transform => "Change the resolution, rotate, flip, crop to 9:16 or 1:1, change fps.",
            Tool::Speed => "Speed up or slow down, with natural-sounding audio.",
            Tool::Audio => "Extract, remove, normalize loudness, change volume or add music.",
            Tool::Frames => "Save a still image, a series of frames or a contact sheet.",
            Tool::Info => "See codecs, resolution, frame rate, bitrates and tracks.",
        }
    }

}

// ---------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------

/// Ask where to save; suggests `<input stem>_<suffix>.<ext>` next to the input.
pub fn save_as(input: Option<&Path>, suffix: &str, ext: &str) -> Option<PathBuf> {
    let stem = input
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or("output".into());
    let mut dlg = rfd::FileDialog::new()
        .add_filter(ext.to_uppercase(), &[ext])
        .set_file_name(format!("{stem}_{suffix}.{ext}"));
    if let Some(dir) = input.and_then(|p| p.parent()) {
        dlg = dlg.set_directory(dir);
    }
    dlg.save_file()
}

pub fn pick_folder(near: Option<&Path>) -> Option<PathBuf> {
    let mut dlg = rfd::FileDialog::new();
    if let Some(dir) = near.and_then(|p| p.parent()) {
        dlg = dlg.set_directory(dir);
    }
    dlg.pick_folder()
}

/// `dir/<stem>_<suffix>.<ext>`, adding ` (2)`, ` (3)`… when the name exists on disk or is
/// already `taken` by another file of the same batch.
pub fn output_in(dir: &Path, input: &Path, suffix: &str, ext: &str, taken: &[PathBuf]) -> PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or("output".into());
    let mut p = dir.join(format!("{stem}_{suffix}.{ext}"));
    let mut n = 2;
    while p.exists() || p == input || taken.contains(&p) {
        p = dir.join(format!("{stem}_{suffix} ({n}).{ext}"));
        n += 1;
    }
    p
}

/// Run one job per file, one after another, sharing the progress bar.
pub fn batch_task(
    label: &str,
    verb: &str,
    items: Vec<(MediaInfo, PathBuf)>,
    build: impl Fn(&MediaInfo, &Path) -> Result<Job> + Send + 'static,
) -> Option<Task> {
    let n = items.len();
    let dir = items
        .first()
        .and_then(|(_, o)| o.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let verb = verb.to_string();
    task(label, move |pr| {
        let mut done = 0;
        let mut failed = Vec::new();
        for (i, (info, out)) in items.iter().enumerate() {
            let from = i as f32 / n as f32;
            let to = (i + 1) as f32 / n as f32;
            let result = build(info, out).and_then(|j| job::execute_in(j, pr, from, to));
            match result {
                Ok(_) => done += 1,
                Err(e) if crate::video::progress::is_cancelled(&e) => return Err(e),
                Err(e) => failed.push(format!("{}: {e:#}", file_name(&info.path))),
            }
        }
        if done == 0 {
            anyhow::bail!("No file could be processed. {}", failed.join(" · "));
        }
        let mut msg = format!("{verb} {done} of {n} files");
        if !failed.is_empty() {
            msg.push_str(&format!(" ({} failed: {})", failed.len(), failed.join(" · ")));
        }
        Ok(format!("{msg} → {}", dir.display()))
    })
}

/// Plan output names for a batch (unique within the batch).
pub fn batch_outputs(infos: Vec<MediaInfo>, dir: &Path, suffix: &str, ext: impl Fn(&MediaInfo) -> String) -> Vec<(MediaInfo, PathBuf)> {
    let mut taken: Vec<PathBuf> = Vec::new();
    infos
        .into_iter()
        .map(|i| {
            let out = output_in(dir, &i.path, suffix, &ext(&i), &taken);
            taken.push(out.clone());
            (i, out)
        })
        .collect()
}

/// The list of input files of a multi-file tool, with Add / Clear. `reorder` adds up/down
/// buttons. Returns true when the list changed.
pub fn file_list(
    ui: &mut egui::Ui,
    frames: &mut Frames,
    files: &mut Vec<Source>,
    what: &str,
    ext: &[&str],
    empty_hint: &str,
    reorder: bool,
) -> bool {
    let ctx = ui.ctx().clone();
    let mut changed = false;
    let add = |files: &mut Vec<Source>, paths: Vec<PathBuf>| {
        for p in paths {
            files.push(Source::new(p, &ctx));
        }
    };
    if files.is_empty() {
        if drop_zone(ui, 170.0, &format!("Drop {what} here or click to browse"), empty_hint) {
            add(files, pick_files(what, ext));
            changed = true;
        }
        return changed;
    }
    let n = files.len();
    let mut action: Option<(usize, i32)> = None;
    for (i, f) in files.iter_mut().enumerate() {
        f.poll();
        media_row(ui, frames, f, |ui| {
            if icon_button(ui, true, I_DELETE, "Remove") {
                action = Some((i, 0));
            }
            if reorder {
                if icon_button(ui, i + 1 < n, I_DOWN, "Move down") {
                    action = Some((i, 1));
                }
                if icon_button(ui, i > 0, I_UP, "Move up") {
                    action = Some((i, -1));
                }
            }
        });
    }
    match action {
        Some((i, 0)) => {
            files.remove(i);
            changed = true;
        }
        Some((i, d)) => {
            files.swap(i, (i as i32 + d) as usize);
            changed = true;
        }
        None => {}
    }
    ui.horizontal(|ui| {
        if ui.button(labeled(I_ADD, &format!("Add {what}…"))).clicked() {
            add(files, pick_files(what, ext));
            changed = true;
        }
        if command(ui, !files.is_empty(), I_CLEAR, "Clear") {
            files.clear();
            changed = true;
        }
    });
    changed
}

/// A dropdown.
pub fn combo<T: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, value: &mut T, items: &[(T, &str)]) {
    let current = items
        .iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| *l)
        .unwrap_or("Custom");
    egui::ComboBox::from_id_salt(id)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (v, l) in items {
                ui.selectable_value(value, *v, *l);
            }
        });
}

/// A row of toggle buttons (one selected).
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, items: &[(T, &str)], min_w: f32) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        for (v, l) in items {
            let btn = egui::Button::selectable(*value == *v, *l).min_size(Vec2::new(min_w, 32.0));
            if ui.add(btn).clicked() && *value != *v {
                *value = *v;
                changed = true;
            }
        }
    });
    changed
}

/// Big two-line choice buttons (title + description), like the compression levels.
pub fn big_choices<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, items: &[(T, &str, &str)]) -> bool {
    let mut changed = false;
    let w = ((ui.available_width() - 8.0 * (items.len() as f32 - 1.0)) / items.len() as f32).clamp(120.0, 220.0);
    ui.horizontal_wrapped(|ui| {
        for (v, name, desc) in items {
            let text = format!("{name}\n{desc}");
            let btn = egui::Button::selectable(*value == *v, text).min_size(Vec2::new(w, 52.0));
            if ui.add(btn).clicked() && *value != *v {
                *value = *v;
                changed = true;
            }
        }
    });
    changed
}

pub fn footer(ui: &mut egui::Ui, enabled: bool, text: &str) -> bool {
    ui.add_space(16.0);
    accent_button(ui, enabled, text)
}

/// A time entry (`1:23.5`). Updates `value` when the text parses and stays within
/// `0..=max`; invalid text is shown in red.
pub fn time_field(ui: &mut egui::Ui, text: &mut String, value: &mut f64, max: f64) -> bool {
    let p = Palette::current(ui);
    let parsed = parse_time(text).filter(|t| max <= 0.0 || *t <= max + 1e-6);
    let mut edit = egui::TextEdit::singleline(text).desired_width(110.0).hint_text("m:ss.mmm");
    if parsed.is_none() {
        edit = edit.text_color(p.error);
    }
    let r = ui.add(edit);
    let mut changed = false;
    if r.changed()
        && let Some(t) = parse_time(text).filter(|t| max <= 0.0 || *t <= max + 1e-6)
    {
        *value = t;
        changed = true;
    }
    if r.lost_focus() {
        // Tidy the text to the canonical form.
        *text = fmt_time(*value, true);
    }
    changed
}

/// Codec dropdown: codecs this FFmpeg build cannot encode are disabled.
pub fn codec_combo(ui: &mut egui::Ui, id: &str, value: &mut VideoCodec, choices: &[VideoCodec], caps: &Caps) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.name())
        .width(260.0)
        .show_ui(ui, |ui| {
            for &c in choices {
                let ok = caps.encoders.is_empty() || caps.has(c.software_encoder());
                ui.add_enabled_ui(ok, |ui| {
                    ui.selectable_value(value, c, c.name())
                        .on_disabled_hover_text("Not included in this FFmpeg build");
                });
            }
        });
}

/// Encoder (CPU / GPU) dropdown. GPUs appear only when a test encode worked on this PC.
pub fn backend_combo(ui: &mut egui::Ui, id: &str, value: &mut Backend, codec: VideoCodec, caps: &Caps) {
    let usable: Vec<Backend> = Backend::ALL
        .into_iter()
        .filter(|&b| b == Backend::Cpu || codec.resolve(b, caps) == b)
        .collect();
    if !usable.contains(value) {
        *value = Backend::Cpu;
    }
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.name())
        .width(260.0)
        .show_ui(ui, |ui| {
            for b in usable {
                ui.selectable_value(value, b, b.name());
            }
        });
}

/// One-line hint under a section.
pub fn hint(ui: &mut egui::Ui, icon: &str, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(secondary(ui, icon).size(13.0));
        caption(ui, text);
    });
}

/// "Needs a video" message for tools that can't work on audio-only files.
pub fn needs_video(ui: &mut egui::Ui, info: Option<&MediaInfo>) -> bool {
    if let Some(i) = info
        && !i.has_video()
    {
        let p = Palette::current(ui);
        ui.label(RichText::new(format!("{I_INFO}  This file has no video. Use Audio Tools for audio files.")).color(p.error));
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_output_names() {
        let dir = std::env::temp_dir().join(format!("beez-out-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("clip.mp4");
        assert_eq!(output_in(&dir, &input, "small", "mp4", &[]), dir.join("clip_small.mp4"));
        std::fs::write(dir.join("clip_small.mp4"), "x").unwrap();
        assert_eq!(output_in(&dir, &input, "small", "mp4", &[]), dir.join("clip_small (2).mp4"));
        let taken = [dir.join("clip_small (2).mp4")];
        assert_eq!(output_in(&dir, &input, "small", "mp4", &taken), dir.join("clip_small (3).mp4"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
