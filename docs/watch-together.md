# Watch Together playback architecture

The desktop remains the authority for the shared timeline. Internet media can
now use a control-only session; local files retain the encoder/relay session.

## Routing

| Source | Default playback on viewer |
| --- | --- |
| YouTube watch, short, embed, live URL | YouTube IFrame Player API with video ID |
| Recognized MP4/M4V, WebM, audio URL | Browser media element |
| HLS URL | Locally vendored hls.js (MSE), native HLS fallback |
| Cinejoy | The already resolved master playlist, with absolute video/audio URLs |
| Local file, other website extractor, other container/DASH | Existing FFmpeg → WebRTC relay |
| “Stream from my device instead” selected | Existing FFmpeg → WebRTC relay |

The source is selected at session creation. Changing videos ends the session,
as before. There is no automatic mid-session switch to broadcasting.

## Control-only session

1. `Player::sharing_browser_media` chooses a provider adapter. Cinejoy shares
   the small playlist contents rather than its temporary local filename. Desktop
   request headers and cookies are never forwarded.
2. `Create.media` carries a validated `Media` descriptor. The server stores it
   in the room and returns it in `Joined.media`, before the cached host state.
   Missing `media` preserves the existing broadcast protocol.
3. The desktop worker opens only a WebSocket. It sends `State` every 250 ms,
   forwards permitted viewer controls to mpv, publishes access updates, tracks
   members, and ends the room on shutdown. It does not resolve YouTube streams,
   discover FFmpeg encoders, create WebRTC tracks, or download media.
4. The viewer creates a provider player and fetches media directly. No peer
   connection is created for this mode. Inline HLS sessions also receive a
   `Joined.media_proxy` URL. If direct playback fails, the viewer retries once
   through the authenticated original-byte HLS fallback.
5. Browser controls send `Control` to the server. Server permissions and desktop
   permissions are enforced before mpv receives a command. All viewers follow
   the subsequent authoritative host state, avoiding control feedback loops.
6. Viewers interpolate the host position using elapsed monotonic time and half
   the measured WebSocket round trip. Playing drift above 1.5 s causes a seek;
   smaller drift above 150 ms uses a 5% rate adjustment for native playback.
   Paused seeks use a 150 ms threshold. YouTube uses its available playback
   rates and seeks to correct larger drift. These are best-effort clocks, not
   frame-accurate synchronization; host-to-service latency remains unmeasured.
7. Host buffering pauses viewers. Missing state for 1.5 s freezes playback.
   Reconnect rejoins the room with the media descriptor and latest state. Ending,
   leaving, or suspending the page disposes the provider player and media loads.

## Browser constraints

Autoplay rejection presents an explicit enable-playback gesture. Loading and
fatal errors show an explanation instead of repeatedly reconnecting the session.
Direct HLS requires cross-origin manifest, segment, and key access. Some Cinejoy
CDNs reject the watch site's Origin/Referer even when the stream is playable in
mpv. Inline HLS sessions retry automatically through `/media/{room}/{asset}`:
the service supplies provider headers, rewrites child playlist/segment/key/map
URLs, and streams original bytes to the browser. This consumes service bandwidth
but does not encode video or use host upload bandwidth. URLs must be discovered
in the room's playlist; arbitrary URL request parameters are not accepted. The
room token is required, assets expire with the room, requests are bounded, and
HTTPS-only upstreams use pinned public DNS addresses with redirects disabled.
Viewer cookies, auth headers, and referrers are never forwarded upstream.

MSE is preferred even if the browser advertises native HLS, since that capability
check alone does not establish support for blob-backed master playlists. Native
HLS remains the fallback when MSE is unavailable. Resolved URLs
can expire or be bound to the host's IP. YouTube must allow embedding and may
apply region, age, or authentication restrictions. The broadcast checkbox remains available before starting a new session if
both direct access and the original-byte fallback fail.

The page CSP permits provider media connections and YouTube scripts/frames.
`strict-origin-when-cross-origin` supplies the origin needed by YouTube without
sending the room path or fragment token as a cross-origin referrer. Room members
necessarily receive direct stream URLs; treat invitations as private. The server
retains existing host-key authentication, invitation tokens, viewer limits, and
permission checks. TURN credentials are still configured for broadcast fallback.

Both the desktop and server must be rebuilt together. An older strict server
will reject `Create.media`; there is no silent downgrade negotiation. The browser
assets, including hls.js, are embedded in the watch-server binary.

## Verification

```sh
cargo test --workspace
cargo build -p nvplay-watch-server
NVPLAY_PLAYWRIGHT_MODULE=/path/to/node_modules/playwright node scripts/check-watch-direct.cjs
```

The browser test needs Node with built-in WebSocket, Playwright Chromium, and
FFmpeg. `NVPLAY_CHROME` selects an installed Chromium executable;
`NVPLAY_FFMPEG` selects FFmpeg; `NVPLAY_DIRECT_TEST_PORT` defaults to 18787.
It generates MP4/HLS fixtures and uses an isolated watch-server process plus a
separate media origin. It exercises multiple viewers, host/viewer commands,
late join, drift, speed, reconnect, permissions, end-session cleanup, HLS decoding,
and source failure. YouTube is checked against a mock of its public player API;
live provider checks are opt-in. Set `NVPLAY_DIRECT_LIVE_YOUTUBE` to a public
YouTube video ID and/or `NVPLAY_DIRECT_CINEJOY_METADATA` to the JSON output of
the existing Cinejoy resolver to exercise real provider playback as well.
`NVPLAY_DIRECT_FORCE_PROXY=1` blocks browser-to-provider requests in that live
Cinejoy check, verifying decoding through the real service fallback.
The default fixture suite does not establish live provider availability.
The Rust direct-worker test verifies session creation without FFmpeg, state and
control delivery, permissions updates, membership, and shutdown on the wire.

Provider API references: [YouTube IFrame API](https://developers.google.com/youtube/iframe_api_reference),
[hls.js](https://hlsjs.video-dev.org/api-docs/hls.js.hls).

Live smoke checks on 2026-10-23 decoded a resolved Cinejoy stream and started a
YouTube embed in Chromium. Cinejoy timed out on the first attempt and succeeded
on retry. These results do not establish availability for every title, network,
or browser. Safari/native HLS has not been exercised in this change.
