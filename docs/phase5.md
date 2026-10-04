# Phase 5

Phase 5 is face clustering and the correction UI (see [plan.md](plan.md)).
Work on it is split into parts; this file records each as it lands.

## 5-A: Show in Finder / Explorer, copy path

"Show in Finder" means the computer that runs `shoebox serve`, not the
browser. An iPad cannot open a Finder window over there, and browsers refuse
`file://` links, so there are two parts:

- **Show in Finder / Explorer** opens the file manager on the shoebox
  computer with the photo selected. Only for requests from that computer.
- **Copy path** copies the photo's path within the library (as in the info
  panel). Works on every device, the iPad included.

Both are in the context menu (right-click on a photo in the grid) and in the
info panel of the viewer.

### API

- `POST /api/files/{id}/reveal` → `{ok, app}`. `403` unless the request comes
  from this computer (loopback peer and a `localhost` or IP `Host`, the same
  test that skips the PIN); `404` for unknown ids and files that are not on
  the drive; `500` with the reason if the command cannot start. Like every
  non-GET request it needs the `X-Shoebox` header.
- `/api/session` has `reveal`: `"Finder"`, `"Explorer"` or `"file manager"`
  for requests from this computer, `null` for everyone else. The UI shows the
  button only when it is set, so over the LAN it is hidden.

### What runs (`core/src/reveal.rs`)

Chosen when shoebox is compiled (`cfg!(target_os)`), started without a shell,
output discarded:

| OS | Command |
|---|---|
| macOS | `open -R -- <file>` (selects the file) |
| Windows | `explorer.exe /select,<file>` (one argument) |
| Linux | `xdg-open <folder>` (opens the folder; there is no common way to select) |

### Decisions

- **The path comes from the index, never from the request.** The request names
  only the id; query string and body are ignored. The id is resolved like
  every other file access (`App::source`): relative, plain components only,
  present on the drive.
- **The original is only shown, never opened.** The Finder may leave a
  `.DS_Store` in the folder; that is not an original and the scanner skips it.
- **Copying works without https.** `navigator.clipboard` only exists on https
  and localhost, which an iPad opening `http://<ip>:7878/` has neither of, so
  the UI falls back to a selected text field and `execCommand('copy')`. If that
  fails too, the path is shown in a dialog, selected.
- **Copy path copies the path within the library** (`2020-07 Urlaub/IMG_1.JPG`),
  not the absolute path on the shoebox computer: it is what the API already
  shows, and it says where to look on the drive.

### Tests

`core/tests/serve.rs`, `show_in_finder_uses_the_indexed_path_and_only_for_this_computer`:
the opener is replaced by a recorder (`serve::Options::reveal`), so no window
opens. Checks the exact indexed path is passed whatever the request says, the
`X-Shoebox` header, unknown and vanished files, a PIN-logged-in LAN device
(no `reveal` in the session, `403`, nothing opened), and that no original
changed. The command lines per OS are unit tests in `reveal.rs`.

### Still to check on real hardware

- [ ] macOS: right-click a photo, "Show in Finder" selects it (also in a folder
      with spaces and umlauts, and with the drive on exFAT).
- [ ] Info panel: both buttons; "Copy path" from the iPad over `--lan`
      (plain http), then paste into a note.
- [ ] Over `--lan` from the iPad the "Show in …" button is not there.
- [ ] Windows (no build yet): `explorer.exe /select,` with a path with spaces.
