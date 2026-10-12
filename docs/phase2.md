# Phase 2: thumbnails and web UI

`shoebox scan` now also makes thumbnails and perceptual hashes, and
`shoebox serve` shows the library in a browser: a timeline grid, the folder
tree, search, and a full-screen viewer with video playback. Like scan and
verify, both only read originals and only write to `.shoebox/`; the guard
integration tests (`core/tests/serve.rs`) cover the thumbnail pass and every
server endpoint.

## Usage

```sh
shoebox scan "/Volumes/<drive>"                # index, thumbnails, full hashes
shoebox scan "/Volumes/<drive>" --no-thumbs    # skip thumbnails (serve makes them on demand)
shoebox serve "/Volumes/<drive>"               # http://localhost:7878/, this computer only
shoebox serve "/Volumes/<drive>" --lan         # also other devices, with a random PIN
shoebox serve "/Volumes/<drive>" --lan --pin 4711 --port 8080
```

`serve` prints the addresses to open and, with `--lan`, the PIN. On the
iPad, open the `http://<ip>:7878/` address and enter the PIN once; the
session cookie lasts 30 days or until shoebox is restarted. macOS asks
whether to accept incoming connections the first time `--lan` is used.

## Thumbnails (`core/src/thumbs.rs`)

- `.shoebox/thumbs.db`, a separate SQLite file (same durability settings as
  `library.db`), attached to the library connection as `thumbs`. Table
  `thumbs(key, width, height, jpeg, error, made_at)`, `key` = the file's
  quick hash: moved files keep their thumbnail, identical copies share one,
  changed files get a new one and the old one is pruned.
- 384 px on the longer edge, JPEG quality 80, never enlarged. Grid cells are
  square and crop with `object-fit: cover`.
- JPEGs are decoded with DCT scaling (`jpeg-decoder`, 1/2 to 1/8 straight
  from the IDCT), which makes a 12-megapixel photo several times cheaper
  than a full decode; ~120 thumbnails/s on 4 cores in Docker. EXIF
  orientation is applied. HEIC is scaled by libheif. PNG uses `image`.
- Videos: one frame at 10% of the length via `ffmpeg` (next to the binary or
  on PATH), piped to stdout; no temp files. Without ffmpeg, videos are left
  for later and the web UI grabs a frame in the browser (session only).
  Getting ffmpeg: [README](../README.md#video-previews-optional).
- A file whose thumbnail fails (damaged) gets a row with `jpeg = NULL` and
  the error, so it is not retried until its content changes. "ffmpeg not
  found" and "changed while being read" are not stored.
- The scan's pass runs on all cores; only the main thread writes, committing
  every 500 files or 5 seconds, so it can be interrupted. Newest photos
  first. Each file is read through `fingerprint::read_unchanged` (stamp
  before and after, against the index).

## Perceptual hash (`core/src/phash.rs`)

64-bit DCT hash of the thumbnail image (32×32 greyscale, 8×8 lowest
frequencies, median threshold), stored as 16 hex digits in `files.phash`.
Resized or re-encoded copies stay within 8 bits; unrelated photos land
around 32. Videos have none. A copy whose twin already has a hash takes it
over. Phase 3 compares them for near-duplicates.

## Web server (`core/src/serve.rs`, UI in `core/web/`)

axum + tokio; the UI (plain HTML/CSS/JS, no build step, ES2017 for older
iPads) is embedded with `rust-embed`.

| Endpoint | |
|---|---|
| `GET /api/timeline?folder=&tag=&q=` | Items newest first, in columns (ids, kind letters, `YYYYMMDD`, 8-char versions, Live Photo pairs); ~2.4 MB for 100,000 items |
| `GET /api/folders` | Folder tree with counts of everything below |
| `GET /api/tags?q=` | Tags by use, for search suggestions |
| `GET /api/files/{id}` | Details for the info panel |
| `GET /api/files/{id}/thumb` | Stored thumbnail, made on first request if missing; cached for good (URL carries the version) |
| `GET /api/files/{id}/view` | Original JPEG/PNG; HEIC rendered to a 2048 px JPEG on demand (not stored) |
| `GET /api/files/{id}/original` | The original byte for byte, with range requests (video seeking); `?download=1` for a download |
| `GET /api/info` | Counts, thumbnail progress, latest jobs, whether a scan is running |

- **Timeline order.** Capture date; without one, the month of the nearest
  `YYYY-MM Name` folder; failing that, the modification date (local time).
  The info panel says which. Grouped by month in the UI. (Phase 12 puts a date
  the user gave a photo first, see "Dates shown for a photo" in plan.md.)
- **Hidden:** RAW files (their JPEG/HEIC twin is shown) and missing files.
- **Live Photos:** a video of at most 6 s with the same folder and name as a
  JPEG/HEIC is folded into the still ("LIVE" badge, played from the viewer).
- **Search** words must each appear in the path or in one of the file's
  tags; case-insensitive, NFC-normalised. Combined with a folder (including
  subfolders) and/or tag filter. Filters live in the URL hash.
- **Grid** is virtualised: only rows near the viewport exist in the DOM
  (~50 cells at any time for 30,000 items in testing). Year jump, current
  month in the top bar.
- **Viewer:** arrow keys / swipe, Esc / swipe down closes, info panel with
  clickable folder and tags, download of the original.
- **Reload:** the UI polls `/api/info`; when a scan has changed the index
  (and is no longer running), it reloads the timeline in place.
- The server caches the timeline and rebuilds it when another process has
  committed to `library.db` (`PRAGMA data_version`).
- `serve` opens the database without marking a concurrent scan's job as
  interrupted (`db::open_shared`).

### Access

- Default: listens on 127.0.0.1 only.
- `--lan`: listens on all interfaces. Requests from this machine that name
  it as `localhost` or by IP need no PIN; everything else must log in with
  the PIN (random 6 digits per start, or `--pin`). Sessions are random
  256-bit tokens in an `HttpOnly; SameSite=Strict` cookie, kept in memory.
- A loopback request with any other `Host` (a web page using DNS rebinding
  to reach localhost) is treated like a remote device.
- At most 5 wrong PINs per minute in total, then HTTP 429.
- Plain HTTP: fine on the home network, not meant for the internet.

## Decisions taken in this phase

- **Live Photo videos are folded into their stills** rather than shown as
  separate grid items (an iPhone library would otherwise show most photos
  twice). Phase 3's move keeps the pair together anyway.
- **Event folder dates** only place files that lack a capture date; a dated
  photo in a wrongly named folder stays at its real date.
- **HEIC full-size views are rendered on demand**, not stored: they would
  double `thumbs.db` and Safari on the iPad shows HEIC directly anyway (the
  UI still requests the rendered JPEG, which works everywhere).
- **No transcoding** of video: HEVC `.MOV` plays in Safari, not necessarily
  in Chrome/Firefox on Linux/Windows.
- **Sessions do not survive a restart.** The iPad asks for the PIN again
  after shoebox restarts; acceptable for now.

## Still to check on real hardware

- [ ] `shoebox scan` on the exFAT drive from the old Intel MacBook: time for
      the thumbnail pass over the whole library, size of `thumbs.db`.
- [ ] `shoebox serve --lan` on the MacBook, browse from the iPad: scrolling
      through ~100,000 items, search, viewer, HEIC, Live Photo playback,
      video seeking.
- [ ] Videos with ffmpeg next to the binary (`.shoebox/bin/ffmpeg`).
