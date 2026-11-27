//! Format conversion and animated GIF / WebP.
//!
//! **Smart remux.** Changing the container rarely needs re-encoding: when the target
//! container can hold the source's video/audio codecs (H.264 in MKV → MP4, …), those
//! streams are *copied*: no quality loss, and it takes seconds instead of minutes. Only
//! streams the container cannot hold are re-encoded.
//!
//! **GIF.** A GIF holds at most 256 colours, so quality depends on choosing them well. FFmpeg
//! makes a palette from the clip itself in two stages (`palettegen` → `paletteuse`):
//! `stats_mode=diff` weights the colours of the *moving* parts, which is where the eye
//! looks, and Sierra-2-4A error-diffusion dithering hides banding in gradients.
//! `diff_mode=rectangle` re-encodes only the changed rectangle of each frame, which keeps
//! files small.

use std::path::Path;

use anyhow::{Result, bail};

use super::codec::{Args, Backend, Container, Rate, Speed, VideoCodec, keep_ten_bit};
use super::compress::{AudioMode, audio_args, input};
use super::ffmpeg::Caps;
use super::job::{Job, Output, Pass};
use super::probe::MediaInfo;

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub container: Container,
    /// `None` = automatic: copy the video when the container accepts it.
    pub video: Option<VideoCodec>,
    /// Quality when re-encoding (x264 CRF scale).
    pub crf: u8,
    pub backend: Backend,
    pub speed: Speed,
    pub audio: AudioMode,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            container: Container::Mp4,
            video: None,
            crf: 20,
            backend: Backend::Cpu,
            speed: Speed::Balanced,
            audio: AudioMode::Keep,
        }
    }
}

/// What will happen to the video stream, for the UI: `Some(true)` copied, `Some(false)`
/// re-encoded, `None` no video.
pub fn copies_video(info: &MediaInfo, opts: &Options) -> Option<bool> {
    let v = info.video.as_ref()?;
    Some(opts.video.is_none() && opts.container.accepts_video(&v.codec))
}

pub fn build(info: &MediaInfo, out: &Path, opts: &Options, caps: &Caps) -> Result<Job> {
    let c = opts.container;
    let mut a = input(&info.path);
    let mut encoder_name = None;
    let mut all_copied = true;

    if let Some(v) = &info.video {
        a.pair("-map", "0:v:0");
        if copies_video(info, opts) == Some(true) {
            a.pair("-c:v", "copy");
            encoder_name = Some(v.codec.clone());
        } else {
            let codec = opts
                .video
                .filter(|vc| c.video_choices().contains(vc))
                .unwrap_or_else(|| c.default_video());
            let backend = codec.resolve(opts.backend, caps);
            a.extend(codec.args(
                backend,
                Rate::Quality(opts.crf),
                opts.speed,
                keep_ten_bit(info, codec),
            ));
            encoder_name = codec.encoder(backend);
            all_copied = false;
        }
    }
    let (audio, _) = audio_args(info, opts.audio, c);
    if !audio.has_pair("-c:a", "copy") && !audio.has("-an") {
        all_copied = false;
    }
    a.extend(audio);
    a.extend(c.subtitle_args(&info.subtitles));
    a.extend(c.mux_args(encoder_name.as_deref()));

    let stage = if all_copied {
        "Copying streams (no quality loss)".to_string()
    } else {
        format!("Converting to {}", c.ext().to_uppercase())
    };
    let mut job = Job::new(Output::File(out.to_path_buf()), "Converted");
    job.source_size = Some(info.size);
    Ok(job.pass(Pass::new(a, stage, info.duration)))
}

// ---------------------------------------------------------------------------------------
// Animated GIF / WebP
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimFormat {
    Gif,
    WebP,
}

impl AnimFormat {
    pub fn ext(self) -> &'static str {
        match self {
            AnimFormat::Gif => "gif",
            AnimFormat::WebP => "webp",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GifOptions {
    pub format: AnimFormat,
    /// Seconds.
    pub start: f64,
    /// Seconds; 0 = to the end.
    pub length: f64,
    /// Output width in pixels (never upscaled).
    pub width: u32,
    pub fps: u32,
    /// GIF palette size (2–256): fewer colours, smaller file.
    pub colors: u32,
    /// Error-diffusion dithering (smoother gradients, slightly bigger file).
    pub dither: bool,
    /// WebP quality 0–100.
    pub quality: u8,
}

impl Default for GifOptions {
    fn default() -> Self {
        Self {
            format: AnimFormat::Gif,
            start: 0.0,
            length: 5.0,
            width: 480,
            fps: 15,
            colors: 256,
            dither: true,
            quality: 75,
        }
    }
}

pub fn build_gif(info: &MediaInfo, out: &Path, o: &GifOptions, caps: &Caps) -> Result<Job> {
    let Some(v) = &info.video else {
        bail!("This file has no video.");
    };
    if info.duration > 0.0 && o.start >= info.duration {
        bail!("The start time is past the end of the video.");
    }
    let mut length = if o.length > 0.0 {
        o.length
    } else {
        (info.duration - o.start).max(0.0)
    };
    if info.duration > 0.0 {
        length = length.min(info.duration - o.start);
    }
    let (w, _) = v.display_size();
    let width = if o.width == 0 {
        w
    } else {
        o.width.min(w.max(2))
    };
    let base = format!("fps={},scale={width}:-2:flags=lanczos", o.fps.clamp(1, 50));

    let mut a = Args::new();
    if o.start > 0.0 {
        a.pair("-ss", format!("{:.3}", o.start));
    }
    if length > 0.0 {
        a.pair("-t", format!("{length:.3}"));
    }
    a.push("-i").push(&info.path);
    match o.format {
        AnimFormat::Gif => {
            let dither = if o.dither { "sierra2_4a" } else { "none" };
            a.pair(
                "-filter_complex",
                format!(
                    "[0:v:0]{base},split[a][b];[a]palettegen=max_colors={}:stats_mode=diff[p];\
                     [b][p]paletteuse=dither={dither}:diff_mode=rectangle",
                    o.colors.clamp(2, 256)
                ),
            );
            a.pair("-loop", "0");
        }
        AnimFormat::WebP => {
            let enc = if caps.has("libwebp_anim") || !caps.has("libwebp") {
                "libwebp_anim"
            } else {
                "libwebp"
            };
            a.pair("-map", "0:v:0").pair("-vf", base);
            a.pair("-c:v", enc)
                .pair("-lossless", "0")
                .pair("-q:v", o.quality.min(100).to_string())
                .pair("-compression_level", "6")
                .pair("-loop", "0");
        }
    }
    a.push("-an");
    let job = Job::new(Output::File(out.to_path_buf()), "Created");
    Ok(job.pass(Pass::new(
        a,
        format!("Making {}", o.format.ext().to_uppercase()),
        length,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::probe::{AudioStream, VideoStream};

    fn info(vcodec: &str, acodec: &str) -> MediaInfo {
        MediaInfo {
            path: "in.mkv".into(),
            size: 1000,
            duration: 30.0,
            video: Some(VideoStream {
                codec: vcodec.into(),
                width: 1920,
                height: 1080,
                fps: 30.0,
                pix_fmt: "yuv420p".into(),
                ..Default::default()
            }),
            audio: vec![AudioStream {
                codec: acodec.into(),
                ..Default::default()
            }],
            subtitles: vec!["subrip".into()],
            ..Default::default()
        }
    }

    #[test]
    fn remux_when_possible() {
        let i = info("h264", "aac");
        let job = build(
            &i,
            Path::new("o.mp4"),
            &Options::default(),
            &Caps::default(),
        )
        .unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-c:v", "copy"));
        assert!(a.has_pair("-c:a", "copy"));
        assert!(a.has_pair("-c:s", "mov_text"));
        assert!(job.passes[0].stage.contains("no quality loss"));
    }

    #[test]
    fn reencodes_what_the_container_cannot_hold() {
        let i = info("h264", "aac");
        let o = Options {
            container: Container::WebM,
            ..Default::default()
        };
        let job = build(&i, Path::new("o.webm"), &o, &Caps::default()).unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-c:v", "libvpx-vp9"));
        assert!(a.has_pair("-c:a", "libopus"));
        assert!(a.has_pair("-c:s", "webvtt"));
        assert!(!a.has("-movflags"));
    }

    #[test]
    fn forced_codec_reencodes() {
        let i = info("h264", "aac");
        let o = Options {
            video: Some(VideoCodec::H265),
            ..Default::default()
        };
        let job = build(&i, Path::new("o.mp4"), &o, &Caps::default()).unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-c:v", "libx265"));
        assert!(a.has_pair("-tag:v", "hvc1"));
        // A codec the container can't take falls back to its default.
        let o = Options {
            container: Container::Avi,
            video: Some(VideoCodec::Av1),
            ..Default::default()
        };
        let job = build(
            &info("vp9", "opus"),
            Path::new("o.avi"),
            &o,
            &Caps::default(),
        )
        .unwrap();
        assert!(job.passes[0].args.has_pair("-c:v", "libx264"));
        assert!(job.passes[0].args.has_pair("-c:a", "libmp3lame"));
    }

    #[test]
    fn gif_uses_two_stage_palette() {
        let i = info("h264", "aac");
        let o = GifOptions {
            start: 2.0,
            length: 4.0,
            width: 4000,
            ..Default::default()
        };
        let job = build_gif(&i, Path::new("o.gif"), &o, &Caps::default()).unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-ss", "2.000") && a.has_pair("-t", "4.000"));
        let fc = a.strings()[a
            .strings()
            .iter()
            .position(|x| x == "-filter_complex")
            .unwrap()
            + 1]
        .clone();
        assert!(fc.contains("scale=1920:-2"), "never upscales: {fc}");
        assert!(fc.contains("palettegen=max_colors=256:stats_mode=diff"));
        assert!(fc.contains("dither=sierra2_4a"));
        assert_eq!(job.passes[0].duration, 4.0);

        let o = GifOptions {
            format: AnimFormat::WebP,
            start: 28.0,
            length: 10.0,
            ..Default::default()
        };
        let job = build_gif(&i, Path::new("o.webp"), &o, &Caps::default()).unwrap();
        assert!(job.passes[0].args.has_pair("-c:v", "libwebp_anim"));
        assert_eq!(job.passes[0].duration, 2.0);

        let o = GifOptions {
            start: 31.0,
            ..Default::default()
        };
        assert!(build_gif(&i, Path::new("o.gif"), &o, &Caps::default()).is_err());
    }
}
