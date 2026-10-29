#!/usr/bin/env bash
# Browser matrix in headless Chromium: auto mode (original first), 3 runs per video.
#   run-browser.sh <videos-dir> <ffmpeg> <results-dir>
set -u
dir=$1 ff=$2 out=$3; here=$(cd "$(dirname "$0")" && pwd); port=8780
run() { # cmd video runs mode
  node "$here/drive.cjs" $port "$2" "$4" "$3" | sed -u "s/^{/{\"cmd\":\"$1\",/" >> "$out/browser.jsonl"; }
for cmd in gop2 design; do
  node "$here/server.mjs" --dir "$dir" --ffmpeg "$ff" --port $port --cmd $cmd > "$out/server-browser-$cmd.log" 2>&1 & pid=$!
  sleep 0.5
  if [ $cmd = gop2 ]; then
    for v in hevc-1080-10min.mov hevc-4k-10min.mov h264-1080-5min.mkv mpeg2-1080i-5min.mts live-motion.mov; do run $cmd $v 3 auto; done
    run $cmd hevc-4k-30min.mov 2 auto
    run $cmd h264-1080-5min.mkv 3 mse
  else
    for v in hevc-1080-10min.mov hevc-4k-10min.mov; do run $cmd $v 3 auto; done
  fi
  kill $pid; wait $pid 2>/dev/null; port=$((port+1))
done
