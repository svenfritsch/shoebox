#!/usr/bin/env python3
"""Median (min-max) per case from drain.jsonl and browser.jsonl.
   python3 summarize.py <results-dir>"""
import json, statistics, sys, os

d = sys.argv[1]

def mm(xs, fmt='{:.1f}'):
    xs = [x for x in xs if x is not None]
    if not xs:
        return '-'
    m = statistics.median(xs)
    if len(xs) == 1 or min(xs) == max(xs):
        return fmt.format(m)
    return f'{fmt.format(m)} ({fmt.format(min(xs))}-{fmt.format(max(xs))})'

def group(rows, key):
    out = {}
    for r in rows:
        out.setdefault(key(r), []).append(r)
    return out

drain = [json.loads(l) for l in open(os.path.join(d, 'drain.jsonl'))]
print('## drain (no browser), first conversion of each run only')
print('| cmd | video | n | first fragment s | speed x realtime | ffmpeg cores | ffmpeg peak RSS MB | fragment s | output Mbit/s |')
print('|---|---|---|---|---|---|---|---|---|')
for (cmd, name), rs in group(drain, lambda r: (r['cmd'], r['name'])).items():
    rs = [r for r in rs if r.get('ttffMs') is not None]
    print(f"| {cmd} | {name} | {len(rs)} | {mm([r['ttffMs']/1000 for r in rs])} | {mm([r['speedX'] for r in rs], '{:.2f}')} | "
          f"{mm([r['ffCpuCores'] for r in rs])} | {mm([r['ffPeakRssMB'] for r in rs], '{:.0f}')} | {mm([r['fragSec'] for r in rs])} | {mm([r['outMbitPerVideoSec'] for r in rs])} |")

p = os.path.join(d, 'browser.jsonl')
if os.path.exists(p):
    br = [json.loads(l) for l in open(p)]
    print('\n## browser (headless Chromium)')
    print('| cmd | video | n | played as | trigger | first picture s | 15 s played | seek out fwd s | seek in buffer s | seek out back s | end | waiting | errors |')
    print('|---|---|---|---|---|---|---|---|---|---|---|---|---|')
    for (cmd, name, mode), rs in group(br, lambda r: (r['cmd'], r['name'], r.get('_req', r['mode'] if r.get('trigger') or r['mode'] == 'original' else 'mse-forced'))).items():
        g = lambda k, f: [f(r[k]) if r.get(k) else None for r in rs]
        ends = [r.get('end', {}) for r in rs]
        print(f"| {cmd} | {name} | {len(rs)} | {rs[0]['mode']} | {rs[0].get('trigger')} | {mm([r['firstPictureMs']/1000 if r.get('firstPictureMs') else None for r in rs], '{:.2f}')} | "
              f"{mm(g('play15', lambda x: x['advanced']), '{:.2f}')} | {mm(g('seekForward', lambda x: x['playingAgainMs']/1000 if x['playingAgainMs'] else None), '{:.2f}')} | "
              f"{mm(g('seekBackInside', lambda x: x['playingAgainMs']/1000 if x['playingAgainMs'] else None), '{:.2f}')} | {mm(g('seekBackOutside', lambda x: x['playingAgainMs']/1000 if x['playingAgainMs'] else None), '{:.2f}')} | "
              f"{sum(1 for e in ends if e.get('ended'))}/{len(rs)} ended | {mm([r.get('waiting') for r in rs], '{:.0f}')} | "
              f"{sum(len(r.get('streamErrors') or []) + len(r.get('pageErrors') or []) for r in rs)} |")
    print('\nserver per browser run: peak buffer MB, node peak RSS MB, ffmpeg peak RSS MB, conversions')
    for r in br:
        s = r.get('server') or {}
        print(r['cmd'], r['name'], r['run'], s.get('peakBufferMB'), s.get('nodePeakRssMB'), max([x['ffPeakRssMB'] for x in s.get('runs', [])] or [0]), s.get('conversions'),
              [x['ttffMs'] for x in s.get('runs', [])])
