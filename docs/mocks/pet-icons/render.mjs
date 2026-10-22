import { chromium } from 'playwright';
const b = await chromium.launch({ executablePath: '/opt/pw-browsers/chromium' });
const p = await b.newPage({ viewport: { width: 1200, height: 800 }, deviceScaleFactor: 2 });
for (const n of ['grid-A', 'grid-B', 'grid-C', 'header', 'options']) {
  await p.goto('file://' + process.cwd() + '/' + n + '.html');
  await p.locator('.card').screenshot({ path: n + '.png' });
}
await b.close();
