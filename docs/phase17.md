# Phase 17: the release folder, and Windows release by hand

Status: **release folder done** (macOS release, launchers, guide; from the next
tag). **Windows workflow: plan, nothing built.** Overview in
[plan.md](plan.md); the Windows build itself is "Decoders per build", step 4.

## Part 1: the release folder (done)

Up to v0.7.4 the archive held nine things side by side: the program, its two
libraries, the launcher, `recognizer/`, `sources/`, `guide/` and two licence
files. People double-clicked `shoebox` itself instead of
`Start shoebox.command`; started like that from a browser download, macOS
blocks it when it loads `libheif.1.dylib` ("cannot be opened because Apple
cannot check it for malicious software"), because only the launcher takes the
quarantine mark off. Now there are three things on top:

```
shoebox-macos/
  Start shoebox.command          ← the only thing to double-click
  guide/
  program/                       ← shoebox, libheif.1.dylib, libde265.0.dylib,
                                   recognizer/, sources/, LICENSE.txt,
                                   THIRD-PARTY-LICENSES.txt (and an ffmpeg the user adds)
  libraries/                     ← only with phase 16: user data, beside program/, never in it
```

| Topic | Decision |
|---|---|
| Name | `program`: plain English, says what is inside |
| Libraries | Stay next to `shoebox` inside `program/`: the program loads them from its own folder (`@executable_path`), the build and the signing check stay as they are |
| Lookups "next to the program" | `recognizer/`, `ffmpeg`, the libraries: all inside `program/`, no code change |
| `sources/` | Stays, inside `program/`: the LGPL wants the source of libheif and libde265 with every copy given to someone else, a drive copy included, not only on the release page |
| User data | Never inside `program/`, so `program/` can be replaced by a newer one. Phase 16's `libraries/` folder moves beside `program/` (done in the phase 16 pull request) |
| Launchers | `Start shoebox.command`, `.sh`, `.bat` look for `program/shoebox` (`program\shoebox.exe`) only. The macOS one takes the quarantine mark off the files in `program/` and `program/recognizer/` (not the add-ons' Python: thousands of files, never from a browser) |
| `.shoebox/bin/` | The older layout (program in the drive's `.shoebox/bin/`, add-ons in `.shoebox/recognizer/`) is removed: nobody uses it. `launcher::drive_of_binary` is gone, `recognize::worker_dirs_of` returns only `recognizer/` next to the program |
| Guide | Get started describes the three things, Open Anyway (macOS 15 dropped right-click → Open), ffmpeg into `program/`; a FAQ entry for the libheif message |
| Check | The release job fails if anything but the three is on top |

## Part 2: Windows release by hand

### Goal

The automatic release (a `v*` tag) stays macOS only. The owner can add a
Windows archive to an existing release afterwards with one click in GitHub:
Actions → *release-windows* → Run workflow → a tag. The same workflow, started
without a tag, makes a complete test folder as an artifact, so a friend can try
Windows before anything is published.

### Today

- `build.yml` has a `windows` job (manual only): builds and tests on
  `windows-latest`, probes, scans and verifies a small folder, uploads
  `shoebox-windows` in the Part 1 layout: `Start shoebox.bat`, and `program/`
  with `shoebox.exe`, `heif.dll`, `libde265.dll` and `recognizer/`
  (`install.ps1`, `fetch-models.ps1`, `recognizer.py`, `README.md`). No
  licences, no guide, no sources.
  Before, the artifact had the program and the DLLs only; a `recognizer`
  folder copied from the macOS download has no `install.ps1`, so the Control
  Panel said the folder was not next to the program (it looks for
  `recognizer\install.ps1` next to `shoebox.exe`).
- The `release` job packs `shoebox-macos` only and runs
  `gh release create --prerelease` on a tag.
- `scripts/third-party-licenses.py` knows only the two macOS targets
  (`TARGETS`), so the Windows build has no licence notice yet.

### Decisions

| Topic | Decision |
|---|---|
| Trigger | A new workflow file `.github/workflows/release-windows.yml`, `workflow_dispatch` only, input `tag` (optional). The macOS pipeline is not touched |
| With a tag | Check out that tag, build, pack, **add** `shoebox-windows.zip` to the release of that tag (`gh release upload --clobber`, `contents: write`) and add its line to `SHA256SUMS`. The release must exist; if it does not the run stops with a message. The other assets are never changed or deleted |
| Without a tag | Same build and packing, result only as the workflow artifact `shoebox-windows` (a zip a person can send). Nothing is published |
| Guide | Not rebuilt: taken from the `guide/` folder in the release's `shoebox-macos.tar.gz`. Without a tag, from the latest release, or left out with a note if none exists |
| Package | One folder `shoebox-windows/` laid out like the macOS one (Part 1): `Start shoebox.bat`, `guide/`, and `program/` with `shoebox.exe`, `heif.dll`, `libde265.dll`, `recognizer/` (`install.ps1`, `fetch-models.ps1`, `recognizer.py`, `README.md`), `LICENSE.txt`, `THIRD-PARTY-LICENSES.txt`, `sources/` (libheif, libde265). Zip, not tar: Explorer opens it. A zip keeps no executable bit, which Windows does not need |
| Reuse | The build and probe steps move into a reusable workflow (`workflow_call`) or are copied from the `windows` job; one place only, so the manual `windows` job in `build.yml` and the release workflow cannot drift apart. Decide when writing it |
| Old tags | The tag must contain the Windows build scripts and `Start shoebox.bat`. For an older tag the workflow checks for them first and stops with a clear message |
| Release marked | The release stays a pre-release as created by the tag run. The workflow adds a line to the release notes only if the owner asks |

### No cloud-synced folders (Windows and macOS)

Found on the first Windows PC: the program folder was in
`C:\Users\…\OneDrive\Dokumente\…` and the add-on install failed. A sync client
holds new files open while it uploads them, so the installer's copy and rename
of the Python runtime (thousands of files, ~200 MB) can be refused, and every
file shoebox writes would be uploaded (the runtime, the models, `library.db`,
the thumbnails) and could later be turned into an online-only placeholder
under it. shoebox must not sync: **a shoebox in a cloud-synced folder is not
allowed**, on Windows and on macOS, whatever the cloud.

| Topic | Decision |
|---|---|
| What is refused | (1) The shoebox folder: the one with `program/` (`shoebox.exe` / `shoebox` and its `recognizer/`) and, with phase 16, `libraries/`. (2) A library: a drive or folder with `.shoebox/` (or one being set up as a library). Both are checked against every sync root found, including a sync root inside them |
| Clouds | Any sync client, not a fixed list. Windows: OneDrive (personal and work, also when Documents and Desktop are moved into it, as on that PC), Dropbox, Google Drive, iCloud for Windows, Box. macOS: iCloud Drive (also "Desktop & Documents Folders" in iCloud), OneDrive, Dropbox, Google Drive, Box |
| Detection, Windows | Ask Windows: `CfGetSyncRootInfoByPath` (Cloud Files API) answers for every client that registers a sync root (OneDrive, Dropbox, iCloud, Box, newer Google Drive); also the sync roots under `HKLM\…\Explorer\SyncRootManager`. Fallbacks: the `OneDrive`, `OneDriveConsumer`, `OneDriveCommercial` variables, Dropbox's `info.json`, `%USERPROFILE%\iCloudDrive`, and a Google Drive virtual drive (its volume label/file system, `fsinfo::filesystem_type`). To verify on a real PC which of these each client sets |
| Detection, macOS | Paths: `~/Library/Mobile Documents/` (iCloud Drive) and `~/Library/CloudStorage/` (File Provider clients: OneDrive, Dropbox, Google Drive, Box). `~/Desktop` and `~/Documents` when "Desktop & Documents Folders" is on (to find out how to read that setting reliably; candidate: `NSFileManager isUbiquitousItemAtURL`). Also the URL resource values for ubiquitous items. To verify on a real Mac |
| What the person sees | Program folder: the Control Panel opens with only one message: the cloud's name and the folder, why it is not allowed, and where to move the folder (`C:\shoebox` or the photo drive on Windows, the Applications folder or the photo drive on a Mac); nothing can be scanned or installed. `Start shoebox.bat` / `.command` print the same before they start the program. A library: the drive is listed but cannot be opened, with the same kind of message. Text through `tr()` in `en.json` and `de.json` |
| Not refused | Export: the destination is the person's own copy (sharing a collection through Dropbox is a reason to export), nothing of shoebox is in it. Reading originals from a cloud folder is not a case: originals are always in a library |
| Unknown clients | A sync client that registers nowhere is not found; the guide names the rule, not only the known clients |
| Rules | When it is built, a line in the rules of CLAUDE.md and in the guide's safety section (both languages) |

### Steps

1. [ ] Licences for Windows: add `x86_64-pc-windows-msvc` to `TARGETS` in
       `scripts/third-party-licenses.py`, run it, commit the changed
       `THIRD-PARTY-LICENSES.txt` (CLAUDE.md: also after a change to
       `Cargo.lock`, `build-deps.sh` or `core/web/vendor/`). Check that the
       macOS `--check` still passes. Until this is done the workflow runs
       without `--check` for the Windows target and says so in its log.
2. [ ] Packing script `scripts/package-windows.sh` (bash, runs in Git Bash on
       the runner; also usable on Linux for a dry run with a fake exe): builds
       the folder above (the Part 1 layout) from `core/target/…/release`, `scripts/launchers`,
       `recognizer/` and the sources, from files only (no network except
       `build-deps.sh --sources`). The `windows` job's Package step already
       copies `Start shoebox.bat` and `recognizer/`; the script takes it over.
3. [ ] `release-windows.yml`: inputs, tag check, checkout of the tag, build
       and tests (as the `windows` job), probe/scan/verify of the small
       folder, package, then either upload to the release or upload the
       artifact. Permissions: `contents: write` only for the upload step.
4. [ ] `build.yml`: the manual `windows` job calls the same steps (or is
       replaced by this workflow); the header comment explains the two ways.
5. [ ] No cloud-synced folders (see above): a `cloud` module with the check
       for Windows and macOS (unit tests with fake sync roots and paths), the
       Control Panel message and refusal for the program folder and for a
       library, the check in `Start shoebox.bat` and `Start shoebox.command`,
       texts in `en.json` and `de.json`, the CLAUDE.md rule, the guide (both
       languages: where to put the folder, why no cloud). Checked on a real
       PC with OneDrive and on a Mac with iCloud Drive.
6. [ ] `docs/plan.md`: "Decoders per build" step 4/5 and the Phase 6 Windows
       items point here; the status row below moves when it is built.
7. [ ] Guide, both languages, once a Windows archive is really offered
       ("macOS today · Linux & Windows next", Get started, the SmartScreen
       note, `Start shoebox.bat`, `install.ps1`); `python3 docs/guide/check.py`.
       The guide describes what exists, so this waits for the first real
       Windows release.
8. [ ] Run it: first without a tag, test the zip on a real Windows PC (start,
       a HEIC, a scan, export, the refusal from a OneDrive folder), then with
       a tag.

### Not in this phase

Code signing (SmartScreen will warn: "Windows protected your PC", More info →
Run anyway; a certificate costs money and is a separate decision), an
installer (MSI/Setup.exe), Linux, Windows ARM, Media Foundation posters
("Decoders per build" step 5), making the Windows build part of the
automatic release.

### Open questions

- Reusable workflow versus copy (see "Reuse").
- Whether the zip name carries the tag (`shoebox-windows-v0.3.0.zip`) or not
  (`shoebox-windows.zip`, as `shoebox-macos.tar.gz`). The macOS name has no
  tag, so the same here unless a release would hold two builds.
- Whether the antivirus of a company laptop lets an unsigned program run; only
  a real laptop can tell. Ask the first user to report it.
