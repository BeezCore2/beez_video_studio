//! One-click FFmpeg install: downloads a static FFmpeg build and keeps only `ffmpeg.exe`
//! and `ffprobe.exe`, in the app's own folder (`%LOCALAPPDATA%\Beez Video Studio\ffmpeg\bin`).
//! No administrator rights are needed and nothing else on the PC is changed.
//!
//! Builds come from BtbN's FFmpeg-Builds on GitHub (GPL build: includes x264, x265,
//! SVT-AV1, libvpx, Opus, LAME and the NVIDIA/Intel/AMD hardware encoders), with gyan.dev's
//! "essentials" build as a fallback.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::ffmpeg;
use super::human_size;
use super::progress::Progress;

/// Download locations for this platform, tried in order.
pub fn sources() -> &'static [&'static str] {
    if cfg!(all(windows, target_arch = "x86_64")) {
        &[
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
            "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip",
        ]
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        &[
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-winarm64-gpl.zip",
        ]
    } else {
        &[]
    }
}

/// Whether the app can install FFmpeg by itself on this platform.
pub fn supported() -> bool {
    !sources().is_empty()
}

/// Download and install FFmpeg. Returns a message for the InfoBar.
pub fn install(progress: &Progress) -> Result<String> {
    if !supported() {
        bail!(
            "Automatic install is not available on this system. Install FFmpeg with your \
             package manager (e.g. `winget install ffmpeg`, `brew install ffmpeg` or \
             `sudo apt install ffmpeg`), then restart the app."
        );
    }
    let dir = ffmpeg::private_dir();
    fs::create_dir_all(&dir).with_context(|| format!("Could not create {}", dir.display()))?;
    let zip_path = dir.join("ffmpeg-download.zip");

    let mut last_error = None;
    for url in sources() {
        match download(url, &zip_path, progress) {
            Ok(()) => {
                last_error = None;
                break;
            }
            Err(e) => {
                let _ = fs::remove_file(&zip_path);
                if super::progress::is_cancelled(&e) {
                    return Err(e);
                }
                last_error = Some(e);
            }
        }
    }
    if let Some(e) = last_error {
        return Err(e.context("FFmpeg could not be downloaded. Check your internet connection."));
    }

    progress.set(0.92, "Unpacking");
    let result = extract(&zip_path, &dir);
    let _ = fs::remove_file(&zip_path);
    result?;

    let tools = ffmpeg::refresh().context("FFmpeg was unpacked but could not be found")?;
    let version = ffmpeg::version(&tools).unwrap_or_default();
    progress.set(1.0, "Done");
    Ok(format!(
        "FFmpeg {version} is installed and ready to use → {}",
        dir.display()
    ))
}

fn download(url: &str, dest: &Path, progress: &Progress) -> Result<()> {
    let host = url.split('/').nth(2).unwrap_or(url);
    progress.set(0.0, format!("Connecting to {host}"));
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .user_agent("BeezVideoStudio")
        .build()
        .into();
    let response = agent
        .get(url)
        .call()
        .with_context(|| format!("Could not download from {host}"))?;
    let total: Option<u64> = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let mut reader = response.into_body().into_reader();

    let mut file = File::create(dest).context("Could not save the download")?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done: u64 = 0;
    let mut last_report = 0;
    loop {
        progress.check()?;
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e).context("The download was interrupted"),
        };
        file.write_all(&buf[..n])
            .context("Could not save the download (is the disk full?)")?;
        done += n as u64;
        if done - last_report >= 512 * 1024 {
            last_report = done;
            match total {
                Some(t) if t > 0 => progress.set(
                    0.9 * done as f32 / t as f32,
                    format!(
                        "Downloading FFmpeg · {} of {}",
                        human_size(done),
                        human_size(t)
                    ),
                ),
                _ => progress.set(0.0, format!("Downloading FFmpeg · {}", human_size(done))),
            }
        }
    }
    file.flush()?;
    if let Some(t) = total
        && done != t
    {
        bail!(
            "The download was incomplete ({} of {})",
            human_size(done),
            human_size(t)
        );
    }
    Ok(())
}

/// Copy `ffmpeg(.exe)` and `ffprobe(.exe)` out of the archive's `bin` folder into `dir`.
fn extract(zip_path: &Path, dir: &Path) -> Result<()> {
    let file = File::open(zip_path).context("Could not open the download")?;
    let mut archive = zip::ZipArchive::new(file).context("The download is not a valid zip file")?;
    let wanted = ["ffmpeg.exe", "ffprobe.exe"];
    let mut found = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let Some(file_name) = name.file_name().map(|f| f.to_string_lossy().to_lowercase()) else {
            continue;
        };
        let in_bin = name
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|p| p.eq_ignore_ascii_case("bin"));
        if !in_bin || !wanted.contains(&file_name.as_str()) || found.contains(&file_name) {
            continue;
        }
        let target = dir.join(&file_name);
        let temp: PathBuf = dir.join(format!("{file_name}.part"));
        {
            let mut out = File::create(&temp)
                .with_context(|| format!("Could not write {}", temp.display()))?;
            io::copy(&mut entry, &mut out).context("Could not unpack FFmpeg")?;
        }
        // Replacing fails if an old copy is running; the old one then stays in use.
        let _ = fs::remove_file(&target);
        fs::rename(&temp, &target)
            .with_context(|| format!("Could not write {}", target.display()))?;
        found.push(file_name);
    }
    if found.len() < wanted.len() {
        bail!("The download did not contain ffmpeg.exe and ffprobe.exe");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn extracts_only_the_tools_from_bin() {
        let dir = std::env::temp_dir().join(format!("beez-dl-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("t.zip");
        {
            let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
            let opts = zip::write::SimpleFileOptions::default();
            for (name, data) in [
                ("ffmpeg-x/bin/ffmpeg.exe", "A"),
                ("ffmpeg-x/bin/ffprobe.exe", "B"),
                ("ffmpeg-x/bin/ffplay.exe", "C"),
                ("ffmpeg-x/doc/ffmpeg.exe", "D"),
            ] {
                w.start_file(name, opts).unwrap();
                w.write_all(data.as_bytes()).unwrap();
            }
            let bytes = w.finish().unwrap().into_inner();
            fs::write(&zip_path, bytes).unwrap();
        }
        extract(&zip_path, &dir).unwrap();
        assert_eq!(fs::read_to_string(dir.join("ffmpeg.exe")).unwrap(), "A");
        assert_eq!(fs::read_to_string(dir.join("ffprobe.exe")).unwrap(), "B");
        assert!(!dir.join("ffplay.exe").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Really downloads FFmpeg into the app folder (for developers):
    /// `cargo test install_ffmpeg -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn install_ffmpeg() {
        let msg = install(&Progress::default()).unwrap();
        println!("{msg}");
        assert!(ffmpeg::tools().is_some());
    }
}
