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

## Move, trash (phase 3, [phase3.md](phase3.md))

- [ ] Select photos by tapping, then "Move…" into an existing and a new
      folder; after ticking "Allow move to trash" in Settings, "Move to trash" and
      restore from the Trash page.
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

- [ ] "Settings" in the sidebar opens the Calibration cards; "Face check"
      opens the page. It scrolls; tapping a crop opens the photo and
      closing the viewer returns to the page; "≈" shows similar faces.

## Faces and people (5c-3, [phase5.md](phase5.md))

- [ ] Sidebar "Faces": tap a group to open it, a person for their photos,
      "Unnamed" for the cards.
- [ ] Unnamed: type a name (the list shows people by group, the keyboard
      does not hide it), "Select" faces in a card and name or ignore them.
- [ ] A person's faces: ✓ and ✗ are easy to hit; "Select", then "Not …".
- [ ] Viewer, info panel: tap a face's crop to see its box; ✓/✗; "+ Name";
      the ⋯ menu ("Not a face", "Ignore").
- [ ] "+ Add face": drag a box with a finger (the photo does not swipe to
      the next one), name it.
- [ ] "Move to group…" and "Merge into…" from a person's ⋯ menu (drag and
      drop is for a mouse).
- [ ] Search: two people from the suggestions, as chips.
- [ ] No action needs hover: naming a card, ✓/✗ in the info panel, drawing a
      face with a finger, "Move to group…" and the person's ⋯ menus all
      work by touch alone (from the combined 5c-2/5c-3 list in
      [phase5.md](phase5.md)).

## Pets (phase 7, [phase7.md](phase7.md))

- [ ] Settings → Pet check: the crops show whole pets with 🐱/🐶 tags;
      "≈" lists the same pet at the top (0.9 and more) and other pets
      far below it; "← Settings" goes back.
- [ ] Unnamed: the filter "Pets (cats and dogs)" lists only pet cards;
      name one, it appears in the sidebar with its pet and can be put in
      a group with people ("Move to group…").
- [ ] "+ Add face or pet" with a finger on a pet the detector missed (a
      small one, or from behind): tick "This is a pet", name it; it shows
      as "🐾 pet" in the panel, with a blue box, and the pet's other photos
      get suggested after a moment.
- [ ] Search "kat" or "hund": the suggestions show 🐱 All cats / 🐶 All dogs
      with counts; tapping one gives a chip and those photos; ✕ removes it.
- [ ] A pet's page and the viewer: its photos, the blue box and "dog" /
      "cat" in the info panel, ✓/✗ for a suggested pet.
