// Render _html/<lang>.html to shoebox-guide-<lang>.pdf. usage: node pdf.mjs <lang>...
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
const { chromium } = createRequire(import.meta.url)('playwright');
const here = path.dirname(fileURLToPath(import.meta.url));
const b = await chromium.launch(process.env.CHROME ? { executablePath: process.env.CHROME } : {});
for (const lang of process.argv.slice(2)) {
  const p = await b.newPage();
  await p.goto('file://' + path.join(here, '_html', lang + '.html'));
  await p.evaluate(() => document.fonts.ready);
  await p.pdf({ path: path.join(here, `shoebox-guide-${lang}.pdf`), preferCSSPageSize: true, printBackground: true });
}
await b.close();
