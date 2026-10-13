---
name: guide
description: Update the shoebox user guide (docs/guide/shoebox-en.html and shoebox-de.html) for a change to what a person can see or do, add or change a guide screenshot, or fix a failing guide check. Use for every pull request that touches the photo app, the launcher, a button, a setting, a command or the safety rules.
---

# Updating the guide

The guide is two hand-written HTML files, `docs/guide/shoebox-en.html` and
`docs/guide/shoebox-de.html`, kept in step. They are the only files you edit
for the text. **Screenshots and the `.txt` copies are not in git**: CI makes
them from the code of the commit (`.github/workflows/guide.yml`) and puts the
finished guide into the run's artifact `guide` and into every release. You do
not build shoebox or run a browser to update the guide, and you never run the
screenshot scripts outside CI.

## 1. Find every place that talks about the change

Take UI names from `core/i18n/en.json` and `core/i18n/de.json` (the guide
uses the exact button and menu words). Then grep both files for the old
behaviour, its old and new UI names, and its i18n words:

```sh
grep -n -i -e '<old word>' -e '<new word>' docs/guide/shoebox-*.html
```

Read the sections that touch the feature, not only the grep hits. The
sections (`<section id=...>`, same order in both files):

| id | what | look out for |
|----|------|--------------|
| `why`, `features` | what sets shoebox apart, the feature cards | one card per feature; a new feature may need a card |
| `glance` | "The screen at a glance": `ui-overview` with numbered dots and a legend | one dot per legend item, in the order of the screen; see step 3 |
| `glance` (panel) | "A typical session" | the everyday flow; names of the launcher buttons |
| `navigate` | "Finding your way around": top bar, sidebar, folding sections | where the screen shows where you are |
| `start` | get started, Control Panel (launcher) tasks | launcher buttons and what they do |
| `browse`, `find`, `dates` | viewer, search, select, tag, move, export, dates | keyboard shortcuts, menus |
| `scanning` | scanning, duplicates, trash | the safety wording |
| `people`, `faces`, `groups` | people and pets, naming, groups | |
| `locations` | maps (opt-in) | |
| `drives` | several drives, backups, iPad | |
| `safety` | "Your photos are safe" | must match the rules in `CLAUDE.md` |
| `faq` | "If something looks off" | |

A removed feature leaves the guide entirely: cards, legend, sections, FAQ.

## 2. Edit both languages in step

- Text and its screenshots sit together in a `topic` block (`<div class="topic has-shots">`: `.txt` and `.shots`), so a picture is always beside the paragraph that describes it. Same sections, same paragraphs, same screenshots in the same order, same
  dots. German uses the informal "du" (docs/phase8.md).
- Inline styles only, no scripts, no build step. Elements with
  `data-txt="skip"` are left out of the text copy.
- Sample screenshots must not show a word that is a feature now (the tag
  "Favorites" is the heart; the samples use "Highlights").

## 3. Screenshots

The screenshots come from `docs/guide/screenshots.mjs` (Playwright), driven by
`docs/guide/screenshots.sh` over a drawn sample library. When the screen
changes, CI makes new ones; you do nothing. Only these need you:

- **A new screenshot**: add a step to the right pass of `screenshots.mjs`
  (`app`, `launcher`, `drives` or `suggestions`; a step is a few clicks and
  `await shot('name')`), then an `<img src="assets/en/name.jpg">` in the
  English file and `assets/de/name.jpg` in the German one.
- **A screenshot that no longer fits the text**: change its step in
  `screenshots.mjs` (selectors by id or by i18n label, not by position).
- **A dot of "The screen at a glance" off its control**: `check.py` (CI)
  names the dot and prints the centre of its control as `left:..%;top:..%`.
  Dots sit just below their header control (top about 7.8%). Change the
  dot's `style` in both files. When a legend item is added or removed, also
  change the `GLANCE` list in `screenshots.mjs` (one selector per dot, in the
  order of the legend).

Never run `screenshots.sh` yourself: it runs only in CI (it creates and
deletes folders where real drives live, and refuses anywhere else). To see the
pictures, push and open the `guide` artifact of the run.

## 4. Check, then say what changed

```sh
python3 docs/guide/check.py   # seconds: en/de parity; with assets/ also images and dots
```

In the pull request, the "Guide" part of the template: what changed in the
guide, or that it is not affected. CI's `guide` job renders the screenshots
for pull requests that touch `core/web`, `core/i18n` or `docs/guide`; its
artifact `guide` is the finished guide to look at.

Look at `main` first: a pull request stacked on a branch that is already
merged never reaches `main`.
