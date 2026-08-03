#!/usr/bin/env bash
# Build the browser playground module into site/nexusfs.wasm.
#
# The artefact is git-ignored: the Pages deploy workflow builds it, so the copy that
# ships is always current. Committing it would also mean permanently dirty diffs,
# since the output is not byte-reproducible — the rustc version and build paths leak
# into it.
#
# Run this to preview the playground locally, then serve site/ over HTTP (a file://
# page cannot fetch the module).
#
# No wasm-bindgen or wasm-pack required: the module has no JS imports by design.
set -euo pipefail

cd "$(dirname "$0")/.."

if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
  echo "installing wasm32-unknown-unknown target..."
  rustup target add wasm32-unknown-unknown
fi

# Keep local build paths out of a binary that gets published. Both the cargo registry
# and the toolchain live under $HOME, so one rule covers the lot.
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=${HOME}=~"

cargo build -p nexusfs-wasm --target wasm32-unknown-unknown --release

built="${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release/nexusfs_wasm.wasm"
cp "$built" site/nexusfs.wasm

echo "site/nexusfs.wasm built ($(wc -c < site/nexusfs.wasm) bytes)"
