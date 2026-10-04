# iPad checklist

Every check that needs the iPad, collected from the phase documents so they
can be done in one sitting. Tick them here; the phase documents keep their
own lists for the MacBook and the drive.

## Setup

- The old Intel MacBook with the exFAT drive plugged in, on the same Wi-Fi
  as the iPad.
- `shoebox serve /Volumes/Fotos --lan` (prints the address and the PIN).
- On the iPad, Safari: open `http://<ip>:7878/`, enter the PIN once.
- Restarting `shoebox serve` asks for the PIN again (sessions do not survive
  a restart; known and accepted).

## Browsing (phase 2, [phase2.md](phase2.md))

- [ ] Scrolling through the whole library (~100,000 items) stays smooth;
      the year menu jumps.
- [ ] Search box and folder tree work with touch; the sidebar closes after
      picking a folder.
- [ ] Viewer: swipe left/right, swipe down to close, info panel.
- [ ] HEIC photos show (rendered by shoebox), also files named `.HEIC`
      that are JPEGs.
- [ ] Live Photos: "LIVE ▶" plays the motion part.
- [ ] Videos play and seeking works (range requests).

## Import, move, trash (phase 3, [phase3.md](phase3.md))

- [ ] Import from the iPad's photo library: Safari may convert HEIC to JPEG
      ("Most Compatible"); note what arrives, its name and its date
      (taken from `File.lastModified`).
- [ ] Select photos by tapping, then "Move…" into an existing and a new
      folder; "Move to trash" and restore from the Trash page.
- [ ] Select a whole month with "Select all" on its heading (Shift-click
      ranges need a keyboard; this is the iPad's way), then deselect it.

## Copy path (5a, [phase5.md](phase5.md))

- [ ] No "Show in Finder" button on the iPad (it only works on the
      MacBook itself).
- [ ] "Copy path" in the info panel: the path is on the clipboard (no
      https, so shoebox falls back to selecting a text field); if that
      fails, a dialog shows the path.

## Own tags (5b, [phase5.md](phase5.md))

- [ ] Info panel: "+ Tag" opens the keyboard, suggestions appear while
      typing, Return adds the tag; ✕ removes an own tag; folder tags have no
      ✕.
- [ ] Select several photos, "Add tag…" and "Remove tag…"; the Tags section
      in the sidebar updates.

## Search by several tags (5b-2, [phase5.md](phase5.md))

- [ ] Combine two own tags and a folder from the search box: the
      suggestion list works with touch, picking adds a chip, the counts
      narrow down.
- [ ] ✕ on a chip, "+", and "Clear all" with two or more terms.
- [ ] A combined search saved as a bookmark opens the same photos again.

## Face check (5c-1, [phase5.md](phase5.md))

- [ ] The Face check page scrolls; tapping a crop opens the photo and
      closing the viewer returns to the page; "≈" shows similar faces.

## Later (5c-3)

- [ ] Naming and correcting faces from the iPad (to be filled in when 5c-3
      is built).
