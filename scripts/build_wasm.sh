#!/usr/bin/env bash
# Rebuild the browser playground artefact.
#
# The site serves a prebuilt module so GitHub Pages needs no build step. Run this
# after changing anything under crates/ that the playground exercises, and commit
# the result — CI checks that site/nexusfs.wasm matches a fresh build.
#
# No wasm-bindgen or wasm-pack required: the module has no JS imports by design.
set -euo pipefail

cd "$(dirname "$0")/.."

if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
  echo "installing wasm32-unknown-unknown target..."
  rustup target add wasm32-unknown-unknown
fi

cargo build -p nexusfs-wasm --target wasm32-unknown-unknown --release

built="${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release/nexusfs_wasm.wasm"
cp "$built" site/nexusfs.wasm

echo "site/nexusfs.wasm updated ($(wc -c < site/nexusfs.wasm) bytes)"
