#!/bin/sh
# Build the shoebox binary for one Rust target, with libheif and libde265 as
# separate libraries copied next to it.
#
#   scripts/build.sh [rust-target] [test]   (default: the host target)
#
# With "test", runs the unit and integration tests for that target instead of a
# release build, with cargo-nextest if it is installed (set SHOEBOX_FIXTURES to
# a make-fixtures.sh output to include the real sample files).
#
# The libraries are built once per target into $DEPS_DIR (default: target/deps).
set -eu
cd "$(dirname "$0")/.."

TARGET=${1:-$(rustc -vV | sed -n 's/^host: //p')}
TARGET_DIR=${CARGO_TARGET_DIR:-$PWD/core/target}
DEPS=${DEPS_DIR:-$TARGET_DIR/deps}/$TARGET

scripts/build-deps.sh "$TARGET" "$DEPS"

# Point libheif-sys straight at our libraries instead of asking pkg-config,
# which could return Homebrew's libheif and cannot handle spaces in paths.
# core/build.rs adds the rpath (next to the program; for debug builds and
# tests also this folder).
export SYSTEM_DEPS_LIBHEIF_NO_PKG_CONFIG=1
export SYSTEM_DEPS_LIBHEIF_SEARCH_NATIVE="$DEPS/lib"
export SYSTEM_DEPS_LIBHEIF_LIB="heif"
export SYSTEM_DEPS_LIBHEIF_INCLUDE="$DEPS/include"
case "$TARGET" in
    x86_64-apple-darwin) export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-10.13} ;;
    aarch64-apple-darwin) export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-11.0} ;;
esac

if [ "${2:-}" = test ]; then
    # cargo-nextest (if installed, as on CI) runs every test in its own process
    # and all test files at once; cargo test runs the files one after another.
    if cargo nextest --version >/dev/null 2>&1; then
        cargo nextest run --manifest-path core/Cargo.toml --target "$TARGET" --no-fail-fast
    else
        cargo test --manifest-path core/Cargo.toml --target "$TARGET"
    fi
else
    cargo build --release --manifest-path core/Cargo.toml --target "$TARGET"
    # The program loads them from its own folder: real files, no symlinks
    # (exFAT), under the names it asks for.
    case "$TARGET" in
        *-apple-darwin) libs="libheif.1.dylib libde265.0.dylib" ;;
        *) libs="libheif.so.1 libde265.so.0" ;;
    esac
    for lib in $libs; do
        rm -f "$TARGET_DIR/$TARGET/release/$lib"
        cp -L "$DEPS/lib/$lib" "$TARGET_DIR/$TARGET/release/$lib"
    done
    echo "built $TARGET_DIR/$TARGET/release/shoebox (with $libs next to it)"
fi
