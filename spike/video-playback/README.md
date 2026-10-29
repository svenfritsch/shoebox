# Video playback spike (not merged)

Step 0 of [docs/design/video-playback.md](../../docs/design/video-playback.md):
ffmpeg writes a fragmented MP4 to a pipe, a small server keeps it in RAM and
streams it, the page plays it with Media Source Extensions. Results are in the
design file, section "Spike results". Nothing here is shoebox code.

Needs Node 18+ and an ffmpeg with libx264 (libx265 too for `make-videos.sh`).
Put test videos in a scratch folder, **never on a photo drive**.

| File | What |
|---|---|
| `make-videos.sh <dir>` | the test videos (HEVC 1080p/4K 10 and 30 min, H.264 `.mkv`, MPEG-2 `.mts`, a Live Photo-like clip) |
| `server.mjs` | the server: `/original`, `/info`, `/playback?start=`, `/stats` |
| `page.html` | the test page: original first, MSE on `error` or `videoWidth == 0` |
| `drive.cjs` | Playwright Chromium: first picture, 15 s play, three seeks, the end |
| `bench-drain.cjs` | no browser: conversion speed, CPU, memory |
| `pause.cjs`, `quota.cjs`, `endseek.cjs` | ffmpeg idles when paused and dies on close; SourceBuffer limit; seek after the end |
| `run-drain.sh`, `run-browser.sh`, `summarize.py` | the matrices and their tables |
| `ffmpeg-strace.sh` | runs ffmpeg under strace (the full trace was taken with `strace -f` on the server) |

```sh
spike/video-playback/make-videos.sh ~/scratch/videos
node spike/video-playback/server.mjs --dir ~/scratch/videos --ffmpeg ./ffmpeg --cmd gop2 --scale box
# open http://127.0.0.1:8765/ and click a video ("auto" = original first)
node spike/video-playback/drive.cjs 8765 hevc-4k-10min.mov auto 3
```

Server options: `--cmd design` (the command as in the design) or `gop2` (plus
a keyframe every 2 s); `--scale box` (longer side at most 1920); `--vt 1`
(macOS: `-hwaccel videotoolbox` and `h264_videotoolbox`, not tried yet);
`--budget-mb`, `--ahead`.

## On a Mac (what this Linux box could not test)

1. Download ffmpeg from evermeet.cx (the release zip) **with curl** into a
   scratch folder; note the URL, the version and whether a checksum or
   signature is published. `xattr -l ffmpeg` (a quarantine mark?),
   `file ffmpeg` (x86_64 or arm64), `./ffmpeg -hide_banner -encoders | grep -E 'libx264|videotoolbox'`,
   `./ffmpeg -hide_banner -hwaccels`. Does it start without a Gatekeeper prompt?
   Do the same once with a copy downloaded in Safari.
2. On Apple Silicon: does that build run (Rosetta asked for?), and how fast
   (step 4)? Same for an arm64 build if there is one.
3. `brew install node` if needed, make the test videos (or copy a few real
   iPhone videos, a `.mts` and an `.mkv` into the scratch folder).
4. Speed, on the old Intel MacBook and on Apple Silicon:
   `node bench-drain.cjs 8765 hevc-4k-10min.mov 3 60` against the server
   started once with `--cmd gop2` and once with `--cmd gop2 --vt 1`.
   "speedX" below 1.0 means playback will stall.
5. Safari, Chrome and Firefox on `http://127.0.0.1:8765/`, "auto" for each
   video: which ones fall back (Safari plays HEVC itself, so try `.mts` and
   `.mkv` there), the first picture, a seek forward and back outside the grey
   buffered bar, the end. In Safari also whether MSE plays at all
   (`window.MediaSource`), and whether `QuotaExceededError` shows in the log
   at the bottom of the page after jumping around a 15 Mbit/s `.mts`.
6. Activity Monitor while a video is paused for two minutes: ffmpeg's CPU
   must drop to 0.

On Windows: the BtbN `win64-gpl` zip (and `winarm64-gpl` on an ARM PC):
`ffmpeg -encoders | findstr libx264`, then steps 4 to 6 in Edge, Chrome
and Firefox (Edge may play HEVC itself with Microsoft's add-on).
