#!/bin/sh
# Exercises actual UI pointer input against libmpv without a display/audio device.
set -eu
cd "$(dirname "$0")/.."
test_directory=$(mktemp -d "${TMPDIR:-/tmp}/nvplay-check.XXXXXX")
trap 'rm -rf "$test_directory"' EXIT HUP INT TERM
cat > "$test_directory/subtitles.srt" <<'SRT'
1
00:00:00,000 --> 00:00:29,000
Subtitle selection test
SRT
ffmpeg -hide_banner -loglevel error \
  -f lavfi -i 'testsrc2=size=640x360:rate=24' \
  -f lavfi -i 'sine=frequency=440:sample_rate=48000' \
  -f lavfi -i 'sine=frequency=880:sample_rate=48000' \
  -i "$test_directory/subtitles.srt" -t 30 \
  -map 0:v -map 1:a -map 2:a -map 3:0 \
  -c:v libx264 -preset ultrafast -g 240 -c:a aac -c:s srt \
  -metadata:s:a:0 language=eng -metadata:s:a:0 title=English \
  -metadata:s:a:1 language=fra -metadata:s:a:1 title=French \
  -metadata:s:s:0 language=eng "$test_directory/fixture.mkv"
export NVPLAY_TEST_MEDIA="$test_directory/fixture.mkv"
cargo test --locked real_engine -- --ignored
cargo test --locked real_ui_pointer -- --ignored

cargo test --locked real_preview -- --ignored
