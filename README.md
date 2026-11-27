# 🐝 Beez Video Studio

A fast desktop app for compressing, converting and editing videos, written in Rust. Everything runs on your own computer and no files are uploaded.

The app uses [FFmpeg](https://ffmpeg.org) as its video engine. On first start it offers to download FFmpeg with one click (about 100 MB, into the app's own folder, no administrator rights needed).

---

## Features

The app is laid out like a video editing studio, in one window:

```
┌ Top bar: logo · Import media ························· theme ┐
├ Media bin ┬─────────── Viewer ───────────┬ Inspector ┬ Tool rail ┤
│ clips with│ frame at the playhead, or    │ settings  │ one icon  │
│ thumbnails│ playback · ⏮ ◀ ▶ ▶ ⏭         │ + action  │ per tool  │
├───────────┴──────────────────────────────┴───────────┴───────────┤
│ Timeline: ruler · playhead · V1 filmstrip · A1 waveform · In/Out │
├ Status bar: job progress / result ············ FFmpeg · updates  ┤
```

- **Media bin:** import any number of clips (button, or drop files on the window). Click a clip to select it; right-click to reorder, show in folder or remove.
- **Viewer:** shows the frame at the playhead. **Play** previews the clip in real time (video only, without sound). Rotation, flips and crop from Resize & Rotate are previewed live.
- **Timeline:** drag on the ruler or tracks to move the playhead. Mark a range with **Set In / Set Out** (or **I** / **O**), drag the **[ ]** handles, or type exact times. Trim, GIF and Save Frames work on this range.
- **Inspector:** the settings of the tool chosen in the rail on the right, with its action button. Compress and Convert can apply to the selected clip or to all clips in the bin.
- **Status bar:** progress, time left and **Cancel** while a job runs; afterwards **Open**, **Show in folder** and **Add to media** (import the result).

Keyboard: **Space** play/pause · **I** / **O** set In / Out · **← / →** previous / next frame · **Home** / **End** jump to In / Out.

The look follows Windows 11 (Segoe UI, Fluent icons) on warm graphite, with a honey accent taken from the Beez honeycomb badge. Honey marks actions, selections and the timeline range; the playhead is cream. It starts in dark mode, and the sun/moon button switches to light mode.

| Group | Tool | What it does |
|---|---|---|
| Compress & convert | **Compress** | Quality presets (High, Balanced, Small, Tiny) or an exact **target size** in MB; H.264, H.265, AV1 or VP9; CPU or GPU encoding; resolution and frame-rate limits; one file or a whole batch |
| | **Convert** | MP4, MKV, WebM, MOV, AVI, or audio only (MP3, M4A, Opus, FLAC, WAV). Copies streams instead of re-encoding whenever the new format allows it; batch capable |
| | **GIF / WebP** | Sharp, small animated GIF (optimised palette) or animated WebP from the timeline's In–Out range |
| Edit | **Trim & Cut** | Export the timeline's In–Out range (lossless fast cut, or frame-accurate), or cut it out |
| | **Join Clips** | Lossless join when the clips match; otherwise they are scaled, letterboxed and re-encoded to match the first clip |
| | **Resize & Rotate** | Resolution, rotate, flip, crop to 9:16 / 1:1 / 4:5 / 16:9…, frame rate, with a live preview. Rotation alone is saved instantly without re-encoding |
| | **Speed** | 0.1× to 16×, keeping voices natural (or not) |
| Audio & more | **Audio** | Extract audio, remove audio, normalize loudness (EBU R128), change volume, replace the audio or mix in music |
| | **Save Frames** | The frame at the playhead, a frame every N seconds, N evenly spaced frames, or a contact sheet |
| | **Media Info** | Codecs, resolution, rotation, frame rate, bitrates, audio and subtitle tracks |

Other conveniences:

- Drag files onto the window to import them into the media bin.
- Open files from the command line: `beez_video_studio.exe a.mp4 b.mov`
- Work runs in the background with a progress bar, speed and time left. **Cancel** stops FFmpeg at once.
- Output is written to a temporary `*.beez-part.*` file and renamed only when finished, so a cancelled or failed job never leaves a broken file.
- When a task finishes, the status bar shows how much smaller the result is.

---

## How it works

All media work is done by FFmpeg, run as a child process (no FFmpeg libraries are linked into the app). The app builds the command lines; the choices below are where the quality comes from.

### Compression

- **Quality mode (CRF).** Constant rate factor gives every scene the bits it needs for the chosen quality, which is the most efficient way to encode. Quality is expressed on one scale (x264 CRF) and translated per encoder so that a level looks the same whichever codec you choose: x265 ≈ +4, SVT-AV1 / VP9 ≈ ×1.45 on their 0–63 scale, and NVENC, Quick Sync and AMF use their own constant-quality modes.
- **Target size mode.** The video bitrate is computed from the size budget:
  `video bits/s = target bytes × 8 × 0.98 ÷ duration − audio bits/s` (2 % is reserved for container overhead). The video is then encoded in **two passes** (H.264, H.265, VP9): pass 1 analyses the whole video, pass 2 spends the budget where it is needed, so the result lands close to the target.
- **Automatic resolution.** When the budget is too small for the source resolution, the app picks the largest standard size (2160p … 240p) that still gets at least a minimum number of **bits per pixel per frame** for the codec (H.264 0.045, VP9 0.035, H.265 0.03, AV1 0.025). A clean smaller picture looks better than a blocky full-size one.
- **Size preview.** Before you start, the Compress page shows each video's expected size. In quality mode it encodes a few 3-second samples spread over the video with the exact settings (the whole video if it is 15 s or shorter), and extrapolates bytes per second to the full length plus the audio. It refreshes shortly after you change a setting.
- **Scaling** uses the Lanczos filter. Sizes limit the *short* side, so "720p" also works for portrait phone videos, and they are never upscaled.
- **GPU encoding** is offered only for encoders that pass a real test encode on this PC (the FFmpeg build contains NVENC, Quick Sync and AMF even on PCs without such a GPU).
- MP4 files get `+faststart` (instant playback and web streaming) and H.265 gets the `hvc1` tag that Apple devices need. 10-bit sources stay 10-bit with H.265, AV1 and VP9.

### Smart remux

Changing the container rarely needs re-encoding. Each container has a list of codecs it can hold (for example MP4: H.264, H.265, AV1, VP9; WebM: VP8, VP9, AV1 with Opus or Vorbis). Streams the target accepts are **copied**: no quality loss, and it takes seconds instead of minutes. Only the other streams are re-encoded. Text subtitles are carried over where the container supports them (converted to `mov_text` for MP4 or WebVTT for WebM).

### GIF

GIF allows only 256 colours, so the palette decides the quality. The app generates a palette from the clip itself (`palettegen` with `stats_mode=diff`, which favours the colours of moving parts), applies it with Sierra-2-4A error-diffusion dithering, and re-encodes only the changed rectangle of each frame (`diff_mode=rectangle`) to keep files small. Animated WebP is usually 3–5× smaller still.

### Editing

- **Fast trim** copies the streams with fast seeking, so the cut snaps to the keyframe before the start. **Frame-accurate** trim re-encodes at visually lossless quality (CRF 18, same codec family as the source).
- **Cutting a section out** trims the parts before and after and joins them with the `concat` filter.
- **Join** checks codec, size, pixel format, frame rate and audio format. If everything matches, the concat demuxer joins the files without re-encoding. Otherwise every clip is scaled into the first clip's frame (letterboxed, never stretched), converted to one frame rate and audio format, and silence is generated for clips without sound.
- **Lossless rotation** rewrites the video's display-rotation metadata (`-display_rotation`), combined with any rotation the file already has, and copies the streams.
- **Speed** changes timestamps with `setpts`. Audio is time-stretched with a chain of `atempo` steps of 0.5–2.0 each (pitch preserved), or resampled for the "chipmunk" effect.

### Audio

- **Loudness normalization** follows EBU R128 in two passes. Pass 1 measures integrated loudness, true peak, loudness range and threshold. Pass 2 feeds the measurements back to `loudnorm` with `linear=true`, so the whole file gets one even gain change instead of "pumping". Targets: −14 LUFS (streaming), −16 LUFS (podcasts), −23 LUFS (broadcast).
- **Volume boosts** pass through a limiter so they don't clip.
- **Add music** either replaces the audio or mixes the music under it at a chosen level, looping the music if it is shorter than the video.

---

## 1. Requirements

| | Windows | macOS | Linux |
|---|---|---|---|
| Rust | [rustup](https://rustup.rs) (stable, 1.85 or newer for edition 2024) | same | same |
| C/C++ toolchain | Visual Studio Build Tools with the **"Desktop development with C++"** workload | Xcode Command Line Tools | `build-essential` |
| FFmpeg at runtime | one-click install in the app | `brew install ffmpeg` | `sudo apt install ffmpeg` |

The app looks for `ffmpeg` and `ffprobe` in this order: the folder in `BEEZVIDEO_FFMPEG_DIR`, next to the app (also `ffmpeg\` and `ffmpeg\bin\`), a folder chosen with **Locate…** (shown when FFmpeg is missing), the app's own copy in `%LOCALAPPDATA%\Beez Video Studio\ffmpeg\bin`, and the `PATH`.

The one-click install downloads the GPL build from [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds) (with [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) as a fallback) and keeps only `ffmpeg.exe` and `ffprobe.exe`.

---

## 2. Run the app (development)

```sh
cd beez_video_studio
cargo run                 # debug build
cargo run --release       # optimized build
cargo run --release -- "C:\path\clip.mp4"   # open a file on startup
```

---

## 3. Test & check

```sh
cargo test                # FFmpeg command builders, probing, progress parsing, job handling…
cargo test --release e2e -- --ignored --test-threads=1 --nocapture
                          # every tool end-to-end against a real FFmpeg (about a minute)
cargo clippy              # lints
cargo fmt                 # format the code
```

The tests check the generated FFmpeg arguments and parsing, so they do not need FFmpeg. To download FFmpeg into the app folder from the command line (handy on a fresh development PC):

```sh
cargo test install_ffmpeg -- --ignored --nocapture
```

---

## 4. Build the app

```sh
cargo build --release
```

The result is a single executable, `target\release\beez_video_studio.exe`, with no runtime or DLLs to ship (the C runtime is linked statically, see [`.cargo/config.toml`](.cargo/config.toml)). Release builds hide the console window.

---

## 5. Installer & icon

- **Icon:** [`assets/icon.ico`](assets/icon.ico) is embedded into the `.exe` by [`build.rs`](build.rs). To redesign it, edit [`examples/make_icon.rs`](examples/make_icon.rs) and run `cargo run --example make_icon`.
- **Installer:** [`installer/beezvideo.iss`](installer/beezvideo.iss) is an [Inno Setup](https://jrsoftware.org/isinfo.php) script. It installs per user into `%LOCALAPPDATA%\Programs\Beez Video Studio` (no administrator rights, and self-update keeps working), adds a Start menu shortcut, optionally a Desktop shortcut and **Open with** entries for MP4, MOV, MKV, WebM and AVI, installs the user guide [`installer/README.txt`](installer/README.txt) (offered at the end of setup), and removes everything on uninstall.

There are two kinds of installer, built from the same script:

| Installer | Size | FFmpeg |
|---|---|---|
| `Beez-Video-Studio-Setup-vX.Y.Z.exe` | ~7 MB | downloaded with one click on first start |
| `Beez-Video-Studio-Setup-vX.Y.Z-Standalone.exe` | ~100 MB | included: works offline right away |

The script builds the **standalone** installer automatically when `installer\ffmpeg\` contains `ffmpeg.exe` and `ffprobe.exe` (that folder is git-ignored). They are installed into `{app}\ffmpeg\bin`, where the app looks first.

```powershell
cargo build --release
$iscc = "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"

# Small installer (no installer\ffmpeg folder):
& $iscc /DAppVersion=0.1.0 installer\beezvideo.iss
# → dist\Beez-Video-Studio-Setup-v0.1.0.exe

# Standalone installer: copy FFmpeg in first, for example the copy the app downloaded:
New-Item -ItemType Directory -Force installer\ffmpeg
Copy-Item "$env:LOCALAPPDATA\Beez Video Studio\ffmpeg\bin\ff*.exe" installer\ffmpeg\
& $iscc /DAppVersion=0.1.0 installer\beezvideo.iss
# → dist\Beez-Video-Studio-Setup-v0.1.0-Standalone.exe
```

> **FFmpeg licence:** the bundled FFmpeg build is GPL v3. When you distribute the standalone installer, its README.txt points users to the FFmpeg source code, as the GPL requires.

---

## 6. Create a release

Releases are built and published by GitHub Actions ([.github/workflows/release.yml](.github/workflows/release.yml)) when a version tag is pushed:

1. Bump `version` in `Cargo.toml` and check that `cargo test` passes.
2. Commit, then `git tag v0.2.0` and `git push origin master --tags`. The tag **must** be `v` + the version in `Cargo.toml`.
3. The workflow runs the tests, builds the app, zips it as `Beez-Video-Studio-v0.2.0-x86_64-pc-windows-msvc.zip`, builds the small installer, downloads FFmpeg and builds the standalone installer, and creates the GitHub Release with all three files.

---

## 7. Auto-update

The app updates itself from GitHub Releases ([src/updater.rs](src/updater.rs)). On start-up it checks the latest release in the background; when a newer version exists, a dialog offers **Update now**, which replaces the running `.exe` and offers a restart. **Check for updates** is in the status bar.

- The repository is set in `src/updater.rs` (`REPO_OWNER` / `REPO_NAME`) and **must be public**.
- Set `BEEZVIDEO_NO_UPDATE_CHECK=1` to turn off the start-up check while developing.

---

## Project structure

```
beez_video_studio/
├── build.rs                embeds the icon and version info into the .exe
├── assets/                 icon.ico / icon.png
├── examples/make_icon.rs   generates the icon
├── installer/beezvideo.iss Inno Setup installer script
├── .github/workflows/      release workflow
└── src/
    ├── main.rs             window setup, DirectX/Vulkan → OpenGL fallback
    ├── startup.rs          start-up log, crash dialog, graphics marker
    ├── updater.rs          self-update from GitHub Releases
    ├── ui/
    │   ├── mod.rs          editor workspace: panels, shortcuts, jobs, status bar
    │   ├── bin.rs          media bin; each clip's playhead and In/Out range
    │   ├── viewer.rs       viewer and transport controls
    │   ├── player.rs       real-time preview playback (FFmpeg → raw frames)
    │   ├── timeline.rs     ruler, playhead, filmstrip and waveform tracks, In/Out
    │   ├── theme.rs        Windows 11 look: fonts, Fluent icons, colours, cards
    │   ├── media.rs        background frame and waveform caches, preview painting
    │   ├── update.rs       update dialog
    │   └── tools/          one file per inspector tool
    └── video/
        ├── ffmpeg.rs       find FFmpeg, run it with progress/cancel, encoder detection
        ├── probe.rs        ffprobe → media details; time parsing and formatting
        ├── download.rs     one-click FFmpeg install
        ├── codec.rs        codecs, quality mapping, containers, geometry
        ├── job.rs          multi-pass jobs, temp output, cleanup, result messages
        ├── compress.rs     CRF / target-size compression
        ├── convert.rs      smart remux, GIF and WebP
        ├── edit.rs         trim, cut out, join, resize/rotate/crop, speed
        ├── audio.rs        extract, remove, volume, loudness, music
        └── frames.rs       stills, series, contact sheets
```

### Main dependencies

| Crate | Used for |
|---|---|
| [`eframe` / `egui`](https://github.com/emilk/egui) | Native GUI |
| [`rfd`](https://github.com/PolyMeilex/rfd) | Native open, save and folder dialogs |
| [`serde_json`](https://github.com/serde-rs/json) | Reading ffprobe and loudness analysis output |
| [`ureq`](https://github.com/algesten/ureq) / [`zip`](https://github.com/zip-rs/zip2) | Downloading and unpacking FFmpeg |
| [`image`](https://github.com/image-rs/image) | Decoding preview frames |
| [`self_update`](https://github.com/jaemk/self_update) | Checking GitHub Releases and replacing the executable |
| [`winresource`](https://github.com/BenjaminRi/winresource) | Embedding the icon into the .exe |
| [`anyhow`](https://github.com/dtolnay/anyhow) | Error handling |

---

## Windows compatibility

Runs on 64-bit **Windows 10 and 11** (Windows 11 on ARM through x64 emulation). The start-up log is at `%LOCALAPPDATA%\Beez Video Studio\startup.log`. If DirectX/Vulkan is unavailable (old graphics cards, virtual machines, Remote Desktop) the app restarts itself with OpenGL; set `BEEZVIDEO_RENDERER=gl` to force OpenGL.

## Known limitations

- FFmpeg must be installed (one click) before any tool can read a file.
- Fast trim cuts on keyframes, so the clip may start slightly before the chosen time; use frame-accurate mode for exact cuts.
- Converting keeps the first video stream, all audio tracks and text subtitles. Image-based subtitles (PGS, DVD) are kept only in MKV. Data streams such as GoPro telemetry are not kept.
- Joining re-encodes when clips differ in codec, size, frame rate or audio format.
- Lossless rotation needs FFmpeg 7 or newer (the one-click install provides it).
