#!/bin/sh
# Exercises the real OpenGL renderer offscreen; opens no application windows.
set -eu
cd "$(dirname "$0")/.."
[ "$(uname -s)" = Darwin ] || { echo "This check requires macOS CGL." >&2; exit 1; }
test_directory=$(mktemp -d "${TMPDIR:-/tmp}/nvplay-render-check.XXXXXX")
trap 'rm -rf "$test_directory"' EXIT HUP INT TERM
for rate in 24 60; do
    ffmpeg -hide_banner -loglevel error \
        -f lavfi -i "testsrc2=size=1280x720:rate=$rate" \
        -f lavfi -i 'sine=frequency=440:sample_rate=48000' \
        -t 8 -c:v libx264 -preset ultrafast -g "$rate" -pix_fmt yuv420p -c:a aac \
        "$test_directory/$rate.mp4"
    NVPLAY_TEST_MEDIA="$test_directory/$rate.mp4" cargo test --locked real_gl_frame_timing -- --ignored --nocapture
    NVPLAY_TEST_MEDIA="$test_directory/$rate.mp4" cargo test --locked real_ui_gl_plays_at_normal_speed -- --ignored --nocapture
done

# Check that a short clip actually finishes at normal speed, too.
ffmpeg -hide_banner -loglevel error -i "$test_directory/60.mp4" -t 1 -c copy "$test_directory/one-second.mp4"
NVPLAY_TEST_MEDIA="$test_directory/one-second.mp4" cargo test --locked real_ui_gl_plays_at_normal_speed -- --ignored --nocapture
