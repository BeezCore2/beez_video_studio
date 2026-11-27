//! Audio: extract, remove, change volume, normalize loudness, replace or add music.
//!
//! **Loudness normalization** follows EBU R128 with FFmpeg's `loudnorm` filter in two
//! passes. Pass 1 measures the integrated loudness (LUFS), true peak, loudness range and
//! gating threshold of the whole file. Pass 2 feeds those measurements back in with
//! `linear=true`, so the whole track gets one constant gain change (no "pumping") and lands
//! on the target loudness; only when the true-peak limit would be exceeded does it fall
//! back to dynamic processing.

use std::path::Path;

use anyhow::{Context, Result, bail};

use super::codec::{Args, AudioCodec, Container};
use super::compress::input;
use super::edit::container_for;
use super::ffmpeg::{self, Span};
use super::job::{self, Job, Output, Pass};
use super::probe::MediaInfo;
use super::progress::Progress;

/// Output formats for extracted audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// The audio stream exactly as it is (no quality loss).
    Original,
    Mp3,
    Aac,
    Opus,
    Flac,
    Wav,
}

impl Format {
    pub const ALL: [Format; 6] = [
        Format::Original,
        Format::Mp3,
        Format::Aac,
        Format::Opus,
        Format::Flac,
        Format::Wav,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Format::Original => "Original (no quality loss)",
            Format::Mp3 => "MP3",
            Format::Aac => "AAC (.m4a)",
            Format::Opus => "Opus",
            Format::Flac => "FLAC (lossless)",
            Format::Wav => "WAV (uncompressed)",
        }
    }

    pub fn lossy(self) -> bool {
        matches!(self, Format::Mp3 | Format::Aac | Format::Opus)
    }

    /// File extension; for `Original` it depends on the source codec.
    pub fn ext(self, source_codec: &str) -> &'static str {
        match self {
            Format::Original => original_ext(source_codec),
            Format::Mp3 => "mp3",
            Format::Aac => "m4a",
            Format::Opus => "opus",
            Format::Flac => "flac",
            Format::Wav => "wav",
        }
    }

    fn codec(self) -> Option<AudioCodec> {
        match self {
            Format::Original => None,
            Format::Mp3 => Some(AudioCodec::Mp3),
            Format::Aac => Some(AudioCodec::Aac),
            Format::Opus => Some(AudioCodec::Opus),
            Format::Flac => Some(AudioCodec::Flac),
            Format::Wav => Some(AudioCodec::Wav),
        }
    }
}

/// The natural file type for an audio codec, so it can be copied without re-encoding.
pub fn original_ext(codec: &str) -> &'static str {
    match codec {
        "aac" | "alac" => "m4a",
        "mp3" => "mp3",
        "opus" => "opus",
        "vorbis" => "ogg",
        "flac" => "flac",
        "ac3" => "ac3",
        "eac3" => "eac3",
        c if c.starts_with("pcm_") => "wav",
        _ => "mka",
    }
}

/// The audio codec implied by an output file name, for audio-only outputs.
pub fn codec_for_ext(out: &Path) -> Option<AudioCodec> {
    let ext = out.extension()?.to_string_lossy().to_ascii_lowercase();
    Some(match ext.as_str() {
        "mp3" => AudioCodec::Mp3,
        "m4a" | "aac" => AudioCodec::Aac,
        "opus" => AudioCodec::Opus,
        "ogg" => AudioCodec::Vorbis,
        "flac" => AudioCodec::Flac,
        "wav" => AudioCodec::Wav,
        _ => return None,
    })
}

fn require_audio(info: &MediaInfo) -> Result<()> {
    if !info.has_audio() {
        bail!("This file has no audio.");
    }
    Ok(())
}

/// Extract audio track `track` (0-based) as `format`; `kbps` for lossy formats (0 = VBR
/// high quality for MP3).
pub fn build_extract(info: &MediaInfo, out: &Path, format: Format, kbps: u32, track: usize) -> Result<Job> {
    require_audio(info)?;
    let track = track.min(info.audio.len() - 1);
    let mut a = input(&info.path);
    a.push("-vn").push("-sn").pair("-map", format!("0:a:{track}"));
    match format.codec() {
        None => {
            a.pair("-c:a", "copy");
        }
        Some(c) => {
            a.extend(c.args(kbps));
            if let Some(f) = c.filter() {
                a.pair("-af", f);
            }
        }
    }
    let job = Job::new(Output::File(out.to_path_buf()), "Saved");
    let stage = if format == Format::Original { "Copying audio (no quality loss)" } else { "Extracting audio" };
    Ok(job.pass(Pass::new(a, stage, info.duration)))
}

/// Copy everything except the audio (lossless).
pub fn build_remove(info: &MediaInfo, out: &Path) -> Result<Job> {
    if !info.has_video() {
        bail!("This file has no video, so removing the audio would leave nothing.");
    }
    let c = container_for(out);
    let mut a = input(&info.path);
    a.pair("-map", "0:v:0").pair("-c:v", "copy").push("-an");
    let codec = info.video.as_ref().map(|v| v.codec.clone());
    a.extend(c.mux_args(codec.as_deref()));
    let job = Job::new(Output::File(out.to_path_buf()), "Saved");
    Ok(job.pass(Pass::new(a, "Removing audio (no quality loss)", info.duration)))
}

/// Video copied (if any), audio re-encoded through `filter` in the format `out` implies.
fn filtered_audio(info: &MediaInfo, out: &Path, filter: &str) -> Args {
    let mut a = input(&info.path);
    let (codec, container) = match codec_for_ext(out) {
        Some(c) if !info.has_video() => (c, None),
        _ => {
            let c = container_for(out);
            (c.default_audio(), Some(c))
        }
    };
    if let Some(c) = container {
        a.pair("-map", "0:v:0?").pair("-c:v", "copy");
        let vcodec = info.video.as_ref().map(|v| v.codec.clone());
        a.extend(c.mux_args(vcodec.as_deref()));
    } else {
        a.push("-vn");
    }
    let mut chain = filter.to_string();
    if let Some(f) = codec.filter() {
        chain.push(',');
        chain.push_str(f);
    }
    a.pair("-map", "0:a:0").pair("-af", chain);
    a.extend(codec.args(if codec == AudioCodec::Mp3 { 0 } else { 192 }));
    a
}

/// Change the volume by `db` decibels (+6 dB ≈ twice as loud, −6 dB ≈ half).
pub fn build_volume(info: &MediaInfo, out: &Path, db: f64) -> Result<Job> {
    require_audio(info)?;
    if db.abs() < 0.05 {
        bail!("Choose a volume change other than 0 dB.");
    }
    // A limiter stops loud boosts from clipping.
    let filter = if db > 0.0 {
        format!("volume={db:.1}dB,alimiter=limit=0.97:level=false")
    } else {
        format!("volume={db:.1}dB")
    };
    let a = filtered_audio(info, out, &filter);
    let job = Job::new(Output::File(out.to_path_buf()), "Saved");
    Ok(job.pass(Pass::new(a, "Changing volume", info.duration)))
}

/// Loudness targets: (name, integrated LUFS, true peak dBTP).
pub const LOUDNESS: [(&str, f64, f64); 3] = [
    ("Streaming & social media (−14 LUFS)", -14.0, -1.0),
    ("Podcasts & voice (−16 LUFS)", -16.0, -1.5),
    ("Broadcast, EBU R128 (−23 LUFS)", -23.0, -1.0),
];

/// Measurements from loudnorm's first pass.
#[derive(Clone, Debug, PartialEq)]
pub struct Loudness {
    pub input_i: f64,
    pub input_tp: f64,
    pub input_lra: f64,
    pub input_thresh: f64,
    pub offset: f64,
}

/// Read the JSON block loudnorm prints at the end of its log.
pub fn parse_loudness(log: &str) -> Result<Loudness> {
    let start = log.rfind('{').context("The loudness analysis gave no result")?;
    let end = log[start..].find('}').context("The loudness analysis gave no result")? + start;
    let v: serde_json::Value =
        serde_json::from_str(&log[start..=end]).context("Could not read the loudness analysis")?;
    let num = |k: &str| -> Result<f64> {
        let s = v.get(k).and_then(|x| x.as_str()).with_context(|| format!("Missing {k}"))?;
        // Silence measures as "-inf".
        if s.contains("inf") {
            bail!("The audio is silent, so it cannot be normalized.");
        }
        s.trim().parse().with_context(|| format!("Bad value for {k}: {s}"))
    };
    Ok(Loudness {
        input_i: num("input_i")?,
        input_tp: num("input_tp")?,
        input_lra: num("input_lra")?,
        input_thresh: num("input_thresh")?,
        offset: num("target_offset")?,
    })
}

fn loudnorm(target: f64, tp: f64) -> String {
    format!("loudnorm=I={target}:TP={tp}:LRA=11")
}

/// The second-pass filter, using the first pass's measurements.
pub fn loudnorm_pass2(target: f64, tp: f64, m: &Loudness, sample_rate: u32) -> String {
    format!(
        "{}:measured_I={}:measured_TP={}:measured_LRA={}:measured_thresh={}:offset={}:linear=true:print_format=none,aresample={}",
        loudnorm(target, tp),
        m.input_i,
        m.input_tp,
        m.input_lra,
        m.input_thresh,
        m.offset,
        // loudnorm works at 192 kHz internally: return to the source rate.
        sample_rate.clamp(8000, 96000)
    )
}

/// Two-pass loudness normalization (runs the analysis, then the job).
pub fn normalize(info: &MediaInfo, out: &Path, target: f64, tp: f64, progress: &Progress) -> Result<String> {
    require_audio(info)?;
    let mut a = input(&info.path);
    a.pair("-map", "0:a:0")
        .pair("-af", format!("{}:print_format=json", loudnorm(target, tp)))
        .push("-vn");
    a.pair("-f", "null").push(super::codec::null_output());
    let span = Span {
        progress,
        from: 0.0,
        to: 0.35,
        duration: info.duration,
        stage: "Measuring loudness (pass 1 of 2)",
    };
    let log = ffmpeg::run_capture(&a.0, span, "info")?;
    let m = parse_loudness(&log)?;
    let sr = info.audio[0].sample_rate;
    let filter = loudnorm_pass2(target, tp, &m, if sr == 0 { 48000 } else { sr });
    let args = filtered_audio(info, out, &filter);
    let mut job = Job::new(Output::File(out.to_path_buf()), "Normalized");
    job = job.pass(Pass::new(
        args,
        format!("Adjusting {:.1} → {target:.0} LUFS (pass 2 of 2)", m.input_i),
        info.duration,
    ));
    job::execute_in(job, progress, 0.35, 1.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MusicMode {
    /// Use the new audio instead of the original.
    Replace,
    /// Mix the new audio under the original at `music_volume` (0–1); the music loops if it
    /// is shorter than the video.
    Mix { music_volume: f32 },
}

pub fn build_music(info: &MediaInfo, music: &MediaInfo, out: &Path, mode: MusicMode) -> Result<Job> {
    if !info.has_video() {
        bail!("Choose a video to add the audio to.");
    }
    if !music.has_audio() {
        bail!("{} has no audio.", music.path.display());
    }
    let c = container_for(out);
    let codec = c.default_audio();
    let mut a = Args::new();
    a.push("-i").push(&info.path);
    let mix = matches!(mode, MusicMode::Mix { .. }) && info.has_audio();
    if mix {
        a.pair("-stream_loop", "-1");
    }
    a.push("-i").push(&music.path);
    a.pair("-map", "0:v:0").pair("-c:v", "copy");
    let fix = codec.filter().map(|f| format!(",{f}")).unwrap_or_default();
    match mode {
        MusicMode::Mix { music_volume } if mix => {
            a.pair(
                "-filter_complex",
                format!(
                    "[1:a:0]volume={:.2}[m];[0:a:0][m]amix=inputs=2:duration=first:dropout_transition=0:normalize=0{fix}[a]",
                    music_volume.clamp(0.0, 2.0)
                ),
            );
            a.pair("-map", "[a]");
        }
        _ => {
            a.pair("-map", "1:a:0");
            if let Some(f) = codec.filter() {
                a.pair("-af", f);
            }
            // Stop at the end of the video (or of the music, if that is shorter).
            a.push("-shortest");
        }
    }
    a.extend(codec.args(192));
    let vcodec = info.video.as_ref().map(|v| v.codec.clone());
    a.extend(c.mux_args(vcodec.as_deref()));
    let job = Job::new(Output::File(out.to_path_buf()), "Saved");
    let stage = if mix { "Mixing audio" } else { "Replacing audio" };
    Ok(job.pass(Pass::new(a, stage, info.duration)))
}

/// Used by the UI for the "Save as" filter of audio outputs.
pub fn is_container(out: &Path) -> bool {
    out.extension()
        .and_then(|e| Container::from_ext(&e.to_string_lossy()))
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::probe::{AudioStream, VideoStream};

    fn info(video: bool) -> MediaInfo {
        MediaInfo {
            path: "in.mp4".into(),
            duration: 60.0,
            video: video.then(|| VideoStream { codec: "h264".into(), width: 640, height: 360, ..Default::default() }),
            audio: vec![
                AudioStream { codec: "aac".into(), sample_rate: 44100, channels: 2, ..Default::default() },
                AudioStream { codec: "ac3".into(), sample_rate: 48000, channels: 6, ..Default::default() },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn extract() {
        let i = info(true);
        let job = build_extract(&i, Path::new("o.m4a"), Format::Original, 0, 0).unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-map", "0:a:0") && a.has_pair("-c:a", "copy") && a.has("-vn"));
        let job = build_extract(&i, Path::new("o.mp3"), Format::Mp3, 0, 5).unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-map", "0:a:1"), "track index is clamped");
        assert!(a.has_pair("-c:a", "libmp3lame") && a.has_pair("-q:a", "2"));
        let job = build_extract(&i, Path::new("o.opus"), Format::Opus, 96, 1).unwrap();
        assert!(job.passes[0].args.has_pair("-af", "aformat=channel_layouts=7.1|5.1|stereo|mono"));
        assert_eq!(original_ext("pcm_s24le"), "wav");
        assert_eq!(original_ext("dts"), "mka");
        assert_eq!(Format::Original.ext("aac"), "m4a");
    }

    #[test]
    fn remove_and_volume() {
        let i = info(true);
        let a = &build_remove(&i, Path::new("o.mp4")).unwrap().passes[0].args;
        assert!(a.has("-an") && a.has_pair("-c:v", "copy"));
        assert!(build_remove(&info(false), Path::new("o.mp4")).is_err());

        let a = &build_volume(&i, Path::new("o.mp4"), 6.0).unwrap().passes[0].args;
        assert!(a.has_pair("-af", "volume=6.0dB,alimiter=limit=0.97:level=false"));
        assert!(a.has_pair("-c:v", "copy") && a.has_pair("-c:a", "aac"));
        // Audio-only: codec from the extension.
        let a = &build_volume(&info(false), Path::new("o.flac"), -3.0).unwrap().passes[0].args;
        assert!(a.has_pair("-c:a", "flac") && a.has("-vn"));
        assert!(build_volume(&i, Path::new("o.mp4"), 0.0).is_err());
    }

    #[test]
    fn loudness_two_pass() {
        let log = r#"[Parsed_loudnorm_0 @ 000001]
{
	"input_i" : "-27.61",
	"input_tp" : "-4.47",
	"input_lra" : "18.06",
	"input_thresh" : "-39.20",
	"output_i" : "-16.58",
	"output_tp" : "-1.50",
	"output_lra" : "14.78",
	"output_thresh" : "-27.71",
	"normalization_type" : "dynamic",
	"target_offset" : "0.58"
}
"#;
        let m = parse_loudness(log).unwrap();
        assert_eq!(m.input_i, -27.61);
        assert_eq!(m.offset, 0.58);
        let f = loudnorm_pass2(-16.0, -1.5, &m, 44100);
        assert_eq!(
            f,
            "loudnorm=I=-16:TP=-1.5:LRA=11:measured_I=-27.61:measured_TP=-4.47:measured_LRA=18.06:measured_thresh=-39.2:offset=0.58:linear=true:print_format=none,aresample=44100"
        );
        let silent = log.replace("\"-27.61\"", "\"-inf\"");
        assert!(parse_loudness(&silent).unwrap_err().to_string().contains("silent"));
        assert!(parse_loudness("no json here").is_err());
    }

    #[test]
    fn music() {
        let v = info(true);
        let m = MediaInfo { path: "song.mp3".into(), ..info(false) };
        let a = &build_music(&v, &m, Path::new("o.mp4"), MusicMode::Replace).unwrap().passes[0].args;
        assert!(a.has_pair("-map", "1:a:0") && a.has("-shortest"));
        let a = &build_music(&v, &m, Path::new("o.mp4"), MusicMode::Mix { music_volume: 0.3 }).unwrap().passes[0].args;
        assert!(a.has_pair("-stream_loop", "-1"));
        let s = a.strings();
        let fc = &s[s.iter().position(|x| x == "-filter_complex").unwrap() + 1];
        assert!(fc.starts_with("[1:a:0]volume=0.30[m];[0:a:0][m]amix=inputs=2:duration=first"));
    }
}
