//! Application shell: Home (tool launcher) → full-window tool pages with a back button,
//! a Windows 11 style InfoBar for results, and background task handling.

mod media;
mod theme;
mod tools;
mod update;

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use anyhow::Result;
use eframe::egui::{
    self, Align2, Color32, FontId, Margin, RichText, Sense, Stroke, StrokeKind, Vec2,
};

use crate::video::download;
use crate::video::ffmpeg::{self, Caps};
use crate::video::progress::{Progress, is_cancelled};
use media::{AUDIO_EXT, Frames, has_ext, media_ext};
use theme::*;
use tools::*;
use update::UpdateUi;

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
    /// `None` = Home screen.
    tool: Option<Tool>,
    running: Option<Running>,
    /// Last result: (outcome, message).
    status: Option<(Outcome, String)>,
    engine: Engine,
    compress: CompressState,
    convert: ConvertState,
    gif: GifState,
    trim: TrimState,
    join: JoinState,
    transform: TransformState,
    speed: SpeedState,
    audio: AudioState,
    frames_tool: FramesState,
    info: InfoState,
    /// Decoded video frames, shared by all tools.
    frames: Frames,
    update: UpdateUi,
}

fn check_engine(ctx: &egui::Context) -> Engine {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let result = ffmpeg::refresh().map(|t| {
            let version = ffmpeg::version(&t).unwrap_or_else(|| "?".into());
            (version, ffmpeg::caps())
        });
        let _ = tx.send(result);
        ctx.request_repaint();
    });
    Engine::Checking(rx)
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        install_theme(&cc.egui_ctx);
        let mut app = Self {
            tool: None,
            running: None,
            status: None,
            engine: check_engine(&cc.egui_ctx),
            compress: Default::default(),
            convert: Default::default(),
            gif: Default::default(),
            trim: Default::default(),
            join: Default::default(),
            transform: Default::default(),
            speed: Default::default(),
            audio: Default::default(),
            frames_tool: Default::default(),
            info: Default::default(),
            frames: Frames::default(),
            update: UpdateUi::new(&cc.egui_ctx),
        };
        // Files passed on the command line (e.g. "Open with…").
        let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
        if !args.is_empty() {
            app.open_files(&cc.egui_ctx, None, args);
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
                "The operation was cancelled. Nothing was saved.".to_string(),
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

    /// Route opened/dropped files to a tool. On Home, the file type picks the tool.
    fn open_files(&mut self, ctx: &egui::Context, tool: Option<Tool>, files: Vec<PathBuf>) {
        let files: Vec<PathBuf> = files.into_iter().filter(|p| p.is_file()).collect();
        if files.is_empty() {
            return;
        }
        for f in &files {
            self.frames.invalidate(f);
        }
        let all_audio = files.iter().all(|p| has_ext(p, AUDIO_EXT));
        let tool = tool.unwrap_or(if all_audio {
            Tool::Audio
        } else {
            Tool::Compress
        });
        self.tool = Some(tool);
        let first = files[0].clone();
        match tool {
            Tool::Compress => self.compress.add(files, ctx),
            Tool::Convert => self.convert.add(files, ctx),
            Tool::Join => self.join.add(files, ctx),
            Tool::Gif => self.gif.src.set(first, ctx),
            Tool::Trim => self.trim.src.set(first, ctx),
            Tool::Transform => self.transform.src.set(first, ctx),
            Tool::Speed => self.speed.src.set(first, ctx),
            Tool::Audio => self.audio.src.set(first, ctx),
            Tool::Frames => self.frames_tool.src.set(first, ctx),
            Tool::Info => self.info.src.set(first, ctx),
        }
    }

    // ---------------------------------------------------------------------------------
    // Home
    // ---------------------------------------------------------------------------------

    fn engine_card(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        let p = Palette::current(ui);
        let mut task_out = None;
        match &self.engine {
            Engine::Checking(_) => {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            "Looking for FFmpeg and testing the graphics card's video encoders…",
                        );
                    });
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
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(I_DOWNLOAD).size(26.0).color(p.accent));
                            ui.add_space(8.0);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.label(strong("One more step: install FFmpeg"));
                                caption(
                                    ui,
                                    "Beez Video Studio uses FFmpeg, the free video engine, to read and encode video. \
                                     It is downloaded once (about 100 MB) into the app's own folder.",
                                );
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let busy = self.running.is_some();
                                if download::supported()
                                    && ui.add_enabled_ui(!busy, |ui| accent_button(ui, true, &labeled(I_DOWNLOAD, "Install FFmpeg"))).inner
                                {
                                    task_out = task("Installing FFmpeg", download::install);
                                }
                                if ui.button("Locate…").on_hover_text("Choose a folder that contains ffmpeg and ffprobe").clicked() {
                                    locate = true;
                                }
                            });
                        });
                    });
                ui.add_space(4.0);
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

    fn home(&mut self, ui: &mut egui::Ui) -> Option<Task> {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(title("Video Studio"));
                ui.label(secondary(ui, "Choose a tool to get started. Your videos are processed on this PC and never uploaded."));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme_toggle(ui);
            });
        });
        ui.add_space(16.0);
        let task_out = self.engine_card(ui);

        ui.add_space(4.0);
        let clicked = drop_zone(
            ui,
            150.0,
            "Drop videos here or click to browse",
            "Videos open in Compress Video · audio files open in Audio Tools",
        );
        if clicked {
            let files = rfd::FileDialog::new()
                .add_filter("Videos and audio", &media_ext())
                .pick_files()
                .unwrap_or_default();
            if !files.is_empty() {
                let ctx = ui.ctx().clone();
                self.open_files(&ctx, None, files);
            }
        }

        let card_w = ((ui.available_width() - 24.0) / 3.0).clamp(240.0, 360.0);
        for (group, tools) in Tool::GROUPS {
            ui.add_space(24.0);
            ui.label(strong(group).size(15.0));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(12.0);
                for &t in tools {
                    if tool_card(ui, t, card_w) {
                        self.tool = Some(t);
                    }
                }
            });
        }
        ui.add_space(16.0);
        task_out
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        let mut text = String::new();
        {
            if let Engine::Ready { version, caps } = &self.engine {
                text.push_str(&format!(" · FFmpeg {version}"));
                let gpus: Vec<&str> = [("nvenc", "NVIDIA"), ("qsv", "Intel"), ("amf", "AMD")]
                    .iter()
                    .filter(|(k, _)| caps.hardware.iter().any(|h| h.ends_with(k)))
                    .map(|(_, n)| *n)
                    .collect();
                if !gpus.is_empty() {
                    text.push_str(&format!(" · GPU encoding: {}", gpus.join(", ")));
                }
            }
        }
        self.update.footer(ui, &text);
    }

    // ---------------------------------------------------------------------------------
    // Tool page
    // ---------------------------------------------------------------------------------

    fn tool_page(&mut self, ui: &mut egui::Ui, tool: Tool) -> Option<Task> {
        ui.horizontal(|ui| {
            let back = egui::Button::new(RichText::new(I_BACK).size(14.0))
                .frame_when_inactive(false)
                .min_size(Vec2::splat(36.0));
            if ui.add(back).on_hover_text("Back to Home (Esc)").clicked() {
                self.tool = None;
            }
            ui.add_space(4.0);
            let home = ui.add(
                egui::Label::new(title("Home").color(Palette::current(ui).text2))
                    .sense(Sense::click()),
            );
            if home
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                self.tool = None;
            }
            ui.label(
                RichText::new(format!("  {I_RIGHT}  "))
                    .size(14.0)
                    .color(Palette::current(ui).text2),
            );
            ui.label(title(tool.name()));
        });
        ui.label(secondary(ui, tool.description()));
        ui.add_space(8.0);

        if matches!(self.engine, Engine::Missing) {
            let t = self.engine_card(ui);
            if t.is_some() {
                return t;
            }
        }

        let caps = self.caps();
        let body = |app: &mut Self, ui: &mut egui::Ui| {
            let f = &mut app.frames;
            match tool {
                Tool::Compress => app.compress.ui(ui, f, &caps),
                Tool::Convert => app.convert.ui(ui, f, &caps),
                Tool::Gif => app.gif.ui(ui, f, &caps),
                Tool::Trim => app.trim.ui(ui, f, &caps),
                Tool::Join => app.join.ui(ui, f, &caps),
                Tool::Transform => app.transform.ui(ui, f, &caps),
                Tool::Speed => app.speed.ui(ui, f, &caps),
                Tool::Audio => app.audio.ui(ui, f, &caps),
                Tool::Frames => app.frames_tool.ui(ui, f, &caps),
                Tool::Info => app.info.ui(ui, f, &caps),
            }
        };
        let busy = self.running.is_some() || self.update.busy();
        ui.add_enabled_ui(!busy, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(tool.name())
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_width(ui.available_width() - 12.0);
                    let r = body(self, ui);
                    ui.add_space(24.0);
                    r
                })
                .inner
        })
        .inner
    }

    /// Windows 11 InfoBar with progress / success / error.
    fn info_bar(&mut self, ui: &mut egui::Ui) {
        let p = Palette::current(ui);
        if let Some(r) = &self.running {
            let pr = r.progress.clone();
            let cancelling = pr.is_cancelled();
            let elapsed = r.started.elapsed().as_secs();
            let fraction = pr.fraction();
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let title = if cancelling {
                            "Cancelling…".to_string()
                        } else {
                            format!("{}…", r.label)
                        };
                        ui.label(strong(title));
                        let stage = pr.stage();
                        let mut detail = if stage.is_empty() {
                            "Starting".to_string()
                        } else {
                            stage
                        };
                        if let Some(f) = fraction {
                            detail = format!("{:.0}%  ·  {detail}", f * 100.0);
                        }
                        detail.push_str(&format!(
                            "  ·  {}:{:02} elapsed",
                            elapsed / 60,
                            elapsed % 60
                        ));
                        caption(ui, &detail);
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn = egui::Button::new(labeled(I_CLOSE, "Cancel"))
                            .min_size(Vec2::new(100.0, 32.0));
                        if ui
                            .add_enabled(!cancelling, btn)
                            .on_hover_text("Stop now. Nothing is saved.")
                            .clicked()
                        {
                            pr.cancel();
                        }
                    });
                });
                ui.add_space(6.0);
                let bar = match fraction {
                    Some(f) => egui::ProgressBar::new(f),
                    None => egui::ProgressBar::new(((ui.input(|i| i.time) * 0.6) % 1.0) as f32),
                };
                ui.add(bar.desired_height(6.0).fill(p.accent).corner_radius(3.0));
            });
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        let Some((outcome, msg)) = self.status.clone() else {
            return;
        };
        let ok = outcome == Outcome::Done;
        let (bg, fg, icon, head) = match outcome {
            Outcome::Done => (p.success_bg, p.success, I_CHECK, "Done"),
            Outcome::Failed => (p.error_bg, p.error, I_ERROR, "Something went wrong"),
            Outcome::Cancelled => (p.card, p.text2, I_INFO, "Cancelled"),
        };
        let mut close = false;
        egui::Frame::new()
            .fill(bg)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(8.0)
            .inner_margin(Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon).color(fg).size(16.0));
                    ui.add_space(6.0);
                    ui.label(strong(head));
                    // Show the produced file name rather than the whole path.
                    let (text, out) = match msg.rsplit_once(" → ") {
                        Some((t, path)) => (t.to_string(), Some(PathBuf::from(path.trim()))),
                        None => (msg.clone(), None),
                    };
                    ui.add(egui::Label::new(text).wrap());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(ui, true, I_CLOSE, "Dismiss") {
                            close = true;
                        }
                        if let Some(out) = out.filter(|_| ok) {
                            if out.is_file() && ui.button(labeled(I_PLAY, "Open")).clicked() {
                                let _ = open_path(&out);
                            }
                            let dir = if out.is_dir() {
                                Some(out.clone())
                            } else {
                                out.parent().map(Path::to_path_buf)
                            };
                            if let Some(dir) = dir
                                && ui.button(labeled(I_FOLDER, "Show in folder")).clicked()
                            {
                                let _ = show_in_folder(&out, &dir);
                            }
                        }
                    });
                });
            });
        if close {
            self.status = None;
        }
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

/// A launcher card on the Home screen. Returns true when clicked.
fn tool_card(ui: &mut egui::Ui, tool: Tool, width: f32) -> bool {
    let p = Palette::current(ui);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 92.0), Sense::click());
    let painter = ui.painter();
    let hovered = resp.hovered();
    painter.rect_filled(rect, 8.0, if hovered { p.card_hover } else { p.card });
    painter.rect_stroke(rect, 8.0, Stroke::new(1.0, p.border), StrokeKind::Inside);

    let icon_box = egui::Rect::from_min_size(rect.min + Vec2::new(16.0, 16.0), Vec2::splat(40.0));
    painter.rect_filled(
        icon_box,
        6.0,
        p.accent.gamma_multiply(if p.dark { 0.18 } else { 0.12 }),
    );
    painter.text(
        icon_box.center(),
        Align2::CENTER_CENTER,
        tool.icon(),
        FontId::proportional(20.0),
        p.accent,
    );

    let x = icon_box.right() + 14.0;
    painter.text(
        egui::pos2(x, rect.top() + 16.0),
        Align2::LEFT_TOP,
        tool.name(),
        FontId::new(15.0, semibold()),
        p.text,
    );
    let desc = painter.layout(
        tool.description().to_string(),
        FontId::proportional(12.0),
        p.text2,
        rect.right() - x - 16.0,
    );
    painter.galley(egui::pos2(x, rect.top() + 40.0), desc, p.text2);
    if hovered {
        painter.text(
            rect.right_top() + Vec2::new(-14.0, 18.0),
            Align2::RIGHT_TOP,
            I_RIGHT,
            FontId::proportional(11.0),
            p.text2,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
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

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() && self.running.is_none() {
            self.open_files(&ctx, self.tool, dropped);
        }

        let bg = Palette::current(ui).bg;
        if self.tool.is_none() {
            egui::Panel::bottom("home_footer")
                .frame(egui::Frame::new().fill(bg).inner_margin(Margin {
                    left: 40,
                    right: 40,
                    top: 6,
                    bottom: 10,
                }))
                .show(ui, |ui| self.footer(ui));
        }
        if self.running.is_some() || self.status.is_some() {
            egui::Panel::bottom("infobar")
                .frame(egui::Frame::new().fill(bg).inner_margin(Margin {
                    left: 40,
                    right: 40,
                    top: 8,
                    bottom: 16,
                }))
                .show(ui, |ui| self.info_bar(ui));
        }

        let mut action = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(bg).inner_margin(Margin {
                left: 40,
                right: 40,
                top: 28,
                bottom: 16,
            }))
            .show(ui, |ui| match self.tool {
                None => {
                    action = egui::ScrollArea::vertical()
                        .id_salt("home")
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .auto_shrink([false, false])
                        .show(ui, |ui| self.home(ui))
                        .inner;
                }
                Some(tool) => action = self.tool_page(ui, tool),
            });

        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let p = Palette::current(ui);
            let rect = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("drop"),
            ));
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(150));
            let target = self.tool.map(Tool::name).unwrap_or("Video Studio");
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                format!("{I_OPEN}   Drop to open in {target}"),
                FontId::new(26.0, semibold()),
                p.accent,
            );
        }

        if self.tool.is_some()
            && self.running.is_none()
            && !self.update.dialog_open()
            && ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.tool = None;
        }
        self.update.dialog(&ctx);

        if let Some(t) = action {
            self.start(&ctx, t);
        }
    }
}
