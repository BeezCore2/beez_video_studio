//! Preview playback in the viewer (video only, no sound).
//!
//! FFmpeg decodes from the playhead at real-time pace (`-re`) into raw RGBA frames at a
//! reduced size; a reader thread passes them to the UI, which shows the newest one. The
//! playhead follows the time of the frame on screen.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use crate::video::codec::even;
use crate::video::ffmpeg;
use crate::video::probe::MediaInfo;

struct Running {
    child: Child,
    rx: Receiver<(f64, ColorImage)>,
    path: PathBuf,
}

#[derive(Default)]
pub struct Player {
    running: Option<Running>,
    /// The frame on screen while playing (kept after stopping until the next seek).
    pub texture: Option<TextureHandle>,
    /// Time of the frame on screen.
    pub time: f64,
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Player {
    pub fn is_playing(&self, path: &Path) -> bool {
        self.running.as_ref().is_some_and(|r| r.path == path)
    }

    pub fn playing(&self) -> bool {
        self.running.is_some()
    }

    /// Start playing `info` from `from` seconds.
    pub fn play(&mut self, info: &MediaInfo, from: f64) {
        self.stop();
        let Some(v) = &info.video else { return };
        let (dw, dh) = v.display_size();
        if dw == 0 || dh == 0 {
            return;
        }
        // Preview size: at most 960 px wide (or 540 tall for portrait videos).
        let scale = (960.0 / dw as f64).min(540.0 / dh as f64).min(1.0);
        let (w, h) = (even(dw as f64 * scale), even(dh as f64 * scale));
        let fps = if v.fps > 0.0 { v.fps.min(30.0) } else { 25.0 };
        let Ok(mut child) = ffmpeg::spawn_preview(&info.path, from, w, h, fps) else {
            return;
        };
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            return;
        };
        let (tx, rx) = mpsc::sync_channel(3);
        let frame_bytes = (w * h * 4) as usize;
        std::thread::Builder::new()
            .name("preview".into())
            .spawn(move || {
                let mut buf = vec![0u8; frame_bytes];
                let mut n: u64 = 0;
                while stdout.read_exact(&mut buf).is_ok() {
                    let img = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &buf);
                    let t = from + n as f64 / fps;
                    if tx.send((t, img)).is_err() {
                        break;
                    }
                    n += 1;
                }
            })
            .ok();
        self.time = from;
        self.running = Some(Running {
            child,
            rx,
            path: info.path.clone(),
        });
    }

    pub fn stop(&mut self) {
        if let Some(mut r) = self.running.take() {
            let _ = r.child.kill();
            let _ = r.child.wait();
        }
    }

    /// Forget the last frame (after a seek, so the viewer shows the new position).
    pub fn clear(&mut self) {
        self.texture = None;
    }

    /// Show the newest decoded frame. Returns `Some(time)` while playing, `None` when
    /// stopped or finished.
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<f64> {
        let r = self.running.as_mut()?;
        let mut latest = None;
        let mut ended = false;
        loop {
            match r.rx.try_recv() {
                Ok(f) => latest = Some(f),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    ended = true;
                    break;
                }
            }
        }
        if let Some((t, img)) = latest {
            self.time = t;
            match &mut self.texture {
                Some(tex) => tex.set(img, TextureOptions::LINEAR),
                None => {
                    self.texture = Some(ctx.load_texture("preview", img, TextureOptions::LINEAR))
                }
            }
        }
        if ended {
            self.stop();
            return None;
        }
        ctx.request_repaint();
        Some(self.time)
    }
}
