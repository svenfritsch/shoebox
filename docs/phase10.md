# Phase 10: locations (geolocation)

Where a photo was taken: read from the file, shown on a map, set by hand for
photos without it, and named places on the map that collect their photos.
Maps are optional and off by default.

## Decisions

| Topic | Decision |
|---|---|
| Map library | Leaflet 1.9.4 and Leaflet.markercluster 1.5.3, vendored in `core/web/vendor/` (BSD-2 and MIT, licence files next to them), embedded in the binary, loaded by the page only when a map is first needed. No CDN: the app still starts without internet. |
| Tiles | OpenStreetMap raster tiles, fetched **by the browser** (`img-src` of the page allows `https://tile.openstreetmap.org` and nothing else). shoebox stores no tiles. The first idea was a tile cache in `.shoebox/` that is emptied on exit; dropped because the aim is as little cache data on the drive as possible, and the browser's own HTTP cache does the job. The tile `<img>`s carry `referrerpolicy="origin"` (Leaflet option `referrerPolicy`): the pages send `Referrer-Policy: no-referrer`, and OSM answers a request without Referer with a 403 "Access blocked" tile that still is a PNG, so it shows up as a map full of text and no console error. Consequence: no pre-loading, and "no cached tiles" cannot be asked of the browser, so the offline label is shown when the tiles of a view fail to load (or the browser reports offline). |
| Opt in | Setting "Show maps (OpenStreetMap)" in Settings, default off. It belongs to the application, not to a drive: the browser keeps it in `localStorage` (`shoebox-maps`), so it is the same for every drive, and the server has no endpoint for it. Off: no Leaflet, no request to the internet, no Locations entry in the sidebar. The GPS row in the info panel (position as text, "Set location…") and manual entry stay available, because they need no map. |
| Position | `files.lat/lon` from the file (EXIF GPS; MP4/MOV location), read by the scan. Out of range or exactly 0,0 is "no position". The user's own: `geo_overrides` (key = quick hash, like `taken_overrides`), only for a photo whose file has none; the file's position always wins. Never written into a photo. |
| Existing index | Schema v10 adds the columns and `geo_done = 0`; the next scan reads the metadata of every file that was indexed earlier once more (opened read-only, under the guard). A first scan after the update therefore takes about as long as reading the metadata of the library. |
| Places | `places` (name, south, west, north, east, position). A place is an axis-aligned rectangle; membership is a query over the positions, so there is no bookkeeping when photos arrive or the rectangle is redrawn. One across the date line is not supported (west must be left of east). Names are unique case-insensitively. |
| Scope | One library at a time (ids are per drive), as with favorites. Not on the "All drives" page. |
| Backup of user data | `userdata.json` version 5 adds `geo_overrides` (with the files they belong to) and `places`. |
| Moving content | A turned JPEG changes its quick hash: `organize` carries `geo_overrides` to the new key like the capture-date override. Removing a duplicate copy gives its position to the survivor when that has none. |

## API (all under `/api/lib/{lib}/`)

- `GET /geo/points`: `{ids, lats, lons}` of every shown photo with a position (no RAW, no Live Photo clip).
- `POST /files/{id}/position {lat, lon}` or `{clear: true}`: set or forget the user's position (400 for bad numbers, or when the file has its own).
- `GET /files/{id}`: gains `position: {lat, lon, source: file|user}`.
- `GET/POST /places`, `POST /places/{id}/rename {name}`, `/area {south, west, north, east}`, `/delete`.
- `GET /timeline?place=<id>` and `?area=south,west,north,east`: the photos of a place or rectangle (400 for an unknown place or a bad area); they combine with the other filters (AND).

## UI

- Info panel: "GPS data" row at the bottom, always (also without a position): the position, or the button "Set location…" to the right of the heading. With maps on and a position: a square map across the whole panel width fixed to the bottom, pin in the centre, "Offline" label when no tile loads.
- "Set location…" dialog: map (click to place the pin, drag it) and latitude/longitude fields, kept in step; validation per field, Save disabled until both are valid; "Remove location" for a position the user gave.
- Sidebar: "Locations" section below Folders (only with maps on): the places with counts, "+ New place".
- Locations page (`#view=locations[&id=<place>]`): world map with clustered pins; click a pin or cluster: the screen splits and the photos fill a grid on the right (collapsible, remembered); click a place rectangle or a place in the sidebar for its photos, with Rename / Draw area again / Delete. New place and redraw: two clicks for opposite corners (works with touch), Esc cancels.
- Texts: `geo.*`, `side.locations`, `settings.maps*` in `core/i18n/{en,de}.json`; code in `core/web/geo.js`.

## Search

A place is a search term: suggestions "Places" (only with maps on), a chip `📍 name` (`place=` in the URL) that combines with the others (AND), at most one (a second replaces the first). The rule when picking one in the search box on the timeline: no other chip (folder, tag, person, pet, favorite; the text being typed does not count, a pick replaces it as for tags; the Type filter and the year jump are view options, not chips) opens the Locations page with the place, as a click in the sidebar does (which drops the Type filter too); with another chip, the place joins as a chip and the timeline stays. On the Locations page the search box finds places (typing does not leave the page; Enter searches the text); a place chosen there goes along as a chip when another term is added. The chip has a button to show the place on the map.

## Release and licences

`THIRD-PARTY-LICENSES.txt` (generated by `scripts/third-party-licenses.py`, checked by the release job) is copied to the top of the archive next to the binary. It covers the Rust crates of the macOS builds, Leaflet and markercluster, libheif and libde265 (LGPL-3.0, linked statically: the notice says how to rebuild them), and points to the downloads of the recognizer. `LICENSE.txt` (all rights reserved, private use) goes next to it. See "Before giving shoebox to others" in plan.md for what is still open (LGPL).

## Tests

`core/tests/geo.rs`: EXIF GPS read by the scan (a JPEG with a hand-built GPS block), setting/changing/clearing a position, refusal for a photo with its own and for bad numbers, `area=` and `place=` filters, place create/rename/redraw/delete with validation, counts, `userdata.json` v5, the rescan of an index from before v10, and the guard (originals unchanged, header needed for changes).

## Not done / ideas

- [ ] Real-hardware check (iPad: the dialog's map, two taps for a rectangle, pinch on the info map, the side list on a phone-sized screen; real tiles; the first scan after the update on the real drive).
- [ ] Search box term for places ("Home") like tags and people.
- [ ] Places across the date line; polygons instead of rectangles.
- [ ] Locations over all drives.
- [ ] Looking up an address or place name for the pin (needs a geocoding service, so another thing that leaves the machine).
