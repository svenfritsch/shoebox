// Spike: ffmpeg -> fragmented MP4 on a pipe -> one buffer in RAM -> HTTP stream.
// Nothing is written anywhere: ffmpeg writes to pipe:1, stderr is kept in memory.
//
//   node server.mjs --dir <videos> [--ffmpeg <path>] [--port 8765]
//                   [--budget-mb 768] [--ahead 60] [--cmd design|gop2]
//                   [--scale design|box] [--vt 1]   (--vt: VideoToolbox, macOS)
//
// GET /                         the test page (page.html)
// GET /list                     video names in --dir
// GET /original/<name>          the file as it is (Range), opened read-only
// GET /info/<name>              {duration, audio} from `ffmpeg -i` (stderr)
// GET /playback/<name>?start=s  converted stream; header X-Start = the time the
//                               stream's own timestamps start from
// GET /stats                    counters and measurements, POST /stats/reset
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const args = Object.fromEntries(process.argv.slice(2).reduce((a, x, i, all) =>
  x.startsWith('--') ? [...a, [x.slice(2), all[i + 1]]] : a, []));
const DIR = path.resolve(args.dir || '.');
const FFMPEG = args.ffmpeg || 'ffmpeg';
const PORT = +(args.port || 8765);
const BUDGET = +(args['budget-mb'] || 768) * 1024 * 1024;
const AHEAD = +(args.ahead || 60);
const CMD = args.cmd || 'design';
const SCALE = args.scale || 'design';
const VT = args.vt === '1';
const LINGER_MS = 5000;
const HERE = path.dirname(fileURLToPath(import.meta.url));

// ---------- measurements ----------
const stats = { conversions: 0, nodePeakRss: 0, peakBufferBytes: 0, runs: [] };
function resetStats() {
  stats.conversions = 0; stats.peakBufferBytes = 0; stats.runs = [];
  stats.nodePeakRss = process.memoryUsage().rss;
}
const TICK = 100; // USER_HZ on Linux
function procSample(pid) {
  try {
    const st = fs.readFileSync(`/proc/${pid}/stat`, 'utf8');
    const f = st.slice(st.lastIndexOf(')') + 2).split(' ');
    const cpu = (+f[11] + +f[12]) / TICK; // utime + stime
    const status = fs.readFileSync(`/proc/${pid}/status`, 'utf8');
    const kb = (k) => +((status.match(new RegExp(`${k}:\\s+(\\d+)`)) || [0, 0])[1]);
    return { cpu, rss: kb('VmRSS') * 1024, hwm: kb('VmHWM') * 1024 };
  } catch { return null; }
}
setInterval(() => {
  stats.nodePeakRss = Math.max(stats.nodePeakRss, process.memoryUsage().rss);
  if (conv && conv.proc && !conv.exited) {
    const s = procSample(conv.proc.pid);
    if (s) {
      const r = conv.run;
      r.ffCpuSec = s.cpu; r.ffPeakRss = Math.max(r.ffPeakRss, s.hwm);
      r.cpuTrace.push([+((Date.now() - conv.t0) / 1000).toFixed(1), +s.cpu.toFixed(2), conv.paused ? 1 : 0]);
      if (r.cpuTrace.length > 2000) r.cpuTrace.shift();
    }
  }
}, 250).unref();

// ---------- MP4 box parsing ----------
function boxes(buf, start = 0, end = buf.length) {
  const out = [];
  for (let p = start; p + 8 <= end;) {
    let size = buf.readUInt32BE(p); const type = buf.toString('latin1', p + 4, p + 8);
    let hdr = 8;
    if (size === 1) { size = Number(buf.readBigUInt64BE(p + 8)); hdr = 16; }
    if (size === 0) size = end - p;
    out.push({ type, start: p, body: p + hdr, end: p + size });
    p += size;
  }
  return out;
}
const child = (buf, b, type) => boxes(buf, b.body, b.end).find((x) => x.type === type);
function trackInfo(moov) { // track_ID -> {timescale, kind}
  const m = new Map();
  for (const trak of boxes(moov, 8).filter((b) => b.type === 'trak')) {
    const tkhd = child(moov, trak, 'tkhd'); const mdia = child(moov, trak, 'mdia');
    const v = moov[tkhd.body];
    const id = moov.readUInt32BE(tkhd.body + (v === 1 ? 20 : 12));
    const mdhd = child(moov, mdia, 'mdhd'); const hdlr = child(moov, mdia, 'hdlr');
    const ts = moov.readUInt32BE(mdhd.body + (moov[mdhd.body] === 1 ? 20 : 12));
    m.set(id, { timescale: ts, kind: moov.toString('latin1', hdlr.body + 8, hdlr.body + 12) });
  }
  return m;
}
function moofTime(buf, moof, tracks) { // seconds of the video track's tfdt
  let best = null;
  for (const traf of boxes(buf, moof.body, moof.end).filter((b) => b.type === 'traf')) {
    const tfhd = child(buf, traf, 'tfhd'); const tfdt = child(buf, traf, 'tfdt');
    if (!tfhd || !tfdt) continue;
    const id = buf.readUInt32BE(tfhd.body + 4); const t = tracks.get(id);
    if (!t) continue;
    const dt = buf[tfdt.body] === 1 ? Number(buf.readBigUInt64BE(tfdt.body + 4)) : buf.readUInt32BE(tfdt.body + 4);
    const sec = dt / t.timescale;
    if (t.kind === 'vide') return sec;
    best = best === null ? sec : Math.min(best, sec);
  }
  return best ?? 0;
}

// ---------- ffmpeg ----------
function ffArgs(src, start) {
  const a = ['-hide_banner', '-nostdin', '-nostats', '-loglevel', 'error'];
  if (VT) a.push('-hwaccel', 'videotoolbox'); // macOS only, untested in the spike
  if (start > 0) a.push('-ss', String(start));
  // design: scale='min(1920,iw)':-2 (a portrait 4K video becomes 1920x3414);
  // box: the longer side at most 1920 (portrait 4K becomes 1080x1920)
  const scale = SCALE === 'box'
    ? "scale=w='if(gte(iw,ih),min(1920,iw),-2)':h='if(gte(iw,ih),-2,min(1920,ih))'"
    : "scale='min(1920,iw)':-2";
  a.push('-i', src, '-map', '0:v:0', '-map', '0:a:0?', '-vf', scale);
  if (VT) a.push('-c:v', 'h264_videotoolbox', '-b:v', '6M', '-profile:v', 'high');
  else a.push('-c:v', 'libx264', '-preset', 'veryfast', '-crf', '23');
  a.push('-pix_fmt', 'yuv420p');
  // gop2: a keyframe every 2 s, so a fragment (frag_keyframe) is about 2 s long
  if (CMD === 'gop2') a.push('-force_key_frames', 'expr:gte(t,n_forced*2)');
  a.push('-c:a', 'aac', '-movflags', 'frag_keyframe+empty_moov+default_base_moof', '-f', 'mp4', 'pipe:1');
  return a;
}

function probe(src) {
  return new Promise((resolve) => {
    const p = spawn(FFMPEG, ['-hide_banner', '-nostdin', '-i', src], { stdio: ['ignore', 'ignore', 'pipe'] });
    let err = ''; p.stderr.on('data', (d) => { err += d; });
    p.on('close', () => {
      const m = err.match(/Duration: (\d+):(\d+):(\d+(?:\.\d+)?)/);
      const duration = m ? +m[1] * 3600 + +m[2] * 60 + +m[3] : null;
      resolve({ duration, audio: /Stream #0:\d+.*: Audio:/.test(err), video: (err.match(/Stream #0:\d+.*: Video: ([^\n]+)/) || [])[1] || null });
    });
  });
}

// ---------- the one buffer ----------
let conv = null; // {name, start, proc, init, frags[], dropped, bytes, done, error, cursors:Set, ...}

function stopConv(why) {
  if (!conv) return;
  const c = conv; conv = null;
  c.run.stoppedBy = why; finishRun(c);
  if (!c.exited) c.proc.kill('SIGKILL');
  for (const cur of c.cursors) cur.abort(why);
  wake(c); c.frags = []; c.init = null; c.bytes = 0;
}
function finishRun(c) {
  const r = c.run; if (r.wallSec) return;
  r.wallSec = (Date.now() - c.t0) / 1000;
  r.producedSec = c.producedEnd - c.start;
  r.bufferBytesAtEnd = c.bytes;
  r.stderr = c.stderr.slice(-2000);
}

function startConv(name, start) {
  stopConv('new conversion');
  const src = path.join(DIR, name);
  const t0 = Date.now();
  const proc = spawn(FFMPEG, ffArgs(src, start), { stdio: ['ignore', 'pipe', 'pipe'] });
  const run = { name, start, cmd: CMD, ttffMs: null, ffCpuSec: 0, ffPeakRss: 0, cpuTrace: [], pausedMs: 0, fragments: 0, evicted: 0, exitCode: null };
  stats.conversions++; stats.runs.push(run); if (stats.runs.length > 50) stats.runs.shift();
  const c = conv = { name, start, proc, t0, run, init: null, tracks: null, frags: [], dropped: 0, bytes: 0, producedEnd: start,
    pending: Buffer.alloc(0), moof: null, done: false, exited: false, error: null, stderr: '', cursors: new Set(),
    paused: false, pausedAt: 0, waiters: new Set(), lingerTimer: null };
  proc.stderr.on('data', (d) => { c.stderr = (c.stderr + d).slice(-16384); });
  proc.stdout.on('data', (chunk) => {
    c.pending = c.pending.length ? Buffer.concat([c.pending, chunk]) : chunk;
    let p = 0;
    for (;;) {
      if (c.pending.length - p < 8) break;
      let size = c.pending.readUInt32BE(p);
      if (size === 1) { if (c.pending.length - p < 16) break; size = Number(c.pending.readBigUInt64BE(p + 8)); }
      if (c.pending.length - p < size) break;
      const type = c.pending.toString('latin1', p + 4, p + 8);
      const box = c.pending.subarray(p, p + size); p += size;
      if (type === 'ftyp') c.ftyp = Buffer.from(box);
      else if (type === 'moov') { c.init = Buffer.concat([c.ftyp || Buffer.alloc(0), box]); c.tracks = trackInfo(Buffer.from(box)); wake(c); }
      else if (type === 'moof') c.moof = Buffer.from(box);
      else if (type === 'mdat' && c.moof) {
        const t = start + moofTime(c.moof, { body: 8, end: c.moof.length }, c.tracks);
        const buf = Buffer.concat([c.moof, box]); c.moof = null;
        if (c.frags.length) c.frags.at(-1).d = t - c.frags.at(-1).t;
        c.frags.push({ t, buf }); c.bytes += buf.length; run.fragments++;
        if (run.ttffMs === null) run.ttffMs = Date.now() - t0;
        c.producedEnd = t;
        evict(c); stats.peakBufferBytes = Math.max(stats.peakBufferBytes, c.bytes);
        wake(c); throttle(c);
      }
    }
    c.pending = p === c.pending.length ? Buffer.alloc(0) : c.pending.subarray(p);
  });
  proc.on('close', (code) => {
    c.exited = true; run.exitCode = code;
    if (c.frags.length) { const last = c.frags.at(-1); c.producedEnd = last.t + (last.d || 0); }
    if (code === 0) c.done = true; else if (conv === c) c.error = `ffmpeg exit ${code}: ${c.stderr.slice(-300)}`;
    if (c.paused) run.pausedMs += Date.now() - c.pausedAt;
    finishRun(c); wake(c);
  });
  return c;
}

function evict(c) { // over the budget: drop the oldest fragments nobody still has to send
  const minIdx = Math.min(...[...c.cursors].map((x) => x.idx), c.dropped + c.frags.length);
  while (c.bytes > BUDGET && c.dropped < minIdx && c.frags.length > 1) {
    c.bytes -= c.frags.shift().buf.length; c.dropped++; c.run.evicted++;
  }
}
function sentTime(c) { // the furthest time any open response has been sent
  let t = -Infinity;
  for (const cur of c.cursors) t = Math.max(t, cur.sentT);
  return t;
}
function throttle(c) {
  if (c.exited) return;
  const ahead = c.producedEnd - sentTime(c);
  if (!c.paused && ahead > AHEAD) { c.proc.stdout.pause(); c.paused = true; c.pausedAt = Date.now(); }
  else if (c.paused && ahead <= AHEAD) { c.proc.stdout.resume(); c.paused = false; c.run.pausedMs += Date.now() - c.pausedAt; }
}
function wake(c) { for (const w of [...c.waiters]) w(); }
const change = (c, ms) => new Promise((r) => {
  const f = () => { c.waiters.delete(f); clearTimeout(t); r(); };
  const t = ms ? setTimeout(f, ms) : null; c.waiters.add(f);
});

function inside(c, name, t) {
  if (!c || c.name !== name || c.error || !c.init) return -1;
  const first = c.frags[0]; if (!first || t < first.t) return -1;
  if (t > c.producedEnd + (c.done ? 0.5 : 10)) return -1;
  let i = c.frags.findIndex((f, j) => j === c.frags.length - 1 || c.frags[j + 1].t > t);
  return c.dropped + Math.max(0, i);
}

async function playback(req, res, name, start) {
  let c = conv; let idx = inside(c, name, start);
  const reused = idx >= 0;
  if (!reused) { c = startConv(name, start); idx = 0; }
  clearTimeout(c.lingerTimer);
  // wait for the init segment
  while (!c.init && !c.exited && conv === c) await change(c, 1000);
  if (!c.init) { res.writeHead(c === conv ? 500 : 409, { 'content-type': 'text/plain' }); res.end(c.error || c.stderr.slice(-300) || 'stopped'); return; }
  res.writeHead(200, { 'content-type': 'video/mp4', 'cache-control': 'no-store', 'x-start': String(c.start), 'x-reused': reused ? '1' : '0', 'access-control-expose-headers': 'x-start, x-reused' });
  let closed = false;
  const cur = { idx, sentT: idx > c.dropped ? c.frags[idx - c.dropped - 1]?.t ?? start : start, abort: () => { closed = true; res.destroy(); } };
  c.cursors.add(cur);
  req.on('close', () => { closed = true; c.cursors.delete(cur); throttle(c); wake(c);
    if (!c.cursors.size && conv === c && !c.exited) c.lingerTimer = setTimeout(() => { if (!c.cursors.size && conv === c) stopConv('no reader'); }, LINGER_MS); });
  const write = (b) => new Promise((r) => { if (res.write(b)) r(); else res.once('drain', r); });
  await write(c.init);
  while (!closed) {
    const i = cur.idx - c.dropped;
    if (i < 0) { cur.abort('evicted'); break; } // should not happen: evict respects cursors
    if (i < c.frags.length) {
      const f = c.frags[i]; await write(f.buf); cur.idx++; cur.sentT = f.t; throttle(c); continue;
    }
    if (c.exited) break;
    await change(c);
  }
  if (!closed) { if (c.error) res.destroy(); else res.end(); }
}

function sendFile(req, res, file) {
  const st = fs.statSync(file); const size = st.size;
  const type = { '.mov': 'video/quicktime', '.mp4': 'video/mp4', '.mkv': 'video/x-matroska', '.mts': 'video/mp2t' }[path.extname(file).toLowerCase()] || 'application/octet-stream';
  const m = /bytes=(\d*)-(\d*)/.exec(req.headers.range || '');
  if (m) {
    const a = m[1] ? +m[1] : size - +m[2]; const b = m[1] && m[2] ? +m[2] : size - 1;
    res.writeHead(206, { 'content-type': type, 'accept-ranges': 'bytes', 'content-range': `bytes ${a}-${b}/${size}`, 'content-length': b - a + 1 });
    fs.createReadStream(file, { start: a, end: b, flags: 'r' }).pipe(res);
  } else {
    res.writeHead(200, { 'content-type': type, 'accept-ranges': 'bytes', 'content-length': size });
    fs.createReadStream(file, { flags: 'r' }).pipe(res);
  }
}

const safeName = (n) => { const d = decodeURIComponent(n); return d && !d.includes('/') && !d.startsWith('.') && fs.existsSync(path.join(DIR, d)) ? d : null; };

http.createServer(async (req, res) => {
  const u = new URL(req.url, 'http://x'); const parts = u.pathname.split('/').filter(Boolean);
  try {
    if (u.pathname === '/') { res.writeHead(200, { 'content-type': 'text/html' }); res.end(fs.readFileSync(path.join(HERE, 'page.html'))); return; }
    if (u.pathname === '/list') { res.end(JSON.stringify(fs.readdirSync(DIR).filter((n) => /\.(mov|mp4|mkv|mts)$/i.test(n) && !n.startsWith('base-')))); return; }
    if (u.pathname === '/stats') {
      if (req.method === 'POST') resetStats();
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ ...stats, nodeRss: process.memoryUsage().rss, bufferBytes: conv?.bytes ?? 0, paused: conv?.paused ?? false,
        conv: conv && { name: conv.name, start: conv.start, producedEnd: conv.producedEnd, frags: conv.frags.length, dropped: conv.dropped, done: conv.done, readers: conv.cursors.size } }));
      return;
    }
    const name = parts[1] && safeName(parts[1]);
    if (!name) { res.writeHead(404); res.end(); return; }
    if (parts[0] === 'original') return sendFile(req, res, path.join(DIR, name));
    if (parts[0] === 'info') { res.end(JSON.stringify(await probe(path.join(DIR, name)))); return; }
    if (parts[0] === 'playback') {
      const s = u.searchParams.get('start') ?? '0';
      if (!/^\d+(\.\d+)?$/.test(s)) { res.writeHead(400); res.end('bad start'); return; }
      return await playback(req, res, name, +s);
    }
    res.writeHead(404); res.end();
  } catch (e) { console.error(e); try { res.writeHead(500); res.end(String(e)); } catch {} }
}).listen(PORT, '127.0.0.1', () => console.log(`spike server on http://127.0.0.1:${PORT}/ dir=${DIR} ffmpeg=${FFMPEG} cmd=${CMD} scale=${SCALE} vt=${VT}`));
