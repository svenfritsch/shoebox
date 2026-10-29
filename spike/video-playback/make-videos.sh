#!/usr/bin/env bash
# Test videos for the playback spike. Usage: make-videos.sh <out-dir>
# Synthetic content (testsrc2 + noise so it does not compress to nothing),
# bitrates like an iPhone (HEVC 4K30 ~22 Mbit/s, 1080p30 ~6 Mbit/s).
# One minute of each is encoded, the long ones are that minute joined with -c copy.
set -euo pipefail
out=${1:?out dir}; mkdir -p "$out"; cd "$out"
ff() { ffmpeg -hide_banner -loglevel error -nostdin -y "$@"; }
src() { echo "testsrc2=size=$1:rate=30,noise=alls=10:allf=t"; }
tone='sine=frequency=440:sample_rate=48000'
x265='keyint=60:min-keyint=60:log-level=error'

[ -f base-4k.mov ] || ff -f lavfi -i "$(src 3840x2160)" -f lavfi -i "$tone" -t 60 \
  -c:v libx265 -preset ultrafast -b:v 22M -maxrate 25M -bufsize 30M -x265-params "$x265" \
  -tag:v hvc1 -pix_fmt yuv420p10le -c:a aac -ac 2 -b:a 160k base-4k.mov
[ -f base-1080.mov ] || ff -f lavfi -i "$(src 1920x1080)" -f lavfi -i "$tone" -t 60 \
  -c:v libx265 -preset ultrafast -b:v 6M -maxrate 8M -bufsize 12M -x265-params "$x265" \
  -tag:v hvc1 -pix_fmt yuv420p -c:a aac -ac 2 -b:a 160k base-1080.mov

join() { # base n out
  : > list.txt; for _ in $(seq "$2"); do echo "file '$1'" >> list.txt; done
  ff -f concat -safe 0 -i list.txt -c copy -tag:v hvc1 "$3"; rm list.txt; }
[ -f hevc-1080-10min.mov ] || join base-1080.mov 10 hevc-1080-10min.mov
[ -f hevc-4k-10min.mov ] || join base-4k.mov 10 hevc-4k-10min.mov
[ -f hevc-4k-30min.mov ] || join base-4k.mov 30 hevc-4k-30min.mov

# H.264 + AAC in Matroska, 5 min, x264 default GOP (250)
[ -f h264-1080-5min.mkv ] || ff -f lavfi -i "$(src 1920x1080)" -f lavfi -i "$tone" -t 300 \
  -c:v libx264 -preset veryfast -b:v 8M -pix_fmt yuv420p -c:a aac -b:a 160k h264-1080-5min.mkv
# MPEG-2 interlaced + AC-3 in MPEG-TS (camcorder style), 5 min, GOP 15
[ -f mpeg2-1080i-5min.mts ] || ff -f lavfi -i "$(src 1920x1080)" -f lavfi -i "$tone" -t 300 \
  -c:v mpeg2video -b:v 15M -maxrate 25M -bufsize 9M -g 15 -flags +ilme+ildct -top 1 \
  -pix_fmt yuv420p -c:a ac3 -b:a 256k -f mpegts mpeg2-1080i-5min.mts
# Live Photo motion: 3 s HEVC 1440x1080 in .mov, rotated 90 degrees by metadata
[ -f live-motion.mov ] || { ff -f lavfi -i "$(src 1440x1080)" -f lavfi -i "$tone" -t 3 \
  -c:v libx265 -preset ultrafast -b:v 8M -x265-params "$x265" -tag:v hvc1 -pix_fmt yuv420p \
  -c:a aac -b:a 96k live-tmp.mov
  ff -display_rotation 90 -i live-tmp.mov -c copy live-motion.mov; rm live-tmp.mov; }
ls -la
