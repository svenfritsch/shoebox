// After 'ended' (endOfStream), seek back outside the buffer and play again.
//   node endseek.cjs <port> <name>
const { chromium } = require('playwright');
const [port, name] = process.argv.slice(2);
(async () => {
  const b = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
  const p = await b.newPage();
  await p.goto(`http://127.0.0.1:${port}/`);
  await p.evaluate((n) => spike.open(n, { mode: 'mse' }), name);
  await p.waitForFunction(() => document.querySelector('video').currentTime > 0.05, null, { timeout: 90000 });
  await p.evaluate(() => { const v = document.querySelector('video'); v.currentTime = v.duration - 4; });
  const ended = await p.waitForFunction(() => document.querySelector('video').ended, null, { timeout: 60000 }).then(() => true, () => false);
  const t0 = Date.now();
  await p.evaluate(() => { const v = document.querySelector('video'); v.currentTime = v.duration / 2; v.play(); });
  const again = await p.waitForFunction(() => { const v = document.querySelector('video'); return !v.seeking && v.currentTime > v.duration / 2 + 2; }, null, { timeout: 60000 }).then(() => true, () => false);
  const st = await p.evaluate(() => ({ ranges: spike.ranges(), msState: spike.P.ms.readyState, errs: spike.log.filter((e) => /error|quota/.test(e.ev)), pe: [] }));
  console.log(JSON.stringify({ name, ended, playedAgainAfterEnd: again, ms: Date.now() - t0, ...st }));
  await b.close();
})();
