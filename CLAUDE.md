# shoebox

Portable photo library manager that lives on an external exFAT drive next to
the photos. Rust core (`core/`), optional Python recognition worker
(`recognizer/`).

**Read [docs/plan.md](docs/plan.md) first**: decisions, architecture, phases,
current status and next step. Update its status table when a phase moves.

## Rules

- Originals are read-only. Never write metadata or thumbnails into photo
  files; never change capture dates or created dates.
- Any code that reads originals must be covered by the guard (snapshot size,
  mtime, created, full hash before and after; fail on any difference).
- Originals change only through explicit user actions (move, rename, trash)
  in `organize.rs`: rename only, never copy, never replace, and only files
  that still match the index. There is no import: photos are copied onto the
  drive in the file manager (which keeps their dates) and a scan finds them.
- Moving photos (`organize::move_files_with`) and trashing them from the photo
  view or selection (`organize::trash_files_tidy`, `/api/trash`) or removing
  copies on the duplicates screen or with "Delete the new copies"
  (`duplicates::remove_copies_tidy`) also remove a
  folder the photos left, only if nothing is in it but system bookkeeping files
  (`classify::is_ignored`: `.DS_Store`, `._*`, `Thumbs.db`; those go with it) and
  `remove_dir` succeeds (so never one with a subfolder or any other file),
  never the library root, and report it (`removed_folders`). A restore from
  the trash creates the folder again (`ensure_folder`); the trash keeps the
  full path.
- After a scan, `arrivals::cleanup` (launcher button "Delete the new copies")
  may remove files the scan added whose content (full hash) was on the drive
  before that scan, through `duplicates::remove_copies` (the older copy stays;
  trash or, if asked, for good). Never anything but those new copies (and the
  folder they leave empty, see above).
- The one exception: turning a JPEG (`organize::rotate`) overwrites the two
  bytes of its EXIF Orientation tag in place, only when the file matches the
  index, and checks the full hash afterwards (old content plus those two
  bytes). Never anything else in an original.
- The other change to a drive that is not the photo app's own: `backup::cleanup`
  (launcher button "Delete duplicates from backup as well", after a backup
  check). It takes only copies named by `multi::removable_on_backup` off the
  backup: copies removed on the original's duplicates screen
  (`removed_copies`), still there with the same path and content, while the
  kept content is on both drives and the backup keeps another file with it.
  Each goes through `organize::trash_files` (a rename into the backup's own
  trash, only if the file still matches the backup's index), emptied again
  only if the user asked for "for good". Never anything on the original.
- Export (`export.rs`, phase 13; the dialog is `core/web/export.js`) only
  reads originals: it copies the photos asked for (plus, if ticked, their RAW
  files and Live Photo videos) into a new folder the user chose, which must not
  be inside the library. Every read is under the guard (the file matches the
  index before, does not change while it is read; it is hashed while copied and
  the copy is hashed again from the destination). Nothing is written next to an
  original or into an index. An existing export folder is merged: never
  replaced, nothing deleted, only files that are ours (a number and a name in
  the set) get a new number. A destination on a drive that has a `.shoebox`
  folder is allowed after a question (its next scan finds the copies). It only
  answers to a browser on this computer (`/api/export/*`, `info.export`).
  Collection tag and numbering are browser settings, like Maps.
- A library registered in `libraries/libraries.json` next to the program folder (phase 16,
  [docs/phase16.md](docs/phase16.md); index in `libraries/<name>/`, not in the
  root) is read-only: `libraries::ensure_writable(root)` is the first line of
  every function that changes an original (move, rename, trash, restore,
  rotate, remove copies), and nothing at all is written into its root, not
  even `.shoebox`: an export refuses a destination inside such a folder
  (`libraries::read_only_root_of`, called by `export::destination`). A new
  function that changes originals or writes into a library folder must go
  through one of the two.
- The recognizer (Python) runs only from `recognizer/` next to the program
  (`recognize::worker_dirs`; in the release that is `program/recognizer/`),
  never from the `.shoebox/` of a drive: a drive from somebody else could bring
  code of its own.
- The release folder has only three things on top: the start file
  (`Start shoebox.command`, `.bat`, `.sh`), `guide/` and `program/`
  (everything else; `scripts/package.sh` makes it for every system), plus
  `libraries/` once a library is kept next to the program (phase 16). User data never goes into
  `program/` (see "The release folder" in docs/plan.md).
- Moving to the trash from the photo view and the timeline selection is off by
  default: the library setting `allow_trash` (Settings, "Allow move to trash")
  shows the trash icon and the selection button.
- Maps (phase 10) are an opt-in setting (off by default, one for the whole application: kept by the
  browser as `shoebox-maps`, not per drive). The browser
  loads the tiles; shoebox never fetches or stores one. A position the user
  gives a photo lives in `library.db` (`geo_overrides`) only, never in the file.
- Estimated dates (phase 12): a date the user gives a photo (`date_estimates`,
  year, month or day) lives in `library.db` only, never in the file, and wins on
  the timeline over the capture date in the file; the file's date is shown in the
  info panel row "EXIF" and returns when the estimate is removed.
- Carry over data (phase 15a, `transfer.rs`, `/api/all/transfer*`): reads another
  drive's `library.db` read-only and writes only the target drive's
  `library.db` (own tags, hearts, people and face decisions, corrected or
  estimated dates, hand-given places), matched by content. Target data always
  wins, one transaction, never a backup, no photo touched. Phase 15b: the source
  can instead be a `userdata.json` the user picks on this computer
  (`transfer::read_userdata`, only a `.json` file, so never an original; every
  entry checked like typed input; only for a browser on this computer). Hidden photos and folders (phase 20) are a box of their own, not ticked unless the
  user does, and never part of the Tags box. A new table in
  `library.db` must be listed in `transfer::TRANSFERRED_TABLES` or `NOT_TRANSFERRED_TABLES`.
- Hidden from the timeline (phase 20, [docs/phase20.md](docs/phase20.md)): the own tag
  `hidden` (`tags::HIDDEN`) and the folders in `hidden_folders` live in `library.db`
  only, never in a photo or a folder, so a library kept next to the program takes them
  too. Hidden photos are left out of the photo grid in every form (`browse::Snapshot::query`,
  the folder and tag counts, the map) unless the Hidden chip (`hidden=1`), the typed word or a
  selected hidden folder asks for them; the pages for looking after the library (duplicates,
  face checks, trash, stats) show everything. A new list of photos for looking must apply
  the rule (`Item::hidden`). No original is touched, so no guard. It is tidiness, not
  privacy: texts say it only keeps photos out of the timeline.
- Licences: `LICENSE.txt` (all rights reserved, private use) and the generated
  `THIRD-PARTY-LICENSES.txt` go into the release archive. After a change to
  `Cargo.lock`, `scripts/build-deps.sh` or `core/web/vendor/` run
  `python3 scripts/third-party-licenses.py`. Do not call shoebox "open source"
  in texts. Icons are our own line drawings (`lineIcon` in `core/web/app.js`,
  the guide's symbols). Every icon is drawn by us and must look visibly
  different from any existing icon set (Google Material Symbols, Feather,
  Apple SF Symbols, ...), even when the idea comes from one: a different
  construction (shapes, proportions, stroke ends, where parts sit), never a
  trace, a copy of path data, or a small tweak of an original. If an icon
  still looks like the well-known one, change it. The exception: plain
  everyday shapes that every icon set has in some form (a cross, a plus, a
  trash can, a map marker, a tray with an arrow going up) may look like
  theirs, as long as we draw the path ourselves; the selection bar's Export
  (tray with an arrow) and Set location (marker with a plus) are such
  icons. Anything with a more distinctive design still needs a different
  construction. Such an icon needs no
  licence entry only because it is ours. Never paste an icon set's SVG into
  the code or the guide.
  No emojis anywhere in the pages (`core/tests/no_emoji.rs` fails on one); plain
  text symbols (✓ ✗ ✕ ♥ ☰ ↺ ⇄) may stay.
  Open items before giving it to others: see "Before giving shoebox
  to others" in docs/plan.md.
- Tests of the photo app come in three layers (API tests in Rust, the page
  against a fake API, a few against the real server): a change to what the
  page asks for or a reply's shape updates them as "Browser tests" in
  docs/plan.md says.
- **Tests are part of every change, without being asked.** Before writing
  code, say which tests the change needs (in the plan or the first reply);
  write them in the same pull request. For each new or changed behaviour:
  - a new or changed endpoint or reply: a Rust API test of that endpoint
    (its reply's shape, each case it handles, what it leaves out, a broken
    parameter refused) and, where it repeats another endpoint, that both
    answer the same;
  - something the page does on its own (when it asks, what it calls off,
    what it shows from a reply): a test in `faces-mocked.mjs` on the fake
    clock, and the fake (`fake-api.mjs`) answering as the server does;
  - a test against the real server (`faces-e2e.mjs`, `keyboard.mjs`) only
    for what the fake cannot show (that page and server agree, a real
    grouping);
  - a cache or a counter kept between requests: a test that a change in
    between is seen at once.
  Prove each new test fails without the change (break the code on purpose,
  `APP_JS=` for the page) and run it several times for flakiness. Weigh what
  a test protects against what it costs: Rust API tests and mocked browser
  tests take milliseconds to seconds, a real-server test seconds and a
  library; no fixed sleeps, a time window only to show that something does
  not happen. Do not add a test that only repeats another one or cannot
  fail. The pull request says which tests were added and why, and which were
  left out and why (e.g. not observable, like a request the server drops
  from its queue: measure it in a `scripts/bench-*.py` instead).
- UI text is never hard-coded: `tr()`/`trn()`/`data-i18n` with a key in both
  `core/i18n/en.json` and `de.json` (see docs/phase8.md; `core/tests/i18n.rs`
  checks it).
- Paths may contain spaces and decomposed Unicode: quote everything, compare
  NFC-normalised.
- **The guide is part of every change.** `docs/guide/shoebox-en.html` and
  `shoebox-de.html` (kept in step) are what users read about the app. A change
  that adds, removes or alters something a person can see or do (the photo app,
  the launcher, a button, a setting, what a command does, the safety rules
  above) updates both files in the same pull request, and removing a feature
  removes it from the guide. Follow the `guide` skill
  (`.claude/skills/guide/SKILL.md`): which sections to read, how to keep both
  languages in step, when a screenshot step or a dot needs you. Screenshots and
  the `.txt` copies are not in git: CI makes them from the code
  (`.github/workflows/guide.yml`, artifact `guide`, and every release), so do
  not build shoebox or run a browser just for the guide. Run
  `python3 docs/guide/check.py` and say in the pull request what changed in the
  guide, or that it is not affected. Look at `main` first: a pull request
  stacked on a branch that is already merged never reaches `main`.

## Build and test

```sh
# Linux, in Docker
docker build -t shoebox-dev -f docker/Dockerfile.dev docker
docker/dev.sh bash -c 'DEPS_DIR=/target/deps scripts/build.sh'
docker/dev.sh bash -c 'DEPS_DIR=/target/deps scripts/build.sh "" test'
# integration tests with the full fixtures (HEIC, video, EXIF dates)
docker/dev.sh bash -c 'scripts/make-fixtures.sh /target/fixtures && SHOEBOX_FIXTURES=/target/fixtures DEPS_DIR=/target/deps scripts/build.sh "" test'
docker/dev.sh bash -c 'scripts/make-fixtures.sh /target/fixtures && /target/*/release/shoebox probe /target/fixtures'
# the real recognizer too (after recognizer/fetch-models.sh; the image has OpenCV)
docker/dev.sh bash -c 'python3 -m unittest -v recognizer/test_recognizer.py'
docker/dev.sh bash -c 'SHOEBOX_RECOGNIZER=$PWD/recognizer/recognizer.py DEPS_DIR=/target/deps scripts/build.sh "" test'
# browser tests of the photo app, on the host, Linux or macOS: needs the host's
# release build (scripts/build.sh), Pillow, Node with Playwright and a Chromium
core/tests/browser/run.sh
# only the Unnamed page and people list against a fake API (no build needed)
NODE_PATH=$(npm root -g) node core/tests/browser/faces-mocked.mjs

# macOS (Homebrew rustup is keg-only)
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
scripts/build.sh x86_64-apple-darwin
scripts/build.sh aarch64-apple-darwin
scripts/build.sh aarch64-apple-darwin test
```

Fixture generation needs ffmpeg, heif-enc and exiftool (all in the Docker
image). Recognizer tests use the fake worker unless `SHOEBOX_RECOGNIZER`
points at `recognizer/recognizer.py`. Pitfalls from earlier phases are listed at the end of docs/plan.md.
