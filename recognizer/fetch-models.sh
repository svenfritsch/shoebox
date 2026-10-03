#!/bin/sh
# Download the ONNX models the recognizer needs (OpenCV model zoo, Apache 2.0)
# and check their SHA-256.
#
#   recognizer/fetch-models.sh [dir]   (default: recognizer/models)
set -eu
DIR=${1:-$(dirname "$0")/models}
BASE=https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models
mkdir -p "$DIR"

fetch() {
    name=$1 path=$2 sum=$3
    if [ -f "$DIR/$name" ] && echo "$sum  $DIR/$name" | shasum -a 256 -c - >/dev/null 2>&1; then
        echo "have $name"
        return
    fi
    curl -fsSL -o "$DIR/$name.part" "$BASE/$path/$name"
    if ! echo "$sum  $DIR/$name.part" | shasum -a 256 -c - >/dev/null; then
        rm -f "$DIR/$name.part"
        echo "checksum mismatch for $name" >&2
        exit 1
    fi
    mv "$DIR/$name.part" "$DIR/$name"
    echo "fetched $name"
}

fetch face_detection_yunet_2023mar.onnx face_detection_yunet \
    8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4
fetch face_recognition_sface_2021dec.onnx face_recognition_sface \
    0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79
