// node shoot.mjs: every *.html here -> *.png, cropped to the size in its <meta name=w/h>, dark theme, 2x
import { chromium } from '/opt/node-tools/node_modules/playwright/index.mjs';
import fs from 'node:fs'; import path from 'node:path'; import { fileURLToPath } from 'node:url';
const dir = path.dirname(fileURLToPath(import.meta.url));
const b = await chromium.launch({ executablePath: process.env.CHROME || undefined });
const p = await b.newPage({ viewport: { width: 900, height: 600 }, deviceScaleFactor: 2, colorScheme: 'dark' });
for (const f of fs.readdirSync(dir).filter(f => f.endsWith('.html'))) {
  await p.goto('file://' + path.join(dir, f)); await p.waitForTimeout(150);
  const w = +(await p.getAttribute('meta[name=w]', 'content')), h = +(await p.getAttribute('meta[name=h]', 'content'));
  await p.screenshot({ path: path.join(dir, f.replace('.html', '.png')), clip: { x: 0, y: 0, width: w, height: h } });
}
await b.close();
