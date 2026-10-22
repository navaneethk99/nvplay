# Public watch parties

This deployment runs the browser viewer and WebSocket signaling service behind
Caddy HTTPS, with coturn for relayed WebRTC connections. Media is H.264/Opus over
encrypted WebRTC. The host encodes once and sends one stream per viewer, directly
or through TURN. Six viewers are supported; this is a small watch-party topology,
not an SFU for large audiences. At the default ~2 Mbps video rate, budget about
2.2 Mbps of host upload per viewer (roughly 13 Mbps for six), plus protocol overhead.

## Deploy on a Linux VPS

1. Point `watch.example.com` and `turn.example.com` DNS A records to the VPS's
   public IPv4 address. TURN DNS must resolve directly to this host, with no HTTP
   CDN proxy. If using IPv6, configure coturn's external/listening addresses too.
2. Copy `.env.example` to `.env`. Set both domains and the public IP, and generate
   **two different secrets** using `openssl rand -hex 32`. Set `WATCH_HOST_KEY`
   and `TURN_SECRET` to those values. Never commit `.env`.
3. Allow inbound TCP 80/443 (HTTPS), UDP/TCP 3478 (TURN), and UDP 49160–49259
   (relayed media) in the host and cloud firewall. This Compose file uses Linux
   host networking for coturn; run it on the public Linux host, not Docker Desktop.
   If the VPS has a separate private interface behind NAT, set coturn's
   `TURN_PUBLIC_IP=PUBLIC_IP/PRIVATE_IP` and `TURN_LISTEN_IP=PRIVATE_IP` in `.env`.
4. From this directory, run `docker compose up -d --build`.
5. Check `https://watch.example.com/health` returns `ok`.
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

Install FFmpeg with the `libx264` and `libopus` encoders on the desktop. The app
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
Sharing is fixed to a maximum width of 1280, 30 fps, ~2 Mbps, with the first video
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
