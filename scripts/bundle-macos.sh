#!/bin/sh
# Creates an unsigned local app bundle. The installed libmpv is still required.
set -eu
cd "$(dirname "$0")/.."
mode="${1:-release}"
case "$mode" in
    release) cargo build --locked --release ;;
    debug) cargo build --locked ;;
    *) echo "Usage: $0 [release|debug]" >&2; exit 1 ;;
esac
version=$(awk '/^version = / { gsub(/"/, "", $3); print $3; exit }' Cargo.toml)
bundle="target/nvplay.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp src/app/assets/nvplay.icns "$bundle/Contents/Resources/nvplay.icns"
cp "target/$mode/nvplay" "$bundle/Contents/MacOS/.nvplay-new"
mv -f "$bundle/Contents/MacOS/.nvplay-new" "$bundle/Contents/MacOS/nvplay"
cat > "$bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>nvplay</string>
  <key>CFBundleIdentifier</key><string>io.nvplay.player</string>
  <key>CFBundleName</key><string>nvplay</string>
  <key>CFBundleDisplayName</key><string>nvplay</string>
  <key>CFBundleIconFile</key><string>nvplay.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Video and audio media</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.movie</string><string>public.video</string><string>public.audio</string>
        <string>public.mpeg-4</string><string>com.apple.quicktime-movie</string>
        <string>public.mpeg</string><string>public.avi</string><string>public.mp3</string>
        <string>public.mpeg-4-audio</string><string>com.microsoft.waveform-audio</string>
      </array>
      <key>CFBundleTypeExtensions</key>
      <array>
        <string>mp4</string><string>m4v</string><string>mov</string><string>mkv</string>
        <string>webm</string><string>avi</string><string>mpg</string><string>mpeg</string>
        <string>ts</string><string>mts</string><string>m2ts</string><string>vob</string>
        <string>ogv</string><string>wmv</string><string>flv</string><string>3gp</string>
        <string>mp3</string><string>m4a</string><string>aac</string><string>flac</string>
        <string>wav</string><string>aiff</string><string>ogg</string><string>opus</string>
      </array>
    </dict>
  </array>
</dict></plist>
PLIST
echo "Created $bundle (requires an installed libmpv; unsigned local build)"
