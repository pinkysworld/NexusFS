#!/usr/bin/env bash
# Build the browser playground module into site/nexusfs.wasm.
#
# The artefact is git-ignored and built by the Pages deploy workflow, because it is
# not reproducible across machines: the rustc version and the absolute paths baked
# into panic messages both end up in the binary. Run this locally to preview the
# playground, or after changing anything the playground exercises.
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
