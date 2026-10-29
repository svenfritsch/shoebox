// Drives page.html in Playwright's Chromium against a running server.mjs.
//   node drive.cjs <port> <name> <mode auto|mse> <runs> [out.json]
// Per run: open, time to first picture, play 15 s, seek forward outside the
// buffer, seek back inside it, seek back outside it, seek to the end and wait
// for 'ended'. Prints one JSON line per run.
const { chromium } = require('playwright');
const fs = require('fs');
const [port, name, mode = 'mse', runs = '3', out] = process.argv.slice(2);
const base = `http://127.0.0.1:${port}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function stats(reset) {
  const r = await fetch(`${base}/stats`, { method: reset ? 'POST' : 'GET' });
  return r.json();
}

// set currentTime and wait until 'seeked' and playback has moved past the target
const SEEK = `async ([to, timeout]) => {
  const v = document.querySelector('video');
  const t0 = performance.now();
  let seekedAt = null;
  const seeked = new Promise((r) => v.addEventListener('seeked', () => { seekedAt = performance.now() - t0; r(); }, { once: true }));
  const before = spike.ranges();
  v.currentTime = to;
  // playing again: the first animation frame on which currentTime is past the target
  const moved = new Promise((r) => { const f = () => { if (!v.seeking && v.currentTime > to + 0.04) r(true); else requestAnimationFrame(f); }; requestAnimationFrame(f); });
  const ok = await Promise.race([Promise.all([seeked, moved]).then(() => true), new Promise((r) => setTimeout(() => r(false), timeout))]);
  return { to, ok, seekedMs: seekedAt && +seekedAt.toFixed(0), playingAgainMs: ok ? +(performance.now() - t0).toFixed(0) : null,
           ct: +v.currentTime.toFixed(2), before, after: spike.ranges(), streams: spike.P.streams };
}`;

async function oneRun(browser, i) {
  await stats(true);
  const ctx = await browser.newContext();
  const p = await ctx.newPage();
  const errors = [];
  p.on('pageerror', (e) => errors.push(String(e)));
  await p.goto(base + '/');
  const r = { name, mode, run: i };
  const t0 = Date.now();
  await p.evaluate(([n, m]) => spike.open(n, { mode: m }), [name, mode]);
  // first picture: 'playing' with videoWidth > 0 and currentTime moving
  const first = await p.waitForFunction(() => { const v = document.querySelector('video'); return v.videoWidth > 0 && v.currentTime > 0.05 && !v.paused; }, null, { timeout: 90000, polling: 50 }).then(() => true, () => false);
  r.firstPictureMs = first ? Date.now() - t0 : null;
  const meta = await p.evaluate(() => ({ trigger: spike.P.trigger, mode: spike.P.mode, w: document.querySelector('video').videoWidth, h: document.querySelector('video').videoHeight, duration: document.querySelector('video').duration }));
  Object.assign(r, meta);
  if (!first) { r.log = await p.evaluate(() => spike.log); r.errors = errors; await ctx.close(); return r; }
  // play 15 s
  const a = await p.evaluate(() => ({ ct: document.querySelector('video').currentTime, q: spike.quality().totalVideoFrames }));
  const w0 = Date.now(); await sleep(15000);
  const b = await p.evaluate(() => ({ ct: document.querySelector('video').currentTime, q: spike.quality() }));
  r.play15 = { advanced: +(b.ct - a.ct).toFixed(2), wall: (Date.now() - w0) / 1000, frames: b.q.totalVideoFrames - a.q, dropped: b.q.droppedVideoFrames };
  const d = meta.duration;
  const doSeek = (to, timeout = 60000) => p.evaluate(eval(SEEK), [to, timeout]);
  r.seekForward = await doSeek(+(d * 0.6).toFixed(2));
  await sleep(3000);
  r.seekBackInside = await doSeek(5);
  await sleep(2000);
  r.seekBackOutside = await doSeek(+(d * 0.3).toFixed(2));
  await sleep(2000);
  // the end: seek to 6 s before the end, wait for 'ended'
  const endT0 = Date.now();
  await p.evaluate((to) => { document.querySelector('video').currentTime = to; }, Math.max(0, d - 6));
  const ended = await p.waitForFunction(() => document.querySelector('video').ended, null, { timeout: 60000, polling: 100 }).then(() => true, () => false);
  r.end = { ended, ms: Date.now() - endT0, ...(await p.evaluate(() => { const v = document.querySelector('video'); return { ct: +v.currentTime.toFixed(2), duration: +v.duration.toFixed(2), ranges: spike.ranges() }; })) };
  r.waiting = await p.evaluate(() => spike.log.filter((e) => e.ev === 'waiting').length);
  r.streamErrors = await p.evaluate(() => spike.log.filter((e) => /error|quota|http/.test(e.ev)));
  r.pageErrors = errors;
  const s = await stats(false);
  r.server = { conversions: s.conversions, nodePeakRssMB: +(s.nodePeakRss / 1048576).toFixed(0), peakBufferMB: +(s.peakBufferBytes / 1048576).toFixed(1),
    runs: s.runs.map((x) => ({ start: x.start, ttffMs: x.ttffMs, ffCpuSec: x.ffCpuSec, wallSec: x.wallSec, ffPeakRssMB: +(x.ffPeakRss / 1048576).toFixed(0), fragments: x.fragments, pausedMs: x.pausedMs, stoppedBy: x.stoppedBy, exitCode: x.exitCode, stderr: x.stderr })) };
  if (process.env.KEEP_LOG) r.log = await p.evaluate(() => spike.log);
  await ctx.close();
  return r;
}

(async () => {
  const browser = await chromium.launch({ args: ['--autoplay-policy=no-user-gesture-required'] });
  const all = [];
  for (let i = 0; i < +runs; i++) {
    const r = await oneRun(browser, i);
    console.log(JSON.stringify(r));
    all.push(r);
    await sleep(6000); // let the server's linger timer stop ffmpeg
  }
  await browser.close();
  if (out) fs.writeFileSync(out, JSON.stringify(all, null, 1));
})();
