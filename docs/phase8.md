# Phase 8: translation (German and English)

The photo app and the launcher speak English and German. Flat JSON message
files, no dependencies, no build step.

## Design

- Files: `core/i18n/en.json`, `core/i18n/de.json` and `core/i18n/i18n.js`,
  embedded by both servers (the photo app and the launcher) under `/i18n/…`.
- Keys are flat, `area.name` (`launcher.step1`, `dups.title`). Placeholders
  are `{name}`; numbers passed to them are formatted for the language.
- In code: `tr('key', {name: x})`; plurals `trn('key', n)` with `key.one` and
  `key.other` (`{n}` is the count, formatted). The plural rule comes from
  `Intl.PluralRules`, so a language that needs `few`/`many` only adds keys.
- In HTML: `data-i18n="key"` for the text, `data-i18n-title`,
  `-placeholder` and `-aria-label` for attributes.
- English is the fallback for any key missing in another language.
- Language: the saved choice (`localStorage`, key `shoebox.lang`), else the
  first browser language we have, else English. A selector in the sidebar
  footer (launcher: page footer) saves the choice and reloads, because the
  UI is built once from the messages.
- Dates and numbers use `I18n.date` / `I18n.number` (the chosen language, not
  the browser's).
- German uses the informal "du".
- The app waits for `I18n.ready` before it starts, so nothing is drawn with
  keys.

## Not translated (on purpose)

- Messages made by the Rust side and shown as they are: API error texts, job
  progress labels and result notes in the launcher, the CLI output. They
  would need stable error codes; later, if it matters.
- File names, folder names, tag and people names: yours.

## Tests

`core/tests/i18n.rs` (reads files only): German has exactly English's keys
with the same placeholders, plural messages have an `.other` form, every
`area.name` the code mentions exists, and no message is unused.

## Adding a message or a language

1. Add the key to `en.json` and `de.json` (the test fails if one is missing).
2. Use it with `tr()` / `trn()` / `data-i18n`.
3. A new language: `xx.json` with all keys, an entry in `LANGS` in `i18n.js`.

## Status

| Step | Status |
|---|---|
| Loader, embedding, key test, launcher | Done |
| Photo app: static page, login, sidebar, toolbar, language selector | Done |
| Photo app: grid, viewer, info panel, status line | Done |
| Photo app: selection, move, trash, import, tags | Done |
| Photo app: duplicates, trash page | Done |
| Photo app: faces and people | Done |
| Photo app: drives | Done |
| Check on real hardware (iPad: language switch, dates, long German texts in the dialogs) | Open |
| Confirm the GitHub Actions run is green (`core/tests/i18n.rs` could only be run on its own here) | Open |
| Server-side texts (error messages, job labels in the launcher) | Open, optional (see above) |

The German texts were written without a native review: read through them once
in the UI.
