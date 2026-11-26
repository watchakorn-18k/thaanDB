#!/usr/bin/env bash
# Build the browser demo into target/web (open it with any static file server, or publish
# it to GitHub Pages). Needs nightly Rust with the wasm target and wasm-bindgen-cli at the
# version in Cargo.lock:
#   rustup toolchain install nightly --profile minimal --target wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <version of wasm-bindgen in Cargo.lock> --locked
# Usage: scripts/build-web.sh [--dev]
set -euo pipefail

cd "$(dirname "$0")/.."
profile=release
flags=(--release)
if [[ "${1:-}" == "--dev" ]]; then
  profile=debug
  flags=()
fi
cargo +nightly build -p thaan-web --target wasm32-unknown-unknown "${flags[@]}"

out=target/web
rm -rf "$out"
mkdir -p "$out/assets"
wasm-bindgen --target web --no-typescript --out-dir "$out" \
  "target/wasm32-unknown-unknown/$profile/thaan_web.wasm"
cp crates/thaan-web/web/index.html "$out/"
# The kit loads icons over HTTP from ./assets/icons on the web.
icons=$(ls -d ~/.cargo/registry/src/*/gpui-kit-assets-0.11.0/assets/icons | head -1)
cp -R "$icons" "$out/assets/icons"
# GitHub Pages: serve files as they are (no Jekyll).
touch "$out/.nojekyll"
echo "built $out ($profile): $(du -sh "$out/thaan_web_bg.wasm" | cut -f1) wasm"
