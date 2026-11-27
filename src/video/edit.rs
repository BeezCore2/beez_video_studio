//! Editing: trim, join, resize/rotate/crop and speed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::codec::{
    Args, Backend, Container, Rate, Speed, VideoCodec, crop_to_ratio, even,
    fit_inside, fit_short_side, keep_ten_bit,
};
use super::compress::{AudioMode, audio_args, input};
use super::ffmpeg::Caps;
use super::job::{Job, Output, Pass, temp_prefix};
use super::probe::{MediaInfo, fmt_time};

/// Quality used when an edit has to re-encode: visually lossless.
pub const EDIT_CRF: u8 = 18;

/// The container implied by the output file's extension (MP4 when unknown).
pub fn container_for(out: &Path) -> Container {
    out.extension()
        .and_then(|e| Container::from_ext(&e.to_string_lossy()))
        .unwrap_or(Container::Mp4)
}

/// The codec to re-encode with: the source's codec when we have an encoder for it and the
/// container takes it (so an H.265 clip stays H.265), otherwise the container's default.
fn reencode_codec(info: &MediaInfo, c: Container) -> VideoCodec {
    let src = info.video.as_ref().map(|v| v.codec.as_str()).unwrap_or("");
    VideoCodec::ALL
        .into_iter()
        .find(|vc| vc.probe_name() == src && c.video_choices().contains(vc))
        .unwrap_or_else(|| c.default_video())
}

/// Video encode settings for edits.
fn video_encode(info: &MediaInfo, c: Container, caps: &Caps, backend: Backend) -> (Args, Option<String>) {
    let codec = reencode_codec(info, c);
    let backend = codec.resolve(backend, caps);
    let args = codec.args(backend, Rate::Quality(EDIT_CRF), Speed::Balanced, keep_ten_bit(info, codec));
    (args, codec.encoder(backend))
}

fn audio_encode(c: Container) -> Args {
    let codec = c.default_audio();
    let mut a = codec.args(192);
    if let Some(f) = codec.filter() {
        a.pair("-af", f);
    }
    a
}

// ---------------------------------------------------------------------------------------
// Trim
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimMode {
    /// Stream copy: instant and lossless, but cuts land on keyframes (the clip can start
    /// up to a few seconds early).
    Fast,
    /// Re-encode: frame-accurate cuts.
    Precise,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrimOptions {
    pub start: f64,
    pub end: f64,
    pub mode: TrimMode,
    /// Cut the selection *out* and join what is left (always precise).
    pub remove: bool,
}

pub fn build_trim(info: &MediaInfo, out: &Path, o: &TrimOptions, caps: &Caps) -> Result<Job> {
    let dur = info.duration;
    let (start, mut end) = (o.start.max(0.0), o.end);
    if dur > 0.0 {
        end = end.min(dur);
    }
    if end <= start {
        bail!("The end time must be after the start time.");
    }
    let c = container_for(out);
    if o.remove {
        let at_start = start <= 0.01;
        let at_end = dur > 0.0 && end >= dur - 0.01;
        if at_start && at_end {
            bail!("That would remove the whole video.");
        }
        if at_start || at_end {
            // Removing an edge is the same as keeping the rest.
            let keep = if at_start {
                TrimOptions { start: end, end: dur, mode: TrimMode::Precise, remove: false }
            } else {
                TrimOptions { start: 0.0, end: start, mode: TrimMode::Precise, remove: false }
            };
            return build_trim(info, out, &keep, caps);
        }
        return build_cut_out(info, out, start, end, c, caps);
    }

    let length = end - start;
    let mut a = Args::new();
    a.pair("-ss", format!("{start:.3}"));
    a.push("-i").push(&info.path);
    a.pair("-t", format!("{length:.3}"));
    let mut encoder = None;
    match o.mode {
        TrimMode::Fast => {
            a.pair("-map", "0:v?").pair("-map", "0:a?").pair("-c", "copy");
            // Start timestamps at zero so players don't show a black gap.
            a.pair("-avoid_negative_ts", "make_zero");
            encoder = info.video.as_ref().map(|v| v.codec.clone());
        }
        TrimMode::Precise => {
            if info.video.is_some() {
                let (v, e) = video_encode(info, c, caps, Backend::Cpu);
                a.pair("-map", "0:v:0").extend(v);
                encoder = e;
            }
            if info.has_audio() {
                a.pair("-map", "0:a?").extend(audio_encode(c));
            }
        }
    }
    a.extend(c.mux_args(encoder.as_deref()));
    let job = Job::new(Output::File(out.to_path_buf()), "Saved");
    let stage = format!("Trimming {} – {}", fmt_time(start, false), fmt_time(end, false));
    Ok(job.pass(Pass::new(a, stage, length)))
}

/// Keep `0..start` and `end..`, joined (re-encoded with trim + concat filters).
fn build_cut_out(info: &MediaInfo, out: &Path, start: f64, end: f64, c: Container, caps: &Caps) -> Result<Job> {
    let has_v = info.has_video();
    let has_a = info.has_audio();
    let mut g = Vec::new();
    let mut pairs = String::new();
    for (i, range) in [format!("end={start:.3}"), format!("start={end:.3}")].iter().enumerate() {
        if has_v {
            g.push(format!("[0:v:0]trim={range},setpts=PTS-STARTPTS[v{i}]"));
            pairs.push_str(&format!("[v{i}]"));
        }
        if has_a {
            g.push(format!("[0:a:0]atrim={range},asetpts=PTS-STARTPTS[a{i}]"));
            pairs.push_str(&format!("[a{i}]"));
        }
    }
    let mut outs = String::new();
    if has_v {
        outs.push_str("[v]");
    }
    if has_a {
        outs.push_str("[a]");
    }
    g.push(format!("{pairs}concat=n=2:v={}:a={}{outs}", has_v as u8, has_a as u8));

    let mut a = input(&info.path);
    a.pair("-filter_complex", g.join(";"));
    let mut encoder = None;
    if has_v {
        let (v, e) = video_encode(info, c, caps, Backend::Cpu);
        a.pair("-map", "[v]").extend(v);
        encoder = e;
    }
    if has_a {
        a.pair("-map", "[a]").extend(audio_encode(c));
    }
    a.extend(c.mux_args(encoder.as_deref()));
    let length = (info.duration - (end - start)).max(0.0);
    let job = Job::new(Output::File(out.to_path_buf()), "Saved");
    let stage = format!("Removing {} – {}", fmt_time(start, false), fmt_time(end, false));
    Ok(job.pass(Pass::new(a, stage, length)))
}

// ---------------------------------------------------------------------------------------
// Join
// ---------------------------------------------------------------------------------------

/// Why clips can't be joined losslessly (`None` = they can).
pub fn join_mismatch(clips: &[MediaInfo]) -> Option<String> {
    let first = clips.first()?;
    let fv = first.video.as_ref()?;
    for c in &clips[1..] {
        let name = c.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(v) = &c.video else {
            return Some(format!("{name} has no video"));
        };
        if v.codec != fv.codec {
            return Some(format!("{name} uses {} instead of {}", v.codec.to_uppercase(), fv.codec.to_uppercase()));
        }
        if v.display_size() != fv.display_size() {
            let (w, h) = v.display_size();
            return Some(format!("{name} is {w}×{h}"));
        }
        if v.pix_fmt != fv.pix_fmt || (v.fps - fv.fps).abs() > 0.01 {
            return Some(format!("{name} has a different frame rate or colour format"));
        }
        if c.audio.len() != first.audio.len() {
            return Some(format!("{name} has a different number of audio tracks"));
        }
        if let (Some(a), Some(b)) = (c.audio.first(), first.audio.first())
            && (a.codec != b.codec || a.sample_rate != b.sample_rate || a.channels != b.channels)
        {
            return Some(format!("{name} has different audio"));
        }
    }
    None
}

/// Quote a path for an FFmpeg concat list: `'…'`, with `'` written as `'\''`.
fn concat_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
}

pub fn build_join(clips: &[MediaInfo], out: &Path, force_reencode: bool, caps: &Caps) -> Result<Job> {
    if clips.len() < 2 {
        bail!("Add at least two videos to join.");
    }
    if let Some(c) = clips.iter().find(|c| !c.has_video()) {
        bail!("{} has no video.", c.path.display());
    }
    let total: f64 = clips.iter().map(|c| c.duration).sum();
    let c = container_for(out);
    let mut job = Job::new(Output::File(out.to_path_buf()), "Joined");

    if !force_reencode && join_mismatch(clips).is_none() {
        // Lossless: the concat demuxer reads the files one after another.
        let list: PathBuf = temp_prefix("join").with_extension("txt");
        let mut text = String::from("ffconcat version 1.0\n");
        for clip in clips {
            text.push_str(&format!("file {}\n", concat_quote(&clip.path)));
        }
        std::fs::write(&list, text).context("Could not write the list of clips")?;
        let mut a = Args::new();
        a.pair("-f", "concat").pair("-safe", "0").push("-i").push(&list);
        a.pair("-map", "0:v?").pair("-map", "0:a?").pair("-c", "copy");
        let codec = clips[0].video.as_ref().map(|v| v.codec.clone());
        a.extend(c.mux_args(codec.as_deref()));
        job.temp.push(list);
        return Ok(job.pass(Pass::new(a, "Joining (no quality loss)", total)));
    }

    // Re-encode: scale every clip into the first clip's frame (letterboxed, never
    // stretched), one frame rate, one audio format, then the concat filter.
    let fv = clips[0].video.as_ref().expect("checked above");
    let (w, h) = fv.display_size();
    let (w, h) = (even(w as f64), even(h as f64));
    let fps = if fv.fps > 0.0 { fv.fps.min(120.0) } else { 30.0 };
    let any_audio = clips.iter().any(|c| c.has_audio());

    let mut a = Args::new();
    for clip in clips {
        a.push("-i").push(&clip.path);
    }
    let mut g = Vec::new();
    let mut extra = clips.len();
    let mut pairs = String::new();
    for (i, clip) in clips.iter().enumerate() {
        g.push(format!(
            "[{i}:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease:flags=lanczos,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,fps={fps:.3},format=yuv420p[v{i}]"
        ));
        pairs.push_str(&format!("[v{i}]"));
        if any_audio {
            let src = if clip.has_audio() {
                format!("{i}:a:0")
            } else {
                // Silent clip: add a matching stretch of silence as an extra input.
                a.pair("-f", "lavfi")
                    .pair("-t", format!("{:.3}", clip.duration.max(0.1)))
                    .pair("-i", "anullsrc=r=48000:cl=stereo");
                extra += 1;
                format!("{}:a:0", extra - 1)
            };
            g.push(format!(
                "[{src}]aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo[a{i}]"
            ));
            pairs.push_str(&format!("[a{i}]"));
        }
    }
    let n = clips.len();
    if any_audio {
        g.push(format!("{pairs}concat=n={n}:v=1:a=1[v][a]"));
    } else {
        g.push(format!("{pairs}concat=n={n}:v=1:a=0[v]"));
    }
    a.pair("-filter_complex", g.join(";"));
    a.pair("-map", "[v]");
    let codec = c.default_video();
    let backend = codec.resolve(Backend::Cpu, caps);
    a.extend(codec.args(backend, Rate::Quality(20), Speed::Balanced, false));
    if any_audio {
        a.pair("-map", "[a]").extend(c.default_audio().args(192));
    }
    a.extend(c.mux_args(codec.encoder(backend).as_deref()));
    Ok(job.pass(Pass::new(a, format!("Joining {n} clips"), total)))
}

// ---------------------------------------------------------------------------------------
// Resize, rotate, flip, crop
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resize {
    Keep,
    /// Limit the short side (720 = 720p).
    Short(u32),
    /// Fit inside this box, keeping the aspect ratio.
    Fit(u32, u32),
    /// Exactly this size: fit inside and fill the rest with black bars.
    Pad(u32, u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TransformOptions {
    pub resize: Resize,
    /// Clockwise: 0, 90, 180 or 270.
    pub rotate: u32,
    pub flip_h: bool,
    pub flip_v: bool,
    /// Centre-crop to this aspect ratio (e.g. (9, 16) for Shorts / Reels).
    pub crop: Option<(u32, u32)>,
    pub fps: Option<f64>,
    pub backend: Backend,
}

impl Default for TransformOptions {
    fn default() -> Self {
        Self {
            resize: Resize::Keep,
            rotate: 0,
            flip_h: false,
            flip_v: false,
            crop: None,
            fps: None,
            backend: Backend::Cpu,
        }
    }
}

impl TransformOptions {
    /// Only a rotation: this can be done by changing the rotation metadata, without
    /// re-encoding (instant and lossless).
    pub fn rotation_only(&self) -> bool {
        self.rotate % 360 != 0
            && self.resize == Resize::Keep
            && !self.flip_h
            && !self.flip_v
            && self.crop.is_none()
            && self.fps.is_none()
    }
}

/// Filter chain and output size for a transform of a `w × h` (display) picture.
pub fn transform_filters(w: u32, h: u32, o: &TransformOptions) -> (Vec<String>, (u32, u32)) {
    let mut f = Vec::new();
    let (mut w, mut h) = (w, h);
    match o.rotate % 360 {
        90 => {
            f.push("transpose=clock".to_string());
            (w, h) = (h, w);
        }
        180 => f.push("hflip,vflip".to_string()),
        270 => {
            f.push("transpose=cclock".to_string());
            (w, h) = (h, w);
        }
        _ => {}
    }
    if o.flip_h {
        f.push("hflip".into());
    }
    if o.flip_v {
        f.push("vflip".into());
    }
    if let Some((rw, rh)) = o.crop {
        let (cw, ch, x, y) = crop_to_ratio(w, h, rw, rh);
        if (cw, ch) != (w, h) {
            f.push(format!("crop={cw}:{ch}:{x}:{y}"));
            (w, h) = (cw, ch);
        }
    }
    match o.resize {
        Resize::Keep => {}
        Resize::Short(l) => {
            if let Some((nw, nh)) = fit_short_side(w, h, l) {
                f.push(format!("scale={nw}:{nh}:flags=lanczos"));
                (w, h) = (nw, nh);
            }
        }
        Resize::Fit(bw, bh) => {
            let (nw, nh) = fit_inside(w, h, bw, bh);
            f.push(format!("scale={nw}:{nh}:flags=lanczos"));
            (w, h) = (nw, nh);
        }
        Resize::Pad(bw, bh) => {
            let (bw, bh) = (even(bw as f64), even(bh as f64));
            let (nw, nh) = fit_inside(w, h, bw, bh);
            f.push(format!(
                "scale={nw}:{nh}:flags=lanczos,pad={bw}:{bh}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1"
            ));
            (w, h) = (bw, bh);
        }
    }
    if let Some(fps) = o.fps {
        f.push(format!("fps={fps}"));
    }
    (f, (w, h))
}

pub fn build_transform(info: &MediaInfo, out: &Path, o: &TransformOptions, caps: &Caps) -> Result<Job> {
    let Some(v) = &info.video else {
        bail!("This file has no video.");
    };
    let c = container_for(out);
    let job = Job::new(Output::File(out.to_path_buf()), "Saved");

    if o.rotation_only() && matches!(c, Container::Mp4 | Container::Mov | Container::Mkv) {
        // Lossless: rewrite the display-rotation metadata. FFmpeg's value is
        // counter-clockwise and replaces the current one, so combine both.
        let ccw = (v.rotation - o.rotate as i32).rem_euclid(360);
        let mut a = Args::new();
        a.pair("-display_rotation", ccw.to_string());
        a.push("-i").push(&info.path);
        a.pair("-map", "0:v:0").pair("-map", "0:a?").pair("-c", "copy");
        a.extend(c.mux_args(Some(&v.codec)));
        return Ok(job.pass(Pass::new(a, "Rotating (no quality loss)", info.duration)));
    }

    let (w, h) = v.display_size();
    let (filters, _) = transform_filters(w, h, o);
    if filters.is_empty() {
        bail!("Choose a change first: a new size, rotation, flip, crop or frame rate.");
    }
    let mut a = input(&info.path);
    a.pair("-map", "0:v:0").pair("-vf", filters.join(","));
    let (venc, enc) = video_encode(info, c, caps, o.backend);
    a.extend(venc);
    let (audio, _) = audio_args(info, AudioMode::Keep, c);
    a.extend(audio);
    a.extend(c.mux_args(enc.as_deref()));
    Ok(job.pass(Pass::new(a, "Transforming", info.duration)))
}

// ---------------------------------------------------------------------------------------
// Speed
// ---------------------------------------------------------------------------------------

/// Split a tempo factor into `atempo` steps of 0.5–2.0 each (the range every FFmpeg
/// version accepts without quality problems).
pub fn atempo_chain(factor: f64) -> Vec<f64> {
    let mut f = factor.clamp(0.01, 100.0);
    let mut steps = Vec::new();
    while f > 2.0 {
        steps.push(2.0);
        f /= 2.0;
    }
    while f < 0.5 {
        steps.push(0.5);
        f /= 0.5;
    }
    if (f - 1.0).abs() > 1e-9 || steps.is_empty() {
        steps.push(f);
    }
    steps
}

#[derive(Clone, Debug, PartialEq)]
pub struct SpeedOptions {
    /// 2.0 = twice as fast.
    pub factor: f64,
    /// Keep the voice natural (time-stretch) instead of chipmunk/slow-motion pitch.
    pub keep_pitch: bool,
    pub mute: bool,
    pub backend: Backend,
}

impl Default for SpeedOptions {
    fn default() -> Self {
        Self { factor: 2.0, keep_pitch: true, mute: false, backend: Backend::Cpu }
    }
}

pub fn build_speed(info: &MediaInfo, out: &Path, o: &SpeedOptions, caps: &Caps) -> Result<Job> {
    if !(0.1..=16.0).contains(&o.factor) {
        bail!("Choose a speed between 0.1× and 16×.");
    }
    if (o.factor - 1.0).abs() < 1e-6 {
        bail!("The speed is 1×, so nothing would change.");
    }
    let c = container_for(out);
    let mut a = input(&info.path);
    let mut enc = None;
    if info.has_video() {
        a.pair("-map", "0:v:0").pair("-vf", format!("setpts=PTS/{}", o.factor));
        let (venc, e) = video_encode(info, c, caps, o.backend);
        a.extend(venc);
        enc = e;
    }
    if info.has_audio() && !o.mute {
        let sr = info.audio[0].sample_rate.max(8000);
        let mut chain: Vec<String> = if o.keep_pitch {
            atempo_chain(o.factor).iter().map(|s| format!("atempo={s}")).collect()
        } else {
            vec![format!("asetrate={}", (sr as f64 * o.factor).round() as u64), format!("aresample={sr}")]
        };
        let codec = c.default_audio();
        if let Some(f) = codec.filter() {
            chain.push(f.to_string());
        }
        a.pair("-map", "0:a:0").pair("-af", chain.join(","));
        a.extend(codec.args(192));
    } else {
        a.push("-an");
    }
    a.extend(c.mux_args(enc.as_deref()));
    let job = Job::new(Output::File(out.to_path_buf()), "Saved");
    Ok(job.pass(Pass::new(a, format!("Changing speed to {}×", o.factor), info.duration / o.factor)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::probe::{AudioStream, VideoStream};

    fn clip(name: &str, w: u32, h: u32, codec: &str, audio: bool) -> MediaInfo {
        MediaInfo {
            path: name.into(),
            size: 1000,
            duration: 10.0,
            video: Some(VideoStream {
                codec: codec.into(),
                width: w,
                height: h,
                fps: 30.0,
                pix_fmt: "yuv420p".into(),
                ..Default::default()
            }),
            audio: if audio {
                vec![AudioStream { codec: "aac".into(), sample_rate: 44100, channels: 2, ..Default::default() }]
            } else {
                vec![]
            },
            ..Default::default()
        }
    }

    fn arg_after(a: &Args, k: &str) -> String {
        let s = a.strings();
        s[s.iter().position(|x| x == k).unwrap() + 1].clone()
    }

    #[test]
    fn fast_and_precise_trim() {
        let i = clip("in.mp4", 1920, 1080, "h264", true);
        let o = TrimOptions { start: 2.0, end: 5.5, mode: TrimMode::Fast, remove: false };
        let job = build_trim(&i, Path::new("o.mp4"), &o, &Caps::default()).unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-ss", "2.000") && a.has_pair("-t", "3.500"));
        assert!(a.has_pair("-c", "copy"));
        // -ss comes before -i (fast seek).
        let s = a.strings();
        assert!(s.iter().position(|x| x == "-ss") < s.iter().position(|x| x == "-i"));

        let o = TrimOptions { mode: TrimMode::Precise, ..o };
        let job = build_trim(&i, Path::new("o.mp4"), &o, &Caps::default()).unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-c:v", "libx264") && a.has_pair("-crf", "18"));
        assert!(a.has_pair("-c:a", "aac"));

        // H.265 sources stay H.265.
        let i265 = clip("in.mp4", 1920, 1080, "hevc", true);
        let job = build_trim(&i265, Path::new("o.mp4"), &o, &Caps::default()).unwrap();
        assert!(job.passes[0].args.has_pair("-c:v", "libx265"));

        let bad = TrimOptions { start: 5.0, end: 5.0, ..o };
        assert!(build_trim(&i, Path::new("o.mp4"), &bad, &Caps::default()).is_err());
    }

    #[test]
    fn cut_out_middle() {
        let i = clip("in.mp4", 1920, 1080, "h264", true);
        let o = TrimOptions { start: 3.0, end: 4.0, mode: TrimMode::Fast, remove: true };
        let job = build_trim(&i, Path::new("o.mp4"), &o, &Caps::default()).unwrap();
        let fc = arg_after(&job.passes[0].args, "-filter_complex");
        assert!(fc.contains("[0:v:0]trim=end=3.000,setpts=PTS-STARTPTS[v0]"));
        assert!(fc.contains("[0:a:0]atrim=start=4.000,asetpts=PTS-STARTPTS[a1]"));
        assert!(fc.ends_with("[v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"));
        assert_eq!(job.passes[0].duration, 9.0);

        // Removing the beginning = keeping the rest.
        let o = TrimOptions { start: 0.0, end: 4.0, mode: TrimMode::Fast, remove: true };
        let job = build_trim(&i, Path::new("o.mp4"), &o, &Caps::default()).unwrap();
        assert!(job.passes[0].args.has_pair("-ss", "4.000"));
        let all = TrimOptions { start: 0.0, end: 10.0, mode: TrimMode::Fast, remove: true };
        assert!(build_trim(&i, Path::new("o.mp4"), &all, &Caps::default()).is_err());
    }

    #[test]
    fn join_lossless_when_compatible() {
        let clips = [clip("a.mp4", 1280, 720, "h264", true), clip("b's.mp4", 1280, 720, "h264", true)];
        assert_eq!(join_mismatch(&clips), None);
        let job = build_join(&clips, Path::new("o.mp4"), false, &Caps::default()).unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-f", "concat") && a.has_pair("-c", "copy"));
        let list = std::fs::read_to_string(&job.temp[0]).unwrap();
        assert!(list.contains("file 'b'\\''s.mp4'"), "{list}");
        assert_eq!(job.passes[0].duration, 20.0);
        let _ = std::fs::remove_file(&job.temp[0]);
    }

    #[test]
    fn join_reencodes_mixed_clips() {
        let clips = [
            clip("a.mp4", 1920, 1080, "h264", true),
            clip("b.mp4", 1080, 1920, "hevc", false),
        ];
        assert!(join_mismatch(&clips).unwrap().contains("HEVC"));
        let job = build_join(&clips, Path::new("o.mp4"), false, &Caps::default()).unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-i", "anullsrc=r=48000:cl=stereo"));
        let fc = arg_after(a, "-filter_complex");
        assert!(fc.contains("[1:v:0]scale=1920:1080:force_original_aspect_ratio=decrease"));
        assert!(fc.contains("[2:a:0]aformat"), "silence for the silent clip: {fc}");
        assert!(fc.ends_with("[v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"));
    }

    #[test]
    fn transforms() {
        let o = TransformOptions { rotate: 90, crop: Some((1, 1)), resize: Resize::Short(720), ..Default::default() };
        let (f, size) = transform_filters(1920, 1080, &o);
        assert_eq!(f, ["transpose=clock", "crop=1080:1080:0:420", "scale=720:720:flags=lanczos"]);
        assert_eq!(size, (720, 720));

        let o = TransformOptions { resize: Resize::Pad(1080, 1920), ..Default::default() };
        let (f, size) = transform_filters(1920, 1080, &o);
        assert_eq!(f[0], "scale=1080:608:flags=lanczos,pad=1080:1920:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1");
        assert_eq!(size, (1080, 1920));
    }

    #[test]
    fn lossless_rotation_combines_with_existing() {
        let mut i = clip("in.mp4", 1920, 1080, "h264", true);
        i.video.as_mut().unwrap().rotation = 270; // phone video: shown 90° clockwise
        let o = TransformOptions { rotate: 90, ..Default::default() };
        assert!(o.rotation_only());
        let job = build_transform(&i, Path::new("o.mp4"), &o, &Caps::default()).unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-display_rotation", "180"));
        assert!(a.has_pair("-c", "copy"));
        let none = TransformOptions::default();
        assert!(build_transform(&i, Path::new("o.mp4"), &none, &Caps::default()).is_err());
    }

    #[test]
    fn speed_changes() {
        assert_eq!(atempo_chain(2.0), [2.0]);
        assert_eq!(atempo_chain(4.0), [2.0, 2.0]);
        assert_eq!(atempo_chain(3.0), [2.0, 1.5]);
        assert_eq!(atempo_chain(0.25), [0.5, 0.5]);
        assert_eq!(atempo_chain(1.0), [1.0]);
        let i = clip("in.mp4", 1280, 720, "h264", true);
        let job = build_speed(&i, Path::new("o.mp4"), &SpeedOptions { factor: 4.0, ..Default::default() }, &Caps::default()).unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-vf", "setpts=PTS/4"));
        assert!(a.has_pair("-af", "atempo=2,atempo=2"));
        assert_eq!(job.passes[0].duration, 2.5);
        let o = SpeedOptions { factor: 0.5, keep_pitch: false, ..Default::default() };
        let job = build_speed(&i, Path::new("o.mp4"), &o, &Caps::default()).unwrap();
        assert!(job.passes[0].args.has_pair("-af", "asetrate=22050,aresample=44100"));
        let o = SpeedOptions { factor: 1.0, ..Default::default() };
        assert!(build_speed(&i, Path::new("o.mp4"), &o, &Caps::default()).is_err());
    }
}
