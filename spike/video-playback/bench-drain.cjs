// Reads /playback as fast as it comes (no browser, so no pausing by the page)
// and reports conversion speed, ffmpeg CPU, memory.
//   node bench-drain.cjs <port> <name> <runs> [maxVideoSeconds] [start]
const [port, name, runs = '3', maxSec = '120', start = '0'] = process.argv.slice(2);
const base = `http://127.0.0.1:${port}`;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  for (let i = 0; i < +runs; i++) {
    await fetch(`${base}/stats`, { method: 'POST' });
    const ctl = new AbortController();
    const t0 = Date.now();
    const res = await fetch(`${base}/playback/${encodeURIComponent(name)}?start=${start}`, { signal: ctl.signal });
    const headersMs = Date.now() - t0;
    const reader = res.body.getReader();
    let bytes = 0, firstByteMs = null, stopped = false;
    const poll = setInterval(async () => {
      const s = await (await fetch(`${base}/stats`)).json();
      if (s.conv && s.conv.producedEnd - +start >= +maxSec && !stopped) { stopped = true; ctl.abort(); }
    }, 500);
    try {
      for (;;) { const { done, value } = await reader.read(); if (done) break; if (firstByteMs === null) firstByteMs = Date.now() - t0; bytes += value.length; }
    } catch {}
    clearInterval(poll);
    const wall = (Date.now() - t0) / 1000;
    const s = await (await fetch(`${base}/stats`)).json();
    await sleep(5500); // the linger timer stops ffmpeg and records the run
    const s2 = await (await fetch(`${base}/stats`)).json();
    const r = s2.runs.at(-1) || {};
    const produced = (s.conv ? s.conv.producedEnd : 0) - +start;
    console.log(JSON.stringify({ name, run: i, start: +start, ttffMs: r.ttffMs, headersMs, producedSec: +produced.toFixed(1), wallSec: +wall.toFixed(1),
      speedX: +(produced / wall).toFixed(2), ffCpuCores: +(r.ffCpuSec / r.wallSec).toFixed(2), ffPeakRssMB: +(r.ffPeakRss / 1048576).toFixed(0),
      outMB: +(bytes / 1048576).toFixed(1), outMbitPerVideoSec: +(bytes * 8 / 1e6 / produced).toFixed(2), peakBufferMB: +(s2.peakBufferBytes / 1048576).toFixed(1),
      evicted: r.evicted, nodePeakRssMB: +(s2.nodePeakRss / 1048576).toFixed(0), fragments: r.fragments,
      fragSec: +(produced / r.fragments).toFixed(2), exit: r.exitCode, stoppedBy: r.stoppedBy, stderr: r.stderr }));
  }
})();
