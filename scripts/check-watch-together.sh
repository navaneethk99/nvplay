#!/bin/sh
# Local WebRTC end-to-end check; does not provision a public service.
set -eu
cd "$(dirname "$0")/.."
CARGO=${CARGO:-cargo}
if ! command -v "$CARGO" >/dev/null 2>&1; then CARGO="$HOME/.cargo/bin/cargo"; fi
command -v ffmpeg >/dev/null
command -v node >/dev/null
NVPLAY_WATCH_TEST_DIR=$(mktemp -d "${TMPDIR:-/tmp}/nvplay-watch.XXXXXX")
export NVPLAY_WATCH_TEST_DIR
server_pid=
host_pid=
cleanup() {
  if [ -n "$host_pid" ]; then kill "$host_pid" 2>/dev/null || true; wait "$host_pid" 2>/dev/null || true; fi
  if [ -n "$server_pid" ]; then kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true; fi
  rm -rf "$NVPLAY_WATCH_TEST_DIR"
}
trap cleanup EXIT HUP INT TERM
if [ -z "${NVPLAY_PLAYWRIGHT_MODULE:-}" ]; then
  npm install --prefix "$NVPLAY_WATCH_TEST_DIR/browser" playwright --no-audit --no-fund
  NVPLAY_PLAYWRIGHT_MODULE="$NVPLAY_WATCH_TEST_DIR/browser/node_modules/playwright"
  export NVPLAY_PLAYWRIGHT_MODULE
fi
if [ -z "${NVPLAY_CHROME:-}" ] && [ -x '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' ]; then
  NVPLAY_CHROME='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'
  export NVPLAY_CHROME
fi
if [ -z "${NVPLAY_CHROME:-}" ]; then "$NVPLAY_PLAYWRIGHT_MODULE/cli.js" install chromium; fi
NVPLAY_TEST_MEDIA="$NVPLAY_WATCH_TEST_DIR/movie.mp4"
export NVPLAY_TEST_MEDIA
ffmpeg -hide_banner -loglevel error -f lavfi -i 'testsrc2=size=640x360:rate=30' -f lavfi -i 'sine=frequency=440:sample_rate=48000' -t 90 -c:v libx264 -preset ultrafast -pix_fmt yuv420p -c:a aac "$NVPLAY_TEST_MEDIA"
"$CARGO" build --locked -p nvplay-watch-server
NVPLAY_WATCH_BIND=127.0.0.1:8787 NVPLAY_WATCH_PUBLIC_URL=http://localhost:8787 NVPLAY_WATCH_HOST_KEY= NVPLAY_TURN_URLS= target/debug/nvplay-watch-server >"$NVPLAY_WATCH_TEST_DIR/server.log" 2>&1 &
server_pid=$!
# Wait for the actual health endpoint, failing if another server is bound.
sleep 1
if ! kill -0 "$server_pid" 2>/dev/null; then cat "$NVPLAY_WATCH_TEST_DIR/server.log"; exit 1; fi
"$CARGO" test --locked real_watch_party_broadcasts_to_browsers -- --ignored --nocapture >"$NVPLAY_WATCH_TEST_DIR/host.log" 2>&1 &
host_pid=$!
if ! node scripts/check-watch-together.cjs; then cat "$NVPLAY_WATCH_TEST_DIR/host.log"; cat "$NVPLAY_WATCH_TEST_DIR/server.log"; exit 1; fi
wait "$host_pid"
host_pid=
