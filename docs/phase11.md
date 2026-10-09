# Phase 11: screenshots (Type filter)

Built with generated test pictures; the threshold is still to be set on the real drive. Goal: the Type drop-down (phase 5f) gets a fourth
entry, **Screenshots**, so phone and computer screenshots can be shown on
their own or left out of the timeline.

## 1. Decisions

| Topic | Decision |
|---|---|
| Model | **None.** No new model, no Python worker, works without the recognizer. Detection is metadata plus a small pixel check in Rust. A zero-shot CLIP check of the borderline cases is an option once CLIP exists for text search (see Recognition in [plan.md](plan.md)); the score is built so it can be added without a migration |
| Signals | Weighted score from: (1) kind PNG, or JPEG without camera EXIF (`files.camera` empty, no GPS, no exposure data); (2) file name (`Screenshot`, `Screen Shot`, `Bildschirmfoto`, `Bildschirmaufnahme`, `Screenshot (12)`, Android `Screenshot_2024…`); (3) pixel size equal to a known display (phones, iPad, Mac, common monitors) or a phone-like aspect ratio; (4) EXIF `UserComment` "Screenshot" where the OS writes it (to verify on real files); (5) pixel check on the thumbnail that is made anyway: large flat areas, few distinct colours, hard horizontal/vertical edges, no sensor noise |
| Why a score | No single signal is safe: a PNG without EXIF may be a scan or an export; a renamed screenshot loses its name; a WhatsApp-forwarded one loses everything but the pixels. The score is stored, the threshold is not baked into the data |
| Storage | `files` gets a `shot_score` (NULL = not computed yet); the user's own decision (this is / is not a screenshot) is user data like favorites: its own small table in `library.db`, keyed like the other user data, included in the user data backup. The decision always wins over the score. Nothing is ever written into a file |
| Filled | At scan time for the metadata signals; the pixel check when the thumbnail is made. Files already in the index get their score from stored data plus their thumbnail on the next scan (no rehash, no touching originals; the guard covers the decode as for thumbnails) |
| Filter | `type=screenshot` next to `photo|video|live`, same URL hash, same API on the timeline, tag and folder views. Ticked types stay OR, the filter stays AND with folder, tags, people |
| Meaning of "Photos" | **Changes:** Photos = stills that are *not* screenshots. Screenshots = stills with a score over the threshold or marked by the user. Videos and Live Photos as before (a Live Photo is listed under Live Photos even if it looked like a screenshot). With no box ticked everything is shown, as today, so nothing changes unless the filter is used. Ticking Photos, Videos and Live Photos leaves the screenshots out; ticking only Screenshots shows only them |
| No exclude mode | The check boxes are enough for "leave screenshots out". A per-entry "not" for the bigger search ideas is a later step and needs no data change |
| UI | One more entry in the Type drop-down; "This is a screenshot" / "This is not a screenshot" in the info panel and the selection menu (the override). Settings → Calibration gets a **Screenshot check**: the photos closest to the threshold with their score and a slider, as the Face and Pet checks do |
| Text | All via `tr()` with keys in `en.json` and `de.json` |

## 2. Build order (each step its own commit)

1. **Spike on the real drive, no core change:** a small script/test that
   computes the signals for ~200 files (100 screenshots from iPhone, Android,
   Mac and Windows, 100 photos incl. scans and PNG exports). Decide weights
   and the threshold from the numbers; write them here.
2. `core/src/screenshots.rs`: the signals and the score, unit tests with
   small generated images and example names (NFC/NFD, spaces).
3. Schema (`library.db`/`files.shot_score`, override table), scan and
   thumbnail integration, backfill on the next scan.
4. Filter: `MediaType::Screenshot` in `browse.rs`, change of Photos, API
   `type=screenshot`, test in `timeline_order_filters_and_search`.
5. UI: drop-down entry, override in the info panel and the selection,
   Calibration → Screenshot check, i18n keys.
6. Guide (EN + DE, `.txt`; the Type drop-down, the typical session, the FAQ),
   this file as built, status row.

## 3. Tests

- Guard: nothing new reads originals except the thumbnail decode that
  exists; sizes, mtimes, created, full hashes unchanged.
- Score: each signal alone, in combination, the threshold edge, a photo
  from a camera (never a screenshot), a scan PNG.
- Override beats the score, survives a rescan and a move.
- Filter: Photos excludes screenshots, Screenshots only them, Photos +
  Videos + Live leaves them out, no box ticked shows all, Live with a
  screenshot-like still, 400 on an unknown type.

## 4. Open

- [ ] Weights, threshold and the 0.40 to 0.80 flatness range from real files
      (step 1; the test pictures are synthetic).
- [ ] Selection-bar button "Mark as screenshot" and Calibration → Screenshot check.
- [ ] Does iOS write `UserComment` "Screenshot" in the PNG, and in which
      versions.
- [ ] List of display sizes to carry, and how to keep it short (match by
      aspect ratio and multiples instead of an exact list?).
- [ ] Screenshots in video form (screen recordings): not in scope.
- [ ] Real-hardware check on the drive.

## 5. As built

- `core/src/screenshots.rs`: `score()` (name 50, PNG 15 / JPEG 5, no camera
  10, display size 25, flatness up to 50; a picture with a camera model is
  capped at 40; HEIC, RAW, video are 0), `pixel_score()` (share of
  neighbouring pixels with exactly the same colour, 0.40 to 0.80 mapped to 0
  to 100, measured on the stored JPEG thumbnail so a backfill agrees with a
  fresh thumbnail), `THRESHOLD = 60`.
- `library.db` v11: `files.shot_pixels` (filled with the thumbnail and
  backfilled from stored thumbnails or a twin by `thumbs::fill_shot_pixels`;
  reset when a file changes) and `shot_marks(key = quick_hash, is_shot)`.
  The marks are part of `userdata.json` (`shot_marks`).
- `browse.rs`: `Item.shot` (mark, else score ≥ threshold); `Photo` = still and
  not shot, `Screenshot` = still and shot. `serve.rs`: `type=screenshot`,
  `POST /api/screenshots {ids, value: true|false|null}`, `screenshot` and
  `screenshot_mark` in `/api/files/{id}`.
- UI: Type drop-down entry (the entry stays called Photos; a small ⓘ with the
  tooltip "Stills without screenshots"), "Screenshot" row in the info panel:
  a Yes / No drop-down that starts on shoebox's guess (marked "shoebox's
  guess" or "your choice"), with "Back to automatic" once the user decided. Not built: the selection-bar button for many photos, the
  Calibration "Screenshot check".
- Tests: unit tests in `screenshots.rs` and `db.rs` (v11 re-run);
  `screenshots_are_a_type_of_their_own` and the changed Photos counts in
  `timeline_order_filters_and_search` (`core/tests/serve.rs`), with a guard
  snapshot of the originals.
- Guide (EN, DE, `.txt`): the Type entries and a paragraph under Search.
  Guide screenshots not regenerated (the drop-down is closed in them).
