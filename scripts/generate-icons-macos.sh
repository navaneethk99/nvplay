#!/bin/sh
# Render the committed SVG at macOS icon sizes using built-in system tools.
set -eu
cd "$(dirname "$0")/.."
assets="src/app/assets"
iconset="target/nvplay.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    /usr/bin/sips -s format png -z "$size" "$size" "$assets/nvplay.svg" \
        --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    /usr/bin/sips -s format png -z "$double" "$double" "$assets/nvplay.svg" \
        --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
/usr/bin/iconutil -c icns "$iconset" -o "$assets/nvplay.icns"
cp "$iconset/icon_128x128@2x.png" "$assets/nvplay.png"
/usr/bin/sips -s format png "$assets/nvplay-mark.svg" --out "$assets/nvplay-mark.png" >/dev/null
echo "Generated app icons and the transparent in-app brand mark"
