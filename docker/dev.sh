#!/bin/sh
# Run a command inside the shoebox dev container with the repo mounted at /work.
# Cargo registry and build output live in named volumes so rebuilds stay fast
# and Linux build artefacts never land in the repo.
set -e
cd "$(dirname "$0")/.."
exec docker run --rm -i \
    -v "$PWD":/work \
    -v shoebox-cargo:/usr/local/cargo/registry \
    -v shoebox-target:/target \
    -e CARGO_TARGET_DIR=/target \
    -w /work \
    shoebox-dev "$@"
