// Does ffmpeg stop working when nobody watches? Open a video (MSE), play 5 s,
// pause, sample ffmpeg's CPU for <seconds>; play again (does it go on?); close
// the page and check that ffmpeg is gone after the linger time.
//   node pause.cjs <port> <name> [seconds=120]
const { chromium } = require('playwright');
const [port, name, secs = '120'] = process.argv.slice(2);
const base = `http://127.0.0.1:${port}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stats = async () => (await fetch(`${base}/stats`)).json();
(async () => {
  await fetch(`${base}/stats`, { method: 'POST' });
  const b = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
  const p = await b.newPage();
  await p.goto(base + '/');
  await p.evaluate((n) => spike.open(n, { mode: 'mse' }), name);
  await p.waitForFunction(() => document.querySelector('video').currentTime > 0.05, null, { timeout: 90000 });
  await sleep(5000);
  await p.evaluate(() => document.querySelector('video').pause());
  const samples = [];
  let prevCpu = null;
  for (let i = 0; i <= +secs; i += 5) {
    const s = await stats(); const run = s.runs.at(-1); const tr = run.cpuTrace.at(-1) || [0, 0];
    const page = await p.evaluate(() => { const v = document.querySelector('video'); return { ct: +v.currentTime.toFixed(1), ranges: spike.ranges() }; });
    samples.push({ t: i, ffCpuSec: tr[1], cores5s: prevCpu === null ? null : +((tr[1] - prevCpu) / 5).toFixed(2), paused: s.paused,
      producedEnd: s.conv && +s.conv.producedEnd.toFixed(1), bufferMB: +(s.bufferBytes / 1048576).toFixed(1), pageCt: page.ct, pageBufferedEnd: page.ranges.at(-1)?.[1] });
    prevCpu = tr[1];
    await sleep(5000);
  }
  // play again: does the stream go on?
  const ct0 = await p.evaluate(() => document.querySelector('video').currentTime);
  await p.evaluate(() => document.querySelector('video').play());
  await sleep(10000);
  const ct1 = await p.evaluate(() => document.querySelector('video').currentTime);
  const resumed = { advancedIn10s: +(ct1 - ct0).toFixed(2), waiting: await p.evaluate(() => spike.log.filter((e) => e.ev === 'waiting').length) };
  // close: ffmpeg must be gone after the linger time (5 s)
  await p.close();
  const tClose = Date.now();
  let goneAfterMs = null;
  for (let i = 0; i < 40; i++) { const s = await stats(); if (!s.conv || s.conv.done) { goneAfterMs = Date.now() - tClose; break; } await sleep(250); }
  const s = await stats();
  console.log(JSON.stringify({ name, samples, resumed, goneAfterMs, stoppedBy: s.runs.at(-1).stoppedBy, pausedMs: s.runs.at(-1).pausedMs }));
  await b.close();
})();
