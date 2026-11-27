//! Media widgets: the source-file card (with background ffprobe), and a cache of video
//! frames decoded by FFmpeg in the background for thumbnails, filmstrips and previews.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use eframe::egui::{
    self, Align2, Color32, ColorImage, FontId, Pos2, Rect, Sense, Stroke, StrokeKind,
    TextureHandle, TextureOptions, Vec2,
};

use super::theme::*;
use crate::video::probe::{self, MediaInfo};
use crate::video::{ffmpeg, human_size};

pub const VIDEO_EXT: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "ts", "mts", "m2ts",
    "3gp", "ogv", "vob", "gif",
];
pub const AUDIO_EXT: &[&str] = &[
    "mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma", "aiff",
];

pub fn media_ext() -> Vec<&'static str> {
    [VIDEO_EXT, AUDIO_EXT].concat()
}

pub fn has_ext(p: &Path, ext: &[&str]) -> bool {
    p.extension()
        .map(|e| ext.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

pub fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------
// Frame cache
// ---------------------------------------------------------------------------------------

/// (file, time in ms, width in px)
type Key = (PathBuf, u64, u32);

enum Slot {
    Loading,
    Failed,
    Ready(TextureHandle),
}

enum Decoded {
    Image(ColorImage),
    Failed,
    /// Skipped because newer requests piled up; asked for again if still needed.
    Dropped,
}

type Done = (Key, Decoded);

struct Queue {
    pending: Mutex<Vec<Key>>,
    wake: Condvar,
}

/// Decoded video frames as GPU textures. Requests are served newest-first by two worker
/// threads, and stale requests are dropped, so scrubbing a timeline stays responsive.
pub struct Frames {
    slots: HashMap<Key, Slot>,
    queue: Arc<Queue>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    started: bool,
}

impl Default for Frames {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            slots: HashMap::new(),
            queue: Arc::new(Queue {
                pending: Mutex::new(Vec::new()),
                wake: Condvar::new(),
            }),
            tx,
            rx,
            started: false,
        }
    }
}

/// How many queued requests are kept; older ones are dropped (and re-requested if still
/// on screen).
const MAX_PENDING: usize = 12;

impl Frames {
    fn start(&mut self, ctx: &egui::Context) {
        if self.started {
            return;
        }
        self.started = true;
        for _ in 0..2 {
            let queue = self.queue.clone();
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            std::thread::Builder::new()
                .name("frames".into())
                .spawn(move || {
                    loop {
                        let (key, dropped) = {
                            let mut p = queue.pending.lock().unwrap_or_else(|e| e.into_inner());
                            while p.is_empty() {
                                p = queue.wake.wait(p).unwrap_or_else(|e| e.into_inner());
                            }
                            let key = p.pop().expect("not empty");
                            let excess = p.len().saturating_sub(MAX_PENDING);
                            let dropped: Vec<Key> = p.drain(..excess).collect();
                            (key, dropped)
                        };
                        for d in dropped {
                            let _ = tx.send((d, Decoded::Dropped));
                        }
                        let image = ffmpeg::grab_frame(&key.0, key.1 as f64 / 1000.0, key.2)
                            .ok()
                            .and_then(|png| image::load_from_memory(&png).ok())
                            .map(|img| {
                                let rgba = img.to_rgba8();
                                Decoded::Image(ColorImage::from_rgba_unmultiplied(
                                    [rgba.width() as usize, rgba.height() as usize],
                                    rgba.as_raw(),
                                ))
                            })
                            .unwrap_or(Decoded::Failed);
                        if tx.send((key, image)).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                })
                .ok();
        }
    }

    /// Move decoded frames into textures. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((key, result)) = self.rx.try_recv() {
            match result {
                Decoded::Image(img) => {
                    let name = format!("{}@{}x{}", key.0.display(), key.1, key.2);
                    let tex = ctx.load_texture(name, img, TextureOptions::LINEAR);
                    self.slots.insert(key, Slot::Ready(tex));
                }
                Decoded::Dropped => {
                    self.slots.remove(&key);
                }
                Decoded::Failed => {
                    self.slots.insert(key, Slot::Failed);
                }
            }
        }
    }

    /// Forget a file's frames (it may have changed on disk).
    pub fn invalidate(&mut self, path: &Path) {
        self.slots.retain(|k, _| k.0 != path);
    }

    /// The frame at `secs`, `width` pixels wide; requested in the background if needed.
    /// Times are rounded to 0.1 s so nearby requests share a frame.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        secs: f64,
        width: u32,
    ) -> Option<&TextureHandle> {
        let ms = ((secs.max(0.0) * 10.0).round() as u64) * 100;
        let key = (path.to_path_buf(), ms, width);
        if !self.slots.contains_key(&key) {
            self.start(ctx);
            self.slots.insert(key.clone(), Slot::Loading);
            let mut p = self.queue.pending.lock().unwrap_or_else(|e| e.into_inner());
            p.retain(|k| k != &key);
            p.push(key.clone());
            self.queue.wake.notify_one();
        }
        match self.slots.get(&key) {
            Some(Slot::Ready(t)) => Some(t),
            _ => None,
        }
    }

    pub fn failed(&self, path: &Path) -> bool {
        self.slots
            .iter()
            .any(|(k, s)| k.0 == path && matches!(s, Slot::Failed))
    }

    /// Paint the frame aspect-fit into `rect` (a placeholder while it loads).
    /// Returns the painted picture's rect.
    pub fn paint(&mut self, ui: &egui::Ui, path: &Path, secs: f64, width: u32, rect: Rect) -> Rect {
        self.paint_transformed(ui, path, secs, width, rect, &View::default())
    }

    /// Like [`paint`](Self::paint), previewing a rotation, flips and centre crop.
    pub fn paint_transformed(
        &mut self,
        ui: &egui::Ui,
        path: &Path,
        secs: f64,
        width: u32,
        rect: Rect,
        view: &View,
    ) -> Rect {
        if !ui.is_rect_visible(rect) {
            return rect;
        }
        let p = Palette::current(ui);
        let ctx = ui.ctx().clone();
        let failed = self.failed(path);
        match self.get(&ctx, path, secs, width) {
            Some(tex) => {
                let [tw, th] = tex.size();
                let (mut w, mut h) = (tw as f32, th as f32);
                if view.rotate % 180 == 90 {
                    std::mem::swap(&mut w, &mut h);
                }
                let (x0, y0, x1, y1) = view.crop_frac(w, h);
                let (w, h) = (w * (x1 - x0), h * (y1 - y0));
                let s = (rect.width() / w).min(rect.height() / h);
                let shown = Rect::from_center_size(rect.center(), Vec2::new(w * s, h * s));
                let uv = view.uvs((x0, y0, x1, y1));
                let mut mesh = egui::Mesh::with_texture(tex.id());
                let corners = [
                    shown.left_top(),
                    shown.right_top(),
                    shown.right_bottom(),
                    shown.left_bottom(),
                ];
                for (pos, uv) in corners.iter().zip(uv) {
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: *pos,
                        uv,
                        color: Color32::WHITE,
                    });
                }
                mesh.indices.extend([0, 1, 2, 0, 2, 3]);
                ui.painter().add(egui::Shape::mesh(mesh));
                ui.painter().rect_stroke(
                    shown,
                    0.0,
                    Stroke::new(1.0, Color32::from_black_alpha(50)),
                    StrokeKind::Outside,
                );
                shown
            }
            None => {
                let shown = Rect::from_center_size(
                    rect.center(),
                    Vec2::new(rect.width().min(rect.height() * 16.0 / 9.0), rect.height()),
                );
                ui.painter().rect_filled(
                    shown,
                    4.0,
                    if p.dark {
                        Color32::from_gray(45)
                    } else {
                        Color32::from_gray(225)
                    },
                );
                let label = if failed { I_VIDEO } else { "…" };
                ui.painter().text(
                    shown.center(),
                    Align2::CENTER_CENTER,
                    label,
                    FontId::proportional(18.0),
                    p.text2,
                );
                shown
            }
        }
    }
}

/// A preview transform: clockwise rotation, flips (applied after rotating) and a centre
/// crop to an aspect ratio — the same order as the Resize & Rotate filters.
#[derive(Clone, Copy, Default)]
pub struct View {
    pub rotate: u32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub crop: Option<(u32, u32)>,
}

impl View {
    /// The crop as fractions (x0, y0, x1, y1) of a `w × h` rotated picture.
    fn crop_frac(&self, w: f32, h: f32) -> (f32, f32, f32, f32) {
        let Some((rw, rh)) = self.crop else {
            return (0.0, 0.0, 1.0, 1.0);
        };
        let target = rw as f32 / rh as f32;
        if w / h > target {
            let f = (h * target / w).min(1.0);
            ((1.0 - f) / 2.0, 0.0, (1.0 + f) / 2.0, 1.0)
        } else {
            let f = (w / target / h).min(1.0);
            (0.0, (1.0 - f) / 2.0, 1.0, (1.0 + f) / 2.0)
        }
    }

    /// Texture coordinates for the displayed corners (top-left, top-right, bottom-right,
    /// bottom-left) of the crop rectangle `c` (in displayed, rotated space).
    fn uvs(&self, c: (f32, f32, f32, f32)) -> [Pos2; 4] {
        // Source corners in the same order.
        let src = [
            Pos2::new(0.0, 0.0),
            Pos2::new(1.0, 0.0),
            Pos2::new(1.0, 1.0),
            Pos2::new(0.0, 1.0),
        ];
        // After rotating k quarter turns clockwise, displayed corner j shows source corner
        // (j + 4 − k) mod 4.
        let k = (self.rotate / 90 % 4) as usize;
        let mut d: [Pos2; 4] = std::array::from_fn(|j| src[(j + 4 - k) % 4]);
        if self.flip_h {
            d.swap(0, 1);
            d.swap(2, 3);
        }
        if self.flip_v {
            d.swap(0, 3);
            d.swap(1, 2);
        }
        // Bilinear map from displayed (u, v) to texture coordinates.
        let at = |u: f32, v: f32| -> Pos2 {
            let top = d[0] + (d[1] - d[0]) * u;
            let bottom = d[3] + (d[2] - d[3]) * u;
            top + (bottom - top) * v
        };
        let (x0, y0, x1, y1) = c;
        [at(x0, y0), at(x1, y0), at(x1, y1), at(x0, y1)]
    }
}

// ---------------------------------------------------------------------------------------
// Source file
// ---------------------------------------------------------------------------------------

/// One input file: its path and, once ffprobe has read it, its media details.
#[derive(Default)]
pub struct Source {
    pub path: Option<PathBuf>,
    pub info: Option<MediaInfo>,
    pub error: Option<String>,
    loading: Option<Receiver<Result<MediaInfo, String>>>,
}

impl Source {
    pub fn new(p: PathBuf, ctx: &egui::Context) -> Self {
        let mut s = Self::default();
        s.set(p, ctx);
        s
    }

    pub fn set(&mut self, p: PathBuf, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let path = p.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(probe::probe(&path).map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        self.path = Some(p);
        self.info = None;
        self.error = None;
        self.loading = Some(rx);
    }

    pub fn poll(&mut self) {
        let Some(rx) = &self.loading else { return };
        match rx.try_recv() {
            Ok(Ok(info)) => {
                self.info = Some(info);
                self.loading = None;
            }
            Ok(Err(e)) => {
                self.error = Some(e);
                self.loading = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.error = Some("Could not read the file".into());
                self.loading = None;
            }
        }
    }

    pub fn loading(&self) -> bool {
        self.loading.is_some()
    }

    /// Media with a video stream, ready to use.
    pub fn video(&self) -> Option<&MediaInfo> {
        self.info.as_ref().filter(|i| i.has_video())
    }

    /// A time for the card's thumbnail: a tenth of the way in (skips black intros).
    pub fn poster_time(&self) -> f64 {
        self.info
            .as_ref()
            .map(|i| (i.duration * 0.1).min(30.0))
            .unwrap_or(0.0)
    }

    /// File card (or a drop zone when empty). Returns true when the file changed.
    pub fn ui(&mut self, ui: &mut egui::Ui, frames: &mut Frames, what: &str, ext: &[&str]) -> bool {
        self.poll();
        let ctx = ui.ctx().clone();
        if self.path.is_none() {
            let clicked = drop_zone(
                ui,
                170.0,
                &format!("Drop a {what} here or click to browse"),
                "MP4, MOV, MKV, WebM, AVI and more",
            );
            ui.add_space(4.0);
            if clicked && let Some(p) = pick_file(what, ext) {
                frames.invalidate(&p);
                self.set(p, &ctx);
                return true;
            }
            return false;
        }
        let mut action = None;
        media_row(ui, frames, self, |ui| {
            if ui.button(labeled(I_OPEN, "Change…")).clicked() {
                action = pick_file(what, ext);
            }
        });
        if let Some(p) = action {
            frames.invalidate(&p);
            self.set(p, &ctx);
            return true;
        }
        false
    }
}

/// A card for one media file: thumbnail, name, details, and controls on the right.
pub fn media_row(
    ui: &mut egui::Ui,
    frames: &mut Frames,
    src: &Source,
    controls: impl FnOnce(&mut egui::Ui),
) {
    let Some(path) = src.path.clone() else { return };
    let p = Palette::current(ui);
    card(ui, |ui| {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(128.0, 72.0), Sense::hover());
            match &src.info {
                Some(i) if i.has_video() => {
                    frames.paint(ui, &path, src.poster_time(), 256, rect);
                }
                _ => {
                    ui.painter()
                        .rect_filled(rect, 4.0, p.accent.gamma_multiply(0.15));
                    let icon = if src.error.is_some() {
                        I_ERROR
                    } else {
                        I_AUDIO
                    };
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        icon,
                        FontId::proportional(24.0),
                        p.accent,
                    );
                }
            }
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.add_space(6.0);
                ui.label(strong(file_name(&path)));
                if let Some(e) = &src.error {
                    ui.label(RichTextExt::colored(e, p.error));
                } else if let Some(i) = &src.info {
                    caption(ui, &format!("{} · {}", i.summary(), human_size(i.size)));
                } else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        caption(ui, "Reading…");
                    });
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), controls);
        });
    });
}

/// Small helper so error text can be coloured without importing RichText everywhere.
pub struct RichTextExt;

impl RichTextExt {
    pub fn colored(text: &str, color: Color32) -> egui::RichText {
        egui::RichText::new(text).color(color)
    }
}

pub fn pick_files(name: &str, ext: &[&str]) -> Vec<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(name, ext)
        .add_filter("All files", &["*"])
        .pick_files()
        .unwrap_or_default()
}

pub fn pick_file(name: &str, ext: &[&str]) -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(name, ext)
        .add_filter("All files", &["*"])
        .pick_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Pos2, x: f32, y: f32) -> bool {
        (a.x - x).abs() < 1e-5 && (a.y - y).abs() < 1e-5
    }

    #[test]
    fn view_uvs() {
        let full = (0.0, 0.0, 1.0, 1.0);
        let id = View::default().uvs(full);
        assert!(near(id[0], 0.0, 0.0) && near(id[2], 1.0, 1.0));
        // 90° clockwise: the top-left of the result shows the source's bottom-left.
        let r = View {
            rotate: 90,
            ..Default::default()
        }
        .uvs(full);
        assert!(near(r[0], 0.0, 1.0) && near(r[1], 0.0, 0.0) && near(r[2], 1.0, 0.0));
        let f = View {
            flip_h: true,
            ..Default::default()
        }
        .uvs(full);
        assert!(near(f[0], 1.0, 0.0) && near(f[1], 0.0, 0.0));
        // Square crop of a 16:9 frame keeps the middle.
        let v = View {
            crop: Some((1, 1)),
            ..Default::default()
        };
        let c = v.crop_frac(1920.0, 1080.0);
        assert!((c.0 - 0.21875).abs() < 1e-5 && (c.2 - 0.78125).abs() < 1e-5);
    }
}
