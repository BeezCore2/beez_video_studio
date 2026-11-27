//! Media details from `ffprobe -print_format json -show_format -show_streams`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::ffmpeg;

#[derive(Clone, Debug, Default)]
pub struct VideoStream {
    pub codec: String,
    pub codec_long: String,
    pub profile: String,
    /// Stored (coded) size.
    pub width: u32,
    pub height: u32,
    /// Display rotation in degrees (0, 90, 180, 270), from phone videos' metadata.
    pub rotation: i32,
    pub fps: f64,
    pub pix_fmt: String,
    pub bit_rate: Option<u64>,
    pub frames: Option<u64>,
}

impl VideoStream {
    /// Size as shown on screen (width and height swap for 90°/270° rotation).
    pub fn display_size(&self) -> (u32, u32) {
        if self.rotation % 180 != 0 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    /// Whether the pixels are 10/12-bit (HDR or pro footage).
    pub fn high_bit_depth(&self) -> bool {
        self.pix_fmt.contains("10") || self.pix_fmt.contains("12")
    }
}

#[derive(Clone, Debug, Default)]
pub struct AudioStream {
    pub codec: String,
    pub channels: u32,
    pub channel_layout: String,
    pub sample_rate: u32,
    pub bit_rate: Option<u64>,
    pub language: String,
}

#[derive(Clone, Debug, Default)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub size: u64,
    /// Seconds.
    pub duration: f64,
    pub bit_rate: Option<u64>,
    /// e.g. "mov,mp4,m4a,3gp,3g2,mj2".
    pub format: String,
    pub format_long: String,
    pub video: Option<VideoStream>,
    pub audio: Vec<AudioStream>,
    /// Codec of each subtitle stream (e.g. "subrip", "mov_text", "hdmv_pgs_subtitle").
    pub subtitles: Vec<String>,
    pub tags: BTreeMap<String, String>,
}

impl MediaInfo {
    pub fn has_video(&self) -> bool {
        self.video.is_some()
    }

    pub fn has_audio(&self) -> bool {
        !self.audio.is_empty()
    }

    /// Short summary, e.g. "1920×1080 · 30 fps · H264 · 2:15".
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = &self.video {
            let (w, h) = v.display_size();
            parts.push(format!("{w}×{h}"));
            if v.fps > 0.0 {
                parts.push(format!("{} fps", fmt_fps(v.fps)));
            }
            parts.push(v.codec.to_uppercase());
        } else if let Some(a) = self.audio.first() {
            parts.push(format!("Audio · {}", a.codec.to_uppercase()));
        }
        if self.duration > 0.0 {
            parts.push(fmt_time(self.duration, false));
        }
        parts.join(" · ")
    }
}

pub fn fmt_fps(fps: f64) -> String {
    if (fps - fps.round()).abs() < 0.01 {
        format!("{}", fps.round() as u64)
    } else {
        format!("{fps:.2}")
    }
}

/// `h:mm:ss` / `m:ss`, with milliseconds when `millis` is set.
pub fn fmt_time(secs: f64, millis: bool) -> String {
    let total_ms = (secs.max(0.0) * 1000.0).round() as u64;
    let (h, m, s, ms) = (
        total_ms / 3_600_000,
        total_ms / 60_000 % 60,
        total_ms / 1000 % 60,
        total_ms % 1000,
    );
    let base = if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    };
    if millis {
        format!("{base}.{ms:03}")
    } else {
        base
    }
}

/// Parse a time typed by the user: `90`, `1:30`, `0:01:30.5`, `1:30.250`.
pub fn parse_time(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut secs = 0.0;
    for (i, part) in text.split(':').enumerate() {
        if i > 2 {
            return None;
        }
        let v: f64 = part.trim().parse().ok()?;
        if v < 0.0 || !v.is_finite() {
            return None;
        }
        secs = secs * 60.0 + v;
    }
    Some(secs)
}

pub fn fmt_bitrate(bps: u64) -> String {
    if bps >= 1_000_000 {
        format!("{:.1} Mbps", bps as f64 / 1e6)
    } else {
        format!("{} kbps", bps / 1000)
    }
}

// ---------------------------------------------------------------------------------------
// ffprobe JSON
// ---------------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct Probe {
    #[serde(default)]
    streams: Vec<Stream>,
    #[serde(default)]
    format: Format,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Stream {
    codec_type: String,
    codec_name: String,
    codec_long_name: String,
    profile: String,
    width: u32,
    height: u32,
    pix_fmt: String,
    avg_frame_rate: String,
    r_frame_rate: String,
    bit_rate: Option<String>,
    nb_frames: Option<String>,
    channels: u32,
    channel_layout: String,
    sample_rate: Option<String>,
    tags: BTreeMap<String, String>,
    side_data_list: Vec<serde_json::Value>,
    disposition: BTreeMap<String, i64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Format {
    format_name: String,
    format_long_name: String,
    duration: Option<String>,
    size: Option<String>,
    bit_rate: Option<String>,
    tags: BTreeMap<String, String>,
}

fn num<T: std::str::FromStr>(s: &Option<String>) -> Option<T> {
    s.as_deref().and_then(|v| v.trim().parse().ok())
}

/// "30000/1001" → 29.97.
fn rate(s: &str) -> f64 {
    match s.split_once('/') {
        Some((n, d)) => {
            let (n, d): (f64, f64) = (n.parse().unwrap_or(0.0), d.parse().unwrap_or(0.0));
            if d > 0.0 { n / d } else { 0.0 }
        }
        None => s.parse().unwrap_or(0.0),
    }
}

fn rotation(s: &Stream) -> i32 {
    let from_side = s
        .side_data_list
        .iter()
        .find_map(|d| d.get("rotation").and_then(|r| r.as_f64()));
    let from_tag = s.tags.get("rotate").and_then(|r| r.parse::<f64>().ok());
    let deg = from_side.or(from_tag).unwrap_or(0.0).round() as i32;
    deg.rem_euclid(360) / 90 * 90
}

/// Build [`MediaInfo`] from ffprobe's JSON output.
pub fn parse(json: &str, path: &Path, file_size: u64) -> Result<MediaInfo> {
    let p: Probe = serde_json::from_str(json).context("Could not read the media details")?;
    let mut info = MediaInfo {
        path: path.to_path_buf(),
        size: num(&p.format.size).unwrap_or(file_size),
        duration: num(&p.format.duration).unwrap_or(0.0),
        bit_rate: num(&p.format.bit_rate),
        format: p.format.format_name.clone(),
        format_long: p.format.format_long_name.clone(),
        tags: p.format.tags.clone(),
        ..Default::default()
    };
    for s in &p.streams {
        match s.codec_type.as_str() {
            // Cover art in music files is a "video" stream with attached_pic set: skip it.
            "video"
                if info.video.is_none()
                    && s.disposition.get("attached_pic").copied().unwrap_or(0) == 0 =>
            {
                let fps = match rate(&s.avg_frame_rate) {
                    f if f > 0.0 => f,
                    _ => rate(&s.r_frame_rate),
                };
                info.video = Some(VideoStream {
                    codec: s.codec_name.clone(),
                    codec_long: s.codec_long_name.clone(),
                    profile: s.profile.clone(),
                    width: s.width,
                    height: s.height,
                    rotation: rotation(s),
                    fps,
                    pix_fmt: s.pix_fmt.clone(),
                    bit_rate: num(&s.bit_rate),
                    frames: num(&s.nb_frames),
                });
            }
            "audio" => info.audio.push(AudioStream {
                codec: s.codec_name.clone(),
                channels: s.channels,
                channel_layout: s.channel_layout.clone(),
                sample_rate: num(&s.sample_rate).unwrap_or(0),
                bit_rate: num(&s.bit_rate),
                language: s.tags.get("language").cloned().unwrap_or_default(),
            }),
            "subtitle" => info.subtitles.push(s.codec_name.clone()),
            _ => {}
        }
    }
    Ok(info)
}

/// Read a media file's details with ffprobe.
pub fn probe(path: &Path) -> Result<MediaInfo> {
    let t = ffmpeg::require()?;
    let size = std::fs::metadata(path)
        .with_context(|| format!("Could not open {}", path.display()))?
        .len();
    let out = ffmpeg::command(&t.ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()
        .context("Could not start ffprobe")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.lines().last().unwrap_or("").trim();
        bail!("This file could not be read as video or audio. {err}");
    }
    let info = parse(&String::from_utf8_lossy(&out.stdout), path, size)?;
    if info.video.is_none() && info.audio.is_empty() {
        bail!("This file contains no video or audio.");
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: &str = r#"{
      "streams": [
        {"index":0,"codec_name":"h264","codec_long_name":"H.264 / AVC","profile":"High",
         "codec_type":"video","width":1920,"height":1080,"pix_fmt":"yuv420p",
         "avg_frame_rate":"30000/1001","r_frame_rate":"30000/1001","bit_rate":"15000000",
         "nb_frames":"1800","side_data_list":[{"side_data_type":"Display Matrix","rotation":-90}],
         "disposition":{"default":1,"attached_pic":0},"tags":{"language":"und"}},
        {"index":1,"codec_name":"aac","codec_type":"audio","channels":2,
         "channel_layout":"stereo","sample_rate":"48000","bit_rate":"192000",
         "tags":{"language":"eng"}},
        {"index":2,"codec_name":"mov_text","codec_type":"subtitle"}
      ],
      "format": {"format_name":"mov,mp4,m4a,3gp,3g2,mj2","format_long_name":"QuickTime / MOV",
        "duration":"60.060000","size":"113000000","bit_rate":"15051948",
        "tags":{"title":"Holiday"}}
    }"#;

    #[test]
    fn parses_phone_video() {
        let i = parse(PHONE, Path::new("a.mp4"), 0).unwrap();
        let v = i.video.as_ref().unwrap();
        assert_eq!((v.width, v.height), (1920, 1080));
        assert_eq!(v.rotation, 270);
        assert_eq!(v.display_size(), (1080, 1920));
        assert!((v.fps - 29.97).abs() < 0.01);
        assert_eq!(v.bit_rate, Some(15_000_000));
        assert_eq!(v.frames, Some(1800));
        assert_eq!(i.audio.len(), 1);
        assert_eq!(i.audio[0].sample_rate, 48000);
        assert_eq!(i.audio[0].language, "eng");
        assert_eq!(i.subtitles, ["mov_text"]);
        assert_eq!(i.size, 113_000_000);
        assert!((i.duration - 60.06).abs() < 1e-9);
        assert_eq!(i.tags.get("title").map(String::as_str), Some("Holiday"));
        assert_eq!(i.summary(), "1080×1920 · 29.97 fps · H264 · 1:00");
    }

    #[test]
    fn skips_cover_art() {
        let json = r#"{"streams":[
            {"codec_type":"audio","codec_name":"mp3","channels":2,"sample_rate":"44100"},
            {"codec_type":"video","codec_name":"mjpeg","width":500,"height":500,
             "disposition":{"attached_pic":1}}],
          "format":{"format_name":"mp3","duration":"200.5"}}"#;
        let i = parse(json, Path::new("a.mp3"), 1234).unwrap();
        assert!(i.video.is_none());
        assert_eq!(i.size, 1234);
        assert_eq!(i.summary(), "Audio · MP3 · 3:20");
    }

    #[test]
    fn times() {
        assert_eq!(fmt_time(75.5, false), "1:15");
        assert_eq!(fmt_time(75.5, true), "1:15.500");
        assert_eq!(fmt_time(3725.0, false), "1:02:05");
        assert_eq!(parse_time("90"), Some(90.0));
        assert_eq!(parse_time("1:30"), Some(90.0));
        assert_eq!(parse_time("0:01:30.5"), Some(90.5));
        assert_eq!(parse_time(" 2:00.250 "), Some(120.25));
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("1:2:3:4"), None);
        assert_eq!(parse_time("abc"), None);
        assert_eq!(parse_time("-5"), None);
    }

    #[test]
    fn rates() {
        assert_eq!(rate("25/1"), 25.0);
        assert_eq!(rate("0/0"), 0.0);
        assert_eq!(fmt_fps(25.0), "25");
        assert_eq!(fmt_bitrate(2_500_000), "2.5 Mbps");
        assert_eq!(fmt_bitrate(128_000), "128 kbps");
    }
}
