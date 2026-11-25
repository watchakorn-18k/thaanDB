#!/usr/bin/env bash
# Rebuild and restart the desktop app whenever the code changes.
#
#   scripts/dev-watch.sh            # cargo run, restarted on every save
#   scripts/dev-watch.sh --bundle   # rebuild target/ThaanDB.app and relaunch it
#                                   # (the macOS screenshot tools only see the bundle)
#
# This is not hot reload: Rust has no way to swap code into a running GPUI app, so each
# change rebuilds (a few seconds for app-only edits) and starts a fresh window. Open
# connections and tabs are lost on every restart.
#
# Uses target/thaan-home as the store unless THAAN_HOME is already set, so the real
# store is untouched. Needs cargo-watch: `cargo install cargo-watch`.
set -euo pipefail

cd "$(dirname "$0")/.."
export THAAN_HOME="${THAAN_HOME:-$PWD/target/thaan-home}"

if ! command -v cargo-watch >/dev/null 2>&1; then
  echo "cargo-watch is not installed: cargo install cargo-watch" >&2
  exit 1
fi

# Only one copy of the app at a time (an older one may hold the window).
pkill -x thaan-app 2>/dev/null || true

# Watch the code and manifests; docs and build output do not restart the app.
watch=(-w crates -w Cargo.toml -w Cargo.lock -i 'target/*' -i '*.md')

if [[ "${1:-}" == "--bundle" ]]; then
  # `exec` keeps the app as cargo-watch's child, so a save stops it before rebuilding.
  exec cargo watch "${watch[@]}" \
    -s 'scripts/bundle-macos.sh && exec target/ThaanDB.app/Contents/MacOS/thaan-app'
else
  exec cargo watch "${watch[@]}" -x 'run -p thaan-app'
fi
