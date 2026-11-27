//! Codecs, containers and the encoder settings shared by all tools.
//!
//! **Quality scale.** Tools talk about quality in one scale: the x264 CRF value (18 = visually
//! lossless … 23 = default … 28 = small … 35 = tiny). [`VideoCodec::args`] translates it
//! for each encoder so that the same level gives similar visual quality:
//!   * x265 CRF ≈ x264 CRF + 4 (HEVC needs ~40 % fewer bits for the same quality),
//!   * SVT-AV1 / VP9 CRF (0–63 scale) ≈ x264 CRF × 1.45,
//!   * NVENC `-cq`, Intel `-global_quality` and AMD QP: close to the x264 scale.

use std::ffi::OsString;

use super::ffmpeg::Caps;
use super::probe::{MediaInfo, VideoStream};

/// A growing FFmpeg argument list.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args(pub Vec<OsString>);

impl Args {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, a: impl Into<OsString>) -> &mut Self {
        self.0.push(a.into());
        self
    }

    pub fn pair(&mut self, k: impl Into<OsString>, v: impl Into<OsString>) -> &mut Self {
        self.push(k).push(v)
    }

    pub fn extend(&mut self, other: Args) -> &mut Self {
        self.0.extend(other.0);
        self
    }

    /// The arguments as plain strings (for tests and logs).
    pub fn strings(&self) -> Vec<String> {
        self.0
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// Whether `k` is immediately followed by `v`.
    pub fn has_pair(&self, k: &str, v: &str) -> bool {
        self.strings().windows(2).any(|w| w[0] == k && w[1] == v)
    }

    pub fn has(&self, k: &str) -> bool {
        self.strings().iter().any(|a| a == k)
    }
}

/// The output that discards everything (for analysis passes).
pub fn null_output() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

// ---------------------------------------------------------------------------------------
// Video codecs
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    H265,
    Av1,
    Vp9,
}

/// Software (CPU) encoding or a GPU encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Backend {
    #[default]
    Cpu,
    Nvidia,
    Intel,
    Amd,
}

impl Backend {
    pub const ALL: [Backend; 4] = [Backend::Cpu, Backend::Nvidia, Backend::Intel, Backend::Amd];

    pub fn name(self) -> &'static str {
        match self {
            Backend::Cpu => "CPU (best compression)",
            Backend::Nvidia => "NVIDIA GPU (NVENC)",
            Backend::Intel => "Intel GPU (Quick Sync)",
            Backend::Amd => "AMD GPU (AMF)",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Backend::Cpu => "",
            Backend::Nvidia => "nvenc",
            Backend::Intel => "qsv",
            Backend::Amd => "amf",
        }
    }
}

/// How hard the encoder works: faster encodes give bigger files at the same quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Speed {
    Fast,
    #[default]
    Balanced,
    Small,
}

impl Speed {
    pub const ALL: [Speed; 3] = [Speed::Fast, Speed::Balanced, Speed::Small];

    pub fn name(self) -> &'static str {
        match self {
            Speed::Fast => "Fast",
            Speed::Balanced => "Balanced",
            Speed::Small => "Smallest file (slow)",
        }
    }
}

/// How the bitrate is chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rate {
    /// Constant quality, on the x264 CRF scale.
    Quality(u8),
    /// Average bitrate in bits per second (for a target file size).
    Bitrate(u64),
}

impl VideoCodec {
    pub const ALL: [VideoCodec; 4] = [
        VideoCodec::H264,
        VideoCodec::H265,
        VideoCodec::Av1,
        VideoCodec::Vp9,
    ];

    pub fn name(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264 (plays everywhere)",
            VideoCodec::H265 => "H.265 / HEVC (≈40% smaller)",
            VideoCodec::Av1 => "AV1 (smallest, newer devices)",
            VideoCodec::Vp9 => "VP9 (web)",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::H265 => "H.265",
            VideoCodec::Av1 => "AV1",
            VideoCodec::Vp9 => "VP9",
        }
    }

    /// ffprobe's name for streams in this codec (to detect "already in this codec").
    pub fn probe_name(self) -> &'static str {
        match self {
            VideoCodec::H264 => "h264",
            VideoCodec::H265 => "hevc",
            VideoCodec::Av1 => "av1",
            VideoCodec::Vp9 => "vp9",
        }
    }

    pub fn software_encoder(self) -> &'static str {
        match self {
            VideoCodec::H264 => "libx264",
            VideoCodec::H265 => "libx265",
            VideoCodec::Av1 => "libsvtav1",
            VideoCodec::Vp9 => "libvpx-vp9",
        }
    }

    /// The FFmpeg encoder for this codec on `backend`, if one exists.
    pub fn encoder(self, backend: Backend) -> Option<String> {
        if backend == Backend::Cpu {
            return Some(self.software_encoder().to_string());
        }
        let base = match self {
            VideoCodec::H264 => "h264",
            VideoCodec::H265 => "hevc",
            VideoCodec::Av1 => "av1",
            VideoCodec::Vp9 => return None,
        };
        Some(format!("{base}_{}", backend.suffix()))
    }

    /// The backend to really use: the requested GPU when it works for this codec here,
    /// otherwise the CPU.
    pub fn resolve(self, backend: Backend, caps: &Caps) -> Backend {
        match self.encoder(backend) {
            Some(e) if backend != Backend::Cpu && caps.hw(&e) => backend,
            _ => Backend::Cpu,
        }
    }

    /// Whether two-pass encoding is used for a bitrate target (best size accuracy).
    pub fn two_pass(self, backend: Backend) -> bool {
        backend == Backend::Cpu
            && matches!(self, VideoCodec::H264 | VideoCodec::H265 | VideoCodec::Vp9)
    }

    /// Codec arguments (`-c:v …` and rate control), without pass settings.
    /// `ten_bit` keeps 10-bit colour for sources that have it (H.265/AV1/VP9 only).
    pub fn args(self, backend: Backend, rate: Rate, speed: Speed, ten_bit: bool) -> Args {
        let mut a = Args::new();
        let enc = self
            .encoder(backend)
            .unwrap_or_else(|| self.software_encoder().into());
        a.pair("-c:v", &enc);
        let q = |crf: u8| -> u8 {
            match (backend, self) {
                (Backend::Cpu, VideoCodec::H264) => crf,
                (Backend::Cpu, VideoCodec::H265) => crf + 4,
                (Backend::Cpu, _) => ((crf as f32) * 1.45).round().min(63.0) as u8,
                (_, VideoCodec::Av1) => ((crf as f32) * 1.45).round().min(63.0) as u8,
                (Backend::Nvidia, _) => crf + 2,
                _ => crf,
            }
        };
        let kbps = |bps: u64| format!("{}k", (bps / 1000).max(1));

        match backend {
            Backend::Cpu => {
                match self {
                    VideoCodec::H264 | VideoCodec::H265 => {
                        let preset = match speed {
                            Speed::Fast => "veryfast",
                            Speed::Balanced => "medium",
                            Speed::Small => "slow",
                        };
                        a.pair("-preset", preset);
                    }
                    VideoCodec::Av1 => {
                        let preset = match speed {
                            Speed::Fast => "10",
                            Speed::Balanced => "8",
                            Speed::Small => "5",
                        };
                        a.pair("-preset", preset);
                    }
                    VideoCodec::Vp9 => {
                        let cpu = match speed {
                            Speed::Fast => "5",
                            Speed::Balanced => "2",
                            Speed::Small => "1",
                        };
                        a.pair("-deadline", "good").pair("-cpu-used", cpu);
                        a.pair("-row-mt", "1");
                    }
                }
                match rate {
                    Rate::Quality(crf) => {
                        a.pair("-crf", q(crf).to_string());
                        if self == VideoCodec::Vp9 {
                            // VP9 constant-quality mode needs an explicit zero bitrate.
                            a.pair("-b:v", "0");
                        }
                    }
                    Rate::Bitrate(bps) => {
                        a.pair("-b:v", kbps(bps));
                    }
                }
                let pix = if ten_bit && self != VideoCodec::H264 {
                    "yuv420p10le"
                } else {
                    "yuv420p"
                };
                a.pair("-pix_fmt", pix);
                if self == VideoCodec::H265 {
                    // Quieter logs from x265.
                    a.pair("-x265-params", "log-level=error");
                }
            }
            Backend::Nvidia => {
                let preset = match speed {
                    Speed::Fast => "p2",
                    Speed::Balanced => "p5",
                    Speed::Small => "p7",
                };
                a.pair("-preset", preset).pair("-tune", "hq");
                match rate {
                    Rate::Quality(crf) => {
                        a.pair("-rc", "vbr")
                            .pair("-cq", q(crf).to_string())
                            .pair("-b:v", "0");
                    }
                    Rate::Bitrate(bps) => {
                        a.pair("-rc", "vbr").pair("-b:v", kbps(bps));
                        a.pair("-maxrate", kbps(bps * 3 / 2))
                            .pair("-bufsize", kbps(bps * 2));
                        a.pair("-multipass", "fullres");
                    }
                }
                a.pair(
                    "-pix_fmt",
                    if ten_bit && self != VideoCodec::H264 {
                        "p010le"
                    } else {
                        "nv12"
                    },
                );
            }
            Backend::Intel => {
                let preset = match speed {
                    Speed::Fast => "veryfast",
                    Speed::Balanced => "medium",
                    Speed::Small => "veryslow",
                };
                a.pair("-preset", preset);
                match rate {
                    Rate::Quality(crf) => {
                        a.pair("-global_quality", q(crf).to_string());
                    }
                    Rate::Bitrate(bps) => {
                        a.pair("-b:v", kbps(bps))
                            .pair("-maxrate", kbps(bps * 3 / 2));
                    }
                }
                a.pair(
                    "-pix_fmt",
                    if ten_bit && self != VideoCodec::H264 {
                        "p010le"
                    } else {
                        "nv12"
                    },
                );
            }
            Backend::Amd => {
                let quality = match speed {
                    Speed::Fast => "speed",
                    Speed::Balanced => "balanced",
                    Speed::Small => "quality",
                };
                a.pair("-quality", quality);
                match rate {
                    Rate::Quality(crf) => {
                        let qp = q(crf);
                        a.pair("-rc", "cqp")
                            .pair("-qp_i", qp.to_string())
                            .pair("-qp_p", (qp + 2).to_string());
                        if self == VideoCodec::H264 {
                            a.pair("-qp_b", (qp + 4).to_string());
                        }
                    }
                    Rate::Bitrate(bps) => {
                        a.pair("-rc", "vbr_peak")
                            .pair("-b:v", kbps(bps))
                            .pair("-maxrate", kbps(bps * 3 / 2));
                    }
                }
                a.pair(
                    "-pix_fmt",
                    if ten_bit && self != VideoCodec::H264 {
                        "p010le"
                    } else {
                        "nv12"
                    },
                );
            }
        }
        a
    }

    /// Arguments for pass `n` (1 or 2) of a two-pass encode, using `log` as the stats prefix.
    /// x265 gets only the file name (its option string can't hold `C:\`): run FFmpeg in
    /// the log's folder, see [`Self::pass_dir`].
    pub fn pass_args(self, n: u8, log: &str) -> Args {
        let mut a = Args::new();
        match self {
            VideoCodec::H265 => {
                let name = std::path::Path::new(log)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| log.to_string());
                a.pair(
                    "-x265-params",
                    format!("log-level=error:pass={n}:stats={name}.log"),
                );
            }
            _ => {
                a.pair("-pass", n.to_string()).pair("-passlogfile", log);
            }
        }
        a
    }

    /// The working directory two-pass encodes must run in (x265 only).
    pub fn pass_dir(self, log: &str) -> Option<std::path::PathBuf> {
        (self == VideoCodec::H265)
            .then(|| std::path::Path::new(log).parent().map(|p| p.to_path_buf()))
            .flatten()
    }

    /// The minimum sensible bits per pixel per frame for watchable output. Used to pick a
    /// resolution for a size target: below this, a smaller picture looks better than a
    /// blocky big one.
    pub fn min_bits_per_pixel(self) -> f64 {
        match self {
            VideoCodec::H264 => 0.045,
            VideoCodec::Vp9 => 0.035,
            VideoCodec::H265 => 0.03,
            VideoCodec::Av1 => 0.025,
        }
    }
}

/// Fix arguments that conflict when a codec's args are followed by pass args (x265 has a
/// single `-x265-params`): drop the earlier one.
pub fn merge_x265_params(args: &mut Args) {
    let s = args.strings();
    let idx: Vec<usize> = s
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "-x265-params")
        .map(|(i, _)| i)
        .collect();
    if idx.len() > 1 {
        let first = idx[0];
        args.0.drain(first..first + 2);
    }
}

// ---------------------------------------------------------------------------------------
// Audio codecs
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCodec {
    Aac,
    Opus,
    Mp3,
    Flac,
    Wav,
    Vorbis,
}

impl AudioCodec {
    /// An audio filter this encoder needs, to be added to the `-af` chain.
    pub fn filter(self) -> Option<&'static str> {
        match self {
            // libopus rejects some surround layouts such as "5.1(side)": map them to the
            // layouts it supports.
            AudioCodec::Opus => Some("aformat=channel_layouts=7.1|5.1|stereo|mono"),
            _ => None,
        }
    }

    /// `-c:a …` plus bitrate. `kbps` is per stream (ignored by lossless codecs; for MP3,
    /// 0 means high-quality VBR).
    pub fn args(self, kbps: u32) -> Args {
        let mut a = Args::new();
        match self {
            AudioCodec::Aac => {
                a.pair("-c:a", "aac").pair("-b:a", format!("{kbps}k"));
            }
            AudioCodec::Opus => {
                a.pair("-c:a", "libopus").pair("-b:a", format!("{kbps}k"));
            }
            AudioCodec::Mp3 => {
                a.pair("-c:a", "libmp3lame");
                if kbps == 0 {
                    a.pair("-q:a", "2");
                } else {
                    a.pair("-b:a", format!("{kbps}k"));
                }
            }
            AudioCodec::Flac => {
                a.pair("-c:a", "flac").pair("-compression_level", "8");
            }
            AudioCodec::Wav => {
                a.pair("-c:a", "pcm_s16le");
            }
            AudioCodec::Vorbis => {
                a.pair("-c:a", "libvorbis").pair("-q:a", "5");
            }
        }
        a
    }
}

// ---------------------------------------------------------------------------------------
// Containers
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Mp4,
    Mkv,
    WebM,
    Mov,
    Avi,
}

impl Container {
    pub fn ext(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mkv => "mkv",
            Container::WebM => "webm",
            Container::Mov => "mov",
            Container::Avi => "avi",
        }
    }

    pub fn from_ext(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "mp4" | "m4v" => Some(Container::Mp4),
            "mkv" => Some(Container::Mkv),
            "webm" => Some(Container::WebM),
            "mov" => Some(Container::Mov),
            "avi" => Some(Container::Avi),
            _ => None,
        }
    }

    /// Video codecs (ffprobe names) this container can hold without re-encoding.
    pub fn accepts_video(self, codec: &str) -> bool {
        match self {
            Container::Mkv => true,
            Container::Mp4 => matches!(
                codec,
                "h264" | "hevc" | "av1" | "vp9" | "mpeg4" | "mpeg2video"
            ),
            Container::Mov => matches!(codec, "h264" | "hevc" | "prores" | "mpeg4" | "mjpeg"),
            Container::WebM => matches!(codec, "vp8" | "vp9" | "av1"),
            Container::Avi => matches!(
                codec,
                "mpeg4" | "h264" | "mjpeg" | "msmpeg4v3" | "mpeg2video"
            ),
        }
    }

    pub fn accepts_audio(self, codec: &str) -> bool {
        match self {
            Container::Mkv => true,
            Container::Mp4 => matches!(
                codec,
                "aac" | "mp3" | "ac3" | "eac3" | "opus" | "alac" | "flac"
            ),
            Container::Mov => {
                matches!(codec, "aac" | "mp3" | "ac3" | "eac3" | "alac")
                    || codec.starts_with("pcm_")
            }
            Container::WebM => matches!(codec, "opus" | "vorbis"),
            Container::Avi => matches!(codec, "mp3" | "ac3" | "pcm_s16le"),
        }
    }

    /// The codecs used when re-encoding into this container.
    pub fn default_video(self) -> VideoCodec {
        match self {
            Container::WebM => VideoCodec::Vp9,
            _ => VideoCodec::H264,
        }
    }

    pub fn default_audio(self) -> AudioCodec {
        match self {
            Container::WebM => AudioCodec::Opus,
            Container::Avi => AudioCodec::Mp3,
            _ => AudioCodec::Aac,
        }
    }

    /// Video codecs offered for re-encoding into this container.
    pub fn video_choices(self) -> &'static [VideoCodec] {
        match self {
            Container::WebM => &[VideoCodec::Vp9, VideoCodec::Av1],
            Container::Avi => &[VideoCodec::H264],
            Container::Mov => &[VideoCodec::H264, VideoCodec::H265],
            _ => &VideoCodec::ALL,
        }
    }

    /// Muxer flags: MP4/MOV get their index at the front so playback starts instantly
    /// (and streams on the web); H.265 gets the `hvc1` tag Apple devices need.
    pub fn mux_args(self, video: Option<&str>) -> Args {
        let mut a = Args::new();
        if matches!(self, Container::Mp4 | Container::Mov) {
            a.pair("-movflags", "+faststart");
            if video == Some("hevc")
                || video == Some("libx265")
                || video.is_some_and(|v| v.starts_with("hevc_"))
            {
                a.pair("-tag:v", "hvc1");
            }
        }
        a
    }

    /// Subtitle handling: keep text subtitles where the container supports them.
    pub fn subtitle_args(self, subtitle_codecs: &[String]) -> Args {
        let mut a = Args::new();
        if subtitle_codecs.is_empty() {
            return a;
        }
        let text = |c: &str| matches!(c, "subrip" | "ass" | "ssa" | "mov_text" | "webvtt" | "text");
        match self {
            Container::Mkv => {
                a.pair("-map", "0:s?");
                if subtitle_codecs.iter().any(|c| c == "mov_text") {
                    a.pair("-c:s", "srt");
                } else {
                    a.pair("-c:s", "copy");
                }
            }
            Container::Mp4 | Container::Mov if subtitle_codecs.iter().all(|c| text(c)) => {
                a.pair("-map", "0:s?").pair("-c:s", "mov_text");
            }
            Container::WebM if subtitle_codecs.iter().all(|c| text(c)) => {
                a.pair("-map", "0:s?").pair("-c:s", "webvtt");
            }
            _ => {}
        }
        a
    }
}

// ---------------------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------------------

/// Round to the nearest even number (most encoders need even sizes), at least 2.
pub fn even(v: f64) -> u32 {
    ((v / 2.0).round() as u32 * 2).max(2)
}

/// New size so the *short* side is at most `limit` (so "720p" works for portrait videos
/// too). `None` when the video is already small enough.
pub fn fit_short_side(w: u32, h: u32, limit: u32) -> Option<(u32, u32)> {
    let short = w.min(h);
    if limit == 0 || short <= limit {
        return None;
    }
    let f = limit as f64 / short as f64;
    Some((even(w as f64 * f), even(h as f64 * f)))
}

/// Largest size inside `max_w × max_h` that keeps the aspect ratio.
pub fn fit_inside(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let f = (max_w as f64 / w as f64).min(max_h as f64 / h as f64);
    (even(w as f64 * f), even(h as f64 * f))
}

/// Centre crop of `w × h` to the aspect ratio `rw:rh`: (crop_w, crop_h, x, y).
pub fn crop_to_ratio(w: u32, h: u32, rw: u32, rh: u32) -> (u32, u32, u32, u32) {
    let target = rw as f64 / rh as f64;
    let (cw, ch) = if w as f64 / h as f64 > target {
        (even(h as f64 * target).min(w), h - h % 2)
    } else {
        (w - w % 2, even(w as f64 / target).min(h))
    };
    (cw, ch, (w - cw) / 2, (h - ch) / 2)
}

/// Standard heights (short side) offered for resizing.
pub const HEIGHTS: [u32; 7] = [2160, 1440, 1080, 720, 480, 360, 240];

/// For a size target: the largest standard short side (≤ the source) at which the given
/// video bitrate still gives at least the codec's minimum bits per pixel. `None` keeps the
/// source size.
pub fn auto_short_side(v: &VideoStream, video_bps: u64, codec: VideoCodec) -> Option<u32> {
    let (w, h) = v.display_size();
    let fps = if v.fps > 0.0 { v.fps.min(60.0) } else { 30.0 };
    let bpp = |w: u32, h: u32| video_bps as f64 / (w as f64 * h as f64 * fps);
    if w == 0 || h == 0 || bpp(w, h) >= codec.min_bits_per_pixel() {
        return None;
    }
    let short = w.min(h);
    let mut best = None;
    for &limit in HEIGHTS.iter().filter(|&&l| l < short) {
        let (nw, nh) = fit_short_side(w, h, limit).unwrap_or((w, h));
        best = Some(limit);
        if bpp(nw, nh) >= codec.min_bits_per_pixel() {
            break;
        }
    }
    best
}

/// Whether the source is 10/12-bit and the codec can keep that.
pub fn keep_ten_bit(info: &MediaInfo, codec: VideoCodec) -> bool {
    codec != VideoCodec::H264 && info.video.as_ref().is_some_and(|v| v.high_bit_depth())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_maps_per_encoder() {
        let x264 = VideoCodec::H264.args(Backend::Cpu, Rate::Quality(23), Speed::Balanced, false);
        assert!(x264.has_pair("-c:v", "libx264"));
        assert!(x264.has_pair("-crf", "23"));
        assert!(x264.has_pair("-preset", "medium"));
        assert!(x264.has_pair("-pix_fmt", "yuv420p"));

        let x265 = VideoCodec::H265.args(Backend::Cpu, Rate::Quality(23), Speed::Small, true);
        assert!(x265.has_pair("-crf", "27"));
        assert!(x265.has_pair("-preset", "slow"));
        assert!(x265.has_pair("-pix_fmt", "yuv420p10le"));

        let av1 = VideoCodec::Av1.args(Backend::Cpu, Rate::Quality(23), Speed::Balanced, false);
        assert!(av1.has_pair("-c:v", "libsvtav1"));
        assert!(av1.has_pair("-crf", "33"));
        assert!(av1.has_pair("-preset", "8"));

        let vp9 = VideoCodec::Vp9.args(Backend::Cpu, Rate::Quality(23), Speed::Fast, false);
        assert!(vp9.has_pair("-b:v", "0"));
        assert!(vp9.has_pair("-cpu-used", "5"));

        let nv = VideoCodec::H265.args(Backend::Nvidia, Rate::Quality(23), Speed::Balanced, false);
        assert!(nv.has_pair("-c:v", "hevc_nvenc"));
        assert!(nv.has_pair("-cq", "25"));
        assert!(nv.has_pair("-pix_fmt", "nv12"));

        let amd = VideoCodec::H264.args(Backend::Amd, Rate::Bitrate(2_000_000), Speed::Fast, false);
        assert!(amd.has_pair("-c:v", "h264_amf"));
        assert!(amd.has_pair("-b:v", "2000k"));
        assert!(amd.has_pair("-maxrate", "3000k"));
    }

    #[test]
    fn gpu_falls_back_to_cpu() {
        let caps = Caps {
            encoders: vec!["libx264".into(), "h264_nvenc".into()],
            hardware: vec!["h264_nvenc".into()],
        };
        assert_eq!(
            VideoCodec::H264.resolve(Backend::Nvidia, &caps),
            Backend::Nvidia
        );
        assert_eq!(
            VideoCodec::H265.resolve(Backend::Nvidia, &caps),
            Backend::Cpu
        );
        assert_eq!(VideoCodec::Vp9.resolve(Backend::Intel, &caps), Backend::Cpu);
        assert_eq!(VideoCodec::Vp9.encoder(Backend::Amd), None);
    }

    #[test]
    fn two_pass_x265_uses_params() {
        let mut a = VideoCodec::H265.args(
            Backend::Cpu,
            Rate::Bitrate(1_000_000),
            Speed::Balanced,
            false,
        );
        a.extend(VideoCodec::H265.pass_args(1, "log"));
        merge_x265_params(&mut a);
        let s = a.strings();
        assert_eq!(s.iter().filter(|x| *x == "-x265-params").count(), 1);
        assert!(a.has_pair("-x265-params", "log-level=error:pass=1:stats=log.log"));
        let b = VideoCodec::H264.pass_args(2, "p");
        assert!(b.has_pair("-pass", "2") && b.has_pair("-passlogfile", "p"));
    }

    #[test]
    fn containers() {
        assert!(Container::Mp4.accepts_video("hevc"));
        assert!(!Container::WebM.accepts_video("h264"));
        assert!(Container::Mkv.accepts_audio("dts"));
        assert!(!Container::Mp4.accepts_audio("vorbis"));
        assert!(Container::Mov.accepts_audio("pcm_s24le"));
        assert_eq!(Container::from_ext("M4V"), Some(Container::Mp4));
        assert!(
            Container::Mp4
                .mux_args(Some("libx265"))
                .has_pair("-tag:v", "hvc1")
        );
        assert!(!Container::Mp4.mux_args(Some("libx264")).has("-tag:v"));
        assert!(Container::Mkv.mux_args(None).0.is_empty());

        let subs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(
            Container::Mkv
                .subtitle_args(&subs(&["mov_text"]))
                .has_pair("-c:s", "srt")
        );
        assert!(
            Container::Mp4
                .subtitle_args(&subs(&["subrip"]))
                .has_pair("-c:s", "mov_text")
        );
        assert!(
            Container::Mp4
                .subtitle_args(&subs(&["hdmv_pgs_subtitle"]))
                .0
                .is_empty()
        );
        assert!(Container::Mp4.subtitle_args(&[]).0.is_empty());
    }

    #[test]
    fn geometry() {
        assert_eq!(fit_short_side(1920, 1080, 720), Some((1280, 720)));
        assert_eq!(fit_short_side(1080, 1920, 720), Some((720, 1280)));
        assert_eq!(fit_short_side(1280, 720, 720), None);
        assert_eq!(fit_short_side(1280, 720, 0), None);
        // 854.4 → 854 (even)
        assert_eq!(fit_short_side(1920, 1080, 480), Some((854, 480)));
        assert_eq!(fit_inside(1920, 1080, 1000, 1000), (1000, 562));
        assert_eq!(crop_to_ratio(1920, 1080, 9, 16), (608, 1080, 656, 0));
        assert_eq!(crop_to_ratio(1920, 1080, 1, 1), (1080, 1080, 420, 0));
        assert_eq!(crop_to_ratio(1080, 1920, 16, 9), (1080, 608, 0, 656));
        assert_eq!(even(3.0), 4);
        assert_eq!(even(0.2), 2);
    }

    #[test]
    fn auto_resolution_for_low_bitrates() {
        let v = VideoStream {
            width: 1920,
            height: 1080,
            fps: 30.0,
            ..Default::default()
        };
        // Plenty of bits: keep 1080p.
        assert_eq!(auto_short_side(&v, 8_000_000, VideoCodec::H264), None);
        // 1 Mbps H.264 at 30 fps: 720p gives 0.036 bpp, 480p gives 0.081 → 480p.
        assert_eq!(auto_short_side(&v, 1_000_000, VideoCodec::H264), Some(480));
        // AV1 needs fewer bits: 720p is fine at 1 Mbps (0.036 ≥ 0.025).
        assert_eq!(auto_short_side(&v, 1_000_000, VideoCodec::Av1), Some(720));
        // Hopelessly low: smallest standard size.
        assert_eq!(auto_short_side(&v, 10_000, VideoCodec::H264), Some(240));
    }
}
