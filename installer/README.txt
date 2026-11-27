BEEZ VIDEO STUDIO
=================

Compress, convert, trim and edit videos on your own PC. Nothing is uploaded.


GETTING STARTED
---------------

1. Open Beez Video Studio from the Start menu.
2. Click "Import media" (or drag videos onto the window). Your clips appear in
   the media bin on the left; click one to select it.
3. Pick a tool from the icons on the right edge, choose its settings, and press
   its button. Progress, time left and a Cancel button appear in the status bar
   at the bottom. When the work is done, click "Open" or "Show in folder".

The viewer in the middle shows the clip; press Play to preview it (no sound).
The timeline at the bottom shows the whole clip. Drag on it to move the
playhead, and mark a range with "Set In" and "Set Out": Trim, GIF and Save
Frames use that range.

Keyboard: Space play/pause · I / O set In / Out · Left / Right step one frame ·
Home / End jump to In / Out.


TOOLS
-----

Compress          Make videos smaller. Pick a quality level, or an exact size
                  in MB (for example to fit an email or chat limit). The
                  expected size is shown before you start. Can apply to all
                  clips in the media bin at once.
Convert           MP4, MKV, WebM, MOV, AVI, or audio only (MP3, M4A, Opus,
                  FLAC, WAV). When the new format allows it, the video is
                  copied without any quality loss.
GIF / WebP        Turn the marked range into an animated GIF or WebP.
Trim & Cut        Keep the marked range, or cut it out of the video.
Join Clips        Put all clips of the media bin together, top to bottom.
Resize & Rotate   Change the resolution, rotate, flip, crop to 9:16 (Shorts,
                  Reels, TikTok), 1:1 or 4:5, or change the frame rate.
Speed             Speed up or slow down, with natural-sounding voices.
Audio             Extract the sound, remove it, make loudness even, change the
                  volume, or add music.
Save Frames       Save the current frame, a series of images or a contact sheet.
Media Info        See codecs, resolution, frame rate and audio tracks.


TIPS
----

* Smaller files: lower the resolution (720p is plenty for phones), choose
  H.265 or AV1, or limit the frame rate to 30 fps.
* Faster encoding: choose your graphics card under "Encoder" (shown only when
  it works on this PC). The CPU makes smaller files at the same quality.
* "Fast (lossless)" trimming is instant but starts at the nearest keyframe,
  so the clip may begin a moment early. Use "Frame-accurate" for exact cuts.
* Your original files are never changed unless you save over them yourself.


TROUBLESHOOTING
---------------

* The app does not start: a log is saved in
  %LOCALAPPDATA%\Beez Video Studio\startup.log
  Please include it when you report a problem.
* Updates: the app checks for new versions when it starts. You can also click
  "Check for updates" in the status bar at the bottom of the window.


ABOUT FFMPEG
------------

Beez Video Studio uses FFmpeg (https://ffmpeg.org) to read and encode video.
The standalone installer includes a copy of FFmpeg in the "ffmpeg" folder of
the installation. That FFmpeg build is licensed under the GNU General Public
License version 3. Its source code is available from
https://github.com/BtbN/FFmpeg-Builds and https://ffmpeg.org/download.html
