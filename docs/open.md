# Open items

Everything that is still to do, check or decide, in one place. It replaces the
"Open", "Still to check" and "Not done" lists of the old phase documents
(`docs/phaseN.md`, removed in October 2026; `git log -- docs/phase5.md` and so
on still has them).

How to use it:
- Tick an item (`[x]`) and then delete it in the next change that touches this
  file; a ticked item is not history worth keeping (git has it).
- New open work goes here, under the section it belongs to, in one or two
  lines with the place in the code or guide it concerns. A plan for something
  bigger that is not built yet goes to `docs/design/<feature>.md` and is linked
  from here.
- "(phase N)" says where the item came from; it is not a plan.

## 1. Checks on the real hardware (old Intel MacBook, exFAT drives)

Scanning and files
- [ ] CR2 metadata fix (`media::read_tiff_raw_exif`): `shoebox probe` on the
      "Angie wedding" folder shows `RAW 3 3 3` (phase 0).
- [ ] Time-zone shift: plug the drive into a Mac in another time zone (or change
      the Mac's) and rescan: files count as "time-zone shift", not "changed" (phase 1).
- [ ] Rotate: turn a copy of a JPEG and look at it in Finder and Explorer; turn
      a HEIC and a PNG and check Finder shows them as before.
- [ ] Stored browser frames: on a Mac without ffmpeg scroll past a few videos,
      restart the photo app, the previews are there without a new grab.
- [ ] libheif and libde265 as separate files: unpack a release (or the CI
      artifact), start with `Start shoebox.command`, open a HEIC.
- [ ] AVFoundation posters: without ffmpeg, scan a folder of iPhone HEVC
      portrait videos, the previews are upright; `shoebox probe` lists
      "videos: system video framework".

Faces and pets
- [ ] Face clusters with the cohesion rule: `shoebox recognize <drive>` with
      and without `SHOEBOX_FACE_COHESION=off`, note both "Clusters of faces"
      lines; on Unnamed, cards are one person each (small children too). Too
      many singles: try 0.50; cards still mixing people: 0.60 (phase 5).
- [ ] Unnamed follows on its own: name a face from Unnamed in the info panel,
      close the viewer, it is gone without a reload; a name typed into one card
      stays while another card is named (phase 5).
- [ ] Pets: Unnamed → Pets has no big card, cards of a few pets each, none
      mixing two pets; not many more single finds. Tune the cohesion in
      `pets::Space::thresholds` between 0.85 and 0.90 (phase 7).

Features built since phase 8 (each still waits for its first run on the real drive)
- [ ] Translation: read the German texts in the UI once (phase 8).
- [ ] Text in photos: installer and add-on on the old Intel Mac, seconds per
      photo there (`recognizer/text_spike.py`); search terms people really use
      ("Rechnung", a street, a licence plate), precision of the default
      threshold, size of the index on the drive; are sideways documents common
      enough for a rotated text pass (phase 9).
- [ ] Places: the first scan after the update (it reads every file's metadata
      once), real tiles (phase 10).
- [ ] Screenshots: set the weights, `THRESHOLD` and the 0.40 to 0.80 flatness
      range in `screenshots.rs` from real files; does iOS write `UserComment`
      "Screenshot", and in which versions; how to keep the display-size list
      short (phase 11).
- [ ] Export: a USB stick in exFAT, FAT32 and NTFS; a folder with spaces and
      decomposed Unicode names (phase 13).
- [ ] Backup comparison views (phase 14).
- [ ] Carry over data with two real exFAT drives (a Finder copy, then Carry
      over); Ignore folders and identical-once on the couple's two drives
      (phases 15, 18).
- [ ] Groups in the search (phase 19).
- [ ] Hidden: hide photos and a folder, look at the timeline, a person, Places
      and the counts; copy a photo into a hidden folder and scan (phase 20).
- [ ] Trash opt-in: setting off, no trash icon or button; on, both appear.

## 2. iPad checks (one sitting)

Setup: the MacBook with the drive and the iPad on the same Wi-Fi;
`shoebox serve /Volumes/Fotos --lan` prints the address and the PIN; Safari on
the iPad opens `http://<ip>:7878/`. A restart of `serve` asks for the PIN again
(accepted).

- [ ] Browsing: scrolling the whole library (~100,000 items) stays smooth; the
      year strip jumps and drags with a finger; search box and folder tree work
      by touch, the sidebar closes after picking a folder.
- [ ] Viewer: swipe left/right, swipe down to close, info panel; HEIC shows
      (also JPEGs named `.HEIC`); Live Photos play; videos play and seek.
- [ ] Select by tapping, "Move…" into an existing and a new folder; with
      "Allow move to trash" ticked, "Move to trash" and restore; "Select all"
      on a month heading and deselect.
- [ ] No "Show in Finder"; "Copy path" puts the path on the clipboard (or a
      dialog shows it).
- [ ] Tags: "+ Tag" in the info panel (keyboard, suggestions, Return, ✕ on own
      tags only); "Add tag…" / "Remove tag…" for a selection; two tags and a
      folder as chips, counts narrow, ✕, "Clear all"; a search saved as a
      bookmark opens again.
- [ ] Type drop-down opens on tap, closes on a tap outside, the button shows
      what is ticked; the heart in each cell's corner can be tapped; hearts in
      the dark theme.
- [ ] Faces: Face check scrolls, a crop opens its photo and comes back, "≈";
      sidebar Faces (group, person, Unnamed); naming a card with the keyboard
      open; ✓/✗ on a person's faces; the info panel's ✓/✗, "+ Name", ⋯ menu;
      "+ Add face" by dragging a box with a finger (the photo does not swipe);
      "Move to group…", "Merge into…"; two people as chips. Nothing needs hover.
- [ ] Pets: Pet check ("≈" lists the same pet at the top); Unnamed filter
      "Pets"; name a pet, put it in a group; "+ Add face or pet" on a missed
      pet; search "kat" / "hund" gives All cats / All dogs; a pet's page.
- [ ] Places: the dialog's map, two taps for a rectangle, pinch on the info
      map, the side list on a phone-sized screen.
- [ ] Dates: the drop-downs, the pencil's target size, the dialog on a
      phone-sized screen; the language switch and long German texts in dialogs.
- [ ] Hidden: the chip button.

## 3. Windows and Linux

- [ ] Cross-system: a drive scanned on a Mac shows no changed files on Windows,
      and the other way round (created and modified times on exFAT, NFC/NFD,
      case).
- [ ] `release-by-hand.yml` has never run on GitHub: run it with a tag for
      `windows` and for `linux`.
- [ ] Linux on a real PC: `Start shoebox.sh` and the file manager. Known: on a
      `noexec` drive the launcher runs a copy from a temporary folder, which
      does not find `program/recognizer/` or `ffmpeg`.
- [ ] A library kept next to the program on a real Windows share
      (`\\server\share`, a drive letter); measure a sensible thread count for
      network roots (phase 16).
- [ ] `install.ps1 -Text` has never run.
- [ ] Export on Windows: never run; a Downloads folder moved elsewhere is not
      found (needs the known-folder lookup); free space is not checked there.
- [ ] Does a company laptop's antivirus let the unsigned program run? Ask the
      first user (phase 17).

## 4. Known gaps in what is built

- [ ] Hidden photos still count in the sidebar's people counts and in the
      faces of the People pages (`people.rs` counts by content, in a cache the
      Unnamed page shares); a person's grid leaves them out. Decide whether the
      counts follow the grid (phase 20).
- [ ] Not covered by a browser test: the Hidden box of Carry over data (the
      fake API has no transfer route), the Control Panel's tabs, picker and
      dialogs (phase 16, driven by hand only), `backup::cleanup` refusing a
      read-only backup end to end.
- [ ] Rotate: the duplicates, people and trash pages build picture addresses
      without the turn (`take(8)` of the quick hash), so a browser that cached
      the old picture keeps it until a reload.
- [ ] Export: the created date of the copies is not set (macOS and Windows
      could); no size is shown for HEIC conversions.
- [ ] Screenshots: the selection-bar button "Mark as screenshot" and Settings →
      Calibration → Screenshot check are not built (phase 11).
- [ ] Text: the edge-cut rule (drop a line touching the border with a score
      below 0.97) is not built; decide on real photos whether it is needed or
      "hide a line" is enough (phase 9).
- [ ] Pet terms in the All drives suggestions are counted per request
      (`pet_hits_uncached`); a cache like the people's would help with many drives.
- [ ] Metadata: ORF/RW2 may be unsupported by `nom-exif`; Motion Photos (data
      after the JPEG end) get a false "truncated" warning (phase 0).
- [ ] Performance: waits of 2–3 s remain while a regrouping finishes (most
      likely `refresh_people_view` and the rewrite of `recog.clusters`, not
      measured one by one); memory is not measured on macOS and Windows.

## 5. Planned, not built

- [ ] **No cloud-synced folders**: refuse a shoebox folder or a library inside
      OneDrive, iCloud Drive, Dropbox, Google Drive, Box (Windows and macOS).
      Design: [design/cloud-folders.md](design/cloud-folders.md) (phase 17).
- [ ] **Recognition after a scan while the photo app runs**: after a rescan
      that found new files, `serve` starts the recognizer in the background for
      the new photos only (faces, turned pass, pets, as `shoebox recognize`
      resumes), with progress in the status line (`status.finding_faces`
      exists); skipped without a recognizer or while a `recognize` runs. Today
      the launcher refuses jobs while the photo app runs, so new photos wait.
- [ ] **Responsive on large libraries**: the Face check's "Not matched yet"
      took 2.5 minutes and froze the photo app (an id list in SQL, the one
      shared connection, no sign of waiting). The Face check now answers from
      the kept people view (step 1), reads no longer wait for each other
      (step 2), and a request over 1 s prints a line with what it waited for
      (step 3, stderr), and a thin line under the top bar shows a slow request
      (step 4); left: one to measure first (step 5): [design/responsiveness.md](design/responsiveness.md).
- [ ] Media Foundation posters on Windows ("Decoders per build", step 5).
- [ ] Server-side texts in both languages (API errors, job labels in the
      launcher, CLI): needs stable error codes; optional (phase 8).

## 6. Before giving shoebox to others

- [ ] Check with a lawyer before shoebox goes to more than friends or is sold,
      HEVC patents included (libde265 carries no patent licence).
- [ ] When the repository goes private: releases need its own Actions minutes.
- [ ] Code signing (SmartScreen warns on Windows) and an installer are a
      separate decision (cost).
- [ ] Confirm the release job's licence check (`third-party-licenses.py
      --check`, fetches licence texts) on a tag run; probably done since
      v0.7.x, tick it if so.

## 7. Ideas, not planned

Kept so they are not proposed again from scratch; none is promised. Delete the
ones you do not want.

- Dates: a hint with the neighbours' dates in the dialog; "Date" suggestions,
  "Needs a date" and "Set date…" on All drives; a searchable free-text note.
- Text: ignored words (Settings, applied at search time); tolerant matching (a
  line counts when ~70 % of the query's trigrams are found); a second pass on
  crops or a larger send size for small text; French or Italian letters (one
  line in the allow-list and a re-filter).
- Search by what a picture shows (CLIP), also for borderline screenshots.
- Play every video in the browser: converted by ffmpeg into memory only, no
  copy on the drive and no trace on the computer. Design:
  [design/video-playback.md](design/video-playback.md).
- Identical-once in All drives also for renamed copies.
- Run the ONNX models in Rust (`ort` or `tract`) and drop Python (see section 8).

## 8. Decisions to look at again

My recommendations, for you to confirm or reject; the answer goes into
[decisions.md](decisions.md).

1. **The old Intel MacBook (macOS 12) as the machine that decides.** It pins
   OpenCV 4.10 and onnxruntime 1.19, and rules out newer models. If the
   couple's computers are newer now, drop it to "should start" rather than
   "must be fast".
2. **`app.js` as one script of globals (9,400 lines).** Decided as "splitting
   for tests is not worth it". For agents a file this size is the main reason
   front-end changes go wrong: they cannot read it whole and miss the other
   place that does the same. Recommend: move self-contained parts into their
   own files the way `dates.js`, `geo.js`, `export.js` already are (no build
   step, no modules needed), one area per pull request, starting with the
   faces pages and the year strip.
3. **Python for recognition.** Two runtimes (OpenCV and onnxruntime), two
   installers kept in step by hand (`install.sh`, `install.ps1`), 200 MB per
   computer. The text add-on already runs on onnxruntime; running the face and
   pet models with `ort` in Rust would remove the Python runtime. Large; only
   worth it if installing add-ons keeps causing trouble.
4. **Browser-kept settings** (Maps, Collection, numbering, Ignore folders). One
   rule, but the iPad and every other computer start empty. Fine while one
   person uses one computer; revisit if that changes.
