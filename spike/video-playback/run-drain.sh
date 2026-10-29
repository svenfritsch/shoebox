#!/usr/bin/env bash
# Drain matrix: every video, both commands, 3 runs, 60 s of video each.
#   run-drain.sh <videos-dir> <ffmpeg> <results-dir>
set -u
dir=$1 ff=$2 out=$3; here=$(cd "$(dirname "$0")" && pwd); port=8772
for cmd in design gop2; do
  node "$here/server.mjs" --dir "$dir" --ffmpeg "$ff" --port $port --cmd $cmd > "$out/server-drain-$cmd.log" 2>&1 & pid=$!
  sleep 0.5
  for v in hevc-1080-10min.mov hevc-4k-10min.mov h264-1080-5min.mkv mpeg2-1080i-5min.mts live-motion.mov; do
    node "$here/bench-drain.cjs" $port "$v" 3 60 | sed "s/^{/{\"cmd\":\"$cmd\",/" >> "$out/drain.jsonl"
  done
  kill $pid; wait $pid 2>/dev/null; port=$((port+1))
done
