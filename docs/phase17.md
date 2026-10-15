# Phase 17: Windows release by hand

Status: **plan, nothing built.** Overview in [plan.md](plan.md); the Windows
build itself is "Decoders per build", step 4.

## Goal

The automatic release (a `v*` tag) stays macOS only. The owner can add a
Windows archive to an existing release afterwards with one click in GitHub:
Actions → *release-windows* → Run workflow → a tag. The same workflow, started
without a tag, makes a complete test folder as an artifact, so a friend can try
Windows before anything is published.

## Today

- `build.yml` has a `windows` job (manual only): builds and tests on
  `windows-latest`, probes, scans and verifies a small folder, uploads
  `shoebox-windows` with `shoebox.exe`, `heif.dll`, `libde265.dll`,
  `Start shoebox.bat` and `recognizer/` (`install.ps1`, `fetch-models.ps1`,
  `recognizer.py`, `README.md`). No licences, no guide, no sources.
  Before, the artifact had the program and the DLLs only; a `recognizer`
  folder copied from the macOS download has no `install.ps1`, so the Control
  Panel said the folder was not next to the program (it looks for
  `recognizer\install.ps1` next to `shoebox.exe`).
- The `release` job packs `shoebox-macos` only and runs
  `gh release create --prerelease` on a tag.
- `scripts/third-party-licenses.py` knows only the two macOS targets
  (`TARGETS`), so the Windows build has no licence notice yet.

## Decisions

| Topic | Decision |
|---|---|
| Trigger | A new workflow file `.github/workflows/release-windows.yml`, `workflow_dispatch` only, input `tag` (optional). The macOS pipeline is not touched |
| With a tag | Check out that tag, build, pack, **add** `shoebox-windows.zip` to the release of that tag (`gh release upload --clobber`, `contents: write`) and add its line to `SHA256SUMS`. The release must exist; if it does not the run stops with a message. The other assets are never changed or deleted |
| Without a tag | Same build and packing, result only as the workflow artifact `shoebox-windows` (a zip a person can send). Nothing is published |
| Guide | Not rebuilt: taken from the `guide/` folder in the release's `shoebox-macos.tar.gz`. Without a tag, from the latest release, or left out with a note if none exists |
| Package | One folder `shoebox-windows/`: `shoebox.exe`, `heif.dll`, `libde265.dll`, `Start shoebox.bat`, `recognizer/` (`install.ps1`, `fetch-models.ps1`, `recognizer.py`, `README.md`), `LICENSE.txt`, `THIRD-PARTY-LICENSES.txt`, `sources/` (libheif, libde265), `guide/`. Zip, not tar: Explorer opens it. A zip keeps no executable bit, which Windows does not need |
| Reuse | The build and probe steps move into a reusable workflow (`workflow_call`) or are copied from the `windows` job; one place only, so the manual `windows` job in `build.yml` and the release workflow cannot drift apart. Decide when writing it |
| Old tags | The tag must contain the Windows build scripts and `Start shoebox.bat`. For an older tag the workflow checks for them first and stops with a clear message |
| Release marked | The release stays a pre-release as created by the tag run. The workflow adds a line to the release notes only if the owner asks |

## Steps

1. [ ] Licences for Windows: add `x86_64-pc-windows-msvc` to `TARGETS` in
       `scripts/third-party-licenses.py`, run it, commit the changed
       `THIRD-PARTY-LICENSES.txt` (CLAUDE.md: also after a change to
       `Cargo.lock`, `build-deps.sh` or `core/web/vendor/`). Check that the
       macOS `--check` still passes. Until this is done the workflow runs
       without `--check` for the Windows target and says so in its log.
2. [ ] Packing script `scripts/package-windows.sh` (bash, runs in Git Bash on
       the runner; also usable on Linux for a dry run with a fake exe): builds
       the folder above from `core/target/…/release`, `scripts/launchers`,
       `recognizer/` and the sources, from files only (no network except
       `build-deps.sh --sources`). The `windows` job's Package step already
       copies `Start shoebox.bat` and `recognizer/`; the script takes it over.
3. [ ] `release-windows.yml`: inputs, tag check, checkout of the tag, build
       and tests (as the `windows` job), probe/scan/verify of the small
       folder, package, then either upload to the release or upload the
       artifact. Permissions: `contents: write` only for the upload step.
4. [ ] `build.yml`: the manual `windows` job calls the same steps (or is
       replaced by this workflow); the header comment explains the two ways.
5. [ ] `docs/plan.md`: "Decoders per build" step 4/5 and the Phase 6 Windows
       items point here; the status row below moves when it is built.
6. [ ] Guide, both languages, once a Windows archive is really offered
       ("macOS today · Linux & Windows next", Get started, the SmartScreen
       note, `Start shoebox.bat`, `install.ps1`); `python3 docs/guide/check.py`.
       The guide describes what exists, so this waits for the first real
       Windows release.
7. [ ] Run it: first without a tag, test the zip on a real Windows PC (start,
       a HEIC, a scan, export), then with a tag.

## Not in this phase

Code signing (SmartScreen will warn: "Windows protected your PC", More info →
Run anyway; a certificate costs money and is a separate decision), an
installer (MSI/Setup.exe), Linux, Windows ARM, Media Foundation posters
("Decoders per build" step 5), making the Windows build part of the
automatic release.

## Open questions

- Reusable workflow versus copy (see "Reuse").
- Whether the zip name carries the tag (`shoebox-windows-v0.3.0.zip`) or not
  (`shoebox-windows.zip`, as `shoebox-macos.tar.gz`). The macOS name has no
  tag, so the same here unless a release would hold two builds.
- Whether the antivirus of a company laptop lets an unsigned program run; only
  a real laptop can tell. Ask the first user to report it.
