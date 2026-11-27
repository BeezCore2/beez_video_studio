//! Finding and running FFmpeg.
//!
//! The app does not link FFmpeg; it runs `ffmpeg` / `ffprobe` as child processes. They are
//! looked up, in this order:
//!   1. the folder named by the `BEEZVIDEO_FFMPEG_DIR` environment variable,
//!   2. next to the app's executable (also `ffmpeg\` and `ffmpeg\bin\` beside it),
//!   3. a folder the user picked ("Locate FFmpeg…", remembered in `ffmpeg-dir.txt`),
//!   4. the app's own copy in `%LOCALAPPDATA%\Beez Video Studio\ffmpeg\bin` (see `download`),
//!   5. the `PATH`.
//!
//! While encoding, FFmpeg writes machine-readable progress (`-progress pipe:1`) to stdout:
//! `out_time_us=…` tells how far into the media it is, which, divided by the duration,
//! gives the progress bar. Cancelling kills the process.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use super::progress::{Cancelled, Progress};

pub const DIR_ENV: &str = "BEEZVIDEO_FFMPEG_DIR";

#[derive(Clone, Debug)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// The folder where the app keeps its own downloaded copy of FFmpeg.
pub fn private_dir() -> PathBuf {
    crate::startup::data_dir().join("ffmpeg").join("bin")
}

fn chosen_dir_file() -> PathBuf {
    crate::startup::data_dir().join("ffmpeg-dir.txt")
}

/// Remember a folder containing ffmpeg and ffprobe chosen by the user. Returns the tools
/// when the folder (or its `bin` subfolder) really contains them.
pub fn set_dir(dir: &Path) -> Option<Tools> {
    let dir = [dir.to_path_buf(), dir.join("bin")]
        .into_iter()
        .find(|d| d.join(exe("ffmpeg")).is_file() && d.join(exe("ffprobe")).is_file())?;
    let _ = std::fs::create_dir_all(crate::startup::data_dir());
    std::fs::write(chosen_dir_file(), dir.to_string_lossy().as_bytes()).ok()?;
    refresh()
}

fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = std::env::var_os(DIR_ENV) {
        dirs.push(PathBuf::from(d));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        dirs.push(dir.join("ffmpeg").join("bin"));
        dirs.push(dir.join("ffmpeg"));
        dirs.push(dir);
    }
    if let Ok(chosen) = std::fs::read_to_string(chosen_dir_file()) {
        let chosen = chosen.trim();
        if !chosen.is_empty() {
            dirs.push(PathBuf::from(chosen));
        }
    }
    dirs.push(private_dir());
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs
}

fn find() -> Option<Tools> {
    let (ff, fp) = (exe("ffmpeg"), exe("ffprobe"));
    candidate_dirs().into_iter().find_map(|d| {
        let tools = Tools {
            ffmpeg: d.join(&ff),
            ffprobe: d.join(&fp),
        };
        (tools.ffmpeg.is_file() && tools.ffprobe.is_file()).then_some(tools)
    })
}

fn cache() -> &'static Mutex<Option<Option<Tools>>> {
    static CACHE: OnceLock<Mutex<Option<Option<Tools>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// FFmpeg, if installed. The search runs once; call [`refresh`] after installing it.
pub fn tools() -> Option<Tools> {
    let mut c = cache().lock().unwrap_or_else(|e| e.into_inner());
    c.get_or_insert_with(find).clone()
}

/// Forget the cached location (and capabilities) and search again.
pub fn refresh() -> Option<Tools> {
    *cache().lock().unwrap_or_else(|e| e.into_inner()) = None;
    *caps_cache().lock().unwrap_or_else(|e| e.into_inner()) = None;
    tools()
}

pub fn require() -> Result<Tools> {
    tools().context(
        "FFmpeg was not found. Use \"Install FFmpeg\" on the Home screen, or put ffmpeg.exe \
         and ffprobe.exe next to the app.",
    )
}

/// A command that does not flash a console window on Windows.
pub fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null());
    cmd
}

/// First line of `ffmpeg -version`, e.g. "ffmpeg version 7.1 Copyright …".
pub fn version(t: &Tools) -> Option<String> {
    let out = command(&t.ffmpeg).arg("-version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().next()?;
    let v = line
        .strip_prefix("ffmpeg version ")
        .and_then(|r| r.split_whitespace().next())
        .unwrap_or(line);
    Some(v.to_string())
}

// ---------------------------------------------------------------------------------------
// Encoder capabilities
// ---------------------------------------------------------------------------------------

/// Which encoders this FFmpeg build has, and which hardware encoders actually work here.
#[derive(Clone, Debug, Default)]
pub struct Caps {
    pub encoders: Vec<String>,
    /// Hardware encoders that passed a real test encode on this PC.
    pub hardware: Vec<String>,
}

impl Caps {
    pub fn has(&self, encoder: &str) -> bool {
        self.encoders.iter().any(|e| e == encoder)
    }

    pub fn hw(&self, encoder: &str) -> bool {
        self.hardware.iter().any(|e| e == encoder)
    }
}

/// GPU encoders to probe, fastest-quality first per vendor.
pub const HW_ENCODERS: &[&str] = &[
    "h264_nvenc",
    "hevc_nvenc",
    "av1_nvenc",
    "h264_qsv",
    "hevc_qsv",
    "av1_qsv",
    "h264_amf",
    "hevc_amf",
    "av1_amf",
];

fn caps_cache() -> &'static Mutex<Option<Caps>> {
    static CACHE: OnceLock<Mutex<Option<Caps>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Parse `ffmpeg -encoders`: lines like ` V....D libx264   libx264 H.264 / AVC …`.
pub fn parse_encoders(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut started = false;
    for line in text.lines() {
        let line = line.trim_start();
        if line.starts_with("------") {
            started = true;
            continue;
        }
        if !started {
            continue;
        }
        let mut parts = line.split_whitespace();
        if let (Some(flags), Some(name)) = (parts.next(), parts.next())
            && flags.len() == 6
            && matches!(flags.as_bytes()[0], b'V' | b'A' | b'S')
        {
            names.push(name.to_string());
        }
    }
    names
}

/// Detect encoders (cached). Hardware encoders are only listed when a tiny test encode
/// succeeds, because builds include NVENC/QSV/AMF even on PCs without that GPU.
pub fn caps() -> Caps {
    if let Some(c) = caps_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
    {
        return c;
    }
    let Some(t) = tools() else {
        return Caps::default();
    };
    let encoders = command(&t.ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
        .map(|o| parse_encoders(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    let hardware = HW_ENCODERS
        .iter()
        .filter(|e| encoders.iter().any(|x| x == *e))
        .filter(|e| {
            command(&t.ffmpeg)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=black:s=256x256:d=0.2",
                    "-c:v",
                    e,
                    "-f",
                    "null",
                    "-",
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
        .map(|e| e.to_string())
        .collect();
    let caps = Caps { encoders, hardware };
    *caps_cache().lock().unwrap_or_else(|e| e.into_inner()) = Some(caps.clone());
    caps
}

// ---------------------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------------------

/// Where one FFmpeg run sits inside the whole operation's progress bar.
#[derive(Clone, Copy)]
pub struct Span<'a> {
    pub progress: &'a Progress,
    /// Progress-bar range this run fills.
    pub from: f32,
    pub to: f32,
    /// Duration of the output in seconds (0 = unknown → no percentage).
    pub duration: f64,
    pub stage: &'a str,
}

/// Parse a `-progress` value like `out_time_us=12345678` into seconds.
fn out_time(line: &str) -> Option<f64> {
    let v = line
        .strip_prefix("out_time_us=")
        .or_else(|| line.strip_prefix("out_time_ms="))?;
    v.trim()
        .parse::<i64>()
        .ok()
        .filter(|&us| us >= 0)
        .map(|us| us as f64 / 1e6)
}

pub fn format_eta(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Run FFmpeg with `args` (inputs, filters, outputs; global flags are added here).
/// Reports progress into `span`, stops when the user cancels, and turns failures into a
/// readable error built from FFmpeg's own messages.
pub fn run(args: &[OsString], span: Span) -> Result<()> {
    run_capture(args, span, "error").map(|_| ())
}

/// Like [`run`], but logs at `loglevel` and returns FFmpeg's log (stderr), for analysis
/// filters that print their results there (e.g. `loudnorm`).
pub fn run_capture(args: &[OsString], span: Span, loglevel: &str) -> Result<String> {
    let t = require()?;
    span.progress.check()?;
    let mut child = command(&t.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-y", "-loglevel", loglevel])
        .args(["-nostats", "-progress", "pipe:1", "-stats_period", "0.25"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not start FFmpeg")?;

    // stdout: progress lines → channel. stderr: error messages, collected in the background.
    let (tx, rx) = mpsc::channel::<String>();
    let stdout = child.stdout.take().context("No FFmpeg output")?;
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut stderr = child.stderr.take().context("No FFmpeg output")?;
    let errors = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });

    let started = Instant::now();
    let mut speed = String::new();
    span.progress.set(span.from, span.stage);
    loop {
        if span.progress.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Cancelled.into());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if let Some(s) = line.strip_prefix("speed=") {
                    speed = s.trim().to_string();
                } else if let Some(t) = out_time(&line)
                    && span.duration > 0.0
                {
                    let f = (t / span.duration).clamp(0.0, 1.0);
                    let mut stage = span.stage.to_string();
                    if !speed.is_empty() && speed != "N/A" {
                        stage.push_str(&format!(" · {speed}"));
                    }
                    let elapsed = started.elapsed().as_secs_f64();
                    if f > 0.02 && elapsed > 1.0 {
                        let left = elapsed / f - elapsed;
                        stage.push_str(&format!(" · about {} left", format_eta(left)));
                    }
                    span.progress
                        .set(span.from + (span.to - span.from) * f as f32, stage);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait().context("FFmpeg stopped unexpectedly")?;
    let errors = errors.join().unwrap_or_default();
    if !status.success() {
        bail!("{}", explain(&errors, status.code()));
    }
    span.progress.set(span.to, span.stage);
    Ok(errors)
}

/// Turn FFmpeg's error output into a short message.
fn explain(stderr: &str, code: Option<i32>) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return format!(
            "FFmpeg failed (exit code {}).",
            code.map(|c| c.to_string()).unwrap_or("?".into())
        );
    }
    let tail = &lines[lines.len().saturating_sub(4)..];
    format!("FFmpeg failed: {}", tail.join(" · "))
}

/// Decode one frame at `secs` and return it as PNG bytes, scaled to `width` pixels wide.
pub fn grab_frame(path: &Path, secs: f64, width: u32) -> Result<Vec<u8>> {
    let t = require()?;
    let out = command(&t.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error"])
        .args([
            OsStr::new("-ss"),
            OsStr::new(&format!("{:.3}", secs.max(0.0))),
        ])
        .arg("-i")
        .arg(path)
        .args(["-frames:v", "1", "-an", "-sn", "-vf"])
        .arg(format!("scale={width}:-2:flags=bilinear"))
        .args(["-f", "image2pipe", "-c:v", "png", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Could not start FFmpeg")?;
    if out.stdout.is_empty() {
        bail!(
            "{}",
            explain(&String::from_utf8_lossy(&out.stderr), out.status.code())
        );
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_lines() {
        assert_eq!(out_time("out_time_us=2500000"), Some(2.5));
        assert_eq!(out_time("out_time_ms=1000000"), Some(1.0));
        assert_eq!(out_time("out_time_us=N/A"), None);
        assert_eq!(out_time("out_time_us=-500"), None);
        assert_eq!(out_time("frame=10"), None);
    }

    #[test]
    fn eta_format() {
        assert_eq!(format_eta(5.4), "0:05");
        assert_eq!(format_eta(125.0), "2:05");
        assert_eq!(format_eta(3725.0), "1:02:05");
    }

    #[test]
    fn parses_encoder_list() {
        let text = "Encoders:\n V..... = Video\n ------\n V....D libx264              libx264 H.264\n V....D h264_nvenc           NVIDIA NVENC\n A....D aac                  AAC\n S..... srt                  SubRip\n";
        assert_eq!(
            parse_encoders(text),
            ["libx264", "h264_nvenc", "aac", "srt"]
        );
    }

    #[test]
    fn explains_errors() {
        assert_eq!(explain("", Some(1)), "FFmpeg failed (exit code 1).");
        assert_eq!(
            explain("a\n\nb\nc\nd\ne\n", Some(1)),
            "FFmpeg failed: b · c · d · e"
        );
    }
}
