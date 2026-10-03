#!/bin/sh
# Build the shoebox binary for one Rust target with libheif linked statically.
#
#   scripts/build.sh [rust-target] [test]   (default: the host target)
#
# With "test", runs the unit tests for that target instead of a release build.
#
# Static deps are built once per target into $DEPS_DIR (default: target/deps).
set -eu
cd "$(dirname "$0")/.."

TARGET=${1:-$(rustc -vV | sed -n 's/^host: //p')}
TARGET_DIR=${CARGO_TARGET_DIR:-$PWD/core/target}
DEPS=${DEPS_DIR:-$TARGET_DIR/deps}/$TARGET

scripts/build-deps.sh "$TARGET" "$DEPS"

# Point libheif-sys straight at our static libraries instead of asking
# pkg-config, which could return Homebrew's libheif and cannot handle spaces
# in paths. The C++ runtime is added by core/build.rs.
export SYSTEM_DEPS_LIBHEIF_NO_PKG_CONFIG=1
export SYSTEM_DEPS_LIBHEIF_SEARCH_NATIVE="$DEPS/lib"
export SYSTEM_DEPS_LIBHEIF_LIB="heif de265"
export SYSTEM_DEPS_LIBHEIF_INCLUDE="$DEPS/include"
export SYSTEM_DEPS_LIBHEIF_LINK=static
case "$TARGET" in
    x86_64-apple-darwin) export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-10.13} ;;
    aarch64-apple-darwin) export MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-11.0} ;;
esac

if [ "${2:-}" = test ]; then
    cargo test --manifest-path core/Cargo.toml --target "$TARGET"
else
    cargo build --release --manifest-path core/Cargo.toml --target "$TARGET"
    echo "built $TARGET_DIR/$TARGET/release/shoebox"
fi
