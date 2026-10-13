#!/usr/bin/env bash
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

# Windows (Git Bash): cargo and the linker want D:/... paths, not /d/...
# There libheif-sys only knows vcpkg, so its build script is replaced by
# telling cargo directly what to link (it declares `links = "heif"`).
CARGO_CONFIG=()
case "$TARGET" in
    *-windows-msvc)
        DEPS=$(cygpath -m "$DEPS"); TARGET_DIR=$(cygpath -m "$TARGET_DIR")
        CARGO_CONFIG=(--config "target.$TARGET.heif.rustc-link-lib=[\"heif\"]"
                      --config "target.$TARGET.heif.rustc-link-search=[\"native=$DEPS/lib\"]") ;;
esac

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
    # Windows has no rpath: the tests (and the shoebox.exe they start) find
    # the DLLs on PATH.
    # (PATH itself takes Git Bash's /d/... form.)
    case "$TARGET" in *-windows-msvc) PATH="$(cygpath -u "$DEPS")/bin:$PATH"; export PATH ;; esac
    # cargo-nextest (if installed, as on CI) runs every test in its own process
    # and all test files at once; cargo test runs the files one after another.
    if cargo nextest --version >/dev/null 2>&1; then
        cargo "${CARGO_CONFIG[@]}" nextest run --manifest-path core/Cargo.toml --target "$TARGET" --no-fail-fast
    else
        cargo "${CARGO_CONFIG[@]}" test --manifest-path core/Cargo.toml --target "$TARGET"
    fi
else
    cargo "${CARGO_CONFIG[@]}" build --release --manifest-path core/Cargo.toml --target "$TARGET"
    # The program loads them from its own folder: real files, no symlinks
    # (exFAT), under the names it asks for.
    case "$TARGET" in
        *-apple-darwin) libs="lib/libheif.1.dylib lib/libde265.0.dylib" ;;
        *-windows-msvc) libs="bin/heif.dll bin/libde265.dll" ;;
        *) libs="lib/libheif.so.1 lib/libde265.so.0" ;;
    esac
    for lib in $libs; do
        rm -f "$TARGET_DIR/$TARGET/release/${lib#*/}"
        cp -L "$DEPS/$lib" "$TARGET_DIR/$TARGET/release/${lib#*/}"
    done
    echo "built $TARGET_DIR/$TARGET/release/shoebox (with $libs next to it)"
fi
