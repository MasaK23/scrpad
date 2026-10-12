#!/usr/bin/env bash
# Builds dist/scrpad.app: a universal (Apple Silicon + Intel) app bundle with its icon, ad-hoc
# signed. Run on macOS from the repository root, after building both targets:
#
#   rustup target add aarch64-apple-darwin x86_64-apple-darwin
#   cargo build --profile dist --target aarch64-apple-darwin --target x86_64-apple-darwin
#   packaging/macos/bundle.sh
set -euo pipefail

version=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
app=dist/scrpad.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

lipo -create -output "$app/Contents/MacOS/scrpad" \
  target/aarch64-apple-darwin/dist/scrpad target/x86_64-apple-darwin/dist/scrpad
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"

# assets/app-icon/scrpad.png is rendered from scrpad.svg (inkscape -w 1024 -h 1024).
iconset=$(mktemp -d)/scrpad.iconset
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" assets/app-icon/scrpad.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) assets/app-icon/scrpad.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/scrpad.icns"

# Not notarized (that needs an Apple Developer account), but a consistent ad-hoc signature over
# the whole bundle lets it start after the quarantine flag is cleared.
codesign --force --sign - "$app"
codesign --verify --strict "$app"
lipo -info "$app/Contents/MacOS/scrpad"
