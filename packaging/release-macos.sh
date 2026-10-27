#!/usr/bin/env bash
# Genera el .dmg de macOS para el commit actual, igual que el workflow de release.
# Uso: packaging/release-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
case "$(uname -m)" in
  arm64) target="macos-arm64" ;;
  x86_64) target="macos-x86_64" ;;
  *) echo "Arquitectura no reconocida: $(uname -m)" >&2; exit 1 ;;
esac
name="omni-term-v${version}-${target}"

echo "Compilando omni-term v${version} (${target})…"
cargo build --release --locked

rm -rf "dist/$name"
app="dist/$name/omni-term.app/Contents"
mkdir -p "$app/MacOS" "$app/Resources"
cp target/release/omni-term "$app/MacOS/"
cp packaging/omni-term.icns "$app/Resources/"
cat > "$app/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>omni-term</string>
  <key>CFBundleDisplayName</key><string>omni-term</string>
  <key>CFBundleIdentifier</key><string>io.github.pdf3rn.omni-term</string>
  <key>CFBundleExecutable</key><string>omni-term</string>
  <key>CFBundleIconFile</key><string>omni-term</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST

xattr -cr "dist/$name/omni-term.app"
codesign --force --deep --sign - "dist/$name/omni-term.app"
ln -sf /Applications "dist/$name/Applications"

rm -f "dist/$name.dmg"
hdiutil create -volname "omni-term" -srcfolder "dist/$name" -ov -format UDZO "dist/$name.dmg"
rm -rf "dist/$name"

echo "Listo: dist/$name.dmg"
