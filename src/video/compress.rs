//! Video compression.
//!
//! Two ways to choose the size:
//!   * **Quality** (constant rate factor): every scene gets the bits it needs for the chosen
//!     visual quality, which gives the best quality per byte. The final size depends on the
//!     content.
//!   * **Target size**: the bitrate is computed from the size budget:
//!     `video bps = size × 8 × 0.98 / duration − audio bps` (2 % is kept for container
//!     overhead). It is then encoded in **two passes**: pass 1 analyses the whole video,
//!     pass 2 spends the budget where it matters, so the file lands close to the target.
//!     With "Auto" resolution the picture is scaled down when the budget is too small for
//!     the source size (see [`codec::auto_short_side`]): fewer, cleaner pixels look better
//!     than a blocky full-size picture.
//!
//! Downscaling uses the Lanczos filter (sharp, little aliasing). Frame rates above a cap
//! are reduced with the `fps` filter, which drops frames evenly.

use std::path::Path;

use anyhow::{Result, bail};

use super::codec::{
    self, Args, Backend, Container, Rate, Speed, VideoCodec, fit_short_side, keep_ten_bit,
    merge_x265_params,
};
use super::ffmpeg::{self, Caps, Span};
use super::job::{Job, Output, Pass, temp_prefix};
use super::probe::{MediaInfo, fmt_bitrate};
use super::progress::Progress;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// Constant quality on the x264 CRF scale (18 best … 35 smallest).
    Quality(u8),
    /// Target file size in MB (1 MB = 1024 × 1024 bytes, as Windows shows sizes).
    TargetSize(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Original,
    /// Only for target size: pick the resolution from the bitrate.
    Auto,
    /// Limit the short side to this many pixels (720 = 720p).
    Limit(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioMode {
    /// Copy the audio untouched when the container allows it, otherwise re-encode.
    Keep,
    /// Re-encode at this bitrate (kbps per track).
    Encode(u32),
    Remove,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub mode: Mode,
    pub codec: VideoCodec,
    pub backend: Backend,
    pub speed: Speed,
    pub resolution: Resolution,
    /// Cap the frame rate (e.g. 30); `None` keeps it.
    pub max_fps: Option<u32>,
    pub audio: AudioMode,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: Mode::Quality(26),
            codec: VideoCodec::H264,
            backend: Backend::Cpu,
            speed: Speed::Balanced,
            resolution: Resolution::Original,
            max_fps: None,
            audio: AudioMode::Encode(128),
        }
    }
}

/// The quality presets shown as big buttons: (name, description, CRF).
pub const PRESETS: [(&str, &str, u8); 4] = [
    ("High quality", "Hardly any visible loss", 20),
    ("Balanced", "Much smaller, looks good", 26),
    ("Small", "Smaller, some detail lost", 30),
    ("Tiny", "Smallest, for sharing in chats", 34),
];

/// The container for a codec: WebM for VP9, MP4 for the rest.
pub fn container(codec: VideoCodec) -> Container {
    match codec {
        VideoCodec::Vp9 => Container::WebM,
        _ => Container::Mp4,
    }
}

pub fn input(path: &Path) -> Args {
    let mut a = Args::new();
    a.push("-i").push(path);
    a
}

/// Audio arguments for `container`, and the audio bitrate they produce (bits/s, all tracks).
pub fn audio_args(info: &MediaInfo, mode: AudioMode, container: Container) -> (Args, u64) {
    let mut a = Args::new();
    if info.audio.is_empty() || mode == AudioMode::Remove {
        a.push("-an");
        return (a, 0);
    }
    a.pair("-map", "0:a?");
    let tracks = info.audio.len() as u64;
    let encode = |kbps: u32, a: &mut Args| {
        let c = container.default_audio();
        a.extend(c.args(kbps));
        if let Some(f) = c.filter() {
            a.pair("-af", f);
        }
        tracks * kbps as u64 * 1000
    };
    let bps = match mode {
        AudioMode::Keep if info.audio.iter().all(|t| container.accepts_audio(&t.codec)) => {
            a.pair("-c:a", "copy");
            info.audio
                .iter()
                .map(|t| t.bit_rate.unwrap_or(128_000))
                .sum()
        }
        AudioMode::Keep => encode(160, &mut a),
        AudioMode::Encode(k) => encode(k, &mut a),
        AudioMode::Remove => unreachable!(),
    };
    (a, bps)
}

/// The video filter chain (`scale`, `fps`) for the options, given the bitrate in target mode.
fn video_filters(
    info: &MediaInfo,
    opts: &Options,
    codec: VideoCodec,
    video_bps: Option<u64>,
) -> Vec<String> {
    let Some(v) = &info.video else {
        return Vec::new();
    };
    let (w, h) = v.display_size();
    let limit = match (opts.resolution, video_bps) {
        (Resolution::Limit(l), _) => Some(l),
        (Resolution::Auto, Some(bps)) => {
            // Judge the bitrate at the frame rate that will actually be encoded.
            let mut out = v.clone();
            if let Some(max) = opts.max_fps {
                out.fps = out.fps.min(max as f64);
            }
            codec::auto_short_side(&out, bps, codec)
        }
        _ => None,
    };
    let mut f = Vec::new();
    if let Some((nw, nh)) = limit.and_then(|l| fit_short_side(w, h, l)) {
        f.push(format!("scale={nw}:{nh}:flags=lanczos"));
    }
    if let Some(max) = opts.max_fps
        && v.fps > max as f64 + 0.5
    {
        f.push(format!("fps={max}"));
    }
    f
}

/// The smallest target (MB) that leaves the video a usable bitrate.
pub fn min_target_mb(info: &MediaInfo, audio_bps: u64) -> f64 {
    const MIN_VIDEO_BPS: f64 = 60_000.0;
    (MIN_VIDEO_BPS + audio_bps as f64) * info.duration / 0.98 / 8.0 / 1024.0 / 1024.0
}

/// What a compression will do, for the UI and the job.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// Video bitrate for a target size.
    pub video_bps: Option<u64>,
    /// Output picture size, when it changes.
    pub size: Option<(u32, u32)>,
}

/// Work out the bitrate (target size) and output size. `audio_bps` from [`audio_args`].
pub fn plan(info: &MediaInfo, opts: &Options, audio_bps: u64) -> Result<Plan> {
    let video_bps = match opts.mode {
        Mode::Quality(_) => None,
        Mode::TargetSize(mb) => {
            if info.duration <= 0.0 {
                bail!(
                    "The video's length is unknown, so a target size cannot be used. Use a quality level instead."
                );
            }
            let budget = mb * 1024.0 * 1024.0 * 8.0 * 0.98 / info.duration;
            let video_bps = budget - audio_bps as f64;
            if video_bps < 60_000.0 {
                bail!(
                    "{mb:.1} MB is too small for this {:.0}-second video. Choose at least {:.1} MB, or remove the audio.",
                    info.duration,
                    min_target_mb(info, audio_bps).max(0.1)
                );
            }
            if (mb * 1024.0 * 1024.0) as u64 >= info.size {
                bail!(
                    "The video is already smaller than {mb:.1} MB. Choose a smaller target size."
                );
            }
            Some(video_bps as u64)
        }
    };
    let size = video_filters(info, opts, opts.codec, video_bps)
        .first()
        .and_then(|f| f.strip_prefix("scale="))
        .and_then(|f| {
            let mut it = f.split([':', ',']);
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        });
    Ok(Plan { video_bps, size })
}

/// Video arguments (`-map`, filters, codec settings) and the encoder name.
fn video_args(
    info: &MediaInfo,
    opts: &Options,
    caps: &Caps,
    rate: Rate,
    video_bps: Option<u64>,
) -> (Args, String) {
    let codec = opts.codec;
    let backend = codec.resolve(opts.backend, caps);
    let filters = video_filters(info, opts, codec, video_bps);
    let mut video = Args::new();
    video.pair("-map", "0:v:0");
    if !filters.is_empty() {
        video.pair("-vf", filters.join(","));
    }
    video.extend(codec.args(backend, rate, opts.speed, keep_ten_bit(info, codec)));
    (video, codec.encoder(backend).unwrap_or_default())
}

// ---------------------------------------------------------------------------------------
// Size estimate
// ---------------------------------------------------------------------------------------

/// Segments encoded to estimate the size, as (start, length) in seconds: the whole video
/// when it is short, otherwise 3-second samples spread evenly over it (4, or 6 for videos
/// over 10 minutes), so busy and calm scenes are both measured.
pub fn sample_points(duration: f64) -> Vec<(f64, f64)> {
    if duration <= 15.0 {
        return vec![(0.0, duration.max(0.1))];
    }
    let k = if duration > 600.0 { 6 } else { 4 };
    let len = 3.0;
    (0..k)
        .map(|i| {
            let centre = duration * (i as f64 + 0.5) / k as f64;
            let start = (centre - len / 2.0).max(0.0);
            (start, len.min(duration - start))
        })
        .collect()
}

/// Expected file size: the sampled video bytes per second over the whole duration, plus the
/// audio, plus 1 % container overhead.
pub fn extrapolate(duration: f64, sampled_bytes: u64, sampled_secs: f64, audio_bps: u64) -> u64 {
    let video = sampled_bytes as f64 / sampled_secs.max(0.01) * duration;
    let audio = audio_bps as f64 / 8.0 * duration;
    ((video + audio) * 1.01) as u64
}

/// Estimated output size in bytes. A target size is returned as is; a quality level is
/// measured by encoding [`sample_points`] with the exact settings.
pub fn estimate(info: &MediaInfo, opts: &Options, caps: &Caps, progress: &Progress) -> Result<u64> {
    if info.video.is_none() {
        bail!("No video");
    }
    let (_, audio_bps) = audio_args(info, opts.audio, container(opts.codec));
    plan(info, opts, audio_bps)?;
    let crf = match opts.mode {
        Mode::TargetSize(mb) => return Ok((mb * 1024.0 * 1024.0) as u64),
        Mode::Quality(crf) => crf,
    };
    if info.duration <= 0.0 {
        bail!("The video's length is unknown");
    }
    let (video, _) = video_args(info, opts, caps, Rate::Quality(crf), None);
    let points = sample_points(info.duration);
    let n = points.len();
    let (mut bytes, mut secs) = (0u64, 0.0f64);
    for (i, (start, len)) in points.into_iter().enumerate() {
        let out = temp_prefix("estimate").with_extension("mkv");
        let mut a = Args::new();
        a.pair("-ss", format!("{start:.3}"))
            .pair("-t", format!("{len:.3}"));
        a.push("-i").push(&info.path);
        a.extend(video.clone());
        merge_x265_params(&mut a);
        a.push("-an").push("-sn").pair("-f", "matroska").push(&out);
        let span = Span {
            progress,
            from: i as f32 / n as f32,
            to: (i + 1) as f32 / n as f32,
            duration: len,
            stage: "Estimating size",
        };
        let result = ffmpeg::run(&a.0, span);
        let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
        let _ = std::fs::remove_file(&out);
        result?;
        bytes += size;
        secs += len;
    }
    Ok(extrapolate(info.duration, bytes, secs, audio_bps))
}

/// Build the compression job.
pub fn build(info: &MediaInfo, out: &Path, opts: &Options, caps: &Caps) -> Result<Job> {
    if info.video.is_none() {
        bail!("This file has no video. Use the Audio tool for audio files.");
    }
    let codec = opts.codec;
    let backend = codec.resolve(opts.backend, caps);
    let container = container(codec);
    let (audio, audio_bps) = audio_args(info, opts.audio, container);

    let plan = plan(info, opts, audio_bps)?;
    let (rate, video_bps) = match plan.video_bps {
        Some(bps) => (Rate::Bitrate(bps), Some(bps)),
        None => match opts.mode {
            Mode::Quality(crf) => (Rate::Quality(crf), None),
            Mode::TargetSize(_) => unreachable!("target size always has a bitrate"),
        },
    };

    let (video, encoder) = video_args(info, opts, caps, rate, video_bps);
    let mut job = Job::new(Output::File(out.to_path_buf()), "Compressed");
    job.source_size = Some(info.size);

    let stage = match video_bps {
        Some(bps) => format!("Encoding {} at {}", codec.short(), fmt_bitrate(bps)),
        None => format!("Encoding {}", codec.short()),
    };

    let main = |extra: Args| {
        let mut a = input(&info.path);
        a.extend(video.clone()).extend(extra);
        merge_x265_params(&mut a);
        a.extend(audio.clone());
        a.extend(container.subtitle_args(&info.subtitles));
        a.extend(container.mux_args(Some(&encoder)));
        a
    };

    if video_bps.is_some() && codec.two_pass(backend) {
        let log = temp_prefix("2pass");
        let log_s = log.to_string_lossy().into_owned();
        let mut first = input(&info.path);
        first
            .extend(video.clone())
            .extend(codec.pass_args(1, &log_s));
        merge_x265_params(&mut first);
        first.push("-an");
        job = job
            .pass(
                Pass::new(first, "Analysing (pass 1 of 2)", info.duration)
                    .null()
                    .weight(0.35)
                    .in_dir(codec.pass_dir(&log_s)),
            )
            .pass(
                Pass::new(
                    main(codec.pass_args(2, &log_s)),
                    format!("{stage} (pass 2 of 2)"),
                    info.duration,
                )
                .weight(0.65)
                .in_dir(codec.pass_dir(&log_s)),
            );
        job.temp.push(format!("{log_s}*").into());
    } else {
        job = job.pass(Pass::new(main(Args::new()), stage, info.duration));
    }
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::probe::{AudioStream, VideoStream};

    pub fn sample() -> MediaInfo {
        MediaInfo {
            path: "in.mov".into(),
            size: 200 * 1024 * 1024,
            duration: 100.0,
            video: Some(VideoStream {
                codec: "h264".into(),
                width: 1920,
                height: 1080,
                fps: 60.0,
                pix_fmt: "yuv420p".into(),
                ..Default::default()
            }),
            audio: vec![AudioStream {
                codec: "aac".into(),
                channels: 2,
                bit_rate: Some(192_000),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn caps() -> Caps {
        Caps::default()
    }

    #[test]
    fn quality_mode_single_pass() {
        let info = sample();
        let job = build(&info, Path::new("out.mp4"), &Options::default(), &caps()).unwrap();
        assert_eq!(job.passes.len(), 1);
        let a = &job.passes[0].args;
        assert!(a.has_pair("-i", "in.mov"));
        assert!(a.has_pair("-crf", "26"));
        assert!(a.has_pair("-c:a", "aac") && a.has_pair("-b:a", "128k"));
        assert!(a.has_pair("-movflags", "+faststart"));
        assert!(!a.has("-vf"));
    }

    #[test]
    fn keeps_compatible_audio_untouched() {
        let info = sample();
        let opts = Options {
            audio: AudioMode::Keep,
            ..Default::default()
        };
        let job = build(&info, Path::new("o.mp4"), &opts, &caps()).unwrap();
        assert!(job.passes[0].args.has_pair("-c:a", "copy"));
        // WebM cannot hold AAC: re-encoded as Opus.
        let opts = Options {
            audio: AudioMode::Keep,
            codec: VideoCodec::Vp9,
            ..Default::default()
        };
        let job = build(&info, Path::new("o.webm"), &opts, &caps()).unwrap();
        assert!(job.passes[0].args.has_pair("-c:a", "libopus"));
    }

    #[test]
    fn target_size_two_pass_with_auto_resolution() {
        let info = sample();
        let opts = Options {
            mode: Mode::TargetSize(10.0),
            resolution: Resolution::Auto,
            max_fps: Some(30),
            ..Default::default()
        };
        let job = build(&info, Path::new("o.mp4"), &opts, &caps()).unwrap();
        assert_eq!(job.passes.len(), 2);
        // 10 MB over 100 s = 822 kbps after overhead, minus 128 kbps audio ≈ 694 kbps.
        let p1 = &job.passes[0];
        assert_eq!(p1.target, crate::video::job::Target::Null);
        assert!(p1.args.has_pair("-pass", "1") && p1.args.has("-an"));
        assert!(p1.args.has_pair("-b:v", "694k"));
        let p2 = &job.passes[1].args;
        assert!(p2.has_pair("-pass", "2"));
        // 694 kbps for a 1080p60 source → scaled down, and capped to 30 fps.
        let vf = p2
            .strings()
            .iter()
            .position(|a| a == "-vf")
            .map(|i| p2.strings()[i + 1].clone())
            .unwrap();
        assert_eq!(vf, "scale=854:480:flags=lanczos,fps=30");
        assert_eq!(job.temp.len(), 1);
    }

    #[test]
    fn target_size_validation() {
        let info = sample();
        let tiny = Options {
            mode: Mode::TargetSize(0.5),
            ..Default::default()
        };
        let err = build(&info, Path::new("o.mp4"), &tiny, &caps())
            .unwrap_err()
            .to_string();
        assert!(err.contains("too small"), "{err}");
        let huge = Options {
            mode: Mode::TargetSize(500.0),
            ..Default::default()
        };
        assert!(build(&info, Path::new("o.mp4"), &huge, &caps()).is_err());
    }

    #[test]
    fn x265_two_pass_uses_one_params_flag() {
        let info = sample();
        let opts = Options {
            mode: Mode::TargetSize(20.0),
            codec: VideoCodec::H265,
            ..Default::default()
        };
        let job = build(&info, Path::new("o.mp4"), &opts, &caps()).unwrap();
        for p in &job.passes {
            assert_eq!(
                p.args
                    .strings()
                    .iter()
                    .filter(|a| *a == "-x265-params")
                    .count(),
                1
            );
        }
        assert!(job.passes[1].args.has_pair("-tag:v", "hvc1"));
    }

    #[test]
    fn size_estimate_sampling() {
        assert_eq!(sample_points(10.0), [(0.0, 10.0)]);
        let p = sample_points(100.0);
        assert_eq!(p.len(), 4);
        assert_eq!(p[0], (11.0, 3.0));
        assert_eq!(p[3], (86.0, 3.0));
        assert_eq!(sample_points(1200.0).len(), 6);
        // 12 s sampled at 1 MB → 100 s video ≈ 8.33 MB, plus 128 kbps audio (1.6 MB), +1 %.
        let est = extrapolate(100.0, 1_000_000, 12.0, 128_000);
        assert_eq!(est, 10_032_666);
        // Target size needs no sampling.
        let info = sample();
        let o = Options {
            mode: Mode::TargetSize(10.0),
            ..Default::default()
        };
        let size = estimate(&info, &o, &caps(), &Progress::default()).unwrap();
        assert_eq!(size, 10 * 1024 * 1024);
    }

    #[test]
    fn audio_only_file_is_rejected() {
        let mut info = sample();
        info.video = None;
        assert!(build(&info, Path::new("o.mp4"), &Options::default(), &caps()).is_err());
    }
}
