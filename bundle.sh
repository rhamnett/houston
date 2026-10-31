#!/usr/bin/env bash
# Build a minimal macOS .app bundle around the release binary so it can be
# added to System Settings > General > Login Items, and zip it for a release.
#   ./bundle.sh               this Mac's architecture
#   UNIVERSAL=1 ./bundle.sh   Apple silicon + Intel in one binary (what CI ships)
#   HOUSTON_VERSION=0.3.0 ...  the version to stamp (CI sets it from the new tag;
#                              otherwise it comes from git describe, see build.rs)
# Output: dist/Houston.app, dist/Houston-<version>-macos.zip and its .sha256
set -euo pipefail
cd "$(dirname "$0")"
BUILD=${GITHUB_RUN_NUMBER:-1}
APP=dist/Houston.app

rm -rf "$APP" dist/Houston-*-macos.zip*
mkdir -p "$APP/Contents/MacOS"
if [[ "${UNIVERSAL:-0}" == 1 ]]; then
  cargo build --release --target aarch64-apple-darwin --target x86_64-apple-darwin
  lipo -create -output "$APP/Contents/MacOS/houston" \
    target/aarch64-apple-darwin/release/houston target/x86_64-apple-darwin/release/houston
else
  cargo build --release
  cp target/release/houston "$APP/Contents/MacOS/houston"
fi
# The binary knows its version; the bundle and the zip follow it.
VERSION=$("$APP/Contents/MacOS/houston" --version | cut -d' ' -f2)
SHORT=${VERSION%%+*} # Info.plist wants plain numbers: 0.2.0+3.gabc -> 0.2.0
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>houston</string>
  <key>CFBundleIdentifier</key><string>local.houston</string>
  <key>CFBundleName</key><string>Houston</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$SHORT</string>
  <key>CFBundleVersion</key><string>$BUILD</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
# Ad-hoc: no Developer ID yet, so a downloaded copy needs Open from the
# right-click menu the first time (or: xattr -dr com.apple.quarantine).
codesign --force --sign - "$APP" >/dev/null
ZIP="dist/Houston-${VERSION//+/-}-macos.zip"
ditto -c -k --keepParent "$APP" "$ZIP"
(cd dist && shasum -a 256 "$(basename "$ZIP")" > "$(basename "$ZIP").sha256")
echo "built: $PWD/$APP ($(lipo -archs "$APP/Contents/MacOS/houston"))"
echo "       $PWD/$ZIP"
