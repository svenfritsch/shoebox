#!/usr/bin/env bash
# Build libde265 + libheif as static, decoder-only libraries into a prefix.
#
#   scripts/build-deps.sh <rust-target> <prefix>
#
# Only the HEVC decoder (libde265) is compiled in: no encoders (x265 is GPL
# and large), no AV1/JPEG2000 codecs, no plugin loading. The result is linked
# statically into the shoebox binary, so it runs without any system libraries.
#
# On macOS the minimum OS version is taken from MACOSX_DEPLOYMENT_TARGET
# (default 10.13) so the binary runs on older Intel Macs.
set -eu

TARGET=${1:?usage: build-deps.sh <rust-target> <prefix>}
PREFIX=${2:?usage: build-deps.sh <rust-target> <prefix>}

LIBDE265_VERSION=1.1.3
LIBDE265_SHA256=554228bd17788c99a7e63b37ab5634722190e6e2bf60c1dcb01cef328e133905
LIBHEIF_VERSION=1.23.5
LIBHEIF_SHA256=fd9036064c4432f0550d15072ddf34956a248279ee9aeaff0fba3fa0f77d8f1a

if [ -f "$PREFIX/.done-$LIBDE265_VERSION-$LIBHEIF_VERSION" ] && [ -f "$PREFIX/lib/libheif.a" ] \
    && [ -f "$PREFIX/lib/libde265.a" ]; then
    echo "deps already built in $PREFIX"
    exit 0
fi

mkdir -p "$PREFIX"
PREFIX=$(cd "$PREFIX" && pwd)
WORK="$PREFIX/src"
mkdir -p "$WORK"

sha256() {
    if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1
    else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

fetch() { # url sha256 file
    if [ ! -f "$WORK/$3" ]; then
        curl -fsSL -o "$WORK/$3" "$1"
    fi
    actual=$(sha256 "$WORK/$3")
    if [ "$actual" != "$2" ]; then
        echo "checksum mismatch for $3: $actual" >&2
        exit 1
    fi
    tar xzf "$WORK/$3" -C "$WORK"
}

fetch "https://github.com/strukturag/libde265/releases/download/v$LIBDE265_VERSION/libde265-$LIBDE265_VERSION.tar.gz" \
    "$LIBDE265_SHA256" "libde265-$LIBDE265_VERSION.tar.gz"
fetch "https://github.com/strukturag/libheif/releases/download/v$LIBHEIF_VERSION/libheif-$LIBHEIF_VERSION.tar.gz" \
    "$LIBHEIF_SHA256" "libheif-$LIBHEIF_VERSION.tar.gz"

# Arrays keep paths with spaces (e.g. "Application Support") intact.
COMMON=(-DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DCMAKE_POSITION_INDEPENDENT_CODE=ON
        "-DCMAKE_INSTALL_PREFIX=$PREFIX" -DCMAKE_INSTALL_LIBDIR=lib "-DCMAKE_PREFIX_PATH=$PREFIX"
        # Never pick up codecs from Homebrew or the system.
        -DCMAKE_FIND_USE_CMAKE_SYSTEM_PATH=OFF -DCMAKE_FIND_USE_SYSTEM_ENVIRONMENT_PATH=OFF
        -DCMAKE_FIND_USE_PACKAGE_REGISTRY=OFF)

# With the system environment path off, CMake no longer searches PATH for the
# build tool and compilers either, so hand them over explicitly.
tool() { # name-or-path
    command -v "$1" || { echo "$1 not found" >&2; exit 1; }
}
COMMON+=("-DCMAKE_MAKE_PROGRAM=$(tool make)" "-DCMAKE_C_COMPILER=$(tool "${CC:-cc}")"
         "-DCMAKE_CXX_COMPILER=$(tool "${CXX:-c++}")")

case "$TARGET" in
    x86_64-apple-darwin)
        COMMON+=(-DCMAKE_OSX_ARCHITECTURES=x86_64 "-DCMAKE_OSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-10.13}") ;;
    aarch64-apple-darwin)
        COMMON+=(-DCMAKE_OSX_ARCHITECTURES=arm64 "-DCMAKE_OSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-11.0}") ;;
esac

# pkg-config would find Homebrew's libde265 (and chokes on spaces in paths);
# give it an empty search path so CMake finds our libde265 via the prefix.
mkdir -p "$WORK/no-pkgconfig"
export PKG_CONFIG_LIBDIR="$WORK/no-pkgconfig" PKG_CONFIG_PATH=""

JOBS=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)

# Always configure from scratch so no stale CMake cache (found libraries,
# install prefix) survives from an earlier run.
rm -rf "$WORK/build-de265" "$WORK/build-heif"

cmake -S "$WORK/libde265-$LIBDE265_VERSION" -B "$WORK/build-de265" "${COMMON[@]}" \
    -DENABLE_SDL=OFF -DENABLE_DECODER=OFF -DENABLE_ENCODER=OFF
cmake --build "$WORK/build-de265" -j "$JOBS"
cmake --install "$WORK/build-de265"

# Every codec except libde265 is switched off explicitly, so nothing found on
# the build machine can sneak in as a dynamic dependency.
OFF=()
for c in X265 KVAZAAR UVG266 VVDEC VVENC X264 OpenH264_DECODER DAV1D AOM_DECODER AOM_ENCODER \
         SvtEnc RAV1E JPEG_DECODER JPEG_ENCODER OpenJPEG_ENCODER OpenJPEG_DECODER \
         FFMPEG_DECODER OPENJPH_ENCODER; do
    OFF+=("-DWITH_$c=OFF" "-DWITH_${c}_PLUGIN=OFF")
done

cmake -S "$WORK/libheif-$LIBHEIF_VERSION" -B "$WORK/build-heif" "${COMMON[@]}" "${OFF[@]}" \
    -DWITH_LIBDE265=ON -DWITH_LIBDE265_PLUGIN=OFF \
    -DENABLE_PLUGIN_LOADING=OFF -DWITH_LIBSHARPYUV=OFF -DWITH_UNCOMPRESSED_CODEC=OFF \
    -DWITH_HEADER_COMPRESSION=OFF -DWITH_WEBCODECS=OFF \
    -DWITH_EXAMPLES=OFF -DWITH_EXAMPLE_HEIF_THUMB=OFF -DWITH_EXAMPLE_HEIF_VIEW=OFF \
    -DWITH_GDK_PIXBUF=OFF -DBUILD_TESTING=OFF -DBUILD_DOCUMENTATION=OFF -DWITH_FUZZERS=OFF
cmake --build "$WORK/build-heif" -j "$JOBS"
cmake --install "$WORK/build-heif"

touch "$PREFIX/.done-$LIBDE265_VERSION-$LIBHEIF_VERSION"
echo "deps installed to $PREFIX"
