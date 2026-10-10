// node shoot.mjs: every *.html here -> *.png (900x604 CSS px at 2x, like docs/guide/assets)
import { chromium } from '/opt/node-tools/node_modules/playwright/index.mjs';
import fs from 'node:fs'; import path from 'node:path'; import { fileURLToPath } from 'node:url';
const dir = path.dirname(fileURLToPath(import.meta.url));
const b = await chromium.launch({ executablePath: process.env.CHROME || undefined });
const p = await b.newPage({ viewport: { width: 900, height: 604 }, deviceScaleFactor: 2 });
for (const f of fs.readdirSync(dir).filter(f => f.endsWith('.html'))) {
  await p.goto('file://' + path.join(dir, f)); await p.waitForTimeout(200);
  await p.screenshot({ path: path.join(dir, f.replace('.html', '.png')) });
}
await b.close();
