# Build notes and pitfalls

Things that went wrong once and must not again. Add one when a bug had a cause
that the code alone does not show; remove one when the code it warns about is gone.

- **Pets are rows of `recog.faces` with a species**, and everything that
  deletes or compares faces must say which kind: the faces pass only touches
  `species IS NULL`, the pets pass only the others, clustering and
  suggestions are per model (`pets::Space`), and a decision matches only
  a face of its own kind (`face_decisions.species`). A new query over
  `recog.faces` that forgets this mixes embeddings of different lengths.
- **Embedding scales differ per model.** PP-ResNet50 features give 0.50–0.68
  for different pets (SFace: well under 0.35), so thresholds belong to
  the model (`Space::thresholds`), never to a constant shared with faces.

- **An open response keeps `serve` from stopping.** The graceful shutdown
  waits for every request, and an event stream (`/api/events`) never ends on
  its own: such a stream must end on `Shared::closing` (set when stopping
  begins), as `serve::events` does. `core/tests/people.rs` stops a server
  with a stream open.

- **Spaces in paths.** The repo may live under a path with spaces.
  `build-deps.sh` uses bash arrays for CMake args; never unquote paths.
- **No double quotes in arguments from Windows PowerShell 5.1.** It drops
  the `"` inside an argument it passes to a program, so
  `python -c 'print("x")'` reached Python as `print(x)` split in two (on the
  first PC: "SyntaxError: '(' was never closed", reported as "OpenCV does not
  start"). Python code for `-c` in `install.ps1` quotes with `''`;
  `core/tests/recognize.rs` checks it.
- **No pkg-config for libheif.** Homebrew's libheif leaked into the link once.
  `scripts/build.sh` sets `SYSTEM_DEPS_LIBHEIF_NO_PKG_CONFIG` and points
  `SEARCH_NATIVE`/`LIB`/`INCLUDE` at our build; `build-deps.sh` gives
  pkg-config an empty search path and disables CMake system paths. CI fails
  if the program links anything from `/opt` or `/usr/local`.
- **libheif and libde265 are shared libraries next to the program** (LGPL).
  The release binary's only rpath is `@executable_path` (`$ORIGIN` on Linux);
  debug builds and tests also get the deps folder (`core/build.rs`, by
  `PROFILE`), so no build path ships. libheif finds libde265 through its own
  rpath (`@loader_path` / `$ORIGIN`: on Linux a program's RUNPATH does not
  apply to its libraries' libraries). They bring their C++ and compiler
  runtimes themselves, so `build.rs` no longer links `c++`/`stdc++` or
  `libclang_rt.osx.a` (which x86_64 libde265 needs for
  `__builtin_cpu_supports`; clang adds it when it links the dylib).
- **Re-sign after install_name_tool.** CMake's install rewrites the dylibs'
  rpath and install name, which breaks their ad-hoc signature; Apple Silicon
  refuses to load them then. `build-deps.sh` signs them again (`codesign -s -`).
- **Real files, no symlinks.** CMake installs `libheif.1.dylib` as a symlink;
  `build.sh` copies the file itself (exFAT, and the archive is copied by hand).
- **HEIC EXIF**: `nom-exif` misses EXIF in some HEIC layouts; we extract the
  EXIF block with libheif and parse only the TIFF payload with `nom-exif`.
- **Truncated JPEGs decode "successfully"** (grey bottom); check the FF D9
  end marker.
- **Toolchain on the dev Mac:** Homebrew `rustup` is keg-only; use
  `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`. Homebrew's own `rust`
  (1.82) is too old for the crates.
- **CMake can't find make/cc.** `build-deps.sh` turns off
  `CMAKE_FIND_USE_SYSTEM_ENVIRONMENT_PATH` (so nothing leaks in from
  Homebrew), which also stops CMake searching PATH for the build tool and
  compilers. The script passes `CMAKE_MAKE_PROGRAM` and the compilers (from
  `$CC`/`$CXX`, default `cc`/`c++`) explicitly. A cached deps dir hides this
  bug; it only shows on a cache miss.
- **Integration tests and fixtures.** `core/tests/scan.rs` copies
  `$SHOEBOX_FIXTURES` into each test library when set; an unset or empty
  variable just uses the synthetic files the test writes itself.
- **Perceptual hashes of synthetic images are flaky.** Smooth gradients or
  blocky test patterns leave most DCT coefficients near the median, so
  noise flips bits. Tests use rings (photo-like structure). Exact copies
  take over their twin's hash instead of re-hashing the JPEG thumbnail.
- **`DynamicImage::thumbnail` enlarges** small images; `thumbs::shrink`
  only ever shrinks.
- **`db::open` marks running jobs interrupted**, which would break a
  running scan if the server used it; `serve` uses `db::open_shared`.
- **`PRAGMA data_version` ignores the connection's own commits.** Caches in
  `serve` are keyed by it plus a counter of the server's own changes.
- **Phases 3+ write originals' paths.** Use `organize.rs` helpers
  (`rename_noreplace`, `Names` for case/NFC collisions, `ensure_folder`);
  never `fs::rename` directly, never copy.
- **Two decoder threads deliver out of order.** `recognize` (like the
  thumbnail pass) handles pictures in the order decoding finishes, so tests
  must not depend on the order (successes can come between crashes).
- **The worker restarts lazily**, on the next picture after a crash; the
  `restarts` count depends on what comes after the last crash.
- **python-build-standalone on exFAT:** its archives contain symlinks, so
  `install.sh` builds the runtime in a temporary folder and copies it with
  `cp -RL`, keeping only `bin/python3`. Use the `install_only_stripped`
  archives: the plain ones carry ~250 MB of debug info, and GNU `strip`
  breaks their binaries.
- **File names lie.** Some exports keep `.HEIC` on a JPEG (43 such files on
  the real drive). Images are decoded and their metadata read by their
  first bytes (`media::content_kind`), not their extension; the stored
  `kind` stays the extension's. thumbs.db v2 retries failed thumbnails, and
  a scan re-reads metadata that failed before.
- **OpenCV wheels and old macOS.** OpenCV 4.11+ has Intel macOS wheels for
  macOS 13+ only; without a wheel pip compiles OpenCV from source (hours,
  usually fails). `install.sh` passes `--only-binary :all:`, so macOS 12
  gets 4.10.0.84, the newest release with a `macosx_12_0_x86_64` wheel.
- **Python prints to stdout.** `recognizer.py` keeps the real stdout for the
  protocol and points `sys.stdout` at stderr; the core skips stray lines.
- **Migrations must tolerate a re-run.** Tests simulate an older database
  by lowering `user_version` on a current one, so a step that creates a
  table uses `CREATE TABLE IF NOT EXISTS` (thumbs.db v3).
- **Turned copies and face sizes.** The rotated pass stores boxes upright;
  a face it found lies sideways, so its width (for the 30 px threshold) is
  the box's height (`faces.rs`, `size_px`).
- **Face ids are not stable.** `recog.faces.id` is a plain rowid: SQLite
  can give a new face the id of a deleted one (the largest), and a model
  change or `--retry-failed` replaces a photo's faces. Nothing the user does
  refers to a face id (decisions are keyed by content and box), caches
  that do (`recog.neighbours`) are dropped with the face, and cluster
  similarities are computed again from the embeddings.
- **SIMD without fast-math.** A plain `iter().map(a * b).sum()` cannot be
  vectorised (float addition is not reordered); `ann::dot` keeps eight
  sums side by side, which the compiler turns into SIMD.
- **Every change bumps `index_version`**, people and decisions included, and
  the status poll then reloads the open view. Pages that change their
  cards in place (Unnamed, a person's faces) are left alone by `reloadAll`,
  or the place and typed names are lost.
- **Cluster numbers change after every decision.** Clusters are a cache
  recomputed from scratch, so actions on a card carry the cluster's own
  `generation` (a hash of its faces), not its number or a list-wide one.
- **A Live Photo is name and moment, not name alone.** Cameras and phones
  count the same way, so a Canon `IMG_0139.JPG` and an iPhone `IMG_0139.MOV`
  from another year can share a folder; pairing by folder and stem played the
  wrong video and hid it from the timeline (and moving or trashing one took
  the other along). `live.rs` is the one rule (timeline, export, move/trash
  groups): same folder and stem, video at most 6 s, and the files' own
  capture dates (`files.taken`, not a user's date) within 10 s; with both
  time zones known as instants, else allowing whole quarter hours up to 14 h
  (a container date in UTC next to a JPEG without offset); without a capture
  date on one side, the modification times. Tests in `core/tests/live.rs`.
- **Git push** uses SSH via the 1Password agent with the "GitHub" key pinned
  in this repo's `core.sshCommand` (the keychain's HTTPS login belongs to a
  different account, `svenfritschpeers`).
- **Windows paths.** Roots are canonical (`\\?\E:\`): no file name at the top
  of a drive, and Explorer does not take the prefix. A library is named by
  `volume::library_name` (folder name; at the top of a drive the volume label,
  as on a Mac), never by `file_name()`; the id and `backup_of` hang on it.
  Paths shown or sent to the browser go through `volume::display_path`; file
  access and comparisons keep the canonical path (the server canonicalises what
  the browser sends back). The launcher compares `E:\`, `e:/` and `E:` as one
  place (`winKey`), macOS/Linux paths as written. Names that Windows keeps for
  devices (`CON`, `NUL`, `COM1`...) are refused on every system
  (`organize::is_reserved_device_name`).
- **A turn in shoebox only (HEIC, PNG) is not in the file.** Face boxes are in
  the file's orientation; the viewer turns them for display (`turnedBox`), "Add
  face" is disabled while a photo is turned, and face crops are cut from the file
  as it is. Identical copies (same quick hash) share the turn; it is not carried
  when a JPEG is turned in the file.
- **A change to people, groups or face decisions does not move `generation`.**
  It goes through `change_with(.., false)` (`people_change`); otherwise
  `index_version` changes and every open page reloads folders, tags, people and
  places (`reloadAll`), after every name given on the Unnamed page.
- **`?1 IS NULL OR key = ?1` cannot use an index.** One query read every face,
  decision and file for one photo (400 ms, seconds from a cold drive): write one
  query per case (`faces_sql` in `people.rs`, checked with `EXPLAIN QUERY PLAN`
  in a test). For one face's file use `faces::present_file`; keep
  `faces::PRESENT` for queries over all faces.
- **OpenStreetMap answers a request without a Referer with a 403 tile that is
  still a PNG** (a map full of text, no console error). The pages send
  `Referrer-Policy: no-referrer`, so the tile images carry
  `referrerpolicy="origin"`.
