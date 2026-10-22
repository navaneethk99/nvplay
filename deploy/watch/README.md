# Public watch parties

This deployment runs an H.264/Opus WebRTC forwarding server, browser viewer,
and signaling behind Caddy HTTPS, with coturn for restrictive networks. The
updated desktop encodes once and uploads one stream to Azure. Azure forwards
it to up to six viewers over independent encrypted WebRTC connections, without
transcoding or reducing source resolution. Slow viewers have separate queues
and retransmission caches, so they cannot block the publisher or other viewers.
Legacy desktop builds retain their previous direct-per-viewer transport until
updated. Host upload no longer grows with the number of viewers on updated builds.

The Azure deployment and management commands are documented in [AZURE.md](AZURE.md).

## Deploy on a Linux VPS

1. Point `watch.example.com` and `turn.example.com` DNS A records to the VPS's
   public IPv4 address. TURN DNS must resolve directly to this host, with no HTTP
   CDN proxy. If using IPv6, configure coturn's external/listening addresses too.
2. Copy `.env.example` to `.env`. Set both domains, TURN_PUBLIC_IP, and WATCH_MEDIA_PUBLIC_IP (public IPv4 only), and generate
   **two different secrets** using `openssl rand -hex 32`. Set `WATCH_HOST_KEY`
   and `TURN_SECRET` to those values. Never commit `.env`.
3. Allow inbound TCP 80/443 (HTTPS), UDP/TCP 3478 (TURN), and UDP 49160–49259
   (relayed media) and UDP 50000 (forwarding server) in the host and cloud firewall. This Compose file uses Linux
   host networking for coturn; run it on the public Linux host, not Docker Desktop.
   If the VPS has a separate private interface behind NAT, set coturn's
   `TURN_PUBLIC_IP=PUBLIC_IP/PRIVATE_IP` and `TURN_LISTEN_IP=PRIVATE_IP` in `.env`.
4. From this directory, run `docker compose up -d --build`.
5. Check `https://watch.example.com/health` returns HTTP 200.
6. In nvplay, open **Watch together → Watch service settings**, enter
   `https://watch.example.com` and the `WATCH_HOST_KEY`, then create a viewing
   link. Alternatively launch nvplay with `NVPLAY_WATCH_URL` and
   `NVPLAY_WATCH_HOST_KEY`. The address is saved locally; the host key is kept in
   memory and must be entered again after restarting unless supplied by the
   environment. A deployment can also provision a private `watch-host-key.txt` in
   nvplay's settings folder; the app reads it on startup. Viewers need neither a key nor an account.

Anyone holding an invite can watch and control that room. The random invite
secret is in the URL fragment, so it is not sent in HTTP requests or referrers;
it is submitted over WSS when joining. Ending the session, switching media,
stopping playback, or closing nvplay invalidates the link. Closing just the
sharing dialog leaves the party running. Losing the host/service connection
ends the party; create a new link to restart. Rooms live only in server memory.

The service supports `turns:` URLs as well. For networks that allow only TLS
outbound, configure coturn with a certificate and TLS listener (ideally port 443
on a **separate public IP**), then add e.g.
`turns:turn.example.com:443?transport=tcp` to `NVPLAY_TURN_URLS` on signaling.
Remove `--no-tls` and mount coturn's certificate and key. The default Compose
configuration supplies UDP and TCP TURN on 3478; it cannot bypass a firewall
that blocks both. Do not put TURN behind an HTTP reverse proxy.

Use one signaling instance: rooms are not distributed across replicas. The
public server requires a host key of at least 32 characters and a separate TURN
secret of at least 32 characters. TURN credentials expire after 24 hours and
rooms expire after 23 hours. The deployment caps rooms, viewers, signaling
message size/rate, and TURN allocations. The host key is for a private deployment;
a public multi-user product should issue host credentials from its account service.

## Media support and synchronization

Install FFmpeg with the `libx264` and `libopus` encoders on the desktop. On macOS,
a successfully probed VideoToolbox H.264 encoder is used automatically to offload
encoding to hardware; libx264 is retained as the software fallback. Set
`NVPLAY_WATCH_SOFTWARE_ENCODER=1` to force software encoding for diagnostics. The app
finds it on PATH, in Homebrew, beside the binary or in the macOS bundle; override
with `NVPLAY_FFMPEG`. Website sharing also requires yt-dlp. Local media, direct
seekable streams, and resolved Cinejoy HLS playlists go to FFmpeg. Website pages
are resolved in the background by yt-dlp, including separate video/audio URLs.
A source that requires unsupported browser authentication, DRM, headers, or an
expired signed URL can still fail to share even if libmpv can play it.

The desktop remains authoritative. Browsers send play/pause/absolute seek intents;
the desktop applies them through the existing playback layer and broadcasts
observed state. The browser uses the shared movie timeline rather than the live
MediaStream timeline. Playback changes and drift restart the encoder at the
host's current position, with continuous RTP sequence numbers/timestamps so
seeking does not reset the WebRTC connection. Encoding, networking, and browser
buffers introduce delay; this does not promise frame-perfect synchronization.
Sharing preserves the playing video resolution, using the first video
and audio streams. Track changes, external subtitles, rotation, and desktop audio
volume/mute are not mirrored. Each viewer controls their own listening volume.
Live/nonseekable sources are not accepted in this first version.

## Local development

```sh
cargo run -p nvplay-watch-server
NVPLAY_WATCH_URL=http://localhost:8787 cargo run
```

HTTP is allowed only for localhost development. Local mode may omit TURN and
the host key; this does not produce a public link. Production requires HTTPS
and TURN configuration, and no public service is preconfigured in the app.

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The server tests exercise authentication, participant isolation, multiple viewers,
shared control routing, state broadcasts, and link revocation using real WebSockets.
The opt-in browser check in `scripts/check-watch-together.sh` exercises the
actual native playback → encoder → WebRTC → browser path, with two browser
viewers, shared play/pause/seek, decoded video/audio and session termination.
A cross-network test with your deployed TURN server is still needed to verify
its DNS, certificates, firewall, bandwidth and connectivity.

Sharing preserves the playing video's pixel dimensions and source frame rate,
using H.264 CRF 18 with a short VBV buffer instead of a 720p resolution cap. Odd dimensions receive at most
one pixel of padding for H.264 4:2:0 compatibility. Encoding is still lossy;
CPU and upload requirements increase with resolution. Website inputs resolve a
rendition at the playing video's height before encoding to its decoded dimensions.

Open the sidebar's **Watch together** tab to create/copy/end a viewing link.
**Viewer access** applies to every current and future viewer: allow play/pause,
allow seeking, and allow new joins. Uncheck both controls for watch-only access;
uncheck new joins to lock the invitation without disconnecting current viewers.
The signaling server checks each command, and browsers update controls live.
Deploy the service and desktop together when updating the watch protocol.


## Streaming latency and bandwidth

The encoder preserves pixel dimensions and source frame rate. The bitrate starts
at up to 6 Mbps for one viewer, with a resolution-based ceiling (about 6.2 Mbps for
1080p, at most 20 Mbps). Receiver reports lower the shared encoder bitrate when
any viewer experiences congestion. Severe loss reduces bitrate down to a resolution-based quality floor; healthy links
recover after 20 seconds. The encoding budget is shared as viewers join. Hysteresis limits restarts, and continuous RTP clocks preserve
the existing connections. Quality may soften on constrained links; resolution
is not reduced. One encoder serves everyone, so the weakest connection can
limit party quality. Upload still grows with viewer count.

A 500 ms encoder VBV buffer, paced packet batches, shorter input probing, and
50 ms browser jitter-buffer hints reduce queues. Browsers can override hints;
HLS loading, encoding, network transit and decoding still introduce latency.
No frame-perfect synchronization or zero-delay playback is promised.

`NVPLAY_WATCH_PERF_CHECK=1` adds a sustained frame-rate check for the 1080p30
fixture in the browser test. Inbound stats include packet loss and jitter-buffer
residence time; that measurement is not total host-to-viewer latency.


The web viewer now uses the desktop palette (#1d1d1d panels, #d98422 controls),
a full-window video area, and a compact timeline/control strip. Fullscreen retains
the controls and shortcuts. Marketing text and duplicate information cards are removed.

RTP ingestion runs independently of packet pacing, with a bounded queue sized
for encoding bursts. Sequence gaps remain visible instead of being hidden by
renumbering, preserving receiver loss reporting. Website resolution selection
rejects lower-resolution fallbacks instead of silently upscaling them.


Website sharing verifies selected formats and carries the extractor's HTTP
request headers into FFmpeg. It prefers a playable H.264 rendition at the
current video height before selecting another codec. Headers remain on the host.
Input seeking/decoding runs without `-readrate`; `realtime` and `arealtime` pace
the decoded outputs instead. This avoids pacing discarded pre-roll during deep
seeks. A real long-GOP seek regression checks first-frame delivery within 2 seconds.

For a larger mobile-emulated group, enable the optional performance and embedded
video timestamp checks (requires the Playwright module path used by the script):

```sh
NVPLAY_WATCH_MOBILE=1 NVPLAY_WATCH_VIEWERS=5 \
NVPLAY_WATCH_PERF_CHECK=1 NVPLAY_WATCH_LATENCY_CHECK=1 \
sh scripts/check-watch-together.sh
```

The fixture encodes a binary timestamp in the actual video pixels. The browser
reads it back to measure content delay against the host's observed movie clock;
resolution badges and signaling arrival alone do not count as delivered video.
`NVPLAY_WATCH_BROWSER=webkit` runs the same flow with Playwright's WebKit engine.
Device emulation checks layout/touch behavior and browser engines, not a physical
phone's cellular radio, power state, or hardware decoder. Viewers automatically
rejoin after a disconnected transport and recover after page restoration. Screen
wake lock is requested when available; fullscreen retains the shared controls.

The relay compares negotiated RTP absolute-send-time stamps with arrival progress
and reports uplink queue growth once per second. Stamping packets after pacing
keeps source-fetch and decoder stalls out of the network congestion estimate. The host can then reduce bitrate
when a network queue grows, even if no packets have been lost. These reports are
negotiated before publishing so older desktop builds remain compatible. Viewers
that support browser-initiated offers use that handshake; older viewers keep the
server-offered path. The desktop exposes the invite once its publisher SDP is
accepted, so a copied link cannot race ahead of the forwarding session.
