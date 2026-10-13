#!/usr/bin/env bash
# Build libde265 + libheif as shared, decoder-only libraries into a prefix.
#
#   scripts/build-deps.sh <rust-target> <prefix>
#   scripts/build-deps.sh --sources <dir>   (only fetch and check the source archives)
#
# Only the HEVC decoder (libde265) is compiled in: no encoders (x265 is GPL
# and large), no AV1/JPEG2000 codecs, no plugin loading. Both are LGPL and
# ship as separate files next to the shoebox program (libheif.1.dylib and
# libde265.0.dylib, or libheif.so.1 and libde265.so.0), so whoever has
# shoebox can put in a build of their own (LGPL section 4(d)); the program
# and libheif find them in their own folder (rpath @executable_path /
# @loader_path, $ORIGIN). scripts/build.sh copies them next to the binary.
#
# On macOS the minimum OS version is taken from MACOSX_DEPLOYMENT_TARGET
# (default 10.13) so the binary runs on older Intel Macs.
set -eu

LIBDE265_VERSION=1.1.3
LIBDE265_SHA256=554228bd17788c99a7e63b37ab5634722190e6e2bf60c1dcb01cef328e133905
LIBHEIF_VERSION=1.23.5
LIBHEIF_SHA256=fd9036064c4432f0550d15072ddf34956a248279ee9aeaff0fba3fa0f77d8f1a
LIBDE265_URL="https://github.com/strukturag/libde265/releases/download/v$LIBDE265_VERSION/libde265-$LIBDE265_VERSION.tar.gz"
LIBHEIF_URL="https://github.com/strukturag/libheif/releases/download/v$LIBHEIF_VERSION/libheif-$LIBHEIF_VERSION.tar.gz"

sha256() {
    if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1
    else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

download() { # url sha256 dir/file
    if [ ! -f "$3" ]; then
        curl -fsSL -o "$3" "$1"
    fi
    actual=$(sha256 "$3")
    if [ "$actual" != "$2" ]; then
        echo "checksum mismatch for $3: $actual" >&2
        exit 1
    fi
}

# The unmodified upstream sources, for the release archive (the LGPL wants
# them to go along with the libraries).
if [ "${1:-}" = --sources ]; then
    DIR=${2:?usage: build-deps.sh --sources <dir>}
    mkdir -p "$DIR"
    download "$LIBDE265_URL" "$LIBDE265_SHA256" "$DIR/libde265-$LIBDE265_VERSION.tar.gz"
    download "$LIBHEIF_URL" "$LIBHEIF_SHA256" "$DIR/libheif-$LIBHEIF_VERSION.tar.gz"
    exit 0
fi

TARGET=${1:?usage: build-deps.sh <rust-target> <prefix>}
PREFIX=${2:?usage: build-deps.sh <rust-target> <prefix>}

# Windows (MSVC, from Git Bash): DLLs in bin/ with import libraries in lib/;
# Windows loads a DLL from the program's own folder, no rpath needed.
case "$TARGET" in
    *-apple-darwin) LIBS="lib/libheif.1.dylib lib/libde265.0.dylib"; ORIGIN=@loader_path ;;
    *-windows-msvc) LIBS="bin/heif.dll bin/libde265.dll"; ORIGIN= ;;
    *) LIBS="lib/libheif.so.1 lib/libde265.so.0"; ORIGIN='$ORIGIN' ;;
esac
# Windows tools do not understand Git Bash's /d/... paths.
winpath() { case "$TARGET" in *-windows-msvc) cygpath -m "$1" ;; *) printf '%s' "$1" ;; esac; }

DONE="$PREFIX/.done-shared-$LIBDE265_VERSION-$LIBHEIF_VERSION"
built=yes
for lib in $LIBS; do [ -f "$PREFIX/$lib" ] || built=no; done
if [ -f "$DONE" ] && [ "$built" = yes ]; then
    echo "deps already built in $PREFIX"
    exit 0
fi

mkdir -p "$PREFIX"
PREFIX=$(winpath "$(cd "$PREFIX" && pwd)")
WORK="$PREFIX/src"
mkdir -p "$WORK"

fetch() { # url sha256 file
    download "$1" "$2" "$WORK/$3"
    # From inside the folder: GNU tar takes "D:/..." for a remote host.
    (cd "$WORK" && tar xzf "$3")
}

fetch "$LIBDE265_URL" "$LIBDE265_SHA256" "libde265-$LIBDE265_VERSION.tar.gz"
fetch "$LIBHEIF_URL" "$LIBHEIF_SHA256" "libheif-$LIBHEIF_VERSION.tar.gz"

# Arrays keep paths with spaces (e.g. "Application Support") intact.
# Each library finds the other in its own folder (`$ORIGIN` is literal here:
# the dynamic loader expands it).
COMMON=(-DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=ON -DCMAKE_POSITION_INDEPENDENT_CODE=ON
        "-DCMAKE_INSTALL_PREFIX=$PREFIX" -DCMAKE_INSTALL_LIBDIR=lib "-DCMAKE_PREFIX_PATH=$PREFIX"
        "-DCMAKE_INSTALL_RPATH=$ORIGIN" -DCMAKE_INSTALL_NAME_DIR=@rpath
        # Never pick up codecs from Homebrew or the system.
        -DCMAKE_FIND_USE_CMAKE_SYSTEM_PATH=OFF -DCMAKE_FIND_USE_SYSTEM_ENVIRONMENT_PATH=OFF
        -DCMAKE_FIND_USE_PACKAGE_REGISTRY=OFF)

# With the system environment path off, CMake no longer searches PATH for the
# build tool and compilers either, so hand them over explicitly. On Windows
# the Visual Studio generator finds its own compiler; the C runtime is linked
# into each DLL (/MT), as into the program (crt-static, core/.cargo/config.toml),
# so no Visual C++ Redistributable needs to be installed.
tool() { # name-or-path
    command -v "$1" || { echo "$1 not found" >&2; exit 1; }
}
case "$TARGET" in
    *-windows-msvc)
        COMMON+=(-A x64 -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded
                 -DCMAKE_POLICY_DEFAULT_CMP0091=NEW) ;;
    *)
        COMMON+=("-DCMAKE_MAKE_PROGRAM=$(tool make)" "-DCMAKE_C_COMPILER=$(tool "${CC:-cc}")"
                 "-DCMAKE_CXX_COMPILER=$(tool "${CXX:-c++}")") ;;
esac

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
# install prefix) survives from an earlier run, and drop the static libraries
# of older builds so the linker cannot pick them.
rm -rf "$WORK/build-de265" "$WORK/build-heif"
rm -f "$PREFIX"/lib/libheif.* "$PREFIX"/lib/libde265.* "$PREFIX"/lib/heif.* \
    "$PREFIX"/bin/heif.* "$PREFIX"/bin/libde265.* "$PREFIX"/.done-*

cmake -S "$WORK/libde265-$LIBDE265_VERSION" -B "$WORK/build-de265" "${COMMON[@]}" \
    -DENABLE_SDL=OFF -DENABLE_DECODER=OFF -DENABLE_ENCODER=OFF
cmake --build "$WORK/build-de265" -j "$JOBS" --config Release
cmake --install "$WORK/build-de265" --config Release

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
cmake --build "$WORK/build-heif" -j "$JOBS" --config Release
cmake --install "$WORK/build-heif" --config Release

# install_name_tool (rpath, install name) breaks the linker's ad-hoc
# signature, and Apple Silicon refuses to load an unsigned library.
# (The real files; libheif.1.dylib and the like are symlinks to them.)
case "$TARGET" in
    *-apple-darwin)
        find "$PREFIX/lib" -maxdepth 1 -type f -name '*.dylib' -exec codesign --force --sign - {} \; ;;
esac

for lib in $LIBS; do
    [ -f "$PREFIX/$lib" ] || { echo "$lib was not built" >&2; ls -R "$PREFIX/bin" "$PREFIX/lib" >&2; exit 1; }
done
touch "$DONE"
echo "deps installed to $PREFIX"
