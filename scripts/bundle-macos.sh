#!/usr/bin/env bash
# Build target/ThaanDB.app around the thaan-app binary, with the app icon. The `thaan`
# CLI goes next to it: Settings > MCP points AI agents at it (`thaan mcp`).
# Usage: scripts/bundle-macos.sh [--release | --universal]
#   --universal  release build for Apple Silicon and Intel, joined with lipo (CI uses it;
#                needs `rustup target add aarch64-apple-darwin x86_64-apple-darwin`)
# The bundle is not code-signed (see CLAUDE.md).
set -euo pipefail

cd "$(dirname "$0")/.."
profile=debug
bin_dir=target/debug
if [[ "${1:-}" == "--release" ]]; then
  profile=release
  bin_dir=target/release
  cargo build -p thaan-app -p thaan-cli --release
elif [[ "${1:-}" == "--universal" ]]; then
  profile=universal
  bin_dir=target/universal-release
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build -p thaan-app -p thaan-cli --release --target "$t"
  done
  mkdir -p "$bin_dir"
  for b in thaan-app thaan; do
    lipo -create -output "$bin_dir/$b" \
      "target/aarch64-apple-darwin/release/$b" "target/x86_64-apple-darwin/release/$b"
  done
else
  cargo build -p thaan-app -p thaan-cli
fi

app=target/ThaanDB.app
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>thaan-app</string>
<key>CFBundleIdentifier</key><string>dev.thaandb.app</string>
<key>CFBundleName</key><string>ThaanDB</string>
<key>CFBundleDisplayName</key><string>ThaanDB</string>
<key>CFBundleIconFile</key><string>AppIcon</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>${version}</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
EOF

cp crates/thaan-app/assets/macos/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
# Remove before copying: overwriting a running binary in place gets the process killed.
rm -f "$app/Contents/MacOS/thaan-app" "$app/Contents/MacOS/thaan"
cp "$bin_dir/thaan-app" "$app/Contents/MacOS/thaan-app"
cp "$bin_dir/thaan" "$app/Contents/MacOS/thaan"
# Tell Finder / the Dock that the icon changed.
touch "$app"
echo "built $app ($profile)"
