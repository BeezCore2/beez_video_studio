//! A job: one or more FFmpeg passes producing one output file (or a numbered image series).
//!
//! The output is written to a temporary `name.beez-part.ext` next to the destination and
//! renamed when every pass succeeded. So a cancelled or failed job never leaves a broken
//! file behind, and the destination may even be the source file itself.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::codec::{Args, null_output};
use super::ffmpeg::{self, Span};
use super::human_size;
use super::progress::Progress;

/// Where a pass writes.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// The job's output (appended to the arguments by the runner).
    Main,
    /// Nowhere: an analysis pass (first pass of a two-pass encode).
    Null,
}

#[derive(Clone, Debug)]
pub struct Pass {
    pub args: Args,
    pub target: Target,
    /// Shown in the progress bar, e.g. "Analysing (pass 1 of 2)".
    pub stage: String,
    /// Length of the media this pass produces, in seconds (for the percentage).
    pub duration: f64,
    /// Share of the progress bar (relative to the other passes).
    pub weight: f32,
}

impl Pass {
    pub fn new(args: Args, stage: impl Into<String>, duration: f64) -> Self {
        Self {
            args,
            target: Target::Main,
            stage: stage.into(),
            duration,
            weight: 1.0,
        }
    }

    pub fn null(mut self) -> Self {
        self.target = Target::Null;
        self
    }

    pub fn weight(mut self, w: f32) -> Self {
        self.weight = w;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Output {
    File(PathBuf),
    /// A numbered series such as `dir/name_0001.jpg`; `pattern` is the FFmpeg pattern
    /// (`name_%04d.jpg`).
    Series {
        dir: PathBuf,
        pattern: String,
    },
}

#[derive(Clone, Debug)]
pub struct Job {
    pub passes: Vec<Pass>,
    pub output: Output,
    /// Size of the source, to report how much smaller the result is.
    pub source_size: Option<u64>,
    /// Temporary files to delete afterwards (paths, or prefixes ending in `*`).
    pub temp: Vec<PathBuf>,
    /// Past-tense verb for the result message, e.g. "Compressed".
    pub verb: String,
}

impl Job {
    pub fn new(output: Output, verb: impl Into<String>) -> Self {
        Self {
            passes: Vec::new(),
            output,
            source_size: None,
            temp: Vec::new(),
            verb: verb.into(),
        }
    }

    pub fn pass(mut self, p: Pass) -> Self {
        self.passes.push(p);
        self
    }
}

/// `video.mp4` → `video.beez-part.mp4` (the extension stays last so FFmpeg picks the
/// right format).
pub fn temp_path(out: &Path) -> PathBuf {
    let stem = out
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".into());
    let name = match out.extension() {
        Some(ext) => format!("{stem}.beez-part.{}", ext.to_string_lossy()),
        None => format!("{stem}.beez-part"),
    };
    out.with_file_name(name)
}

/// A unique prefix in the temp folder for two-pass statistics files.
pub fn temp_prefix(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("beez-{tag}-{}-{nanos}", std::process::id()))
}

/// Delete a temp path; a path ending in `*` deletes every file starting with that prefix.
fn remove_temp(p: &Path) {
    let s = p.to_string_lossy();
    if let Some(prefix) = s.strip_suffix('*') {
        let prefix = Path::new(prefix);
        let (Some(dir), Some(start)) = (prefix.parent(), prefix.file_name()) else {
            return;
        };
        let start = start.to_string_lossy().into_owned();
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                if e.file_name().to_string_lossy().starts_with(&start) {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
    } else {
        let _ = fs::remove_file(p);
    }
}

/// Files in `dir` that belong to a numbered series: `name_%04d.jpg` → starts with `name_`
/// and ends with `.jpg`.
fn series_files(dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let (head, tail) = match pattern.split_once('%') {
        Some((h, t)) => (
            h.to_string(),
            t.split_once('d')
                .map(|(_, e)| e.to_string())
                .unwrap_or_default(),
        ),
        None => (pattern.to_string(), String::new()),
    };
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    let n = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    n.starts_with(&head) && n.ends_with(&tail)
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn percent_change(before: u64, after: u64) -> String {
    if before == 0 {
        return String::new();
    }
    let ratio = after as f64 / before as f64;
    if ratio < 0.995 {
        format!(
            " · {:.0}% smaller than the original ({})",
            (1.0 - ratio) * 100.0,
            human_size(before)
        )
    } else if ratio > 1.005 {
        format!(
            " · {:.0}% larger than the original ({})",
            (ratio - 1.0) * 100.0,
            human_size(before)
        )
    } else {
        format!(
            " · about the same size as the original ({})",
            human_size(before)
        )
    }
}

/// Run every pass, then put the output in place. Returns the message for the InfoBar
/// ("… → path", so the InfoBar offers Open file / Show in folder).
pub fn execute(job: Job, progress: &Progress) -> Result<String> {
    execute_in(job, progress, 0.0, 1.0)
}

/// [`execute`] filling only `from..to` of the progress bar (after an analysis step).
pub fn execute_in(job: Job, progress: &Progress, from: f32, to: f32) -> Result<String> {
    if job.passes.is_empty() {
        bail!("Nothing to do");
    }
    let (temp_out, before) = match &job.output {
        Output::File(out) => {
            if let Some(dir) = out.parent()
                && !dir.as_os_str().is_empty()
            {
                fs::create_dir_all(dir)
                    .with_context(|| format!("Could not create {}", dir.display()))?;
            }
            (Some(temp_path(out)), Vec::new())
        }
        Output::Series { dir, pattern } => {
            fs::create_dir_all(dir)
                .with_context(|| format!("Could not create {}", dir.display()))?;
            (None, series_files(dir, pattern))
        }
    };

    let total: f32 = job
        .passes
        .iter()
        .map(|p| p.weight.max(0.0))
        .sum::<f32>()
        .max(1e-6);
    let mut at = from;
    let n = job.passes.len();
    let mut result = Ok(());
    for (i, pass) in job.passes.iter().enumerate() {
        let span_len = pass.weight.max(0.0) / total * (to - from);
        let mut args = pass.args.clone();
        match pass.target {
            Target::Null => {
                args.pair("-f", "null").push(null_output());
            }
            Target::Main => match (&job.output, &temp_out) {
                (Output::File(_), Some(t)) => {
                    args.push(t.clone());
                }
                (Output::Series { dir, pattern }, _) => {
                    args.push(dir.join(pattern));
                }
                _ => {}
            },
        }
        let stage = if n > 1 && !pass.stage.contains("pass") {
            format!("{} (step {} of {n})", pass.stage, i + 1)
        } else {
            pass.stage.clone()
        };
        let span = Span {
            progress,
            from: at,
            to: at + span_len,
            duration: pass.duration,
            stage: &stage,
        };
        result = ffmpeg::run(&args.0, span);
        if result.is_err() {
            break;
        }
        at += span_len;
    }

    for t in &job.temp {
        remove_temp(t);
    }

    match (&job.output, result) {
        (Output::File(_), Err(e)) => {
            if let Some(t) = &temp_out {
                let _ = fs::remove_file(t);
            }
            Err(e)
        }
        (Output::Series { dir, pattern }, Err(e)) => {
            for f in series_files(dir, pattern) {
                if !before.contains(&f) {
                    let _ = fs::remove_file(f);
                }
            }
            Err(e)
        }
        (Output::File(out), Ok(())) => {
            let temp = temp_out.expect("temp path for file output");
            let size = fs::metadata(&temp).map(|m| m.len()).unwrap_or(0);
            if size == 0 {
                let _ = fs::remove_file(&temp);
                bail!("FFmpeg finished but produced an empty file");
            }
            if out.exists() {
                fs::remove_file(out).with_context(|| {
                    format!(
                        "Could not replace {} (is it open in another program?)",
                        out.display()
                    )
                })?;
            }
            fs::rename(&temp, out).with_context(|| format!("Could not save {}", out.display()))?;
            let name = out
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let change = job
                .source_size
                .map(|b| percent_change(b, size))
                .unwrap_or_default();
            Ok(format!(
                "{} {name} · {}{change} → {}",
                job.verb,
                human_size(size),
                out.display()
            ))
        }
        (Output::Series { dir, pattern }, Ok(())) => {
            let made = series_files(dir, pattern)
                .into_iter()
                .filter(|f| !before.contains(f))
                .count();
            if made == 0 {
                bail!("FFmpeg finished but no images were written");
            }
            let what = if made == 1 { "image" } else { "images" };
            Ok(format!("{} {made} {what} → {}", job.verb, dir.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names_keep_the_extension() {
        assert_eq!(
            temp_path(Path::new("C:/v/clip.final.mp4")),
            Path::new("C:/v/clip.final.beez-part.mp4")
        );
        assert_eq!(temp_path(Path::new("out")), Path::new("out.beez-part"));
    }

    #[test]
    fn size_change_messages() {
        assert_eq!(
            percent_change(1000, 250),
            " · 75% smaller than the original (1000 B)"
        );
        assert_eq!(
            percent_change(1000, 1500),
            " · 50% larger than the original (1000 B)"
        );
        assert_eq!(
            percent_change(1000, 1001),
            " · about the same size as the original (1000 B)"
        );
        assert_eq!(percent_change(0, 5), "");
    }

    #[test]
    fn series_and_prefix_cleanup() {
        let dir = std::env::temp_dir().join(format!("beez-job-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for f in [
            "shot_0001.jpg",
            "shot_0002.jpg",
            "shot_0001.png",
            "other.jpg",
            "log-0.log",
            "log-0.log.mbtree",
        ] {
            fs::write(dir.join(f), "x").unwrap();
        }
        let s = series_files(&dir, "shot_%04d.jpg");
        assert_eq!(s.len(), 2);
        remove_temp(&dir.join("log*"));
        assert!(!dir.join("log-0.log").exists());
        assert!(!dir.join("log-0.log.mbtree").exists());
        assert!(dir.join("other.jpg").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
