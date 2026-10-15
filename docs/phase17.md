# Phase 17: the release folder, and Windows and Linux releases by hand

Status: **release folder done** (macOS release, launchers, guide; from the next
tag). **Windows and Linux by hand: built** (`release-by-hand.yml`), not run yet;
no cloud-folder check yet. Overview in [plan.md](plan.md); the Windows build
itself is "Decoders per build", step 4.

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

## Part 2: Windows and Linux releases by hand

### How to use it

The automatic release (a pushed `v*` tag, `build.yml`) stays macOS only. Windows
and Linux are added afterwards, each on its own:

1. Push the tag as always; `build.yml` makes the release with
   `shoebox-macos.tar.gz` and `SHA256SUMS`.
2. Actions → **release by hand** → Run workflow. "Use workflow from": `main`.
   **System**: `windows` or `linux`. **Tag**: the release's tag (`v0.7.5`).
3. The run builds and tests the code of that tag, packs the folder and adds
   `shoebox-windows.zip` (or `shoebox-linux.tar.gz`) and its line in
   `SHA256SUMS` to that release. A second run replaces what the first added;
   nothing else in the release changes.

With the tag left empty, the run builds the branch it was started from and the
folder is only the artifact `shoebox-windows` (or `shoebox-linux`) of the run,
to try it on a real PC before anything is published.

A run of `build.yml` started by hand on a tag no longer tries to make a release
(it would fail: the release exists): the `guide` and `release` jobs run only for
a pushed tag.

### What a release then holds

```
shoebox-macos.tar.gz      (tag push)
shoebox-windows.zip       (release by hand, windows)
shoebox-linux.tar.gz      (release by hand, linux)
SHA256SUMS                (one line per archive)
```

```
shoebox-windows/                     shoebox-linux/
  Start shoebox.bat                    Start shoebox.sh
  guide/                               guide/
  program/                             program/
    shoebox.exe                          shoebox
    heif.dll, libde265.dll               libheif.so.1, libde265.so.0
    recognizer/  install.ps1,            recognizer/  install.sh,
      fetch-models.ps1,                    fetch-models.sh,
      recognizer.py, README.md             recognizer.py, README.md
    sources/  (libheif, libde265)        sources/
    LICENSE.txt                          LICENSE.txt
    THIRD-PARTY-LICENSES.txt             THIRD-PARTY-LICENSES.txt
```

### Decisions

| Topic | Decision |
|---|---|
| Workflow | `.github/workflows/release-by-hand.yml`, `workflow_dispatch` only, inputs `system` (`windows` / `linux`) and `tag` (optional). The Windows and Linux jobs moved there from `build.yml`, so they exist in one place and are started one at a time |
| Jobs | `check` (the release exists; the tag has `scripts/package.sh`, else it is older than this workflow and the run stops with a message), `windows` or `linux` (build, tests, probe/scan/verify; uploads the raw build for one day), `package` (on Ubuntu: guide, licence check, `scripts/package.sh`, archive, artifact, upload). `contents: write` only for `package` |
| With a tag | Checks out the tag, builds and **adds** the archive with `gh release upload --clobber`; `SHA256SUMS` is downloaded, this archive's line replaced, uploaded again. The release must exist. Other assets are never changed or deleted |
| Without a tag | Builds the branch it was started from; the result is only the workflow artifact (the folder; GitHub zips it for the download). Nothing is published |
| Guide | Not rebuilt: the `guide/` folder of the same release's `shoebox-macos.tar.gz` (without a tag: of the newest release, pre-releases included). A tag whose release has no guide stops the run; without a tag the folder goes without one, with a warning |
| Package | `scripts/package.sh <system> <build dir> <guide> <out>`, the same script the macOS release uses (Part 1 layout, checks that only the three things are on top). Windows: zip (Explorer opens it; no executable bit needed). Linux: tar.gz (keeps the executable bit) |
| Licences | `scripts/third-party-licenses.py` covers all four targets (both Macs, Windows, Linux): one `THIRD-PARTY-LICENSES.txt` for every archive, checked (`--check`) by every release job |
| Linux | x86-64, built on Ubuntu 24.04, so it needs glibc 2.39 or newer (Ubuntu 24.04, Debian 13, Fedora 40 and later). The Linux run also tests the real recognizer under the guard, as the old manual job did |
| Probe report | The probe of the test folder writes its report to the runner's temp folder, not into the artifact: it is only the input of the "both pictures got a preview" check (Windows), nothing a person needs. The same for the macOS build |
| Names | No tag in the file name (`shoebox-windows.zip`, as `shoebox-macos.tar.gz`): a release holds one build per system |
| Release marked | The release stays a pre-release as created by the tag run |

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

1. [x] Licences for Windows and Linux: both targets in `TARGETS` of
       `scripts/third-party-licenses.py`, `THIRD-PARTY-LICENSES.txt` written
       again (the native-library part names the Windows and Linux file names
       and `program/sources/`).
2. [x] Packing script `scripts/package.sh` for all three systems; the macOS
       release job uses it too.
3. [x] `release-by-hand.yml` (above).
4. [x] `build.yml`: the manual `windows` and `linux` jobs are gone (moved);
       `guide` and `release` run only for a pushed tag.
5. [ ] No cloud-synced folders (see above): a `cloud` module with the check
       for Windows and macOS (unit tests with fake sync roots and paths), the
       Control Panel message and refusal for the program folder and for a
       library, the check in `Start shoebox.bat` and `Start shoebox.command`,
       texts in `en.json` and `de.json`, the CLAUDE.md rule, the guide (both
       languages: where to put the folder, why no cloud). Checked on a real
       PC with OneDrive and on a Mac with iCloud Drive.
6. [x] `docs/plan.md`: the status row, "Decoders per build" and the Phase 6
       Windows items point here.
7. [ ] Guide, both languages, once a Windows or Linux archive is really
       offered ("macOS today · Linux & Windows next", Get started, the
       SmartScreen note, `Start shoebox.bat`, `install.ps1`, the Linux start);
       `python3 docs/guide/check.py`. The guide describes what exists, so this
       waits for the first real Windows release. Until then the guide in those
       archives is the macOS one.
8. [ ] Run it: first without a tag, test the zip on a real Windows PC (start,
       a HEIC, a scan, export, the refusal from a OneDrive folder), then with
       a tag. The same for Linux.

### Not in this phase

Code signing (SmartScreen will warn: "Windows protected your PC", More info →
Run anyway; a certificate costs money and is a separate decision), an
installer (MSI/Setup.exe), Linux packages (deb, rpm, AppImage), Windows and
Linux on ARM, Media Foundation posters ("Decoders per build" step 5), making
Windows or Linux part of the automatic release.

Known on Linux: `Start shoebox.sh` runs a copy of the program from a temporary
folder when the drive is mounted `noexec`; that copy does not find
`program/recognizer/` (the add-ons) or an `ffmpeg` next to the program.

### Open questions

- Whether the antivirus of a company laptop lets an unsigned program run; only
  a real laptop can tell. Ask the first user to report it.
