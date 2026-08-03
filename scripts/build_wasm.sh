#!/usr/bin/env bash
# Build the browser playground module into site/nexusfs.wasm.
#
# The artefact IS committed, because GitHub Pages for this repo serves the branch
# contents directly rather than a workflow artefact. Run this and commit the result
# after changing anything the playground exercises.
#
# Note that the output is not byte-reproducible across machines — the rustc version
# leaks into it — so CI cannot meaningfully diff it against a fresh build. CI instead
# checks that the crate builds and lints on the wasm target and that the module still
# has no JS imports.
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
