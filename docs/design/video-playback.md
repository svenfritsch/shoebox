# Playing every video in the browser (planned)

October 2026, not built. Linked from [open.md](../open.md). When it is built:
the entry "Decoders" in [decisions.md](../decisions.md) loses "No
transcoding", a rule in `CLAUDE.md` (Video playback, below), the guide in both
languages, and this file is deleted.

## Problem

Today `GET /api/lib/<id>/file/<file>/original` sends the file as it is and the
lightbox sets `<video src>` (`core/web/app.js`, `serve.rs: original`). A
browser plays codecs, not file endings: HEVC (iPhones since 2017, many
cameras and drones) does not play in Chrome and Firefox on most systems, and
the old camcorder formats (`.avi`, `.mkv`, `.mts`, `.m2ts`, `.mpg`, `.3gp`
with MPEG-2, DivX, AC-3) play in no browser. Safari on a Mac plays most. Today
the person gets a notice (`lb.video_unplayable`) and "Show in Finder".

Other photo apps convert to H.264 at upload and keep the copy. We cannot:

- **No copy on the drive** (nothing next to the photos, not under `.shoebox/`).
- **No trace on the computer that runs the server**: no temporary file, no
  cache folder, no log with file names. What is converted exists in memory
  only and is gone when the next video starts or the server stops.

This replaces the open item "A playable copy of an old video … kept under
`.shoebox/playback/`" (removed from open.md with this file).

## Options that were weighed

| | Idea | Verdict |
|---|---|---|
| A | Live stream from ffmpeg's pipe, nothing kept | Seeking restarts ffmpeg; alone it is a poor player. Used as the fall-back when the RAM budget is full |
| B | Convert the whole video, keep it in RAM, serve with Range | Simple, but the person waits, and a 30 minute 4K video is gigabytes. Kept as the idea, made bounded (below) |
| C | Segments made on demand (HLS), small RAM window | Best seeking, most code (segmenter, hls.js under `core/web/vendor/` with licence). Only if the design below is not enough in daily use |
| D | Remux only (`-c copy`) | Free, but helps only H.264 in a foreign container. A step inside the design, not a solution |
| E | Decode in the browser (WebAssembly, WebCodecs) | HEVC in WebAssembly is too slow above 1080p; WebCodecs works only where the browser plays HEVC anyway |
| F | A file on disk, deleted afterwards | Excluded by the rule above |

## Design

**The browser decides.** The server cannot know what this browser plays
(Safari: HEVC yes; Chrome on Linux: no). So the lightbox first sets
`<video src=…/original>` as today. Only if the video fires `error`, or has
loaded its metadata with `videoWidth == 0` (sound but no picture), it asks for
the converted stream. Nothing changes for a video that plays, and no ffmpeg is
needed for it. A button "Play converted" in the info panel (shown when ffmpeg
is there) covers the case the browser cannot detect (picture, but no sound
from AC-3).

**Converted stream.** `GET /api/lib/<id>/file/<file>/playback?start=<seconds>`:

- Runs ffmpeg (`media::find_ffmpeg`, the one next to the program) with the
  original as input and a **pipe as output**: `-nostdin -ss <start> -i <src>
  -map 0:v:0 -map 0:a:0? -vf scale='min(1920,iw)':-2 -c:v libx264 -preset
  veryfast -crf 23 -pix_fmt yuv420p -c:a aac -movflags
  frag_keyframe+empty_moov+default_base_moof -f mp4 pipe:1`. On macOS
  `h264_videotoolbox` is tried first (much faster, and no GPL encoder is
  needed in the build); `libx264` only if the ffmpeg next to the program has
  it. No file, no `-report`, stderr is read into memory and kept for the error
  message only.
- A video that is already H.264/AAC in a container the browser does not know
  (`.mkv`, `.avi`, `.ts`, `.mov`) is remuxed with `-c copy` instead (detected
  from ffmpeg's `-i` stream lines). A step of its own, built second.
- Scaled down to 1080p at most: it is a viewing copy, "Download" still gives
  the original. One preview size, no setting.
- The output is cut at keyframes into fragments; the reader task appends them to
  **one in-memory buffer owned by the app** (`App::playback`): the video id,
  the start, the fragments with their times, a byte budget (default 768 MB,
  a constant). One buffer for the whole server: starting another video (or
  another start time that is not inside the buffer) stops ffmpeg, drops the
  buffer and starts again. Over the budget the oldest fragments are dropped;
  a seek back there starts a new conversion at that time.
- The reply is a stream of the fragments from the buffer, so it begins after
  the first fragment (about a second) and is not blocked by the rest. ffmpeg
  is paused (its pipe is not read) when the buffer is more than 60 s ahead of
  what the page has asked for, so a person who stops watching does not keep a
  CPU busy; it is killed when the connection is closed and no new request
  comes within a few seconds.
- Only one conversion runs at a time (the library's ffmpeg slot); the request must come from this computer (`is_local`), otherwise `403`. A second
  request while one runs gets `503` with the reason, shown
  in the lightbox as a notice.
- The run is under the guard as every other read of an original is
  (`fingerprint::read_unchanged`): the file must match the index (size, mtime)
  before, and size, mtime and created time must be the same after ffmpeg ends
  or is stopped, otherwise the run is an error and the page shows it. No
  hash per play: the full hash is what the Rust tests and `shoebox probe`
  compare, as for every other reader (see "Tests").

**Player for the converted stream.** A fragmented MP4 has no index, so a plain
`<video src>` cannot seek in it. The lightbox uses Media Source Extensions for
the converted stream only: a `MediaSource`, one `SourceBuffer` fed from
`playback?start=…`, the duration set from `GET …/playback/info` (the duration
ffmpeg reads from the original). The ordinary `<video controls>` stays (look,
keys, full screen). On `seeking` outside the buffered range the page asks
`playback?start=<t>` again and appends the new stream. In Safari without MSE
(iPhone before iOS 17.1) there is no converted playback; the notice stays.

**What is not done.** No second size, no audio track choice, no subtitles, no
cut of a video, no keeping the result for the next time (that is the point),
no conversion for videos in a **read-only library** that cannot be read (the
rules for those do not change: reading is allowed, nothing is written).

## Getting ffmpeg

Decided: ffmpeg is an **add-on that is downloaded, not shipped**, like the
recognizer's runtime (`recognizer/install.sh`, `install.ps1`; "Add-ons are
decided by the files present" in [decisions.md](../decisions.md)). The release
archive stays small and carries no GPL binary, so the old "never bundled" in
the README and decisions.md still holds for the archive.

- `Start shoebox.*` finds
  out the system and processor (macOS Intel or Apple Silicon, Windows x64,
  Linux x64 or arm64) and downloads the one build listed for it into
  `program/ffmpeg`. The download is checked against a SHA-256 kept in the
  repository, never run before it matches, and a failed or interrupted one
  leaves nothing behind. Needs the network once; without it the notice stays.
- A `ffmpeg` the person already put there (as the README describes) or one on
  PATH is used as before; the installer does not replace it.
- shoebox only starts it as a program of its own (no linking), so its GPL
  does not reach ours. `THIRD-PARTY-LICENSES.txt` gets an entry: ffmpeg (GPL),
  not part of the release, downloaded at the person's request from the
  source named there (`scripts/third-party-licenses.py` and its check know
  about it). The guide names the download and the source. Not legal advice;
  look at it again before giving shoebox to others (open.md, section 6).
- macOS: a file fetched by the program itself carries no quarantine mark;
  verify on a real Mac, and that the evermeet build runs on Apple Silicon
  (Rosetta) or choose an arm64 one.

## Rules to add to CLAUDE.md when built

Video playback (`playback.rs`): the converted stream is made by an ffmpeg
that reads the original under the guard and writes only to a pipe; its
result lives in memory only, for one video at a time; nothing about it is
written to a drive, to the program folder or anywhere on the computer (no
temporary file, no log); it answers only to a request for a video in the
index; the original is never replaced.

## Tests (by the rules in CLAUDE.md)

- **Rust API tests** (`core/tests`, with a fake `ffmpeg` script like the fake
  recognizer, and with the real one when `SHOEBOX_FIXTURES` is set):
  `playback` answers 200 with a fragmented MP4 (first box `ftyp`, then `moov`,
  then `moof`); `?start=` is passed on; an unknown id is 404; a broken `start`
  (negative, text, larger than the video) is refused with 400; no ffmpeg next to
  the program gives a clear 404/501 reply, not a hang; a second request while
  one runs gets 503; a request from another device (the peer address is
  not local) gets 403; `playback/info` answers the duration; the buffer is
  dropped when another video is asked for (a counter on `App::playback` is
  visible in `info`); the original has the same size, mtime and hash after
  the run and after a request that is closed early; **no file is created**
  anywhere under the temp directory, the program folder or the library during
  a run (directory listings before and after); a video in a read-only library
  plays and the library root is unchanged.
- **Page against the fake API** (`faces-mocked.mjs`, fake clock; `fake-api.mjs`
  answers `playback` and `playback/info` as the server does): the converted
  stream is asked for only after `error` or `videoWidth == 0`, never for a
  video that plays; a seek outside the buffer asks `?start=` again; leaving
  the lightbox or stepping to the next photo aborts the request; the 503
  and the 403 notice are shown; the motion of a Live Photo falls back the same way; the button "Play converted" only when the server says
  ffmpeg exists.
- **Real server** (`keyboard.mjs` or a new short file): one HEVC fixture is
  played converted in Chromium, `currentTime` advances, and a seek works. Only
  this layer shows that page and the real ffmpeg output agree.
- Not tested: ffmpeg speed (a `scripts/bench-playback.py` on a 4K HEVC file:
  time to first fragment, CPU, peak memory against the budget).
- Each test is proved by breaking the code on purpose, and run several times.

## To build, in this order

0. **Spike** (a branch, not merged): ffmpeg to a pipe, one fragmented MP4 in
   MSE in Chromium, Firefox and Safari, with a seek. Decides whether MSE as
   above is enough or option C (segments) is needed. Measure time to first
   fragment and memory for 1080p and 4K HEVC. Also try the ffmpeg builds of
   third parties (see "Decided") on each system.
1. The ffmpeg add-on installer (`install-ffmpeg.sh` / `.ps1` next to the
   recognizer's, list of URLs and checksums, called from the start file only,
   no button in the Control Panel), the licence entry, a test that the
   checksums in the list and in the installer agree (like
   `core/tests/recognize.rs`). The spike has checked the builds first.
2. `playback.rs`: the buffer, the ffmpeg run under the guard, `playback` and
   `playback/info`; Rust tests.
3. Lightbox: the error fallback, MSE player, notice, button; texts in
   `en.json` and `de.json`; mocked tests; the common classes only (frontend
   skill).
4. Remux (`-c copy`) for H.264 in foreign containers.
5. The guide (both languages: when a video is converted, that nothing is
   saved, that it needs `ffmpeg` next to the program; "Where does ffmpeg
   come from" already exists for posters), the rule in CLAUDE.md, the entry
   in decisions.md (the download of ffmpeg, no transcoding any more), the
   README section "Video previews", and this file deleted.

## Decided

- **Source of the download: builds of third parties** (evermeet.cx for
  macOS, johnvansickle.com for Linux, BtbN or gyan.dev for Windows), one
  URL, version and SHA-256 per system in one list. The spike checks each
  build: `libx264` (or the hardware encoder), runs on Intel and Apple
  Silicon, URL stays put, quarantine behaviour on a Mac. A copy of our own is
  only a fall-back if a link dies.
- **No button in the Control Panel**: the start file does the download.
- **Guard**: the same as every reader of originals (`read_unchanged`, see
  Design); no rule in CLAUDE.md changes. The full hash of the original is
  compared in the Rust test only.
- **Only the computer that runs the server** may start a conversion, like
  `reveal_file` and export (`is_local`): a request from another device gets
  `403`, and the lightbox shows the notice that is shown today. Opening the
  setting to the LAN later is a change of its own.
- **Live Photo videos use the same path.** The fall-back works on any
  `<video>` of the lightbox, so it also covers the motion of a Live Photo
  (`$('lb-live')` in `app.js` has no `error` handling today and gets it), in
  case a future browser stops playing them. Where the browser plays them
  nothing changes.

## Spike results

October 2026, branch `claude/video-playback-spike` (not merged), code and raw
numbers in `spike/video-playback/`. **Recommendation: MSE with changes** (below).
Segments (option C) are not needed: a seek outside the buffer costs a restart
of ffmpeg plus decoding from the source's keyframe, and segments made on
demand would pay the same.

**Setup.** Cloud VM, 4 vCPU Intel Xeon 2.1 GHz, Linux. ffmpeg: BtbN
`ffmpeg-n9.0-latest-linux64-gpl-9.0` (n9.0.2, 2026-10-28; SHA-256 checked against
its `checksums.sha256`). Browser: Playwright's headless Chromium 153. It has
H.264/AAC (`MediaSource.isTypeSupported` true, frames really decoded: 450 in
15 s) and no HEVC, like Chrome on Linux, so the fall-back ran for real.
Synthetic videos (testsrc2 + noise, `make-videos.sh`): HEVC 1080p 6 Mbit/s and 4K
Main10 22 Mbit/s (10 and 30 min, keyframe every 2 s), H.264 `.mkv`, MPEG-2
1080i + AC-3 `.mts` (starts at 1.428 s), a 3 s HEVC Live Photo-like clip
rotated 90°. Every case ran 3 times (the 30 min one twice). Median, range in brackets.

| Video | Command | First fragment (s) | Speed (× real time) | ffmpeg CPU (cores) | ffmpeg RSS (MB) | First picture in page (s) | Seek outside buffer (s) | Seek inside buffer (s) |
|---|---|---|---|---|---|---|---|---|
| HEVC 1080p | design | 3.5 (3.3-4.5) | 2.21 | 3.4 | 405 | 4.58 (4.54-5.30) | 3.74 (3.62-4.08) | 1.03 |
| HEVC 1080p | gop2 | 1.1 (1.0-1.8) | 2.39 | 3.0 | 401 | 2.38 (2.16-2.45) | 1.50 (1.44-1.55) | 0.36 |
| HEVC 4K 10-bit | design | 7.1 (6.4-7.2) | 1.28 | 3.8 | 716 | 7.34 (7.34-7.79) | 7.5 (7.30-7.70) | 0.69 |
| HEVC 4K 10-bit | gop2 | 2.3 (2.2-2.7) | 1.39 | 3.5 | 713 | 3.07 (3.01-3.08) | 3.4 (2.98-3.64) | 0.26 |
| HEVC 4K, 30 min | gop2 | 2.7 (full run) | 1.54 (full run) | 3.75 | 717 | 3.25 (3.12-3.38) | 3.4 (3.37-3.47) | 0.26 |
| H.264 `.mkv` | gop2 | 1.2 | 2.62 | 3.1 | 409 | 0.35 as the original; 2.31 forced through MSE | 1.65 forward, 1.96 back (MSE) | 0.28 (MSE) |
| MPEG-2 `.mts` | gop2 | 1.1 (1.0-1.2) | 2.21 | 3.2 | 364 | 2.62 (2.25-2.63) | 1.42 (1.37-1.55) | 0.54 |
| Live Photo clip | gop2 | 1.0 | - | 2.8 | 324 | 1.72 (0.30 when still in the buffer) | - | - |

"design" is the command as written above; "gop2" is the same plus
`-force_key_frames expr:gte(t,n_forced*2)`. "First fragment" and "Speed" were
measured without a browser (the stream read as fast as it came, 60 s of video).
"Seek" is the time from setting `currentTime` until the first frame past the
target. The page times include `/info` and the 0.3 s until the original fails.

What worked, in every run:

- **Fall-back trigger.** HEVC (`.mov`, the Live Photo too) gives
  `loadedmetadata` with `videoWidth 0`. Chromium plays its sound meanwhile.
  `.mts` gives `error` 4 (`DEMUXER_ERROR_COULD_NOT_OPEN`). The H.264 `.mkv`
  plays as the original in Chromium.
- **Playback** keeps time (15.0 s in 15 s, 0-2 dropped frames in the MSE runs;
  one run of the `.mkv` played as the original dropped 96, not looked into).
  `ended` fires at the end with the duration from `ffmpeg -i` (20 of 20 MSE runs). After
  `endOfStream()` a seek back opens the MediaSource again and plays (3 of 3).
  The Live Photo comes out upright (1080×1440, ffmpeg's autorotate).
- **Pausing.** Once the stream is far enough ahead, ffmpeg's CPU drops to 0
  and stays there; playing again goes on. When the page closes, ffmpeg is
  killed after the 5 s linger.
- **Memory.** The buffer is small next to ffmpeg itself: 2.7-7.6 Mbit/s here
  (synthetic, real footage is more), 577 MB for the full 30 min 4K
  video. The budget holds (64 MB test: 85 fragments dropped, never more than
  63.7 MB). A start inside the buffer is served from RAM in about 13 ms. Chromium's
  SourceBuffer never threw `QuotaExceededError`: at 15 Mbit/s it kept about 86 s
  and evicted older data by itself.
- **Nothing written.** `strace -f` over the server and every ffmpeg during two
  runs: no `O_CREAT`, `mkdir`, `unlink` or `rename`; the videos opened
  `O_RDONLY` only; the only write-capable open was `/dev/null`. Size, mtime,
  birth time and SHA-256 of all videos were the same after all runs. stderr
  stayed empty with `-loglevel error`.

Changes the design needs:

1. **A keyframe every 2 s** (`-force_key_frames expr:gte(t,n_forced*2)`,
   independent of the frame rate). `frag_keyframe` alone follows libx264's
   default GOP (250 frames): 4-7 s fragments, and every start and seek waits for
   one. With it: the first picture after 2.4 s (1080p) and 3.1 s (4K), instead of
   4.6 s and 7.3 s. Same output size, slightly faster.
2. **Scale on the longer side.** `scale='min(1920,iw)':-2` makes a portrait
   4K phone video 1920×3414 (H.264 level 5.1). Use
   `scale=w='if(gte(iw,ih),min(1920,iw),-2)':h='if(gte(iw,ih),-2,min(1920,ih))'`
   (1080×1920).
3. **Timestamps.** With `-ss <t>` the output starts at 0, and a reply served
   from the buffer starts at the buffer's start, not at the request's. The reply
   says where its times start (`X-Start` in the spike); the page sets
   `SourceBuffer.timestampOffset` to it after `abort()`. On an ended MediaSource,
   `abort()` throws, so set `timestampOffset` first, which opens it again.
   (`-output_ts_offset` would make the times absolute; not tried.)
4. **"60 s ahead" counts what was sent, not what was played.** The page stopped
   reading at 60 s ahead and the server at 60 s past what it had sent; 15-23 MB
   were in the socket and fetch buffers between them. ffmpeg stopped 145 s (1080p)
   and 163 s (4K) ahead of the paused picture, after 55-105 s at 3.5-3.9 of the 4 cores.
   Use 30 s on each side, or let the page report its position.
5. **Pause the original at once** when falling back (its sound plays with
   `videoWidth 0`).
6. **Budget.** ffmpeg needs 400 MB (1080p HEVC) to 720 MB (4K 10-bit, of which
   about 300 MB is x264) on top of the buffer. 768 MB of buffer is more than
   needed: the browser keeps its own ~150 MB. 256 MB covers several minutes of
   seeking back.
7. **Speed is the risk, not MSE.** 4K HEVC 10-bit → 1080p ran at only 1.3-1.5×
   real time on 4 cores, and the HEVC decode alone took 60 % of it (2.4×,
   3.6 cores). A 2-core Intel MacBook will be below 1×, so on macOS use
   `-hwaccel videotoolbox` for the decode as well as `h264_videotoolbox` (the
   spike server has `--vt 1`, not tried). Interlaced `.mts` needs `yadif`
   (not added in the spike).

**ffmpeg builds of third parties.** This VM's network policy refused
johnvansickle.com, evermeet.cx, gyan.dev and osxexperts.net (also through the
web fetcher), so those four are **not checked**. GitHub was reachable:

| Build | libx264 | Systems | URL stays put | SHA-256 |
|---|---|---|---|---|
| BtbN `*-gpl` (`*-lgpl` has no x264) | yes (checked: `-encoders` lists libx264; build script enables it for gpl only) | linux64, linuxarm64, win64, winarm64; no macOS | `latest` is deleted and remade every day (its files repacked, other hashes): pin a dated `autobuild-YYYY-MM-DD-HH-MM` tag; the last of each month is kept 24 months, the last 14 daily ones too | `checksums.sha256` in every release |
| johnvansickle.com (Linux) | not checked | - | - | - |
| evermeet.cx (macOS) | not checked | - | - | - |
| gyan.dev (Windows) | not checked | - | - | - |

The BtbN Linux build is not fully static: it needs glibc 2.28 (Debian 10,
Ubuntu 18.10 and newer) and kernel 4.18. It ran here (glibc 2.39). The archive is
146 MB. BtbN could serve Linux and Windows from one list, which leaves macOS as
the only open source.

**Not tested here** (no Firefox or WebKit could be downloaded; no Mac, no
Windows): Firefox, Safari (MSE; HEVC plays there natively, so the fall-back
matters for `.mts`/`.mkv` only), macOS speed with and without VideoToolbox
(Intel and Apple Silicon), the evermeet build (libx264, architecture, Rosetta,
quarantine of a file fetched by the program), Windows builds and browsers. The
steps for a person with a Mac are in `spike/video-playback/README.md`, "On a Mac".

**Where the numbers are weak.** A cloud VM, not the old MacBook. Synthetic
content (x264's output rate and CPU differ on real footage; HEVC decode cost is
close because the bitrate is like an iPhone's). Headless Chromium (the
`headless_shell` build); frames were decoded, but nothing was drawn to a
screen. The browser and ffmpeg shared the 4 cores. All runs went over
localhost, never over Wi-Fi to an iPad. Node's RSS (up to 685 MB) says little
about a Rust server: Node holds freed buffers until garbage collection.
