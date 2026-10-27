# No cloud-synced folders (planned)

From phase 17 (October 2026), not built. Linked from [open.md](../open.md).
When it is built: a rule in `CLAUDE.md`, an entry in `decisions.md`, the
guide's safety section in both languages, and this file is deleted.

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


## To build

A `cloud` module with the check for Windows and macOS (unit tests with fake
sync roots and paths), the Control Panel message and refusal for the program
folder and for a library, the check in `Start shoebox.bat` and
`Start shoebox.command`, texts in `en.json` and `de.json`, the CLAUDE.md rule,
the guide (both languages: where to put the folder, why no cloud). Checked on a
real PC with OneDrive and on a Mac with iCloud Drive.
