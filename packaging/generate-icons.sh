#!/usr/bin/env bash
# Regenerates every platform icon from the SVG sources in assets/logo/. The
# outputs are committed, so this only needs running after the logo changes.
#   ./packaging/generate-icons.sh
#
# Needs ImageMagick with the librsvg delegate, icotool (icoutils) and Python 3.
#
# maestro-app-icon-16.svg is the small-size cut of the app icon (thicker
# strokes, no ring); anything under 48px is rendered from it, since the ring
# turns to mush below that.
set -euo pipefail
cd "$(dirname "$0")/.."

LOGO=assets/logo
SMALL="$LOGO/maestro-app-icon-16.svg"
LARGE="$LOGO/maestro-app-icon.svg"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# render <svg> <size> <out.png>: rasterises at the target size directly rather
# than scaling a large bitmap down, so edges land on the pixel grid.
render() {
    local density
    density=$(awk -v n="$2" -v w="$(svg_width "$1")" 'BEGIN { print 72 * n / w }')
    magick -background none -density "$density" "RSVG:$1" \
        -resize "$2x$2!" -define png:color-type=6 "$3"
}

svg_width() {
    sed -n 's/.*<svg[^>]* width="\([0-9.]*\)".*/\1/p' "$1" | head -n 1
}

source_for() {
    if [ "$1" -lt 48 ]; then printf '%s\n' "$SMALL"; else printf '%s\n' "$LARGE"; fi
}

# macOS: the Big Sur grid puts an 824px body on a 1024px canvas, with a soft
# drop shadow in the margin. Wrap the app icon's contents in that layout.
mac_svg() {
    local body
    body=$(sed -e '/<svg/d' -e '/<\/svg>/d' "$1")
    cat <<EOF
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
  <defs>
    <filter id="shadow" x="-10%" y="-10%" width="120%" height="125%">
      <feDropShadow dx="0" dy="10" stdDeviation="12" flood-color="#000" flood-opacity="0.3"/>
    </filter>
  </defs>
  <g filter="url(#shadow)">
    <g transform="translate(100 100) scale(12.875)">
$body
    </g>
  </g>
</svg>
EOF
}

# Linux: hicolor PNG sizes; the scalable SVG is maestro-app-icon.svg itself.
mkdir -p "$LOGO/linux"
rm -f "$LOGO"/linux/maestro-*.png
for n in 16 22 24 32 48 64 96 128 256 512; do
    render "$(source_for "$n")" "$n" "$LOGO/linux/maestro-$n.png"
done

# Windows: the sizes the shell and the scaled-DPI title bars ask for. 256 is
# stored as PNG, the rest as BMP for the older loaders.
ico_args=()
for n in 16 20 24 32 40 48 64 96 128; do
    render "$(source_for "$n")" "$n" "$work/ico-$n.png"
    ico_args+=("$work/ico-$n.png")
done
render "$LARGE" 256 "$work/ico-256.png"
icotool -c -o "$LOGO/maestro.ico" "${ico_args[@]}" -r "$work/ico-256.png"

# macOS: every size iconutil would produce (16-512 @1x and @2x).
mac_svg "$SMALL" >"$work/mac-small.svg"
mac_svg "$LARGE" >"$work/mac-large.svg"
for n in 16 32 64 128 256 512 1024; do
    if [ "$n" -lt 48 ]; then src="$work/mac-small.svg"; else src="$work/mac-large.svg"; fi
    render "$src" "$n" "$work/icns-$n.png"
done
# Written by hand: Pillow's writer leaves out the 16 and 32 @1x slots, which
# would make Finder downscale the large cut instead of using the small one.
python3 - "$work" "$LOGO/maestro.icns" <<'EOF'
import struct
import sys

work, out = sys.argv[1], sys.argv[2]
# OSType -> pixel size. Every entry is PNG data, supported since 10.7.
slots = [
    (b"icp4", 16), (b"icp5", 32), (b"ic11", 32), (b"ic12", 64),
    (b"ic07", 128), (b"ic08", 256), (b"ic13", 256),
    (b"ic09", 512), (b"ic14", 512), (b"ic10", 1024),
]
body = b""
for ostype, n in slots:
    with open(f"{work}/icns-{n}.png", "rb") as f:
        png = f.read()
    body += ostype + struct.pack(">I", 8 + len(png)) + png
with open(out, "wb") as f:
    f.write(b"icns" + struct.pack(">I", 8 + len(body)) + body)
EOF

echo "icons written to $LOGO/"
