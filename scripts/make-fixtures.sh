#!/bin/sh
# Create a small sample library covering every case the probe checks.
# Needs ffmpeg, heif-enc and exiftool (all in docker/Dockerfile.dev).
#
#   scripts/make-fixtures.sh <dir>
#
# It empties <dir> first, so it runs only where nothing real can be in the
# way: in CI, in the Docker dev container (docker/dev.sh) or in a Claude Code
# cloud sandbox, never directly on a Mac or another real computer.
set -eu
if [ "${GITHUB_ACTIONS:-}" != true ] && [ ! -f /.dockerenv ] && [ "${CLAUDE_CODE_REMOTE:-}" != true ]; then
    echo "make-fixtures.sh runs only in CI, in the dev container (docker/dev.sh) or in a cloud sandbox: it empties the folder it is given." >&2
    exit 1
fi
OUT=${1:?usage: make-fixtures.sh <dir>}
rm -rf "$OUT"
EVENT="$OUT/2020-07 Urlaub Griechenland"
TOPIC="$OUT/Familie/Weihnachten"
# "Österreich" written decomposed (O + combining diaeresis), as macOS often does.
NFD_DIR="$OUT/$(printf '2019-08 Urlaub O\314\210sterreich')"
mkdir -p "$EVENT" "$TOPIC" "$NFD_DIR"

photo() { # out-file color date
    ffmpeg -v error -f lavfi -i "testsrc2=size=4032x3024:duration=1" -vf "drawbox=c=$2@0.5:t=fill" \
        -frames:v 1 -q:v 3 -y "$1"
    exiftool -q -overwrite_original -Make=Apple "-Model=iPhone 12" \
        "-DateTimeOriginal=$3" "-CreateDate=$3" "-OffsetTimeOriginal=+03:00" "$1"
}

photo "$EVENT/IMG_0001.JPG" blue "2020:07:14 18:32:05"
photo "$TOPIC/DSC_2001.jpg" red "2012:12:24 19:05:00"
photo "$NFD_DIR/IMG_0100.JPG" green "2019:08:03 10:00:00"

# HEIC with EXIF (heif-enc carries EXIF over from the JPEG).
photo "$EVENT/tmp.jpg" yellow "2020:07:15 09:10:11"
heif-enc -q 60 -o "$EVENT/IMG_0002.HEIC" "$EVENT/tmp.jpg" >/dev/null
rm "$EVENT/tmp.jpg"

# Live Photo: the HEIC above plus a short MOV with the same stem.
ffmpeg -v error -f lavfi -i "testsrc2=size=1920x1440:rate=30:duration=3" -c:v libx264 -pix_fmt yuv420p \
    -metadata creation_time=2020-07-15T06:10:11Z -y "$EVENT/IMG_0002.MOV"

# Normal video.
ffmpeg -v error -f lavfi -i "testsrc2=size=1280x720:rate=30:duration=12" -c:v libx264 -pix_fmt yuv420p \
    -metadata creation_time=2020-07-16T17:00:00Z -y "$EVENT/VID_0003.mp4"

# RAW stand-in: a TIFF with EXIF named .DNG (DNG is TIFF-based), paired with a JPEG.
photo "$EVENT/IMG_0004.JPG" purple "2020:07:17 12:00:00"
ffmpeg -v error -f lavfi -i "testsrc2=size=600x400:duration=1" -frames:v 1 -y "$EVENT/IMG_0004.tif"
exiftool -q -overwrite_original "-DateTimeOriginal=2020:07:17 12:00:00" "-Model=Test RAW" "$EVENT/IMG_0004.tif"
mv "$EVENT/IMG_0004.tif" "$EVENT/IMG_0004.DNG"

# Canon-style CR2 stand-in: TIFF header with the CR\x02\x00 marker, the Exif IFD
# (DateTimeOriginal) at offset 34000, past the bytes nom-exif loads first.
python3 - "$EVENT/IMG_0005.CR2" <<'PY'
import struct, sys
def ent(t, f, c, v): return struct.pack('<HHII', t, f, c, v)
date, model, exif_off = b"2020:07:18 09:15:00\0", b"Canon EOS Test\0", 34000
b = b"II*\0" + struct.pack('<I', 16) + b"CR\x02\x00" + struct.pack('<I', 0)
b += struct.pack('<H', 3) + ent(0x110, 2, len(model), 16 + 2 + 36 + 4) + ent(0x112, 3, 1, 1) + ent(0x8769, 4, 1, exif_off) + struct.pack('<I', 0) + model
b += b"\0" * (exif_off - len(b))
b += struct.pack('<H', 1) + ent(0x9003, 2, len(date), exif_off + 18) + struct.pack('<I', 0) + date + b"\x55" * 200000
open(sys.argv[1], 'wb').write(b)
PY

# PNG screenshot without EXIF.
ffmpeg -v error -f lavfi -i "testsrc2=size=1170x2532:duration=1" -frames:v 1 -y "$TOPIC/Screenshot.png"

# Broken file: JPEG cut off after 20 KB.
head -c 20000 "$TOPIC/DSC_2001.jpg" > "$TOPIC/kaputt.jpg"

# macOS bookkeeping that must be skipped.
printf 'AppleDouble' > "$EVENT/._IMG_0001.JPG"
printf 'junk' > "$OUT/.DS_Store"
mkdir -p "$OUT/.Spotlight-V100" && printf 'x' > "$OUT/.Spotlight-V100/store.db"
printf 'not a photo' > "$EVENT/notes.txt"

echo "fixtures in $OUT"
