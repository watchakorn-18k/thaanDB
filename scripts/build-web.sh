#!/usr/bin/env bash
# Build the browser demo into target/web (open it with any static file server, or publish
# it to GitHub Pages). Needs nightly Rust with the wasm target and wasm-bindgen-cli at the
# version in Cargo.lock:
#   rustup toolchain install nightly --profile minimal --target wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <version of wasm-bindgen in Cargo.lock> --locked
# and a clang with the WebAssembly backend (macOS: brew install llvm; Linux: clang, llvm).
# Usage: scripts/build-web.sh [--dev]
set -euo pipefail

cd "$(dirname "$0")/.."
profile=release
flags=(--release)
if [[ "${1:-}" == "--dev" ]]; then
  profile=debug
  flags=()
fi
# SQLite: sqlite-wasm-rs compiles it for the browser and needs a clang with the wasm
# backend (Homebrew's LLVM on macOS; Apple's clang has none). rusqlite is built without
# `bundled` and still asks the linker for a libsqlite3: give it an empty one, so the
# sqlite3_* symbols come from sqlite-wasm-rs.
# On Linux (CI) the distribution's clang and llvm-ar have it.
llvm_bin=$(brew --prefix llvm 2>/dev/null || true)/bin
if [[ -x "$llvm_bin/clang" ]]; then
  export CC_wasm32_unknown_unknown="$llvm_bin/clang" AR_wasm32_unknown_unknown="$llvm_bin/llvm-ar"
else
  export CC_wasm32_unknown_unknown="${CC_wasm32_unknown_unknown:-clang}"
  export AR_wasm32_unknown_unknown="${AR_wasm32_unknown_unknown:-llvm-ar}"
fi
stub=target/web-sqlite-stub
mkdir -p "$stub"
"$AR_wasm32_unknown_unknown" rc "$stub/libsqlite3.a"
export SQLITE3_LIB_DIR="$PWD/$stub" SQLITE3_STATIC=1
cargo +nightly build -p thaan-web --target wasm32-unknown-unknown "${flags[@]}"

out=target/web
rm -rf "$out"
mkdir -p "$out/assets"
wasm-bindgen --target web --no-typescript --out-dir "$out" \
  "target/wasm32-unknown-unknown/$profile/thaan_web.wasm"
cp crates/thaan-web/web/index.html "$out/"
cp docs/brand/logo/{favicon.ico,favicon.svg,apple-touch-icon.png} "$out/"
# The kit loads icons over HTTP from ./assets/icons on the web.
icons=$(ls -d ~/.cargo/registry/src/*/gpui-kit-assets-0.11.0/assets/icons | head -1)
cp -R "$icons" "$out/assets/icons"
# The demo's own icons, prefixed so they never shadow a lucide name.
for f in crates/thaan-web/icons/*.svg; do
  cp "$f" "$out/assets/icons/brand-$(basename "$f")"
done
# GitHub Pages: serve files as they are (no Jekyll).
touch "$out/.nojekyll"
echo "built $out ($profile): $(du -sh "$out/thaan_web_bg.wasm" | cut -f1) wasm"
