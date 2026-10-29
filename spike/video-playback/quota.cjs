// What happens when the SourceBuffer grows past the browser's limit? Jump to
// several places, let each fill AHEAD seconds, then play at each for a moment.
//   node quota.cjs <port> <name> <t1,t2,...>
const { chromium } = require('playwright');
const [port, name, list] = process.argv.slice(2);
const base = `http://127.0.0.1:${port}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  const b = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
  const p = await b.newPage();
  await p.goto(base + '/');
  await p.evaluate((n) => spike.open(n, { mode: 'mse' }), name);
  await p.waitForFunction(() => document.querySelector('video').currentTime > 0.05, null, { timeout: 90000 });
  const steps = [];
  for (const t of list.split(',').map(Number)) {
    await p.evaluate((to) => { const v = document.querySelector('video'); v.pause(); v.currentTime = to; }, t);
    // wait until 55 s ahead are buffered, at most 90 s
    const t0 = Date.now();
    await p.waitForFunction((to) => { const v = document.querySelector('video'); const r = spike.ranges().find(([a, e]) => to >= a - 0.2 && to <= e); return r && r[1] - to > 55; }, t, { timeout: 90000, polling: 500 }).catch(() => {});
    const filled = (Date.now() - t0) / 1000;
    await p.evaluate(() => document.querySelector('video').play());
    const ct0 = await p.evaluate(() => document.querySelector('video').currentTime);
    await sleep(3000);
    const st = await p.evaluate(() => ({ ct: document.querySelector('video').currentTime, ranges: spike.ranges(), quota: spike.log.filter((e) => e.ev === 'quota').length, errs: spike.log.filter((e) => /stream:error|stream:http/.test(e.ev)).map((e) => e.msg || e.status) }));
    const totalBuffered = st.ranges.reduce((s, [a, e]) => s + e - a, 0);
    steps.push({ t, filledSec: filled, played3s: +(st.ct - ct0).toFixed(2), bufferedSec: +totalBuffered.toFixed(0), ranges: st.ranges, quotaEvents: st.quota, errors: st.errs });
    console.log(JSON.stringify(steps.at(-1)));
  }
  await b.close();
})();
