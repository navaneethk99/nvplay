#!/bin/sh
# Stage and sign a complete bundle before replacing the installed app.
set -eu
source_bundle="${1:-target/nvplay.app}"
destination="${2:-/Applications/nvplay.app}"
[ -f "$source_bundle/Contents/MacOS/nvplay" ] || { echo "nvplay bundle not found: $source_bundle" >&2; exit 1; }
parent=$(dirname "$destination")
stage=$(mktemp -d "$parent/.nvplay-install.XXXXXX")
cleanup() {
    if [ -d "$stage/previous.app" ] && [ ! -e "$destination" ]; then
        mv "$stage/previous.app" "$destination"
    fi
    rm -rf "$stage"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
/usr/bin/ditto "$source_bundle" "$stage/nvplay.app"
/usr/bin/codesign --force --deep --sign - "$stage/nvplay.app"
/usr/bin/codesign --verify --deep --strict "$stage/nvplay.app"
if [ -e "$destination" ]; then
    [ -f "$destination/Contents/MacOS/nvplay" ] || { echo "Destination is not an nvplay app bundle" >&2; exit 1; }
    mv "$destination" "$stage/previous.app"
fi
mv "$stage/nvplay.app" "$destination"
echo "Installed $destination"
