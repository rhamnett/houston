#!/usr/bin/env bash
# Build a minimal macOS .app bundle around the release binary so the panel can be
# added to System Settings > General > Login Items. Output: dist/Houston.app
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release
rm -rf "dist/Houston.app"
mkdir -p "dist/Houston.app/Contents/MacOS"
cp target/release/houston "dist/Houston.app/Contents/MacOS/houston"
cat > "dist/Houston.app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>houston</string>
  <key>CFBundleIdentifier</key><string>local.houston</string>
  <key>CFBundleName</key><string>Houston</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
codesign --force --sign - "dist/Houston.app" >/dev/null
echo "built: $PWD/dist/Houston.app"
