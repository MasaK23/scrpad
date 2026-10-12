#!/usr/bin/env bash
# Renders every icon file the app and its packages use from scrpad.svg. Needs inkscape and
# python3; run it from anywhere after editing the SVG and commit the results.
#
#   scrpad.png         1024 px, macOS icon grid (padding around the body): the .app icon and
#                      the runtime icon on macOS
#   scrpad-square.svg  the same drawing cropped close to the body, for Linux (hicolor scalable)
#   scrpad-256.png     256 px of the square crop: the runtime icon on Windows and Linux
#   scrpad.ico         the square crop at 16–256 px: the Windows .exe icon
set -euo pipefail
cd "$(dirname "$0")"

# The body spans 100..924; keep a 24 px margin so the edge stays crisp after scaling.
sed 's/viewBox="0 0 1024 1024"/viewBox="76 76 872 872"/; s/width="1024" height="1024"/width="872" height="872"/' \
  scrpad.svg > scrpad-square.svg

inkscape scrpad.svg -w 1024 -h 1024 -o scrpad.png 2>/dev/null
inkscape scrpad-square.svg -w 256 -h 256 -o scrpad-256.png 2>/dev/null

tmp=$(mktemp -d)
for size in 16 20 24 32 40 48 64 128; do
  inkscape scrpad-square.svg -w "$size" -h "$size" -o "$tmp/$size.png" 2>/dev/null
done
cp scrpad-256.png "$tmp/256.png"
# An ICO whose entries are PNGs (Windows Vista+): a tenth of the size of ImageMagick's bitmaps.
python3 - "$tmp" <<'EOF'
import struct, sys
sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
pngs = [open(f"{sys.argv[1]}/{s}.png", "rb").read() for s in sizes]
out = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
for size, png in zip(sizes, pngs):
    out += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(png), offset)
    offset += len(png)
open("scrpad.ico", "wb").write(out + b"".join(pngs))
EOF
rm -rf "$tmp"
