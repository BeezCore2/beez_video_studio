//! Video engine. Everything that touches media goes through FFmpeg, driven as a child
//! process; this module builds its command lines, runs it with progress and cancellation,
//! and reads media details with ffprobe.

pub mod audio;
pub mod codec;
pub mod compress;
pub mod convert;
pub mod download;
pub mod edit;
pub mod ffmpeg;
pub mod frames;
pub mod job;
pub mod probe;
pub mod progress;

/// Human-readable file size, e.g. "12.4 MB".
pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} GB", b / 1024.0 / 1024.0 / 1024.0)
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / 1024.0 / 1024.0)
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(5 * 1024 * 1024 + 300 * 1024), "5.3 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.00 GB");
    }
}
