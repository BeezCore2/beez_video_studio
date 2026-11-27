//! End-to-end tests against a real FFmpeg (opt-in, they take a minute):
//! `cargo test e2e -- --ignored --test-threads=1 --nocapture`
//!
//! Test clips are generated with FFmpeg's built-in sources, every tool's job is run, and
//! each result is checked with ffprobe.

use std::path::{Path, PathBuf};

use super::codec::{Args, Backend, Container, Speed, VideoCodec};
use super::compress::{self, AudioMode, Mode, Resolution};
use super::convert::{self, AnimFormat, GifOptions};
use super::edit::{self, Resize, SpeedOptions, TransformOptions, TrimMode, TrimOptions};
use super::ffmpeg::{self, Span};
use super::frames::{self, ImageFormat};
use super::job::{self, Job};
use super::probe::{self, MediaInfo};
use super::progress::Progress;
use super::{audio, audio::Format};

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join("beez-e2e");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Generate a clip: test pattern + tone (or silent), `w×h`, `secs` long.
fn make_clip(name: &str, w: u32, h: u32, secs: u32, fps: u32, audio: bool) -> MediaInfo {
    let out = dir().join(name);
    let mut a = Args::new();
    a.pair("-f", "lavfi").pair(
        "-i",
        format!("testsrc2=size={w}x{h}:rate={fps}:duration={secs}"),
    );
    if audio {
        a.pair("-f", "lavfi")
            .pair(
                "-i",
                format!("sine=frequency=440:sample_rate=48000:duration={secs}"),
            )
            .pair("-c:a", "aac")
            .pair("-b:a", "128k");
    }
    a.pair("-c:v", "libx264")
        .pair("-preset", "ultrafast")
        .pair("-crf", "12");
    a.pair("-pix_fmt", "yuv420p").push(&out);
    run(&a);
    probe::probe(&out).unwrap()
}

fn run(a: &Args) {
    let pr = Progress::default();
    ffmpeg::run(
        &a.0,
        Span {
            progress: &pr,
            from: 0.0,
            to: 1.0,
            duration: 0.0,
            stage: "",
        },
    )
    .unwrap();
}

fn exec(job: anyhow::Result<Job>) -> String {
    let msg = job::execute(job.unwrap(), &Progress::default()).unwrap();
    println!("  {msg}");
    msg
}

fn out(name: &str) -> PathBuf {
    let p = dir().join(name);
    let _ = std::fs::remove_file(&p);
    p
}

fn check(p: &Path) -> MediaInfo {
    probe::probe(p).unwrap_or_else(|e| panic!("{}: {e:#}", p.display()))
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
#[ignore]
fn e2e_all_tools() {
    assert!(
        ffmpeg::tools().is_some(),
        "FFmpeg is required for this test"
    );
    let caps = ffmpeg::caps();
    println!(
        "FFmpeg {:?}, GPU: {:?}",
        ffmpeg::tools().and_then(|t| ffmpeg::version(&t)),
        caps.hardware
    );

    let clip = make_clip("src.mp4", 1280, 720, 20, 30, true);
    assert!(near(clip.duration, 20.0, 0.2));
    let portrait = make_clip("portrait.mp4", 720, 1280, 4, 25, false);
    let same = make_clip("same.mp4", 1280, 720, 5, 30, true);

    println!("compress: quality");
    let o = compress::Options {
        mode: Mode::Quality(30),
        ..Default::default()
    };
    let est = compress::estimate(&clip, &o, &caps, &Progress::default()).unwrap();
    exec(compress::build(&clip, &out("c_q.mp4"), &o, &caps));
    let r = check(&out_keep("c_q.mp4"));
    println!("  estimate {est} vs real {}", r.size);
    assert!(r.size < clip.size);
    let ratio = est as f64 / r.size as f64;
    assert!((0.5..2.0).contains(&ratio), "estimate ratio {ratio}");

    println!("compress: target size, two-pass, auto resolution, H.265");
    let o = compress::Options {
        mode: Mode::TargetSize(1.0),
        codec: VideoCodec::H265,
        resolution: Resolution::Auto,
        audio: AudioMode::Encode(64),
        ..Default::default()
    };
    exec(compress::build(&clip, &out("c_t.mp4"), &o, &caps));
    let r = check(&out_keep("c_t.mp4"));
    let mb = r.size as f64 / 1024.0 / 1024.0;
    println!(
        "  target 1.0 MB → {mb:.2} MB, {:?}",
        r.video.as_ref().map(|v| (v.width, v.height))
    );
    assert!((0.7..1.15).contains(&mb), "{mb} MB");
    assert_eq!(r.video.as_ref().unwrap().codec, "hevc");

    for (codec, name) in [
        (VideoCodec::Av1, "c_av1.mp4"),
        (VideoCodec::Vp9, "c_vp9.webm"),
    ] {
        println!("compress: {}", codec.short());
        let o = compress::Options {
            codec,
            speed: Speed::Fast,
            max_fps: Some(24),
            resolution: Resolution::Limit(360),
            ..Default::default()
        };
        exec(compress::build(&clip, &out(name), &o, &caps));
        let r = check(&out_keep(name));
        let v = r.video.unwrap();
        assert_eq!((v.width, v.height), (640, 360));
        assert!(near(v.fps, 24.0, 0.1));
    }
    for b in [Backend::Nvidia, Backend::Intel, Backend::Amd] {
        if VideoCodec::H264.resolve(b, &caps) == b {
            println!("compress: GPU {b:?}");
            let o = compress::Options {
                backend: b,
                ..Default::default()
            };
            exec(compress::build(&clip, &out("c_gpu.mp4"), &o, &caps));
            check(&out_keep("c_gpu.mp4"));
        }
    }

    println!("convert: remux to MKV (copy)");
    exec(convert::build(
        &clip,
        &out("v.mkv"),
        &convert::Options {
            container: Container::Mkv,
            ..Default::default()
        },
        &caps,
    ));
    let r = check(&out_keep("v.mkv"));
    assert_eq!(r.video.unwrap().codec, "h264");
    println!("convert: WebM");
    let o = convert::Options {
        container: Container::WebM,
        speed: Speed::Fast,
        ..Default::default()
    };
    exec(convert::build(&same, &out("v.webm"), &o, &caps));
    let r = check(&out_keep("v.webm"));
    assert_eq!(r.audio[0].codec, "opus");
    println!("convert: AVI");
    exec(convert::build(
        &same,
        &out("v.avi"),
        &convert::Options {
            container: Container::Avi,
            ..Default::default()
        },
        &caps,
    ));
    check(&out_keep("v.avi"));
    println!("convert: MOV");
    exec(convert::build(
        &same,
        &out("v.mov"),
        &convert::Options {
            container: Container::Mov,
            ..Default::default()
        },
        &caps,
    ));
    check(&out_keep("v.mov"));

    println!("gif + webp");
    exec(convert::build_gif(
        &clip,
        &out("a.gif"),
        &GifOptions {
            start: 2.0,
            length: 2.0,
            width: 320,
            ..Default::default()
        },
        &caps,
    ));
    let r = check(&out_keep("a.gif"));
    assert_eq!(r.video.unwrap().width, 320);
    exec(convert::build_gif(
        &clip,
        &out("a.webp"),
        &GifOptions {
            format: AnimFormat::WebP,
            length: 2.0,
            ..Default::default()
        },
        &caps,
    ));
    assert!(std::fs::metadata(out_keep("a.webp")).unwrap().len() > 1000);

    println!("trim");
    for (mode, name) in [
        (TrimMode::Fast, "t_fast.mp4"),
        (TrimMode::Precise, "t_precise.mp4"),
    ] {
        exec(edit::build_trim(
            &clip,
            &out(name),
            &TrimOptions {
                start: 5.0,
                end: 9.0,
                mode,
                remove: false,
            },
            &caps,
        ));
        let r = check(&out_keep(name));
        println!("  {mode:?}: {:.2} s", r.duration);
        match mode {
            // Lossless cuts start at the keyframe before 5 s (this clip has one every
            // ~8 s, i.e. at 0 s) and still end at 9 s.
            TrimMode::Fast => assert!((4.0..=9.2).contains(&r.duration), "{}", r.duration),
            TrimMode::Precise => assert!(near(r.duration, 4.0, 0.15), "{}", r.duration),
        }
    }
    exec(edit::build_trim(
        &clip,
        &out("t_cut.mp4"),
        &TrimOptions {
            start: 5.0,
            end: 9.0,
            mode: TrimMode::Precise,
            remove: true,
        },
        &caps,
    ));
    let r = check(&out_keep("t_cut.mp4"));
    assert!(near(r.duration, 16.0, 0.2), "{}", r.duration);

    println!("join: lossless and mixed");
    let a = check(&out_keep("t_precise.mp4"));
    exec(edit::build_join(
        &[same.clone(), same.clone()],
        &out("j_copy.mp4"),
        false,
        &caps,
    ));
    let r = check(&out_keep("j_copy.mp4"));
    assert!(near(r.duration, 10.0, 0.2), "{}", r.duration);
    exec(edit::build_join(
        &[same.clone(), portrait.clone(), a],
        &out("j_mix.mp4"),
        false,
        &caps,
    ));
    let r = check(&out_keep("j_mix.mp4"));
    let v = r.video.as_ref().unwrap();
    assert_eq!((v.width, v.height), (1280, 720));
    assert!(r.has_audio());
    assert!(near(r.duration, 13.0, 0.4), "{}", r.duration);

    println!("transform: lossless rotate");
    exec(edit::build_transform(
        &same,
        &out("r90.mp4"),
        &TransformOptions {
            rotate: 90,
            ..Default::default()
        },
        &caps,
    ));
    let r = check(&out_keep("r90.mp4"));
    let v = r.video.unwrap();
    println!("  rotation {} → display {:?}", v.rotation, v.display_size());
    assert_eq!(v.display_size(), (720, 1280));
    println!("transform: crop 9:16 + pad");
    let o = TransformOptions {
        crop: Some((9, 16)),
        resize: Resize::Pad(1080, 1920),
        fps: Some(25.0),
        ..Default::default()
    };
    exec(edit::build_transform(&same, &out("crop.mp4"), &o, &caps));
    let v = check(&out_keep("crop.mp4")).video.unwrap();
    assert_eq!((v.width, v.height), (1080, 1920));

    println!("speed");
    exec(edit::build_speed(
        &same,
        &out("s2.mp4"),
        &SpeedOptions {
            factor: 2.0,
            ..Default::default()
        },
        &caps,
    ));
    let r = check(&out_keep("s2.mp4"));
    assert!(near(r.duration, 2.5, 0.2), "{}", r.duration);
    exec(edit::build_speed(
        &same,
        &out("s05.mp4"),
        &SpeedOptions {
            factor: 0.5,
            keep_pitch: false,
            ..Default::default()
        },
        &caps,
    ));
    assert!(near(check(&out_keep("s05.mp4")).duration, 10.0, 0.3));

    println!("audio");
    exec(audio::build_extract(
        &clip,
        &out("a.m4a"),
        Format::Original,
        0,
        0,
    ));
    exec(audio::build_extract(
        &clip,
        &out("a.mp3"),
        Format::Mp3,
        0,
        0,
    ));
    exec(audio::build_extract(
        &clip,
        &out("a.opus"),
        Format::Opus,
        96,
        0,
    ));
    exec(audio::build_extract(
        &clip,
        &out("a.flac"),
        Format::Flac,
        0,
        0,
    ));
    assert_eq!(check(&out_keep("a.mp3")).audio[0].codec, "mp3");
    exec(audio::build_remove(&clip, &out("silent.mp4")));
    assert!(!check(&out_keep("silent.mp4")).has_audio());
    exec(audio::build_volume(&clip, &out("loud.mp4"), 6.0));
    check(&out_keep("loud.mp4"));
    let msg = audio::normalize(&clip, &out("norm.mp4"), -16.0, -1.5, &Progress::default()).unwrap();
    println!("  {msg}");
    check(&out_keep("norm.mp4"));
    let music = check(&out_keep("a.mp3"));
    exec(audio::build_music(
        &same,
        &music,
        &out("mix.mp4"),
        audio::MusicMode::Mix { music_volume: 0.3 },
    ));
    assert!(near(check(&out_keep("mix.mp4")).duration, 5.0, 0.2));
    exec(audio::build_music(
        &portrait,
        &music,
        &out("rep.mp4"),
        audio::MusicMode::Replace,
    ));
    assert!(check(&out_keep("rep.mp4")).has_audio());

    println!("frames");
    exec(frames::build(
        &clip,
        &out("f.jpg"),
        frames::Mode::Single(3.0),
        ImageFormat::Jpg,
    ));
    exec(frames::build(
        &clip,
        &out("sheet.png"),
        frames::Mode::Sheet {
            cols: 4,
            rows: 3,
            width: 160,
        },
        ImageFormat::Png,
    ));
    let series = dir().join("series");
    let _ = std::fs::remove_dir_all(&series);
    let msg = exec(frames::build(
        &clip,
        &series.join("clip.jpg"),
        frames::Mode::Count(5),
        ImageFormat::Jpg,
    ));
    assert!(msg.contains("5 images"), "{msg}");
    let msg = exec(frames::build(
        &clip,
        &series.join("every.png"),
        frames::Mode::Every(5.0),
        ImageFormat::Png,
    ));
    assert!(
        msg.contains("4 images") || msg.contains("5 images"),
        "{msg}"
    );

    println!("thumbnail grab");
    let png = ffmpeg::grab_frame(&clip.path, 1.0, 160).unwrap();
    assert!(png.starts_with(b"\x89PNG"));

    println!("cancel leaves no file");
    let pr = Progress::default();
    pr.cancel();
    let o = compress::Options::default();
    let target = out("cancelled.mp4");
    let r = job::execute(compress::build(&clip, &target, &o, &caps).unwrap(), &pr);
    assert!(r.is_err() && !target.exists() && !job::temp_path(&target).exists());
}

/// Path of an output without deleting it.
fn out_keep(name: &str) -> PathBuf {
    dir().join(name)
}

#[test]
#[ignore]
fn e2e_preview_stream() {
    use std::io::Read;
    let clip = dir().join("src.mp4");
    let (w, h) = (640u32, 360u32);
    let started = std::time::Instant::now();
    let mut child = ffmpeg::spawn_preview(&clip, 2.0, w, h, 30.0).unwrap();
    let mut out = child.stdout.take().unwrap();
    let mut buf = vec![0u8; (w * h * 4) as usize];
    for _ in 0..30 {
        out.read_exact(&mut buf).unwrap();
    }
    let secs = started.elapsed().as_secs_f64();
    let _ = child.kill();
    let _ = child.wait();
    println!("30 frames in {secs:.2} s");
    // Real-time pacing: 30 frames at 30 fps take about a second.
    assert!((0.6..2.5).contains(&secs), "{secs}");
}
