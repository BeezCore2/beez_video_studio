//! Still images from a video: one frame, a frame every N seconds, N evenly spaced frames,
//! or a contact sheet (a grid of thumbnails, made with the `tile` filter).

use std::path::Path;

use anyhow::{Result, bail};

use super::codec::Args;
use super::job::{Job, Output, Pass};
use super::probe::{MediaInfo, fmt_time};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    Jpg,
    Png,
}

impl ImageFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ImageFormat::Jpg => "jpg",
            ImageFormat::Png => "png",
        }
    }

    fn args(self) -> Args {
        let mut a = Args::new();
        if self == ImageFormat::Jpg {
            // MJPEG quality scale 2 (best) … 31: 2 is near-lossless.
            a.pair("-q:v", "2");
        }
        a
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// One frame at this time (seconds).
    Single(f64),
    /// One frame every N seconds.
    Every(f64),
    /// N frames spread evenly over the video.
    Count(u32),
    /// A grid of `cols × rows` thumbnails, each `width` pixels wide.
    Sheet { cols: u32, rows: u32, width: u32 },
}

/// Build the job. `out` is a file for `Single`/`Sheet`; for the others it is the first file
/// name of the series (e.g. `dir/clip.jpg` → `dir/clip_0001.jpg`, `clip_0002.jpg`, …).
pub fn build(info: &MediaInfo, out: &Path, mode: Mode, format: ImageFormat) -> Result<Job> {
    let Some(v) = &info.video else {
        bail!("This file has no video.");
    };
    let dur = info.duration;
    let mut a = Args::new();
    match mode {
        Mode::Single(t) => {
            if dur > 0.0 && t >= dur {
                bail!(
                    "The time is past the end of the video ({}).",
                    fmt_time(dur, true)
                );
            }
            a.pair("-ss", format!("{:.3}", t.max(0.0)));
            a.push("-i").push(&info.path);
            a.pair("-map", "0:v:0")
                .pair("-frames:v", "1")
                .pair("-update", "1");
            a.extend(format.args());
            let job = Job::new(Output::File(out.to_path_buf()), "Saved");
            return Ok(job.pass(Pass::new(
                a,
                format!("Saving frame at {}", fmt_time(t, true)),
                0.0,
            )));
        }
        Mode::Sheet { cols, rows, width } => {
            if dur <= 0.0 {
                bail!("The video's length is unknown.");
            }
            let n = (cols * rows).max(1);
            // Sample slightly more often than n per duration so the grid is always full.
            let rate = n as f64 * 1.02 / dur;
            let (w, _) = v.display_size();
            let tw = width.min(w.max(16));
            a.push("-i").push(&info.path);
            a.pair(
                "-vf",
                format!(
                    "fps={rate:.6},scale={tw}:-2:flags=lanczos,tile={cols}x{rows}:padding=6:margin=6:color=white"
                ),
            );
            a.pair("-map", "0:v:0")
                .pair("-frames:v", "1")
                .pair("-update", "1");
            a.extend(format.args());
            let job = Job::new(Output::File(out.to_path_buf()), "Saved");
            return Ok(job.pass(Pass::new(a, "Making contact sheet", dur)));
        }
        Mode::Every(secs) => {
            if secs <= 0.0 {
                bail!("Choose an interval above zero.");
            }
            a.push("-i").push(&info.path);
            a.pair("-vf", format!("fps=1/{secs}"));
        }
        Mode::Count(n) => {
            if dur <= 0.0 {
                bail!("The video's length is unknown.");
            }
            let n = n.max(1);
            a.push("-i").push(&info.path);
            a.pair("-vf", format!("fps={:.6}", n as f64 / dur));
            a.pair("-frames:v", n.to_string());
        }
    }
    a.pair("-map", "0:v:0");
    a.extend(format.args());
    let dir = out.parent().map(Path::to_path_buf).unwrap_or_default();
    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "frame".into());
    // `%` in a file name would be read as part of the pattern.
    let stem = stem.replace('%', "_");
    let pattern = format!("{stem}_%04d.{}", format.ext());
    let job = Job::new(Output::Series { dir, pattern }, "Saved");
    Ok(job.pass(Pass::new(a, "Saving frames", dur)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::probe::VideoStream;

    fn info() -> MediaInfo {
        MediaInfo {
            path: "in.mp4".into(),
            duration: 100.0,
            video: Some(VideoStream {
                width: 1920,
                height: 1080,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn single_frame() {
        let job = build(
            &info(),
            Path::new("d/shot.jpg"),
            Mode::Single(12.5),
            ImageFormat::Jpg,
        )
        .unwrap();
        let a = &job.passes[0].args;
        assert!(
            a.has_pair("-ss", "12.500") && a.has_pair("-frames:v", "1") && a.has_pair("-q:v", "2")
        );
        assert_eq!(job.output, Output::File("d/shot.jpg".into()));
        assert!(
            build(
                &info(),
                Path::new("x.png"),
                Mode::Single(100.0),
                ImageFormat::Png
            )
            .is_err()
        );
    }

    #[test]
    fn series_and_sheet() {
        let job = build(
            &info(),
            Path::new("d/my%clip.png"),
            Mode::Every(5.0),
            ImageFormat::Png,
        )
        .unwrap();
        assert!(job.passes[0].args.has_pair("-vf", "fps=1/5"));
        assert_eq!(
            job.output,
            Output::Series {
                dir: "d".into(),
                pattern: "my_clip_%04d.png".into()
            }
        );

        let job = build(
            &info(),
            Path::new("d/c.jpg"),
            Mode::Count(10),
            ImageFormat::Jpg,
        )
        .unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair("-vf", "fps=0.100000") && a.has_pair("-frames:v", "10"));

        let job = build(
            &info(),
            Path::new("s.jpg"),
            Mode::Sheet {
                cols: 4,
                rows: 3,
                width: 320,
            },
            ImageFormat::Jpg,
        )
        .unwrap();
        let a = &job.passes[0].args;
        assert!(a.has_pair(
            "-vf",
            "fps=0.122400,scale=320:-2:flags=lanczos,tile=4x3:padding=6:margin=6:color=white"
        ));
    }
}
