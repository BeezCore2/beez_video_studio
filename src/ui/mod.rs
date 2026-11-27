//! The editor workspace, laid out like a video editing studio:
//!
//! ```text
//! ┌ top bar: logo · Import ······························· theme ┐
//! ├ media bin ┬────────── viewer ──────────┬ inspector ┬ tool rail ┤
//! │           │  frame / playback          │ settings  │ icons     │
//! │           │  transport controls        │ + action  │           │
//! ├───────────┴────────────────────────────┴───────────┴───────────┤
//! │ timeline: ruler · playhead · V1 filmstrip · A1 waveform · In/Out│
//! ├ status bar: job progress / result ·········· FFmpeg · updates  ┤
//! ```

mod bin;
mod media;
mod player;
mod theme;
mod timeline;
mod tools;
mod update;
mod viewer;

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::Result;
use eframe::egui::{
    self, Align2, Color32, FontId, Key, Margin, RichText, Sense, Stroke, TextureHandle, Vec2,
};

use crate::video::download;
use crate::video::ffmpeg::{self, Caps};
use crate::video::progress::{Progress, is_cancelled};
use bin::Bin;
use media::{Frames, View, Waves, media_ext, pick_files};
use player::Player;
use theme::*;
use timeline::Timeline;
use tools::*;
use update::UpdateUi;
use viewer::{Transport, Viewer};

struct Running {
    label: String,
    rx: mpsc::Receiver<Result<String>>,
    /// Shared with the task: progress reports flow in, the cancel request flows out.
    progress: Progress,
    started: std::time::Instant,
}

/// How the last task ended.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Done,
    Failed,
    Cancelled,
}

/// Whether FFmpeg is available, and what it can do.
enum Engine {
    Checking(mpsc::Receiver<Option<(String, Caps)>>),
    Ready { version: String, caps: Caps },
    Missing,
}

pub struct App {
    tool: Tool,
    running: Option<Running>,
    /// Last result: (outcome, message).
    status: Option<(Outcome, String)>,
    engine: Engine,
    bin: Bin,
    frames: Frames,
    waves: Waves,
    player: Player,
    viewer: Viewer,
    timeline: Timeline,
    compress: CompressState,
    convert: ConvertState,
    trim: TrimState,
    transform: TransformState,
    speed: SpeedState,
    audio: AudioState,
    gif: GifState,
    frames_tool: FramesState,
    join: JoinState,
    info: InfoState,
    update: UpdateUi,
    logo: Option<TextureHandle>,
}

fn check_engine(ctx: &egui::Context) -> Engine {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let result = ffmpeg::refresh().map(|t| {
            let version = ffmpeg::version(&t)
                .map(|v| ffmpeg::friendly_version(&v))
                .unwrap_or_else(|| "?".into());
            (version, ffmpeg::caps())
        });
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    Engine::Checking(rx)
}

fn load_logo(ctx: &egui::Context) -> Option<TextureHandle> {
    let img = image::load_from_memory(include_bytes!("../../assets/icon.png")).ok()?;
    let img = img
        .resize(64, 64, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let ci = egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    );
    Some(ctx.load_texture("logo", ci, egui::TextureOptions::LINEAR))
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        install_theme(&cc.egui_ctx);
        // Editors are dark by default; the sun/moon button switches.
        cc.egui_ctx.set_theme(egui::Theme::Dark);
        let mut app = Self {
            tool: Tool::Compress,
            running: None,
            status: None,
            engine: check_engine(&cc.egui_ctx),
            bin: Bin::default(),
            frames: Frames::default(),
            waves: Waves::default(),
            player: Player::default(),
            viewer: Viewer::default(),
            timeline: Timeline::default(),
            compress: Default::default(),
            convert: Default::default(),
            trim: Default::default(),
            transform: Default::default(),
            speed: Default::default(),
            audio: Default::default(),
            gif: Default::default(),
            frames_tool: Default::default(),
            join: Default::default(),
            info: Default::default(),
            update: UpdateUi::new(&cc.egui_ctx),
            logo: load_logo(&cc.egui_ctx),
        };
        // Files passed on the command line (e.g. "Open with…").
        let args: Vec<PathBuf> = std::env::args_os()
            .skip(1)
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .collect();
        if !args.is_empty() {
            app.bin.import(args, &cc.egui_ctx);
        }
        app
    }

    fn caps(&self) -> Caps {
        match &self.engine {
            Engine::Ready { caps, .. } => caps.clone(),
            _ => Caps::default(),
        }
    }

    fn start(&mut self, ctx: &egui::Context, (label, job): Task) {
        self.player.stop();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let progress = Progress::default();
        let task_progress = progress.clone();
        std::thread::spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&task_progress)))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("Unexpected internal error")));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.status = None;
        self.running = Some(Running {
            label,
            rx,
            progress,
            started: std::time::Instant::now(),
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Engine::Checking(rx) = &self.engine {
            match rx.try_recv() {
                Ok(Some((version, caps))) => self.engine = Engine::Ready { version, caps },
                Ok(None) | Err(mpsc::TryRecvError::Disconnected) => self.engine = Engine::Missing,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some(r) = &self.running else { return };
        let result = match r.rx.try_recv() {
            Ok(Ok(msg)) => (Outcome::Done, msg),
            Ok(Err(e)) if is_cancelled(&e) => (
                Outcome::Cancelled,
                "Cancelled. Nothing was saved.".to_string(),
            ),
            Ok(Err(e)) => (Outcome::Failed, format!("{e:#}")),
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                (Outcome::Failed, "The task stopped unexpectedly".into())
            }
        };
        self.status = Some(result);
        self.running = None;
        // FFmpeg may just have been installed.
        if matches!(self.engine, Engine::Missing) {
            self.engine = check_engine(ctx);
        }
    }

    // ---------------------------------------------------------------------------------
    // Playback
    // ---------------------------------------------------------------------------------

    fn toggle_play(&mut self) {
        if self.player.playing() {
            self.player.stop();
            return;
        }
        let Some(clip) = self.bin.selected() else {
            return;
        };
        let Some(info) = clip.info().cloned() else {
            return;
        };
        // From the start of the range when the playhead is at (or past) its end.
        let from = if clip.playhead >= clip.range_out - 0.05 {
            clip.range_in
        } else {
            clip.playhead
        };
        self.player.play(&info, from);
    }

    fn seek(&mut self, t: f64) {
        self.player.stop();
        self.player.clear();
        if let Some(c) = self.bin.selected_mut() {
            c.set_playhead(t);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        // Not while typing in a text field.
        if ctx.egui_wants_keyboard_input() || self.bin.selected().is_none() {
            return;
        }
        // Consume the keys so a focused button doesn't also react to them.
        let (space, i, o, left, right, home, end) = ctx.input_mut(|inp| {
            let none = egui::Modifiers::NONE;
            (
                inp.consume_key(none, Key::Space),
                inp.consume_key(none, Key::I),
                inp.consume_key(none, Key::O),
                inp.consume_key(none, Key::ArrowLeft),
                inp.consume_key(none, Key::ArrowRight),
                inp.consume_key(none, Key::Home),
                inp.consume_key(none, Key::End),
            )
        });
        if space {
            self.toggle_play();
        }
        let Some(c) = self.bin.selected_mut() else {
            return;
        };
        let (t, step, rin, rout) = (c.playhead, c.frame_len(), c.range_in, c.range_out);
        if i {
            c.set_in(t);
        }
        if o {
            c.set_out(t);
        }
        if left {
            self.seek(t - step);
        }
        if right {
            self.seek(t + step);
        }
        if home {
            self.seek(rin);
        }
        if end {
            self.seek(rout);
        }
    }

    // ---------------------------------------------------------------------------------
    // Panels
    // ---------------------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            if let Some(logo) = &self.logo {
                ui.add(egui::Image::from_texture((logo.id(), Vec2::splat(26.0))));
            }
            ui.label(strong("Beez Video Studio").size(15.0));
            ui.add_space(18.0);
            if ui.button(labeled(I_IMPORT, "Import media")).clicked() {
                let files = pick_files("Videos and audio", &media_ext());
                self.bin.import(files, ui.ctx());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme_toggle(ui);
            });
        });
    }

    fn engine_card(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        let p = Palette::current(ui);
        let mut task_out = None;
        match &self.engine {
            Engine::Checking(_) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Looking for FFmpeg and testing the graphics card's video encoders…");
                });
            }
            Engine::Missing => {
                let mut locate = false;
                egui::Frame::new()
                    .fill(p.accent.gamma_multiply(if p.dark { 0.16 } else { 0.08 }))
                    .stroke(Stroke::new(1.0, p.accent.gamma_multiply(0.5)))
                    .corner_radius(8.0)
                    .inner_margin(Margin::symmetric(20, 16))
                    .show(ui, |ui| {
                        ui.set_max_width(560.0);
                        ui.label(RichText::new(I_DOWNLOAD).size(26.0).color(p.accent));
                        ui.label(strong("One more step: install FFmpeg"));
                        ui.add(egui::Label::new(secondary(
                            ui,
                            "Beez Video Studio uses FFmpeg, the free video engine, to read and encode video. \
                             It is downloaded once (about 100 MB) into the app's own folder.",
                        )).wrap());
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let busy = self.running.is_some();
                            if download::supported() && ui.add_enabled_ui(!busy, |ui| accent_button(ui, true, &labeled(I_DOWNLOAD, "Install FFmpeg"))).inner {
                                task_out = task("Installing FFmpeg", download::install);
                            }
                            if ui.button("Locate…").on_hover_text("Choose a folder that contains ffmpeg and ffprobe").clicked() {
                                locate = true;
                            }
                        });
                    });
                if locate
                    && let Some(dir) = rfd::FileDialog::new()
                        .set_title("Folder with ffmpeg and ffprobe")
                        .pick_folder()
                {
                    if ffmpeg::set_dir(&dir).is_some() {
                        self.engine = check_engine(ui.ctx());
                    } else {
                        self.status = Some((
                            Outcome::Failed,
                            format!("No ffmpeg and ffprobe were found in {}", dir.display()),
                        ));
                    }
                }
            }
            Engine::Ready { .. } => {}
        }
        task_out
    }

    /// The viewer area when nothing is imported yet.
    fn empty_state(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        let mut t = None;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.12).max(10.0));
            t = self.engine_card(ui);
            ui.add_space(16.0);
            ui.set_max_width(640.0);
            if drop_zone(ui, 200.0, "Import media to start", "Click to browse, or drop videos and audio files here") {
                let files = pick_files("Videos and audio", &media_ext());
                self.bin.import(files, ui.ctx());
            }
            ui.add_space(12.0);
            caption(ui, "Space to play.   I and O mark In and Out.   ← and → step one frame.   Home and End jump to In and Out.");
        });
        t
    }

    fn tool_rail(&mut self, ui: &mut egui::Ui) {
        let p = Palette::current(ui);
        ui.add_space(6.0);
        for t in Tool::ALL {
            let selected = self.tool == t;
            let (rect, resp) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::click());
            let painter = ui.painter();
            if selected {
                painter.rect_filled(
                    rect.shrink2(Vec2::new(5.0, 2.0)),
                    6.0,
                    p.accent.gamma_multiply(if p.dark { 0.25 } else { 0.18 }),
                );
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        rect.left_center() - Vec2::new(0.0, 10.0),
                        Vec2::new(3.0, 20.0),
                    ),
                    2.0,
                    p.accent,
                );
            } else if resp.hovered() {
                painter.rect_filled(rect.shrink2(Vec2::new(5.0, 2.0)), 6.0, p.card_hover);
            }
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                t.icon(),
                FontId::proportional(18.0),
                if selected { p.accent } else { p.text2 },
            );
            if resp.on_hover_text(t.name()).clicked() {
                self.tool = t;
            }
        }
    }

    /// Status bar: job progress or the last result on the left, engine and version right.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let p = Palette::current(ui);
        ui.horizontal(|ui| {
            ui.set_height(30.0);
            if let Some(r) = &self.running {
                let pr = r.progress.clone();
                let cancelling = pr.is_cancelled();
                let elapsed = r.started.elapsed().as_secs();
                ui.spinner();
                ui.label(strong(if cancelling {
                    "Cancelling…".to_string()
                } else {
                    format!("{}…", r.label)
                }));
                if let Some(f) = pr.fraction() {
                    ui.add(
                        egui::ProgressBar::new(f)
                            .desired_width(180.0)
                            .desired_height(6.0)
                            .fill(p.accent)
                            .corner_radius(3.0),
                    );
                    ui.label(format!("{:.0}%", f * 100.0));
                }
                let stage = pr.stage();
                ui.add(
                    egui::Label::new(secondary(
                        ui,
                        format!("{stage} · {}:{:02}", elapsed / 60, elapsed % 60),
                    ))
                    .truncate(),
                );
                if ui
                    .add_enabled(!cancelling, egui::Button::new(labeled(I_CLOSE, "Cancel")))
                    .clicked()
                {
                    pr.cancel();
                }
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            if let Some((outcome, msg)) = self.status.clone() {
                let (fg, icon) = match outcome {
                    Outcome::Done => (p.success, I_CHECK),
                    Outcome::Failed => (p.error, I_ERROR),
                    Outcome::Cancelled => (p.text2, I_INFO),
                };
                ui.label(RichText::new(icon).color(fg));
                let (text, out) = match msg.rsplit_once(" → ") {
                    Some((t, path)) => (t.to_string(), Some(PathBuf::from(path.trim()))),
                    None => (msg.clone(), None),
                };
                if let Some(out) = out.filter(|_| outcome == Outcome::Done) {
                    if out.is_file() && ui.small_button(labeled(I_PLAY, "Open")).clicked() {
                        let _ = open_path(&out);
                    }
                    let dir = if out.is_dir() {
                        out.clone()
                    } else {
                        out.parent().map(Path::to_path_buf).unwrap_or_default()
                    };
                    if ui
                        .small_button(labeled(I_FOLDER, "Show in folder"))
                        .clicked()
                    {
                        let _ = show_in_folder(&out, &dir);
                    }
                    if out.is_file()
                        && ui
                            .small_button(labeled(I_ADD, "Add to media"))
                            .on_hover_text("Import the result into the media bin")
                            .clicked()
                    {
                        self.bin.import(vec![out.clone()], ui.ctx());
                    }
                }
                if icon_button(ui, true, I_CLOSE, "Dismiss") {
                    self.status = None;
                }
                ui.add(
                    egui::Label::new(RichText::new(text).color(if outcome == Outcome::Failed {
                        p.error
                    } else {
                        p.text
                    }))
                    .truncate(),
                )
                .on_hover_text(&msg);
            }
        });
    }

    fn footer_text(&self) -> String {
        let mut text = String::new();
        if let Engine::Ready { version, caps } = &self.engine {
            text.push_str(&format!("     FFmpeg {version}"));
            let gpus: Vec<&str> = [("nvenc", "NVIDIA"), ("qsv", "Intel"), ("amf", "AMD")]
                .iter()
                .filter(|(k, _)| caps.hardware.iter().any(|h| h.ends_with(k)))
                .map(|(_, n)| *n)
                .collect();
            if !gpus.is_empty() {
                text.push_str(&format!("     GPU encoding: {}", gpus.join(", ")));
            }
        }
        text
    }
}

fn theme_toggle(ui: &mut egui::Ui) {
    let dark = ui.visuals().dark_mode;
    let (icon, tip) = if dark {
        (I_SUN, "Switch to light mode")
    } else {
        (I_MOON, "Switch to dark mode")
    };
    if icon_button(ui, true, icon, tip) {
        ui.ctx().set_theme(if dark {
            egui::Theme::Light
        } else {
            egui::Theme::Dark
        });
    }
}

fn open_path(p: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(p)
        .spawn()
        .map(|_| ())
}

/// Open the folder with the file selected (Windows), or just the folder elsewhere.
fn show_in_folder(file: &Path, dir: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    if file.is_file() {
        use std::os::windows::process::CommandExt;
        // `/select,"path"` must be passed verbatim for paths with spaces.
        return std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", file.display()))
            .spawn()
            .map(|_| ());
    }
    let _ = file;
    open_path(dir)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.update.poll();
        self.frames.poll(&ctx);
        self.waves.poll(&ctx);
        self.bin.poll();

        // Playback moves the playhead; stop at the Out point when a range is set.
        if let Some(t) = self.player.poll(&ctx)
            && let Some(c) = self.bin.selected_mut()
        {
            c.playhead = t.min(c.duration());
            if c.has_range() && t >= c.range_out {
                c.playhead = c.range_out;
                self.player.stop();
            }
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| p.is_file())
                .collect()
        });
        if !dropped.is_empty() {
            self.player.stop();
            self.bin.import(dropped, &ctx);
        }
        self.shortcuts(&ctx);

        let p = Palette::current(ui);
        let panel = panel_fill(ui);
        let busy = self.running.is_some() || self.update.busy();
        let mut action: Option<Task> = None;

        egui::Panel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(panel)
                    .inner_margin(Margin::symmetric(12, 6)),
            )
            .show(ui, |ui| self.top_bar(ui));

        let footer = self.footer_text();
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(panel)
                    .inner_margin(Margin::symmetric(12, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let w = ui.available_width();
                    ui.allocate_ui_with_layout(
                        Vec2::new(w * 0.62, 30.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| self.status_bar(ui),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.update.footer(ui, &footer);
                    });
                });
            });

        egui::Panel::bottom("timeline")
            .frame(
                egui::Frame::new()
                    .fill(panel)
                    .inner_margin(Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| match self.bin.selected_mut() {
                Some(clip) => {
                    let ev = self
                        .timeline
                        .ui(ui, &mut self.frames, &mut self.waves, clip);
                    if ev.seeked {
                        self.player.stop();
                        self.player.clear();
                    }
                }
                None => {
                    ui.set_height(60.0);
                    ui.centered_and_justified(|ui| {
                        caption(ui, "The timeline shows the selected clip.")
                    });
                }
            });

        egui::Panel::left("bin")
            .resizable(true)
            .default_size(250.0)
            .size_range(190.0..=440.0)
            .frame(
                egui::Frame::new()
                    .fill(panel)
                    .inner_margin(Margin::same(10)),
            )
            .show(ui, |ui| {
                if self.bin.ui(ui, &mut self.frames) {
                    self.player.stop();
                    self.player.clear();
                }
            });

        egui::Panel::right("rail")
            .exact_size(54.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(panel))
            .show(ui, |ui| self.tool_rail(ui));

        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(330.0)
            .size_range(280.0..=520.0)
            .frame(
                egui::Frame::new()
                    .fill(panel)
                    .inner_margin(Margin::symmetric(14, 12)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.tool.icon()).size(18.0).color(p.accent));
                    ui.label(strong(self.tool.name()).size(17.0));
                });
                ui.add(egui::Label::new(secondary(ui, self.tool.description()).size(12.0)).wrap());
                ui.separator();
                let caps = self.caps();
                let cx = Ctx {
                    clip: self.bin.selected(),
                    clips: &self.bin.clips,
                    caps: &caps,
                };
                let tool = self.tool;
                let (
                    compress,
                    convert,
                    trim,
                    transform,
                    speed,
                    audio,
                    gif,
                    frames_tool,
                    join,
                    info,
                ) = (
                    &mut self.compress,
                    &mut self.convert,
                    &mut self.trim,
                    &mut self.transform,
                    &mut self.speed,
                    &mut self.audio,
                    &mut self.gif,
                    &mut self.frames_tool,
                    &mut self.join,
                    &mut self.info,
                );
                action = egui::ScrollArea::vertical()
                    .id_salt(tool.name())
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width() - 6.0);
                        ui.add_enabled_ui(!busy, |ui| match tool {
                            Tool::Compress => compress.ui(ui, &cx),
                            Tool::Convert => convert.ui(ui, &cx),
                            Tool::Trim => trim.ui(ui, &cx),
                            Tool::Transform => transform.ui(ui, &cx),
                            Tool::Speed => speed.ui(ui, &cx),
                            Tool::Audio => audio.ui(ui, &cx),
                            Tool::Gif => gif.ui(ui, &cx),
                            Tool::Frames => frames_tool.ui(ui, &cx),
                            Tool::Join => join.ui(ui, &cx),
                            Tool::Info => info.ui(ui, &cx),
                        })
                        .inner
                    })
                    .inner;
            });

        let bg = p.well;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(bg).inner_margin(Margin::same(10)))
            .show(ui, |ui| {
                if self.bin.clips.is_empty() {
                    if let Some(t) = self.empty_state(ui) {
                        action = Some(t);
                    }
                    return;
                }
                if matches!(self.engine, Engine::Missing)
                    && let Some(t) = self.engine_card(ui)
                {
                    action = Some(t);
                }
                let view = if self.tool == Tool::Transform {
                    self.transform.view()
                } else {
                    View::default()
                };
                let Some(clip) = self.bin.selected() else {
                    return;
                };
                match self
                    .viewer
                    .ui(ui, &mut self.frames, &self.player, clip, &view)
                {
                    Some(Transport::TogglePlay) => self.toggle_play(),
                    Some(Transport::Seek(t)) => self.seek(t),
                    None => {}
                }
            });

        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let rect = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("drop"),
            ));
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(150));
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!("{I_IMPORT}   Drop to import into the media bin"),
                FontId::new(26.0, semibold()),
                p.accent,
            );
        }

        self.update.dialog(&ctx);
        if let Some(t) = action {
            self.start(&ctx, t);
        }
    }
}
