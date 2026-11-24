#!/usr/bin/env bash
# Build target/ThaanDB.app around the thaan-app binary, with the app icon.
# Usage: scripts/bundle-macos.sh [--release]
# The bundle is not code-signed on purpose (see CLAUDE.md).
set -euo pipefail

cd "$(dirname "$0")/.."
profile=debug
if [[ "${1:-}" == "--release" ]]; then
  profile=release
  cargo build -p thaan-app --release
else
  cargo build -p thaan-app
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
rm -f "$app/Contents/MacOS/thaan-app"
cp "target/$profile/thaan-app" "$app/Contents/MacOS/thaan-app"
# Tell Finder / the Dock that the icon changed.
touch "$app"
echo "built $app ($profile)"
