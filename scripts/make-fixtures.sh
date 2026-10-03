#!/bin/sh
# Create a small sample library covering every case the probe checks.
# Needs ffmpeg, heif-enc and exiftool (all in docker/Dockerfile.dev).
#
#   scripts/make-fixtures.sh <dir>
set -eu
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
