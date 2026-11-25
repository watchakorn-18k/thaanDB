#!/usr/bin/env bash
# Regenerate the Windows app icon and its compiled resource from the brand icon:
#   crates/thaan-app/assets/windows/AppIcon.ico  (16 to 256 px, PNG entries)
#   crates/thaan-app/assets/windows/app.res      (icon as resource 1 + version info)
# The .res is checked in and linked by crates/thaan-app/build.rs, so building for
# Windows needs no resource compiler. Run this again when the logo or version changes.
# Needs: sips (macOS), python3, llvm-rc (brew install llvm).
set -euo pipefail

cd "$(dirname "$0")/.."
src=docs/brand/logo/thaandb-app-icon-1024.png
out=crates/thaan-app/assets/windows
rc_tool=$(command -v llvm-rc || echo /opt/homebrew/opt/llvm/bin/llvm-rc)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
IFS=. read -r major minor patch <<<"$version"

mkdir -p "$out"
tmp=$(mktemp -d)
trap 'rm -r "$tmp"' EXIT

sizes=(16 24 32 48 64 128 256)
for s in "${sizes[@]}"; do
  sips -z "$s" "$s" "$src" --out "$tmp/$s.png" >/dev/null
done

# An .ico is a small header plus a directory of entries; Vista and later read PNG entries.
python3 - "$tmp" "$out/AppIcon.ico" "${sizes[@]}" <<'EOF'
import struct, sys
tmp, dest, sizes = sys.argv[1], sys.argv[2], [int(s) for s in sys.argv[3:]]
pngs = [open(f"{tmp}/{s}.png", "rb").read() for s in sizes]
head = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
dirs = b""
for s, data in zip(sizes, pngs):
    dim = 0 if s >= 256 else s
    dirs += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
open(dest, "wb").write(head + dirs + b"".join(pngs))
EOF

cat > "$tmp/app.rc" <<EOF
1 ICON "AppIcon.ico"

1 VERSIONINFO
FILEVERSION $major,$minor,$patch,0
PRODUCTVERSION $major,$minor,$patch,0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "ThaanDB contributors"
      VALUE "FileDescription", "ThaanDB"
      VALUE "FileVersion", "$version"
      VALUE "InternalName", "thaan-app"
      VALUE "OriginalFilename", "ThaanDB.exe"
      VALUE "ProductName", "ThaanDB"
      VALUE "ProductVersion", "$version"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
EOF
cp "$out/AppIcon.ico" "$tmp/AppIcon.ico"
"$rc_tool" /nologo /fo "$out/app.res" "$tmp/app.rc"
echo "wrote $out/AppIcon.ico and $out/app.res ($version)"
