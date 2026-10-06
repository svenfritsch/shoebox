#!/usr/bin/env python3
"""Write the guide's HTML, one file per language, into _html/.
Content lives in T below (English and German side by side, so the two stay in
step); `build.sh` turns the HTML into shoebox-guide-en.pdf / -de.pdf."""
import os

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "_html")

# ---------------------------------------------------------------- icons ----
def icon(path):
    return ('<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" '
            'stroke-linecap="round" stroke-linejoin="round">' + path + "</svg>")

ICONS = {
    "grid": icon('<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>'),
    "search": icon('<circle cx="11" cy="11" r="6.5"/><path d="M16 16l5 5"/>'),
    "tag": icon('<path d="M3 12V4h8l10 10-8 8L3 12z"/><circle cx="7.5" cy="8.5" r="1.2"/>'),
    "move": icon('<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7z"/><path d="M9 13h7m-3-3l3 3-3 3"/>'),
    "import": icon('<path d="M12 4v11m-4-4l4 4 4-4"/><path d="M4 19h16"/>'),
    "dup": icon('<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>'),
    "trash": icon('<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13M10 11v6M14 11v6"/>'),
    "people": icon('<circle cx="9" cy="8" r="3.2"/><path d="M3 20c0-3.5 2.7-6 6-6s6 2.5 6 6"/><circle cx="17" cy="9" r="2.4"/><path d="M17 14c2.5 0 4.5 2 4.5 5"/>'),
    "drive": icon('<rect x="3" y="7" width="18" height="10" rx="2"/><circle cx="17" cy="12" r="1"/><path d="M6 12h6"/>'),
    "shield": icon('<path d="M12 3l8 3v6c0 4.5-3.4 8-8 9-4.6-1-8-4.5-8-9V6l8-3z"/><path d="M9 12l2 2 4-4"/>'),
    "tablet": icon('<rect x="5" y="3" width="14" height="18" rx="2"/><path d="M11 18h2"/>'),
    "globe": icon('<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18"/>'),
    "lock": icon('<rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>'),
    "bolt": icon('<path d="M13 3L5 14h6l-1 7 8-11h-6l1-7z"/>'),
}

LOGO = ('<svg viewBox="0 0 64 64" class="logo"><rect x="6" y="22" width="52" height="34" rx="4" fill="#b07d4f"/>'
        '<rect x="2" y="12" width="60" height="14" rx="4" fill="#d9a066"/>'
        '<rect x="10" y="30" width="44" height="3" rx="1.5" fill="#fff" opacity=".35"/></svg>')

# -------------------------------------------------------------- content ----
T = {
"en": {
  "lang": "en", "file": "shoebox-guide-en.pdf",
  "title": "shoebox: your photo library, on the drive",
  "tagline": "Your photo library, on the drive.",
  "lede": "shoebox is a small program that lives on your external drive next to your photos. It indexes them, shows them in a fast timeline in your browser, and helps you tag, sort, import and de-duplicate, without ever copying them or touching their metadata.",
  "badges": [("shield", "Originals stay untouched"), ("drive", "Lives on your drive"), ("lock", "Private: no cloud, no account"), ("tablet", "Browse from the iPad too"), ("globe", "macOS today · Linux &amp; Windows next")],
  "cover_note": "Guide · English",
  "other_lang": "Deutsche Version: shoebox-guide-de.pdf",
  "footer": "shoebox guide",
  "ov_h": "What shoebox can do",
  "ov_sub": "One page in your browser, a timeline of everything on the drive.",
  "features": [
    ("grid", "Timeline", "All photos and videos by month, with a year jump, folder tree and a viewer with full-size info."),
    ("search", "Search & filter", "Type a person, pet, tag or folder. Combine keywords for a more complex search. Filter by photos, videos or Live Photos."),
    ("tag", "Tags", "Your own tags on many photos at once. Folders count as tags too. Stored in the index, never in the photos."),
    ("move", "Move & organise", "Move photos to a folder (created if needed). RAW, Live Photo and sidecar files travel along."),
    ("import", "Import", "Pick the photos of an event or drop them into the window: they land in a new event folder and keep their dates."),
    ("dup", "Duplicates", "Finds identical, resized, edited and look-alike photos. You review; the best copy always stays."),
    ("trash", "Safe trash", "Deleted photos go to a trash on the drive. Put them back, or delete them for good."),
    ("people", "People & pets", "Optional recognition groups faces, cats and dogs. You name them; shoebox suggests the rest."),
    ("drive", "Several drives", "Browse the photos of several drives in one common timeline, even when they sit on different disks. Backups are recognised and checked."),
  ],
  "glance_h": "The screen at a glance",
  "glance_sub": "Everything is on one page. Here is where things are.",
  "glance": [
    ("Folders menu", "☰ shows or hides the sidebar on narrow screens."),
    ("Search box", "People, pets, tags and folders; pick several to combine them."),
    ("Type", "Show only photos, videos or Live Photos."),
    ("Year", "Jump straight to a year."),
    ("Select ☑", "Pick photos to move, tag or trash."),
    ("Import ⤒", "Add photos and videos from this computer or the iPad."),
    ("Quick links", "All photos · Duplicates · Trash · Settings."),
    ("Folders", "Your folders, with counts. Tags, Faces and Drives appear here when you have them."),
    ("Timeline", "Months with counts. Click a photo to open the viewer."),
    ("Language & status", "Switch English / German; see scan, thumbnail and drive status."),
  ],
  "session_h": "A typical session",
  "session": "Plug in the drive and double-click <b>Start shoebox</b>. <b>Scan</b> (quick when little has changed) and open the photo app. <b>Import</b> the photos of an event: pick them, name the event, and shoebox creates the event folder. Then tag and sort. Look at <b>Duplicates</b> now and then, and run the <b>Backup check</b> after every backup.",
  "start_h": "Get started in three steps",
  "start_sub": "macOS, Intel (10.13 or newer) and Apple Silicon. Nothing to install.",
  "steps": [
    ("Put it on the drive", "Unpack <b>shoebox-macos.tar.gz</b> from the release. Copy <b>shoebox-macos</b> and <b>Start shoebox.command</b> to the top folder of your photo drive (next to your photo folders). Optional: the <b>recognizer</b> folder for faces."),
    ("Start the launcher", "Double-click <b>Start shoebox.command</b>. A Terminal window opens (leave it open) and the launcher appears in your browser. If macOS asks, right-click the file and choose <b>Open</b>."),
    ("Scan, then open", "Add your drive (click it, or type <code>/Volumes/MyDrive</code>), press <b>Scan</b>, then <b>Start photo app</b>. The first scan reads every file once and takes a while on a big drive; later scans only look at what changed."),
  ],
  "launcher_cap": "The launcher: 1 choose the drive or folder, 2 pick a task, 3 open the photo app.",
  "launcher_tasks_h": "Launcher tasks",
  "launcher_tasks": [
    ("Scan", "find new, moved and changed photos"),
    ("Verify", "check that nothing was lost or damaged"),
    ("Recognize", "find faces (and, with Recognize pets, cats and dogs)"),
    ("Backup check", "is the 2nd folder a complete copy of the 1st?"),
  ],
  "tip_h": "Good to know",
  "tips_start": [
    "Videos get preview images if you put a static <b>ffmpeg</b> next to the program (<code>.shoebox/bin/ffmpeg</code>). Without it, the browser grabs a frame instead.",
    "shoebox keeps its index and thumbnails in a hidden <code>.shoebox</code> folder on the drive, so the drive can move to another computer.",
    "Closing the Terminal window stops shoebox.",
  ],
  "browse_h": "Browse and view",
  "browse_sub": "Scroll through the timeline, jump by year, open a photo.",
  "browse_items": [
    ("Timeline", "Photos are grouped by capture date, newest first. The <b>Year</b> menu jumps; the month heading shows where you are. A photo without a capture date takes the month in the name of its event folder (<code>2020-07</code>, <code>2020.07</code>, <code>20-07</code> or <code>20.07</code>), or else the file’s created date; the info panel then marks the date as <b>estimated</b>."),
    ("Folders", "The sidebar mirrors the folders on your drive. Click one to see only its photos. Rename or move a folder from its chip."),
    ("Viewer", "Click a photo. <b>ⓘ</b> opens the info panel: date (marked <b>estimated</b> when it is not the day the photo was taken), camera, size, tags, people, and buttons to open the folder or copy the path."),
    ("Rotate", "<b>r</b> turns left, <b>Shift+R</b> right. For JPEGs this changes only the two bytes of the rotation flag; for other formats the turn exists in shoebox only."),
  ],
  "grid_cap": "The timeline: months, folders, search, type filter and year jump.",
  "viewer_cap": "The viewer with its info panel.",
  "keys_h": "Keyboard in the viewer",
  "keys": [("← →", "previous / next"), ("i", "show / hide the info panel"), ("Space", "open / close the viewer (on a photo picked with the arrow keys)"), ("r · Shift+R", "rotate left / right"), ("Esc", "close the viewer")],
  "find_h": "Find and organise",
  "find_sub": "Search, select, tag and move: all of it works on many photos at once.",
  "find_items": [
    ("Search", "Type in the search box: suggestions appear for tags, people and folders. Pick several and they become chips that narrow the result; × removes one, <b>Clear all</b> starts over."),
    ("Select", "Press <b>☑</b>, then click photos (Shift-click for a range; on the iPad just tap). “Select all” on a month heading takes the whole month."),
    ("Tag", "<b>Add tag…</b> puts your own tag on the selection. Tags live in shoebox’s index, never in the photo files."),
    ("Move", "<b>Move…</b> asks for a folder. New folders are created, nothing is ever overwritten, and RAW, Live Photo and sidecar files move along. “Keep tags” decides whether the photos’ own tags go with them; folder tags always follow the new folder."),
  ],
  "search_cap": "Suggestions as you type.",
  "select_cap": "Three photos selected; the bar offers Move, Tag and Trash.",
  "tag_cap": "Adding a tag.",
  "move_cap": "Moving into a new event folder.",
  "import_h": "Import, duplicates and trash",
  "import_items": [
    ("Import", "Press <b>⤒</b> (or drop files on the window). Choose year, month and an event name: shoebox creates the event folder <b>YYYY-MM Event</b>, the files keep their dates, and files already in the library are skipped. <i>Coming soon:</i> in <b>Settings</b> you choose your own pattern, using Y for the year and M for the month (J and M in German), for example <code>YYYY.MM Event</code>."),
    ("Duplicates", "<b>Duplicates</b> lists four kinds: identical, same photo in another size, original and edited, and look-alikes. Tick <b>delete this copy</b> on what you want gone; the best file stays and at least one per group always does. Tags and capture dates of a removed copy carry over."),
    ("Shortcuts", "<b>Clear Same Folder Copies</b> and <b>Clear Lower Quality Copies</b> clean up the clear-cut cases in one go. Groups of similar photos need your decision: choose <b>Different photos</b> or <b>Versions of one photo</b>, and shoebox remembers it so the group does not return."),
    ("Trash", "Nothing is deleted right away. Trashed photos wait in <code>.shoebox/trash</code> on the drive: <b>Put back</b> or <b>Delete for good</b>."),
  ],
  "import_cap": "Import dialog.",
  "dups_cap": "Duplicates: a pre-ticked messenger copy next to its original.",
  "trash_cap": "The trash page.",
  "people_h": "People and pets",
  "people_sub": "Optional: shoebox finds faces, cats and dogs; you give them names.",
  "people_items": [
    ("Find them", "Install the recognizer once (<code>recognizer/install.sh /Volumes/MyDrive</code>). Then close the photo app and run <b>Recognize</b> (and <b>Recognize pets</b>) in the launcher. A <b>Faces</b> section appears in the sidebar."),
    ("Name them", "Under <b>Unnamed</b>, shoebox shows cards of similar faces. Type a name once and the whole card is named; later, confirm (✓) or reject (✗) the suggestions. Group people, merge two cards of one person, mark strangers with <b>Ignore</b>."),
    ("In the viewer", "The info panel lists who is on the photo and draws a box around each face or pet. Click a name to see all their photos, or draw a box around a face or pet that was missed."),
    ("Search", "Names work in the search box like tags, and so do kinds such as “all cats”. Combine a person with a tag, a folder or another person."),
    ("New photos", "Recognition does not start by itself after an import yet. Close the photo app and run <b>Recognize</b> again: it only looks at the new photos."),
  ],
  "unnamed_cap": "Unnamed cards: one name for every face on a card.",
  "info_people_cap": "The info panel names the girl and the cat; boxes show where they are.",
  "people_screens_note": "Sample pictures are drawn comics, not real photos.",
  "drives_h": "Drives, backups and the iPad",
  "drives_items": [
    ("Several drives", "Start shoebox with more than one drive and each one appears in the sidebar. <b>All drives</b> gives you one common timeline across all of them, and shows backups, photos that exist on more than one drive, and people across drives. A drive that is unplugged is shown as offline."),
    ("Backup check", "Choose the original first and the backup second. shoebox compares the two indexes (no photo is read) and lists what is not on the backup yet, what differs and what exists only on the backup. <i>Also re-read the backup</i> finds bit rot."),
    ("iPad & other devices", "In Terminal: <code>shoebox serve /Volumes/MyDrive --lan</code>. It prints an address and a PIN; open the address in Safari on the same Wi-Fi and enter the PIN once. Swipe in the viewer, tap to select."),
  ],
  "ipad_cap": "On the iPad the same page works with touch.",
  "safe_h": "Your photos are safe",
  "safe_lede": "The design rule is simple: shoebox reads your originals and never writes anything into them.",
  "safe_items": [
    ("Read-only", "Metadata, thumbnails and tags are kept in the index on the drive, never inside a photo. Capture dates and created dates never change."),
    ("Guarded", "Every scan is checked: size, dates and a full hash before and after. Any difference is an error."),
    ("Only on your command", "Originals change only through your actions: move, rename, trash, import. Always a rename, never a copy, and only if the file still matches the index."),
    ("Local", "No cloud, no account. By default only this computer can connect; other devices need <code>--lan</code> and a PIN."),
  ],
  "faq_h": "If something looks off",
  "faq": [
    ("The grid is empty.", "Run <b>Scan</b> in the launcher first, then start the photo app."),
    ("New photos are missing.", "Scan again. It is quick: only new and changed files are read."),
    ("A video has no preview.", "Add <code>ffmpeg</code> (see “Good to know”) and restart the photo app."),
    ("The iPad cannot connect.", "Same Wi-Fi? Started with <code>--lan</code>? Enter the PIN printed in Terminal. Restarting shoebox asks for the PIN again."),
    ("A drive shows as offline.", "Plug it in again; shoebox picks it up when you reload."),
  ],
  "cli_h": "Command line (optional)",
  "cli": [
    ("shoebox scan &lt;folder&gt;", "index new, moved and changed files"),
    ("shoebox verify &lt;folder&gt;", "re-read files and compare with the index"),
    ("shoebox recognize &lt;folder&gt; [--pets]", "find faces (and pets)"),
    ("shoebox backup &lt;original&gt; &lt;backup&gt;", "is the backup complete?"),
    ("shoebox serve &lt;folder&gt; [--lan] [--pin …]", "open the photo app"),
  ],
  "lang_note": "The app speaks English and German: pick the language at the bottom of the sidebar.",
  "about": "shoebox is open source · github.com/svenfritsch/shoebox",
},
"de": {
  "lang": "de", "file": "shoebox-guide-de.pdf",
  "title": "shoebox: deine Fotobibliothek auf dem Laufwerk",
  "tagline": "Deine Fotobibliothek, auf dem Laufwerk.",
  "lede": "shoebox ist ein kleines Programm, das auf deinem externen Laufwerk neben deinen Fotos liegt. Es indexiert sie, zeigt sie im Browser in einer schnellen Zeitleiste und hilft beim Verschlagworten, Sortieren, Importieren und Aufräumen von Duplikaten, ohne sie je zu kopieren oder ihre Metadaten anzufassen.",
  "badges": [("shield", "Originale bleiben unberührt"), ("drive", "Liegt auf deinem Laufwerk"), ("lock", "Privat: keine Cloud, kein Konto"), ("tablet", "Auch am iPad nutzbar"), ("globe", "macOS heute · Linux &amp; Windows folgen")],
  "cover_note": "Handbuch · Deutsch",
  "other_lang": "English version: shoebox-guide-en.pdf",
  "footer": "shoebox Handbuch",
  "ov_h": "Was shoebox kann",
  "ov_sub": "Eine Seite im Browser, eine Zeitleiste von allem auf dem Laufwerk.",
  "features": [
    ("grid", "Zeitleiste", "Alle Fotos und Videos nach Monat, mit Jahressprung, Ordnerbaum und einer Ansicht mit Infos zum Foto."),
    ("search", "Suchen & Filtern", "Tippe eine Person, ein Haustier, einen Tag oder Ordner. Kombiniere Schlagwörter für eine komplexere Suche. Filter nach Fotos, Videos oder Live Photos."),
    ("tag", "Tags", "Eigene Tags für viele Fotos auf einmal. Auch Ordner zählen als Tags. Im Index gespeichert, nie in den Fotos."),
    ("move", "Verschieben & ordnen", "Fotos in einen Ordner verschieben (wird bei Bedarf angelegt). RAW-, Live-Photo- und Sidecar-Dateien wandern mit."),
    ("import", "Importieren", "Die Fotos eines Ereignisses auswählen oder ins Fenster ziehen: Sie landen in einem neuen Ereignisordner und behalten ihre Daten."),
    ("dup", "Duplikate", "Findet identische, verkleinerte, bearbeitete und ähnliche Fotos. Du prüfst; die beste Kopie bleibt immer."),
    ("trash", "Sicherer Papierkorb", "Gelöschte Fotos landen in einem Papierkorb auf dem Laufwerk. Zurücklegen oder endgültig löschen."),
    ("people", "Personen & Haustiere", "Optionale Erkennung gruppiert Gesichter, Katzen und Hunde. Du vergibst Namen, shoebox schlägt den Rest vor."),
    ("drive", "Mehrere Laufwerke", "Die Fotos mehrerer Laufwerke in einer gemeinsamen Zeitleiste durchstöbern, auch wenn sie auf verschiedenen Platten liegen. Backups werden erkannt und geprüft."),
  ],
  "glance_h": "Die Oberfläche im Überblick",
  "glance_sub": "Alles ist auf einer Seite. So findest du dich zurecht.",
  "glance": [
    ("Ordner-Menü", "☰ blendet die Seitenleiste auf schmalen Bildschirmen ein oder aus."),
    ("Suchfeld", "Personen, Haustiere, Tags und Ordner; mehrere wählen, um sie zu kombinieren."),
    ("Typ", "Nur Fotos, Videos oder Live Photos zeigen."),
    ("Jahr", "Direkt zu einem Jahr springen."),
    ("Auswählen ☑", "Fotos zum Verschieben, Taggen oder in den Papierkorb wählen."),
    ("Importieren ⤒", "Fotos und Videos von diesem Computer oder vom iPad hinzufügen."),
    ("Schnellzugriff", "Alle Fotos · Duplikate · Papierkorb · Einstellungen."),
    ("Ordner", "Deine Ordner mit Anzahl. Tags, Gesichter und Laufwerke erscheinen hier, sobald es sie gibt."),
    ("Zeitleiste", "Monate mit Anzahl. Ein Klick auf ein Foto öffnet die Ansicht."),
    ("Sprache & Status", "Zwischen Deutsch und Englisch wechseln; Scan-, Vorschau- und Laufwerksstatus."),
  ],
  "session_h": "Ein typischer Ablauf",
  "session": "Laufwerk anstecken und <b>Start shoebox</b> doppelklicken. <b>Scannen</b> (geht schnell, wenn sich wenig geändert hat) und die Foto-App öffnen. Die Fotos eines Ereignisses <b>importieren</b>: auswählen, das Ereignis benennen, und shoebox legt den Ereignisordner an. Dann taggen und sortieren. Ab und zu die <b>Duplikate</b> ansehen und nach jedem Backup die <b>Backup-Prüfung</b> laufen lassen.",
  "start_h": "In drei Schritten loslegen",
  "start_sub": "macOS, Intel (ab 10.13) und Apple Silicon. Nichts zu installieren.",
  "steps": [
    ("Aufs Laufwerk legen", "<b>shoebox-macos.tar.gz</b> aus dem Release entpacken. <b>shoebox-macos</b> und <b>Start shoebox.command</b> in den obersten Ordner deines Foto-Laufwerks kopieren (neben deine Fotoordner). Optional: den Ordner <b>recognizer</b> für Gesichter."),
    ("Starter öffnen", "<b>Start shoebox.command</b> doppelklicken. Ein Terminal-Fenster öffnet sich (offen lassen) und der Starter erscheint im Browser. Fragt macOS nach, die Datei per Rechtsklick mit <b>Öffnen</b> starten."),
    ("Scannen, dann öffnen", "Laufwerk hinzufügen (anklicken oder <code>/Volumes/MeinLaufwerk</code> tippen), <b>Scannen</b> drücken, dann <b>Foto-App starten</b>. Der erste Scan liest jede Datei einmal und dauert bei großen Laufwerken eine Weile; später werden nur Änderungen angesehen."),
  ],
  "launcher_cap": "Der Starter: 1 Laufwerk oder Ordner wählen, 2 Aufgabe wählen, 3 Foto-App öffnen.",
  "launcher_tasks_h": "Aufgaben im Starter",
  "launcher_tasks": [
    ("Scannen", "neue, verschobene und geänderte Fotos finden"),
    ("Prüfen", "prüfen, dass nichts verloren oder beschädigt ist"),
    ("Erkennen", "Gesichter finden (mit „Haustiere erkennen“ auch Katzen und Hunde)"),
    ("Backup-Prüfung", "ist der 2. Ordner eine vollständige Kopie des 1.?"),
  ],
  "tip_h": "Gut zu wissen",
  "tips_start": [
    "Videos bekommen Vorschaubilder, wenn ein statisches <b>ffmpeg</b> neben dem Programm liegt (<code>.shoebox/bin/ffmpeg</code>). Ohne holt sich der Browser ein Bild.",
    "shoebox legt Index und Vorschaubilder im versteckten Ordner <code>.shoebox</code> auf dem Laufwerk ab. Das Laufwerk kann also an einen anderen Computer umziehen.",
    "Schließt du das Terminal-Fenster, beendet das shoebox.",
  ],
  "browse_h": "Stöbern und ansehen",
  "browse_sub": "Durch die Zeitleiste scrollen, nach Jahr springen, ein Foto öffnen.",
  "browse_items": [
    ("Zeitleiste", "Fotos sind nach Aufnahmedatum gruppiert, neueste zuerst. Das Menü <b>Jahr</b> springt; die Monatsüberschrift zeigt, wo du bist. Ein Foto ohne Aufnahmedatum nimmt den Monat aus dem Namen seines Ereignisordners (<code>2020-07</code>, <code>2020.07</code>, <code>20-07</code> oder <code>20.07</code>), sonst das Erstellungsdatum der Datei; das Infofeld markiert das Datum dann als <b>geschätzt</b>."),
    ("Ordner", "Die Seitenleiste spiegelt die Ordner deines Laufwerks. Ein Klick zeigt nur dessen Fotos. Einen Ordner kannst du über seinen Chip umbenennen oder verschieben."),
    ("Ansicht", "Foto anklicken. <b>ⓘ</b> öffnet das Infofeld: Datum (als <b>geschätzt</b> markiert, wenn es nicht der Aufnahmetag ist), Kamera, Größe, Tags, Personen und Knöpfe, um den Ordner zu öffnen oder den Pfad zu kopieren."),
    ("Drehen", "<b>r</b> dreht nach links, <b>Umschalt+R</b> nach rechts. Bei JPEGs ändern sich nur die zwei Bytes der Drehmarke; bei anderen Formaten gilt die Drehung nur in shoebox."),
  ],
  "grid_cap": "Die Zeitleiste: Monate, Ordner, Suche, Typfilter und Jahressprung.",
  "viewer_cap": "Die Ansicht mit Infofeld.",
  "keys_h": "Tastatur in der Ansicht",
  "keys": [("← →", "vorheriges / nächstes"), ("i", "Infofeld ein- / ausblenden"), ("Leertaste", "Ansicht öffnen / schließen (bei einem per Pfeiltasten gewählten Foto)"), ("r · Umschalt+R", "nach links / rechts drehen"), ("Esc", "Ansicht schließen")],
  "find_h": "Finden und ordnen",
  "find_sub": "Suchen, auswählen, taggen und verschieben: alles geht für viele Fotos auf einmal.",
  "find_items": [
    ("Suchen", "Tippe ins Suchfeld: Vorschläge erscheinen für Tags, Personen und Ordner. Mehrere gewählte werden zu Chips, die das Ergebnis eingrenzen; × entfernt einen, <b>Alle entfernen</b> beginnt neu."),
    ("Auswählen", "<b>☑</b> drücken, dann Fotos anklicken (mit Umschalt für einen Bereich; am iPad einfach tippen). „Alle auswählen“ an einer Monatsüberschrift nimmt den ganzen Monat."),
    ("Taggen", "<b>Tag hinzufügen …</b> gibt der Auswahl deinen eigenen Tag. Tags leben im Index von shoebox, nie in den Fotodateien."),
    ("Verschieben", "<b>Verschieben …</b> fragt nach einem Ordner. Neue Ordner werden angelegt, nichts wird überschrieben, RAW-, Live-Photo- und Sidecar-Dateien wandern mit. „Tags behalten“ entscheidet, ob die eigenen Tags der Fotos mitgehen; Ordner-Tags folgen immer dem neuen Ordner."),
  ],
  "search_cap": "Vorschläge beim Tippen.",
  "select_cap": "Drei Fotos ausgewählt; die Leiste bietet Verschieben, Tag und Papierkorb.",
  "tag_cap": "Tag hinzufügen.",
  "move_cap": "In einen neuen Ereignisordner verschieben.",
  "import_h": "Import, Duplikate und Papierkorb",
  "import_items": [
    ("Importieren", "<b>⤒</b> drücken (oder Dateien aufs Fenster ziehen). Jahr, Monat und Ereignisnamen wählen: shoebox legt den Ereignisordner <b>JJJJ-MM Ereignis</b> an, die Dateien behalten ihre Daten, und Dateien, die schon in der Bibliothek sind, werden übersprungen. <i>In Kürze:</i> In den <b>Einstellungen</b> wählst du dein eigenes Muster, mit J für das Jahr und M für den Monat, zum Beispiel <code>JJJJ.MM Ereignis</code>."),
    ("Duplikate", "<b>Duplikate</b> zeigt vier Arten: identisch, dasselbe Foto in anderer Größe, Original und bearbeitet sowie Ähnliche. Hake <b>diese Kopie löschen</b> bei dem an, was weg soll; die beste Datei bleibt, und pro Gruppe bleibt immer mindestens eine. Tags und Aufnahmedatum einer entfernten Kopie gehen auf die übrige über."),
    ("Abkürzungen", "<b>Kopien im selben Ordner entfernen</b> und <b>Kopien geringerer Qualität entfernen</b> räumen die eindeutigen Fälle in einem Zug auf. Gruppen ähnlicher Fotos brauchen deine Entscheidung: Wähle <b>Verschiedene Fotos</b> oder <b>Versionen eines Fotos</b>, und shoebox merkt sie sich, damit die Gruppe nicht wiederkommt."),
    ("Papierkorb", "Nichts wird sofort gelöscht. Gelöschte Fotos warten in <code>.shoebox/trash</code> auf dem Laufwerk: <b>Zurücklegen</b> oder <b>Endgültig löschen</b>."),
  ],
  "import_cap": "Der Import-Dialog.",
  "dups_cap": "Duplikate: eine vorgewählte Messenger-Kopie neben ihrem Original.",
  "trash_cap": "Die Papierkorb-Seite.",
  "people_h": "Personen und Haustiere",
  "people_sub": "Optional: shoebox findet Gesichter, Katzen und Hunde; du vergibst die Namen.",
  "people_items": [
    ("Finden", "Einmal den Recognizer installieren (<code>recognizer/install.sh /Volumes/MeinLaufwerk</code>). Dann die Foto-App schließen und im Starter <b>Erkennen</b> (und <b>Haustiere erkennen</b>) ausführen. In der Seitenleiste erscheint der Bereich <b>Gesichter</b>."),
    ("Benennen", "Unter <b>Unbenannt</b> zeigt shoebox Karten mit ähnlichen Gesichtern. Einmal einen Namen eintippen, und die ganze Karte ist benannt; später Vorschläge bestätigen (✓) oder ablehnen (✗). Personen gruppieren, zwei Karten einer Person zusammenführen, Fremde mit <b>Ignorieren</b> markieren."),
    ("In der Ansicht", "Das Infofeld nennt, wer auf dem Foto ist, und zeichnet um jedes Gesicht oder Tier einen Rahmen. Ein Klick auf einen Namen zeigt alle Fotos der Person; ein übersehenes Gesicht oder Tier kannst du mit einem Rahmen markieren."),
    ("Suchen", "Namen funktionieren im Suchfeld wie Tags, ebenso Arten wie „alle Katzen“. Kombiniere eine Person mit einem Tag, einem Ordner oder einer weiteren Person."),
    ("Neue Fotos", "Die Erkennung startet nach einem Import noch nicht von selbst. Foto-App schließen und <b>Erkennen</b> erneut ausführen: Es werden nur die neuen Fotos angesehen."),
  ],
  "unnamed_cap": "Unbenannt: ein Name für alle Gesichter einer Karte.",
  "info_people_cap": "Das Infofeld nennt das Mädchen und die Katze; Rahmen zeigen, wo sie sind.",
  "people_screens_note": "Die Beispielbilder sind gezeichnete Comics, keine echten Fotos.",
  "drives_h": "Laufwerke, Backups und das iPad",
  "drives_items": [
    ("Mehrere Laufwerke", "shoebox mit mehr als einem Laufwerk starten, und jedes erscheint in der Seitenleiste. <b>Alle Laufwerke</b> zeigt eine gemeinsame Zeitleiste über alle, dazu Backups, Fotos auf mehreren Laufwerken und Personen laufwerksübergreifend. Ein abgestecktes Laufwerk steht als offline da."),
    ("Backup-Prüfung", "Erst das Original wählen, dann das Backup. shoebox vergleicht die beiden Indizes (kein Foto wird gelesen) und listet, was noch nicht auf dem Backup ist, was abweicht und was nur auf dem Backup liegt. <i>Auch das Backup neu lesen</i> findet Bitfäule."),
    ("iPad & andere Geräte", "Im Terminal: <code>shoebox serve /Volumes/MeinLaufwerk --lan</code>. Es zeigt eine Adresse und eine PIN; die Adresse in Safari im selben WLAN öffnen und die PIN einmal eingeben. In der Ansicht wischen, zum Auswählen tippen."),
  ],
  "ipad_cap": "Am iPad funktioniert dieselbe Seite per Touch.",
  "safe_h": "Deine Fotos sind sicher",
  "safe_lede": "Die Grundregel ist einfach: shoebox liest deine Originale und schreibt nie etwas hinein.",
  "safe_items": [
    ("Nur lesen", "Metadaten, Vorschaubilder und Tags liegen im Index auf dem Laufwerk, nie in einem Foto. Aufnahme- und Erstellungsdatum ändern sich nie."),
    ("Bewacht", "Jeder Scan wird kontrolliert: Größe, Daten und ein vollständiger Hash davor und danach. Jeder Unterschied ist ein Fehler."),
    ("Nur auf deinen Befehl", "Originale ändern sich nur durch deine Aktionen: verschieben, umbenennen, Papierkorb, importieren. Immer ein Umbenennen, nie ein Kopieren, und nur wenn die Datei noch zum Index passt."),
    ("Lokal", "Keine Cloud, kein Konto. Standardmäßig kann sich nur dieser Computer verbinden; andere Geräte brauchen <code>--lan</code> und eine PIN."),
  ],
  "faq_h": "Wenn etwas komisch aussieht",
  "faq": [
    ("Das Raster ist leer.", "Erst im Starter <b>Scannen</b>, dann die Foto-App starten."),
    ("Neue Fotos fehlen.", "Noch einmal scannen. Das geht schnell: nur neue und geänderte Dateien werden gelesen."),
    ("Ein Video hat keine Vorschau.", "<code>ffmpeg</code> hinzufügen (siehe „Gut zu wissen“) und die Foto-App neu starten."),
    ("Das iPad verbindet sich nicht.", "Selbes WLAN? Mit <code>--lan</code> gestartet? PIN aus dem Terminal eingeben. Nach einem Neustart von shoebox fragt es wieder nach der PIN."),
    ("Ein Laufwerk steht auf offline.", "Wieder anstecken; shoebox findet es beim Neuladen."),
  ],
  "cli_h": "Kommandozeile (optional)",
  "cli": [
    ("shoebox scan &lt;Ordner&gt;", "neue, verschobene und geänderte Dateien indexieren"),
    ("shoebox verify &lt;Ordner&gt;", "Dateien neu lesen und mit dem Index vergleichen"),
    ("shoebox recognize &lt;Ordner&gt; [--pets]", "Gesichter (und Haustiere) finden"),
    ("shoebox backup &lt;Original&gt; &lt;Backup&gt;", "ist das Backup vollständig?"),
    ("shoebox serve &lt;Ordner&gt; [--lan] [--pin …]", "die Foto-App öffnen"),
  ],
  "lang_note": "Die App spricht Deutsch und Englisch: Die Sprache wählst du unten in der Seitenleiste.",
  "about": "shoebox ist Open Source · github.com/svenfritsch/shoebox",
},
}

CSS = """
@page { size: A4; margin: 0 }
:root { --brown:#a0602c; --tan:#d9a066; --ink:#2a2420; --muted:#6f655c; --cream:#faf8f5; --soft:#f1ece5; --line:#e4dcd1 }
* { box-sizing: border-box; margin: 0; padding: 0 }
html { -webkit-print-color-adjust: exact; print-color-adjust: exact }
body { font: 9.4pt/1.45 Inter, 'Helvetica Neue', Arial, sans-serif; color: var(--ink); background: #fff }
.page { width: 210mm; height: 297mm; position: relative; overflow: hidden; padding: 15mm 16mm 18mm; page-break-after: always; background: var(--cream) }
.page:last-child { page-break-after: auto }
h1 { font-size: 30pt; line-height: 1.08; letter-spacing: -.02em }
h2 { font-size: 19pt; letter-spacing: -.015em; line-height: 1.15; margin-bottom: 1.5mm }
h3 { font-size: 10pt; margin-bottom: .6mm }
.sub { color: var(--muted); font-size: 10pt; margin-bottom: 6mm }
p { margin-bottom: 1.5mm }
code { font: 8.2pt 'DejaVu Sans Mono', monospace; background: var(--soft); padding: .3mm 1.2mm; border-radius: 1mm; color: #6b3d17; white-space: nowrap }
b { font-weight: 650 }
.foot { position: absolute; left: 16mm; right: 16mm; bottom: 8mm; display: flex; justify-content: space-between; color: #9a8f85; font-size: 7.5pt; border-top: .3mm solid var(--line); padding-top: 2mm }
.logo { width: 11mm; height: 11mm; vertical-align: middle }
.brand { display: flex; align-items: center; gap: 3mm; font-weight: 700; font-size: 15pt; letter-spacing: -.01em }

/* cover */
.cover { padding: 0; background: linear-gradient(165deg, #fff3e2 0%, #f6dfbf 45%, #e3b583 100%) }
.cover .top { padding: 16mm 16mm 0 }
.cover .hero { padding: 0 16mm; margin-top: 16mm }
.cover h1 { font-size: 38pt; max-width: 150mm; color: #3a2512 }
.cover .lede { font-size: 11.5pt; line-height: 1.5; max-width: 148mm; margin-top: 6mm; color: #4a3622 }
.badges { display: flex; flex-wrap: wrap; gap: 2.5mm; margin-top: 7mm }
.badge { display: flex; align-items: center; gap: 2mm; background: rgba(255,255,255,.7); border: .3mm solid rgba(160,96,44,.25); padding: 1.8mm 3.4mm 1.8mm 2.4mm; border-radius: 99mm; font-weight: 600; font-size: 8.8pt; color: #5a3416 }
.badge svg { width: 4.6mm; height: 4.6mm; color: var(--brown) }
.cover .shotwrap { position: absolute; left: 16mm; right: 16mm; bottom: 14mm; }
.cover .note { position: absolute; left: 16mm; top: 17mm; right: 16mm; display: flex; justify-content: flex-end; font-size: 8.5pt; color: #7a5632 }
.cover .note span { text-align: right }

/* screenshots */
.shot { background: #fff; border-radius: 2.2mm; box-shadow: 0 1.2mm 5mm rgba(60,35,10,.22), 0 0 0 .25mm rgba(60,35,10,.12); overflow: hidden }
.shot .bar { height: 5.2mm; background: #ece7e0; display: flex; align-items: center; gap: 1.2mm; padding-left: 2.2mm }
.shot .bar i { width: 1.8mm; height: 1.8mm; border-radius: 50%; background: #cfc6bb; display: block }
.shot img { display: block; width: 100% }
.shot.crop img { height: var(--h); object-fit: cover; object-position: top left }
.shot.cropb img { height: var(--h); object-fit: cover; object-position: bottom left }
figure { margin: 0 }
figcaption { color: var(--muted); font-size: 7.8pt; margin-top: 1.6mm }

/* grids */
.cards { display: grid; grid-template-columns: repeat(3, 1fr); gap: 4mm }
.card { background: #fff; border: .3mm solid var(--line); border-radius: 3mm; padding: 4.5mm 4.5mm 4mm }
.card .ic { width: 9mm; height: 9mm; border-radius: 2.4mm; background: #f6e6d0; color: var(--brown); display: flex; align-items: center; justify-content: center; margin-bottom: 3mm }
.card .ic svg { width: 5.4mm; height: 5.4mm }
.card h3 { font-size: 10.5pt }
.card p { color: var(--muted); font-size: 8.7pt; margin: 0; line-height: 1.42 }
.two { display: grid; grid-template-columns: 1fr 1fr; gap: 6mm }
.items > div { margin-bottom: 3.4mm }
.items h3 { color: var(--brown) }
.items p { margin: 0 }
.step { display: flex; gap: 4mm; margin-bottom: 4.2mm }
.step .n { flex: none; width: 8mm; height: 8mm; border-radius: 50%; background: var(--brown); color: #fff; font-weight: 700; font-size: 11pt; display: flex; align-items: center; justify-content: center; margin-top: .5mm }
.step h3 { font-size: 11pt }
.panel { background: #fff; border: .3mm solid var(--line); border-radius: 3mm; padding: 4mm 5mm }
.panel h3 { color: var(--brown); text-transform: uppercase; letter-spacing: .06em; font-size: 7.8pt; margin-bottom: 2mm }
.panel ul { padding-left: 4mm }
.panel li { margin-bottom: 1.8mm }
.kv { display: grid; grid-template-columns: 34mm 1fr; gap: 1.2mm 3mm; font-size: 8.8pt }
.kv .k { font-weight: 650 }
.keys .k { font: 8.2pt 'DejaVu Sans Mono', monospace; background: var(--soft); border-radius: 1mm; padding: .4mm 1.6mm; width: max-content }
.faq p { margin-bottom: 1.8mm }
.faq b { color: var(--brown) }
.safe { display: grid; grid-template-columns: 1fr 1fr; gap: 4mm }
.safe .card .ic { background: #e5efe1; color: #3f7a3a }
.pad { display: flex; gap: 6mm; align-items: flex-start }
.stack > * + * { margin-top: 5mm }
.dot { position: absolute; width: 4.6mm; height: 4.6mm; margin: -2.3mm 0 0 -2.3mm; border-radius: 50%; background: var(--brown); color: #fff; font-weight: 700; font-size: 7.6pt; display: flex; align-items: center; justify-content: center; box-shadow: 0 0 0 .5mm #fff, 0 .4mm 1.4mm rgba(0,0,0,.35) }
.dot.s { position: static; margin: .3mm 0 0 0; flex: none; box-shadow: none }
.legend { display: grid; grid-template-columns: 1fr 1fr; gap: 3.2mm 7mm; margin-top: 7mm }
.lg { display: flex; gap: 2.6mm; font-size: 8.8pt; color: var(--muted) }
.lg b { color: var(--ink) }
"""


def shot(lang, name, cap=None, cls="", style=""):
    c = f"<figcaption>{cap}</figcaption>" if cap else ""
    return (f'<figure><div class="shot {cls}" style="{style}"><div class="bar"><i></i><i></i><i></i></div>'
            f'<img src="shots/{lang}/{name}.jpg"></div>{c}</figure>')


def items(lst):
    return '<div class="items">' + "".join(f"<div><h3>{h}</h3><p>{p}</p></div>" for h, p in lst) + "</div>"


def page(t, n, body, cls=""):
    return (f'<section class="page {cls}">{body}<div class="foot"><span>{t["footer"]}</span>'
            f'<span>@@N@@</span></div></section>')


def build(lang):
    t = T[lang]
    L = lang
    pages = []

    # 1 cover
    badges = "".join(f'<div class="badge">{ICONS[i]}<span>{s}</span></div>' for i, s in t["badges"])
    pages.append(f'''<section class="page cover">
      <div class="top"><div class="brand">{LOGO} shoebox</div></div>
      <div class="note"><span>{t["cover_note"]}<br>{t["other_lang"]}</span></div>
      <div class="hero"><h1>{t["tagline"]}</h1><p class="lede">{t["lede"]}</p><div class="badges">{badges}</div></div>
      <div class="shotwrap">{shot(L, "01-grid", None, "crop", "--h:132mm")}</div></section>''')

    # 2 overview
    cards = "".join(f'<div class="card"><div class="ic">{ICONS[i]}</div><h3>{h}</h3><p>{p}</p></div>' for i, h, p in t["features"])
    pages.append(page(t, 2, f'<h2>{t["ov_h"]}</h2><p class="sub">{t["ov_sub"]}</p><div class="cards">{cards}</div>'
                      f'<div style="margin-top:6mm">{shot(L, "07-search-chips", None, "crop", "--h:84mm")}</div>'))

    # 2b at a glance
    pos = [(5.0, 8.9), (63, 7.8), (82.2, 7.8), (89.2, 7.8), (94.0, 7.8), (97.4, 7.8),
           (20.6, 11.6), (20.6, 26), (58, 33), (19.2, 91.5)]
    dots = "".join(f'<span class="dot" style="left:{x}%;top:{y}%">{i+1}</span>' for i, (x, y) in enumerate(pos))
    legend = "".join(f'<div class="lg"><span class="dot s">{i+1}</span><div><b>{h}</b><br>{p}</div></div>' for i, (h, p) in enumerate(t["glance"]))
    pages.append(page(t, 0, f'''<h2>{t["glance_h"]}</h2><p class="sub">{t["glance_sub"]}</p>
      <div class="shot"><div class="bar"><i></i><i></i><i></i></div><div style="position:relative"><img src="shots/{L}/01-grid.jpg">{dots}</div></div>
      <div class="legend">{legend}</div>
      <div class="panel" style="margin-top:7mm"><h3>{t["session_h"]}</h3><p style="margin:0">{t["session"]}</p></div>'''))

    # 3 start
    steps = "".join(f'<div class="step"><div class="n">{i+1}</div><div><h3>{h}</h3><p>{p}</p></div></div>' for i, (h, p) in enumerate(t["steps"]))
    tasks = "".join(f'<div class="k">{a}</div><div>{b}</div>' for a, b in t["launcher_tasks"])
    tips = "".join(f"<li>{x}</li>" for x in t["tips_start"])
    pages.append(page(t, 3, f'''<h2>{t["start_h"]}</h2><p class="sub">{t["start_sub"]}</p>
      <div class="two" style="grid-template-columns:1.05fr .95fr">
        <div>{steps}<div class="panel" style="margin-top:3mm"><h3>{t["launcher_tasks_h"]}</h3><div class="kv">{tasks}</div></div></div>
        <div>{shot(L, "12-launcher", t["launcher_cap"])}</div>
      </div>
      <div class="panel" style="margin-top:6mm"><h3>{t["tip_h"]}</h3><ul>{tips}</ul><p style="margin:2mm 0 0 0;color:var(--muted)">{t["lang_note"]}</p></div>'''))

    # 4 browse
    keys = "".join(f'<div class="k">{a}</div><div>{b}</div>' for a, b in t["keys"])
    pages.append(page(t, 4, f'''<h2>{t["browse_h"]}</h2><p class="sub">{t["browse_sub"]}</p>
      <div class="stack">
        {shot(L, "01-grid", t["grid_cap"], "crop", "--h:92mm")}
        <div class="two" style="grid-template-columns:1fr 1.12fr">
          <div>{items(t["browse_items"])}</div>
          <div>{shot(L, "02-viewer", t["viewer_cap"])}
            <div class="panel keys" style="margin-top:4mm"><h3>{t["keys_h"]}</h3><div class="kv" style="grid-template-columns:30mm 1fr">{keys}</div></div></div>
        </div>
      </div>'''))

    # 5 find + organise
    pages.append(page(t, 5, f'''<h2>{t["find_h"]}</h2><p class="sub">{t["find_sub"]}</p>
      <div class="two" style="grid-template-columns:1fr 1fr;gap:7mm">
        <div>{items(t["find_items"])}</div>
        <div class="stack">{shot(L, "06-search-suggest", t["search_cap"], "crop", "--h:42mm")}
          {shot(L, "03-select", t["select_cap"], "crop", "--h:58mm")}</div>
      </div>
      <div class="two" style="margin-top:6mm">
        {shot(L, "04-tag", t["tag_cap"], "crop", "--h:70mm")}
        {shot(L, "05-move", t["move_cap"], "crop", "--h:70mm")}
      </div>'''))

    # 6 import / duplicates / trash
    pages.append(page(t, 6, f'''<h2>{t["import_h"]}</h2><p class="sub">&nbsp;</p>
      <div class="two" style="grid-template-columns:1.08fr .92fr;gap:7mm">
        <div>{items(t["import_items"])}</div>
        <div class="stack">{shot(L, "08-import", t["import_cap"], "crop", "--h:50mm")}
          {shot(L, "10-trash", t["trash_cap"], "crop", "--h:50mm")}</div>
      </div>
      <div style="margin-top:5mm">{shot(L, "09-duplicates", t["dups_cap"], "cropb", "--h:86mm")}</div>'''))

    # 7 people and pets
    pages.append(page(t, 7, f'''<h2>{t["people_h"]}</h2><p class="sub">{t["people_sub"]}</p>
      <div class="pad">
        <div style="flex:1.05">{items(t["people_items"])}</div>
        <div style="flex:1" class="stack">{shot(L, "14-unnamed", t["unnamed_cap"], "crop", "--h:64mm")}
          {shot(L, "16-info-people", t["info_people_cap"])}
          <p style="color:var(--muted);font-size:7.8pt">{t["people_screens_note"]}</p></div>
      </div>'''))

    # 8 drives, backups, ipad
    pages.append(page(t, 8, f'''<h2>{t["drives_h"]}</h2><p class="sub">&nbsp;</p>
      <div class="pad">
        <div style="flex:1.7">{items(t["drives_items"])}
          <div class="panel" style="margin-top:6mm"><h3>{t["cli_h"]}</h3><div class="kv" style="grid-template-columns:1fr">{"".join(f'<div><code>{a}</code><br><span style="color:var(--muted)">{b}</span></div>' for a, b in t["cli"])}</div></div></div>
        <div style="flex:1">{shot(L, "13-ipad", t["ipad_cap"], "crop", "--h:150mm")}</div>
      </div>'''))

    # 8 safety + faq
    safe = "".join(f'<div class="card"><div class="ic">{ICONS[i]}</div><h3>{h}</h3><p>{p}</p></div>'
                   for i, (h, p) in zip(["lock", "shield", "bolt", "globe"], t["safe_items"]))
    faq = "".join(f"<p><b>{q}</b> {a}</p>" for q, a in t["faq"])
    pages.append(page(t, 8, f'''<h2>{t["safe_h"]}</h2><p class="sub">{t["safe_lede"]}</p>
      <div class="safe">{safe}</div>
      <h2 style="margin-top:10mm">{t["faq_h"]}</h2><div class="panel faq" style="margin-top:3mm">{faq}</div>
      <p style="margin-top:8mm;color:var(--muted);text-align:center">{t["about"]}</p>'''))

    body = "".join(pages)
    for i in range(1, 100):
        if "@@N@@" not in body: break
        body = body.replace("@@N@@", str(i + 1), 1)
    html = (f'<!doctype html><html lang="{t["lang"]}"><head><meta charset="utf-8"><title>{t["title"]}</title>'
            f'<style>{CSS}</style></head><body>{body}</body></html>')
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, f"{lang}.html"), "w") as f:
        f.write(html)


def copy_shots():
    import shutil
    for lang in T:
        d = os.path.join(OUT, "shots", lang)
        shutil.rmtree(d, ignore_errors=True)
        shutil.copytree(os.path.join(HERE, "shots", lang), d)


copy_shots()
for lang in T:
    build(lang)
print("wrote", OUT)
