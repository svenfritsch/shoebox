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
- Only one conversion runs at a time (the library's ffmpeg slot). A second
  request from another device while one runs gets `503` with the reason, shown
  in the lightbox as a notice. Only the video's own bytes are read from the
  original; **the guard covers the run** (snapshot size, mtime, created, full
  hash before; after ffmpeg ends or is stopped the snapshot is compared and a
  difference is an error; the full hash only when the run reached the end,
  because a stopped read has no meaning for the hash of the file, so the hash
  of an unfinished run is compared by the next scan's normal check). To be
  decided together with the existing guard: see "Open".

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

- `Start shoebox.*` (or the Control Panel button, as for the recognizer) finds
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
  one runs gets 503; `playback/info` answers the duration; the buffer is
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
  notice is shown; the button "Play converted" only when the server says
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
   fragment and memory for 1080p and 4K HEVC.
1. The ffmpeg add-on installer (`install-ffmpeg.sh` / `.ps1` next to the
   recognizer's, list of URLs and checksums, called from the start file and
   the Control Panel), the licence entry, a test that the checksums in the
   list and in the installer agree (like `core/tests/recognize.rs`). Decides
   the source (see Open).
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

## Open

- **Where the ffmpeg download comes from.** Builds of third parties (README
  today: evermeet.cx for macOS, johnvansickle.com for Linux; for Windows
  BtbN or gyan.dev) or a copy of our own in a GitHub release. Third parties
  are single people and links can disappear; our own copy would be a
  distribution by us (GPL source offer). Settle in step 1 by checking that
  each candidate has `libx264` (or the system's hardware encoder), runs on
  Intel and Apple Silicon, and has a stable URL. Until then the installer
  keeps one URL, version and SHA-256 per system in a single list.
- **The guard for a run that is stopped.** The existing guard hashes the whole
  file before and after. For a 20 GB video, twice per play, that is too
  slow. Proposal: size, mtime and created before and after every run, the
  full hash only at the scan. This relaxes a rule in CLAUDE.md and needs the
  user's yes; check first what the poster code in `media.rs` does today.
- **Load from another device.** A conversion uses the server's CPU. The
  `503` for a second request is the only limit; "Only this computer" like
  export is possible.
- **Live Photo videos.** Same path or as they are: decide after the spike.
