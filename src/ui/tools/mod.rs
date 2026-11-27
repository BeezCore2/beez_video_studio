//! The inspector's tools. Each has a state struct whose `ui` draws its settings for the
//! selected clip and returns a [`Task`] when the user presses its action button; the app
//! runs it on a background thread.

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

use super::bin::Clip;
use super::media::file_name;
use super::theme::*;
use crate::video::codec::{Backend, VideoCodec};
use crate::video::ffmpeg::Caps;
use crate::video::job::{self, Job};
use crate::video::probe::{MediaInfo, fmt_time};
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

/// What a tool works on.
pub struct Ctx<'a> {
    /// The clip selected in the bin.
    pub clip: Option<&'a Clip>,
    /// Every clip in the bin, in order.
    pub clips: &'a [Clip],
    pub caps: &'a Caps,
}

impl Ctx<'_> {
    /// Clips that have been read and contain video.
    pub fn videos(&self) -> Vec<MediaInfo> {
        self.clips
            .iter()
            .filter_map(|c| c.info().filter(|i| i.has_video()).cloned())
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Compress,
    Convert,
    Trim,
    Transform,
    Speed,
    Audio,
    Gif,
    Frames,
    Join,
    Info,
}

impl Tool {
    pub const ALL: [Tool; 10] = [
        Tool::Compress,
        Tool::Convert,
        Tool::Trim,
        Tool::Transform,
        Tool::Speed,
        Tool::Audio,
        Tool::Gif,
        Tool::Frames,
        Tool::Join,
        Tool::Info,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Compress => "Compress",
            Tool::Convert => "Convert",
            Tool::Trim => "Trim & Cut",
            Tool::Transform => "Resize & Rotate",
            Tool::Speed => "Speed",
            Tool::Audio => "Audio",
            Tool::Gif => "GIF / WebP",
            Tool::Frames => "Save Frames",
            Tool::Join => "Join Clips",
            Tool::Info => "Media Info",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Tool::Compress => I_PACKAGE,
            Tool::Convert => I_CONVERT,
            Tool::Trim => I_CUT,
            Tool::Transform => I_ROTATE,
            Tool::Speed => I_SPEED,
            Tool::Audio => I_AUDIO,
            Tool::Gif => I_PHOTO,
            Tool::Frames => I_CAMERA,
            Tool::Join => I_MERGE,
            Tool::Info => I_INFO,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Tool::Compress => "Make the video smaller: a quality level or an exact size.",
            Tool::Convert => "Change the format. Streams are copied when possible.",
            Tool::Trim => "Export the In–Out range of the timeline, or cut it out.",
            Tool::Transform => "Resolution, rotation, flips, crop to a shape, frame rate.",
            Tool::Speed => "Speed up or slow down, with natural-sounding voices.",
            Tool::Audio => "Extract, remove, normalize, change volume or add music.",
            Tool::Gif => "Turn the In–Out range into an animated GIF or WebP.",
            Tool::Frames => "Save the current frame, a series, or a contact sheet.",
            Tool::Join => "Join all clips in the media bin, top to bottom.",
            Tool::Info => "Codecs, resolution, frame rate, bitrates and tracks.",
        }
    }
}

// ---------------------------------------------------------------------------------------
// Files
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

/// Plan output names for a batch (unique within the batch).
pub fn batch_outputs(
    infos: Vec<MediaInfo>,
    dir: &Path,
    suffix: &str,
    ext: impl Fn(&MediaInfo) -> String,
) -> Vec<(MediaInfo, PathBuf)> {
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
            msg.push_str(&format!(
                " ({} failed: {})",
                failed.len(),
                failed.join(" · ")
            ));
        }
        Ok(format!("{msg} → {}", dir.display()))
    })
}

/// The extension of `info`'s file when it is a container we can write, else `fallback`.
pub fn same_ext_or(info: &MediaInfo, fallback: &str) -> String {
    let ext = info
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match crate::video::codec::Container::from_ext(&ext) {
        Some(c) => c.ext().to_string(),
        None => fallback.to_string(),
    }
}

// ---------------------------------------------------------------------------------------
// Compact controls (the inspector is a narrow column)
// ---------------------------------------------------------------------------------------

/// A full-width dropdown.
pub fn combo<T: PartialEq + Copy>(ui: &mut egui::Ui, id: &str, value: &mut T, items: &[(T, &str)]) {
    let current = items
        .iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| *l)
        .unwrap_or("Custom");
    egui::ComboBox::from_id_salt(id)
        .selected_text(current)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (v, l) in items {
                ui.selectable_value(value, *v, *l);
            }
        });
}

/// Toggle buttons sharing the row's width (wraps when there are many).
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    items: &[(T, &str)],
) -> bool {
    let mut changed = false;
    let per_row = items.len().min(if items.len() > 4 { 3 } else { 4 });
    let gap = ui.spacing().item_spacing.x;
    let w = ((ui.available_width() - gap * (per_row as f32 - 1.0)) / per_row as f32).floor();
    for row in items.chunks(per_row) {
        ui.horizontal(|ui| {
            for (v, l) in row {
                let btn = egui::Button::selectable(*value == *v, *l).min_size(Vec2::new(w, 30.0));
                if ui.add(btn).clicked() && *value != *v {
                    *value = *v;
                    changed = true;
                }
            }
        });
    }
    changed
}

/// A vertical list of two-line choices (title + description).
pub fn choice_list<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    items: &[(T, &str, &str)],
) -> bool {
    let mut changed = false;
    let p = Palette::current(ui);
    for (v, name, desc) in items {
        let selected = *value == *v;
        let mut job = egui::text::LayoutJob::default();
        job.append(
            name,
            0.0,
            egui::TextFormat::simple(egui::FontId::new(13.5, semibold()), p.text),
        );
        job.append(
            &format!("\n{desc}"),
            0.0,
            egui::TextFormat::simple(egui::FontId::proportional(11.5), p.text2),
        );
        let btn =
            egui::Button::selectable(selected, job).min_size(Vec2::new(ui.available_width(), 40.0));
        if ui.add(btn).clicked() && !selected {
            *value = *v;
            changed = true;
        }
    }
    ui.add_space(4.0);
    changed
}

/// Codec dropdown: codecs this FFmpeg build cannot encode are disabled.
pub fn codec_combo(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut VideoCodec,
    choices: &[VideoCodec],
    caps: &Caps,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.name())
        .width(ui.available_width())
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
pub fn backend_combo(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut Backend,
    codec: VideoCodec,
    caps: &Caps,
) {
    let usable: Vec<Backend> = Backend::ALL
        .into_iter()
        .filter(|&b| b == Backend::Cpu || codec.resolve(b, caps) == b)
        .collect();
    if !usable.contains(value) {
        *value = Backend::Cpu;
    }
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.name())
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for b in usable {
                ui.selectable_value(value, b, b.name());
            }
        });
}

/// A wrapped one-line note with an icon.
pub fn hint(ui: &mut egui::Ui, icon: &str, text: &str) {
    ui.add(egui::Label::new(secondary(ui, format!("{icon}  {text}")).size(12.0)).wrap());
    ui.add_space(4.0);
}

/// A wrapped error note.
pub fn error_note(ui: &mut egui::Ui, text: &str) {
    let p = Palette::current(ui);
    ui.add(
        egui::Label::new(
            RichText::new(format!("{I_ERROR}  {text}"))
                .color(p.error)
                .size(12.0),
        )
        .wrap(),
    );
    ui.add_space(4.0);
}

/// Shown when no clip is selected, or the tool needs something the clip doesn't have.
/// Returns the clip's info when the tool can work.
pub fn require<'a>(ui: &mut egui::Ui, cx: &'a Ctx, video: bool) -> Option<&'a MediaInfo> {
    let Some(clip) = cx.clip else {
        hint(ui, I_IMPORT, "Import a clip into the media bin to start.");
        return None;
    };
    let Some(info) = clip.info() else {
        match &clip.src.error {
            Some(e) => error_note(ui, e),
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    caption(ui, "Reading the clip…");
                });
            }
        }
        return None;
    };
    if video && !info.has_video() {
        error_note(
            ui,
            "This clip has no video. Use the Audio tool for audio files.",
        );
        return None;
    }
    Some(info)
}

/// "Apply to: This clip / All N clips" for batch-capable tools. Returns true for "all".
pub fn apply_to(ui: &mut egui::Ui, all: &mut bool, n: usize) -> bool {
    if n > 1 {
        field(ui, "Apply to", "", |ui| {
            let label = format!("All {n} clips");
            segmented(ui, all, &[(false, "This clip"), (true, label.as_str())]);
        });
    } else {
        *all = false;
    }
    *all
}

/// The In–Out range summary used by range-based tools.
pub fn range_summary(ui: &mut egui::Ui, clip: &Clip) {
    let p = Palette::current(ui);
    egui::Frame::new()
        .fill(p.accent.gamma_multiply(if p.dark { 0.12 } else { 0.08 }))
        .corner_radius(6.0)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{}  →  {}",
                        fmt_time(clip.range_in, true),
                        fmt_time(clip.range_out, true)
                    ))
                    .monospace(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(strong(fmt_time(clip.range_out - clip.range_in, true)));
                });
            });
            caption(
                ui,
                "Set the range on the timeline: drag the [ ] handles, or press I and O.",
            );
        });
    ui.add_space(8.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_output_names() {
        let dir = std::env::temp_dir().join(format!("beez-out-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("clip.mp4");
        assert_eq!(
            output_in(&dir, &input, "small", "mp4", &[]),
            dir.join("clip_small.mp4")
        );
        std::fs::write(dir.join("clip_small.mp4"), "x").unwrap();
        assert_eq!(
            output_in(&dir, &input, "small", "mp4", &[]),
            dir.join("clip_small (2).mp4")
        );
        let taken = [dir.join("clip_small (2).mp4")];
        assert_eq!(
            output_in(&dir, &input, "small", "mp4", &taken),
            dir.join("clip_small (3).mp4")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
